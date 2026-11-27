//! The SQL of a model, as Redgate's "Generate SQL": tables with their keys,
//! indexes, checks and comments, then the foreign keys of its references,
//! in the dialect of the model's connection ([`super::dialects`]). What a
//! database does not have is left out and noted at the top of the script.
//! Pure; [`create_statements`] is what "Create in database" runs, only for
//! the tables the model designs (drafts).

use std::collections::HashSet;

use cubedriver::DatabaseStructure;

use super::cube_model::{
    validate, validate_draft_for_ddl, Definition, DraftColumn, DraftTable, ModelJoin, ModelTable,
    ModelView,
};
use super::dialects::{self, Actions, Auto, Comments, Dialect, Drop, Fk, Index, Key, Suffix};

/// What [`script`] writes.
#[derive(Clone, Debug, Default)]
pub struct ScriptOptions {
    /// DROP statements before the creates.
    pub drop: bool,
    /// Only the tables the model designs.
    pub only_new: bool,
    /// Only these aliases.
    pub only: Option<Vec<String>>,
    /// COMMENT statements (or clauses) for comments.
    pub comments: bool,
}

/// A statement to run. The statements of one draft (its scripts, CREATE
/// TABLE, indexes and comments) come together and carry its `schema.table`;
/// the CREATE TABLE `creates` it, so a run knows which tables now exist.
#[derive(Clone, Debug, PartialEq)]
pub struct Statement {
    pub table: Option<String>,
    pub sql: String,
    pub creates: bool,
}

/// The dialect of `db_type`, when SQL can be written for it.
fn dialect_for(db_type: &str) -> Result<&'static Dialect, String> {
    let d = dialects::of(db_type)
        .ok_or_else(|| format!("Criar tabelas não é suportado para {db_type}"))?;
    match d.no_sql {
        Some(why) => Err(why.to_string()),
        None => Ok(d),
    }
}

/// Writes one model's SQL in one dialect, noting what it leaves out.
struct Gen<'a> {
    d: &'static Dialect,
    db: &'a str,
    comments: bool,
    notes: Vec<String>,
}

impl<'a> Gen<'a> {
    fn new(db: &'a str, comments: bool) -> Result<Self, String> {
        Ok(Self {
            d: dialect_for(db)?,
            db,
            comments,
            notes: Vec::new(),
        })
    }

    fn note(&mut self, what: String) {
        if !self.notes.contains(&what) {
            self.notes.push(what);
        }
    }

    fn q(&self, ident: &str) -> String {
        self.d.quote(self.db, ident)
    }

    fn qualified_name(&self, schema: &str, name: &str) -> String {
        if self.d.schemas {
            format!("{}.{}", self.q(schema), self.q(name))
        } else {
            self.q(name)
        }
    }

    fn qualified(&self, table: &ModelTable) -> String {
        self.qualified_name(&table.schema, &table.table)
    }

    /// `'text'`: quotes doubled, or backslash-escaped where the dialect
    /// reads backslashes (BigQuery, Spark, Hive, Athena).
    fn literal(&self, text: &str) -> String {
        if self.d.backticks || self.d.id == "bigquery" {
            format!("'{}'", text.replace('\\', "\\\\").replace('\'', "\\'"))
        } else {
            format!("'{}'", text.replace('\'', "''"))
        }
    }

    fn columns_list(&self, names: impl IntoIterator<Item = impl AsRef<str>>) -> String {
        names
            .into_iter()
            .map(|n| self.q(n.as_ref()))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn label(&self) -> &'static str {
        self.d.label
    }

    fn sequence_name(table: &ModelTable, column: &DraftColumn) -> String {
        format!("seq_{}_{}", table.table, column.name)
    }

    /// SQLite's `INTEGER PRIMARY KEY AUTOINCREMENT`: one integer key column.
    fn sqlite_auto_key<'d>(&self, draft: &'d DraftTable) -> Option<&'d DraftColumn> {
        if self.d.auto != Auto::SqliteKey {
            return None;
        }
        let keys: Vec<&DraftColumn> = draft.columns.iter().filter(|c| c.primary_key).collect();
        match keys.as_slice() {
            [k] if k.auto_increment && self.d.column_type(&k.kind).eq_ignore_ascii_case("INTEGER") => {
                Some(k)
            }
            _ => None,
        }
    }

    fn column_sql(
        &mut self,
        table: &ModelTable,
        draft: &DraftTable,
        c: &DraftColumn,
        constraints: &mut Vec<String>,
    ) -> String {
        let d = self.d;
        let mut ty = d.column_type(&c.kind);
        let wraps = ["Nullable(", "LowCardinality(", "Array(", "Map(", "Tuple("];
        if d.nullable_wrap && c.nullable && !c.primary_key && !wraps.iter().any(|w| ty.starts_with(w)) {
            ty = format!("Nullable({ty})");
        }
        let mut sql = format!("{} {}", self.q(&c.name), ty);
        if c.unsigned || c.zerofill {
            if d.unsigned {
                if c.unsigned {
                    sql.push_str(" UNSIGNED");
                }
                if c.zerofill {
                    sql.push_str(" ZEROFILL");
                }
            } else {
                self.note(format!("UNSIGNED/ZEROFILL não existem no {}: ignorados", self.label()));
            }
        }
        let mut default = c.default.as_deref().filter(|x| !x.trim().is_empty()).map(str::to_string);
        if c.auto_increment {
            match d.auto {
                Auto::Identity => sql.push_str(" GENERATED BY DEFAULT AS IDENTITY"),
                Auto::AutoIncrement => sql.push_str(" AUTO_INCREMENT"),
                Auto::IdentityFn => sql.push_str(" IDENTITY(1,1)"),
                Auto::Autoincrement => sql.push_str(" AUTOINCREMENT"),
                Auto::SqliteKey => {
                    if self.sqlite_auto_key(draft).is_some_and(|k| k.name == c.name) {
                        sql.push_str(" PRIMARY KEY AUTOINCREMENT");
                    } else {
                        self.note(format!(
                            "No SQLite o auto incremento é só para uma chave primária INTEGER sozinha: {}.{} ficou sem",
                            table.table, c.name
                        ));
                    }
                }
                Auto::Sequence => {
                    let seq = self.qualified_name(&table.schema, &Self::sequence_name(table, c));
                    default = Some(format!("nextval({})", self.literal(&seq)));
                }
                Auto::None => self.note(format!(
                    "O {} não tem auto incremento: {}.{} ficou sem",
                    self.label(),
                    table.table,
                    c.name
                )),
            }
        }
        if d.key == Key::OnColumn && c.primary_key {
            sql.push_str(" PRIMARY KEY");
        }
        let not_null = (!c.nullable || c.primary_key) && d.not_null;
        if (!c.nullable || c.primary_key) && !d.not_null && !d.nullable_wrap {
            self.note(format!("O {} não tem NOT NULL nas colunas", self.label()));
        }
        let default_sql = match default {
            Some(x) if d.default => Some(if d.id == "sqlite" {
                format!(" DEFAULT ({x})")
            } else {
                format!(" DEFAULT {x}")
            }),
            Some(_) => {
                self.note(format!("O {} não tem DEFAULT nas colunas: ignorado", self.label()));
                None
            }
            None => None,
        };
        let not_null_sql = if not_null { " NOT NULL" } else { "" };
        if d.default_first {
            sql.push_str(default_sql.as_deref().unwrap_or(""));
            sql.push_str(not_null_sql);
        } else {
            sql.push_str(not_null_sql);
            sql.push_str(default_sql.as_deref().unwrap_or(""));
        }
        if let Some(check) = c.check.as_deref().filter(|x| !x.trim().is_empty()) {
            if !d.check {
                self.note(format!("O {} não tem CHECK: ignorado", self.label()));
            } else if d.column_check_as_constraint {
                constraints.push(format!(
                    "{}{}",
                    self.q(&format!("chk_{}_{}", table.table, c.name)),
                    format_args!(" CHECK ({check})")
                ));
            } else {
                sql.push_str(&format!(" CHECK ({check})"));
            }
        }
        if self.comments {
            if let Some(comment) = c.comment.as_deref().filter(|x| !x.is_empty()) {
                match d.comments {
                    Comments::MySql | Comments::Inline => {
                        sql.push_str(&format!(" COMMENT {}", self.literal(comment)))
                    }
                    Comments::Options => sql.push_str(&format!(
                        " OPTIONS(description={})",
                        self.literal(comment)
                    )),
                    Comments::On | Comments::Extended => {}
                    Comments::None => {
                        self.note(format!("O {} não guarda comentários", self.label()))
                    }
                }
            }
        }
        sql
    }

    /// The statements a draft needs before its CREATE TABLE (DuckDB's
    /// sequences).
    fn before_create(&self, table: &ModelTable, draft: &DraftTable) -> Vec<String> {
        if self.d.auto != Auto::Sequence {
            return Vec::new();
        }
        draft
            .columns
            .iter()
            .filter(|c| c.auto_increment)
            .map(|c| {
                format!(
                    "CREATE SEQUENCE IF NOT EXISTS {}",
                    self.qualified_name(&table.schema, &Self::sequence_name(table, c))
                )
            })
            .collect()
    }

    /// An inline foreign key of `child` (SQLite, DuckDB).
    fn inline_fks(&mut self, def: &Definition, child: &ModelTable) -> Vec<String> {
        if self.d.fk != Fk::Inline {
            return Vec::new();
        }
        let mut out = Vec::new();
        for j in &def.joins {
            let (parent, child_alias, pairs) = reference_ends(j);
            if child_alias != child.alias {
                continue;
            }
            let Some(parent_table) = def.tables.iter().find(|t| t.alias == parent) else {
                continue;
            };
            // SQLite's REFERENCES takes a table of the same database, unqualified.
            let target = if self.d.id == "sqlite" {
                self.q(&parent_table.table)
            } else {
                self.qualified(parent_table)
            };
            let mut sql = format!(
                "CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({})",
                self.q(&fk_name(j, child_alias, parent)),
                self.columns_list(pairs.iter().map(|(c, _)| *c)),
                target,
                self.columns_list(pairs.iter().map(|(_, p)| *p)),
            );
            sql.push_str(&self.actions(j));
            out.push(sql);
        }
        out
    }

    /// ` ON UPDATE …` / ` ON DELETE …` as the dialect takes them.
    fn actions(&mut self, j: &ModelJoin) -> String {
        let mut out = String::new();
        for (clause, action) in [("ON UPDATE", &j.on_update), ("ON DELETE", &j.on_delete)] {
            let Some(a) = action.as_deref().filter(|a| *a != "none") else { continue };
            let upper = a.to_ascii_uppercase().replace('_', " ");
            let ok = match self.d.actions {
                Actions::All => Some(upper.clone()),
                Actions::NoRestrict => Some(if upper == "RESTRICT" { "NO ACTION".into() } else { upper.clone() }),
                Actions::DeleteOnly => (clause == "ON DELETE" && (upper == "CASCADE" || upper == "SET NULL"))
                    .then(|| upper.clone()),
                Actions::RestrictOnly => (upper == "RESTRICT" || upper == "NO ACTION").then(|| upper.clone()),
                Actions::None => None,
            };
            match ok {
                Some(a) => out.push_str(&format!(" {clause} {a}")),
                None => self.note(format!(
                    "O {} não aceita {clause} {upper} nas chaves estrangeiras: ignorado",
                    self.label()
                )),
            }
        }
        out
    }

    /// What follows the column list: ClickHouse's engine, Delta, Hive's
    /// storage, Athena's location, ksqlDB's topic, the table comment.
    fn table_suffix(&mut self, table: &ModelTable, keys: &[&str], has_default: bool) -> String {
        let d = self.d;
        let comment = table
            .comment
            .as_deref()
            .filter(|c| self.comments && !c.is_empty())
            .map(|c| self.literal(c));
        let mut out = String::new();
        match d.key {
            Key::PrimaryIndex if !keys.is_empty() => {
                out.push_str(&format!(" PRIMARY INDEX {}", self.columns_list(keys)))
            }
            Key::UniqueKey if !keys.is_empty() => {
                out.push_str(&format!(" UNIQUE KEY ({})", self.columns_list(keys)))
            }
            _ => {}
        }
        match d.suffix {
            Suffix::ClickHouse => {
                let order = if keys.is_empty() {
                    "tuple()".to_string()
                } else {
                    format!("({})", self.columns_list(keys))
                };
                out.push_str(&format!(" ENGINE = MergeTree ORDER BY {order}"));
                if let Some(c) = &comment {
                    out.push_str(&format!(" COMMENT {c}"));
                }
            }
            Suffix::Delta => {
                out.push_str(" USING DELTA");
                if let Some(c) = &comment {
                    out.push_str(&format!(" COMMENT {c}"));
                }
                if has_default {
                    out.push_str(" TBLPROPERTIES ('delta.feature.allowColumnDefaults' = 'supported')");
                }
            }
            Suffix::Orc => {
                if let Some(c) = &comment {
                    out.push_str(&format!(" COMMENT {c}"));
                }
                out.push_str(" STORED AS ORC");
            }
            Suffix::Iceberg => {
                if let Some(c) = &comment {
                    out.push_str(&format!(" COMMENT {c}"));
                }
                out.push_str(&format!(
                    " LOCATION 's3://<bucket>/<path>/{}/{}/' TBLPROPERTIES ('table_type' = 'ICEBERG')",
                    table.schema, table.table
                ));
                self.note("Athena: troque <bucket>/<path> no LOCATION de cada tabela".to_string());
            }
            Suffix::Kafka => {
                out.push_str(&format!(
                    " WITH (KAFKA_TOPIC = {}, PARTITIONS = 1, VALUE_FORMAT = 'JSON')",
                    self.literal(&table.table)
                ));
            }
            Suffix::None => match (d.comments, &comment) {
                (Comments::MySql, Some(c)) => out.push_str(&format!(" COMMENT={c}")),
                (Comments::Inline, Some(c)) => out.push_str(&format!(" COMMENT {c}")),
                (Comments::Options, Some(c)) => {
                    out.push_str(&format!(" OPTIONS(description={c})"))
                }
                _ => {}
            },
        }
        out
    }

    /// The CREATE TABLE of a draft, its keys and checks inside, and the
    /// statements that must follow it at once (Databricks' checks).
    fn draft_create(
        &mut self,
        def: &Definition,
        table: &ModelTable,
        draft: &DraftTable,
    ) -> (String, Vec<String>) {
        let d = self.d;
        let mut constraints: Vec<String> = Vec::new();
        let mut parts: Vec<String> = Vec::new();
        for c in &draft.columns {
            let sql = self.column_sql(table, draft, c, &mut constraints);
            parts.push(sql);
        }
        let keys: Vec<&str> = draft
            .columns
            .iter()
            .filter(|c| c.primary_key)
            .map(|c| c.name.as_str())
            .collect();
        let auto_key = self.sqlite_auto_key(draft).is_some();
        if !keys.is_empty() && !auto_key {
            let name = draft.primary_key_name.as_deref().filter(|n| !n.is_empty());
            match d.key {
                Key::Constraint => parts.push(match name {
                    Some(n) => format!(
                        "CONSTRAINT {} PRIMARY KEY ({})",
                        self.q(n),
                        self.columns_list(&keys)
                    ),
                    None => format!("PRIMARY KEY ({})", self.columns_list(&keys)),
                }),
                Key::Informational(clause) => {
                    parts.push(format!("PRIMARY KEY ({}){clause}", self.columns_list(&keys)))
                }
                Key::OnColumn if keys.len() > 1 => self.note(format!(
                    "O {} aceita uma coluna de chave por tabela: {} usa a primeira",
                    self.label(),
                    table.table
                )),
                Key::OrderBy | Key::PrimaryIndex | Key::UniqueKey | Key::OnColumn => {}
                Key::None => self.note(format!("O {} não tem chave primária: ignorada", self.label())),
            }
        }
        for key in &draft.unique_keys {
            if d.unique {
                parts.push(format!(
                    "CONSTRAINT {} UNIQUE ({})",
                    self.q(&key.name),
                    self.columns_list(&key.columns)
                ));
            } else {
                self.note(format!("O {} não tem UNIQUE: ignorado", self.label()));
            }
        }
        let mut after: Vec<String> = Vec::new();
        let delta = d.suffix == Suffix::Delta;
        let mut checks: Vec<String> = draft
            .checks
            .iter()
            .map(|c| format!("{} CHECK ({})", self.q(&c.name), c.expression))
            .collect();
        checks.append(&mut constraints);
        for check in checks {
            if !d.check {
                self.note(format!("O {} não tem CHECK: ignorado", self.label()));
            } else if delta {
                after.push(format!(
                    "ALTER TABLE {} ADD CONSTRAINT {check}",
                    self.qualified(table)
                ));
            } else {
                parts.push(format!("CONSTRAINT {check}"));
            }
        }
        parts.extend(self.inline_fks(def, table));
        if d.index == Index::Inline {
            for index in &draft.indexes {
                parts_suffix_index(self, &mut after, index, table);
            }
        }
        let has_default = draft.columns.iter().any(|c| c.default.as_deref().is_some_and(|x| !x.trim().is_empty()));
        let mut keys_for_suffix = keys.clone();
        if d.key == Key::OnColumn {
            keys_for_suffix.clear();
        }
        let suffix = self.table_suffix(table, &keys_for_suffix, has_default);
        let mut sql = format!(
            "CREATE TABLE {} ({})",
            self.qualified(table),
            parts.join(", ")
        );
        if d.index == Index::Inline {
            // Cube Store: `INDEX n (cols)` follow the column list.
            sql.push_str(&after.join(""));
            after.clear();
        }
        sql.push_str(&suffix);
        (sql, after)
    }

    /// The CREATE TABLE of a table the connection has, from its columns.
    fn existing_create(
        &mut self,
        table: &ModelTable,
        columns: &[cubedriver::SchemaColumn],
        key: Option<&str>,
    ) -> String {
        let mut parts: Vec<String> = columns
            .iter()
            .map(|c| format!("{} {}", self.q(&c.name), c.type_))
            .collect();
        let keys: Vec<&str> = key.into_iter().collect();
        if let Some(key) = key {
            match self.d.key {
                Key::Constraint => parts.push(format!("PRIMARY KEY ({})", self.q(key))),
                Key::Informational(clause) => {
                    parts.push(format!("PRIMARY KEY ({}){clause}", self.q(key)))
                }
                _ => {}
            }
        }
        let suffix = self.table_suffix(table, &keys, false);
        format!(
            "CREATE TABLE {} ({}){suffix}",
            self.qualified(table),
            parts.join(", ")
        )
    }

    fn index_statements(&mut self, table: &ModelTable, draft: &DraftTable) -> Vec<String> {
        let d = self.d;
        let mut out = Vec::new();
        for index in &draft.indexes {
            if index.columns.iter().any(|c| c.desc) && !d.index_desc && d.index != Index::None {
                self.note(format!("O {} não ordena índices com DESC: ignorado", self.label()));
            }
            let columns: Vec<String> = index
                .columns
                .iter()
                .map(|c| {
                    let q = self.q(&c.name);
                    if c.desc && d.index_desc {
                        format!("{q} DESC")
                    } else {
                        q
                    }
                })
                .collect();
            let unique = if index.unique { "UNIQUE " } else { "" };
            match d.index {
                Index::Create => out.push(format!(
                    "CREATE {unique}INDEX {} ON {} ({})",
                    self.q(&index.name),
                    self.qualified(table),
                    columns.join(", ")
                )),
                Index::SchemaOnName => out.push(format!(
                    "CREATE {unique}INDEX {} ON {} ({})",
                    self.qualified_name(&table.schema, &index.name),
                    self.q(&table.table),
                    columns.join(", ")
                )),
                Index::CreatePlain => {
                    if index.unique {
                        self.note(format!("O {} não tem índice único: criado como índice comum", self.label()));
                    }
                    out.push(format!(
                        "CREATE INDEX {} ON {} ({})",
                        self.q(&index.name),
                        self.qualified(table),
                        columns.join(", ")
                    ))
                }
                Index::Skipping => {
                    if index.unique {
                        self.note(format!("O {} não tem índice único: criado como índice de salto", self.label()));
                    }
                    out.push(format!(
                        "ALTER TABLE {} ADD INDEX {} ({}) TYPE minmax GRANULARITY 1",
                        self.qualified(table),
                        self.q(&index.name),
                        columns.join(", ")
                    ))
                }
                Index::Inline => {}
                Index::None => {
                    self.note(format!("O {} não tem índices: ignorados", self.label()));
                }
            }
        }
        out
    }

    fn fk_statement(&mut self, def: &Definition, j: &ModelJoin) -> Option<(String, String)> {
        let (parent, child, pairs) = reference_ends(j);
        let find = |alias: &str| def.tables.iter().find(|t| t.alias == alias);
        let (parent_table, child_table) = (find(parent)?, find(child)?);
        let name = fk_name(j, child, parent);
        let head = || {
            format!(
                "ALTER TABLE {} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({})",
                self.qualified(child_table),
                self.q(&name),
                self.columns_list(pairs.iter().map(|(c, _)| *c)),
                self.qualified(parent_table),
                self.columns_list(pairs.iter().map(|(_, p)| *p)),
            )
        };
        match self.d.fk {
            Fk::Alter => {
                let mut sql = head();
                sql.push_str(&self.actions(j));
                Some((child.to_string(), sql))
            }
            Fk::AlterInformational(clause) => {
                let sql = format!("{}{clause}", head());
                if j.on_update.as_deref().is_some_and(|a| a != "none")
                    || j.on_delete.as_deref().is_some_and(|a| a != "none")
                {
                    self.note(format!(
                        "O {} não executa ações nas chaves estrangeiras: ignoradas",
                        self.label()
                    ));
                }
                Some((child.to_string(), sql))
            }
            Fk::Inline => None,
            Fk::None => {
                self.note(format!(
                    "O {} não tem chaves estrangeiras: as referências ficam só no model",
                    self.label()
                ));
                None
            }
        }
    }

    fn drop_fk(&self, def: &Definition, j: &ModelJoin) -> Option<String> {
        if !matches!(self.d.fk, Fk::Alter | Fk::AlterInformational(_)) {
            return None;
        }
        let (parent, child, _) = reference_ends(j);
        let child_table = def.tables.iter().find(|t| t.alias == child)?;
        let name = fk_name(j, child, parent);
        Some(format!(
            "ALTER TABLE {} DROP {} {}",
            self.qualified(child_table),
            if self.d.id == "mysql" {
                "FOREIGN KEY"
            } else {
                "CONSTRAINT"
            },
            self.q(&name)
        ))
    }

    fn drop_table(&self, kind: &str, name: String) -> String {
        match self.d.drop {
            Drop::IfExists => format!("DROP {kind} IF EXISTS {name}"),
            Drop::Plain => format!("DROP {kind} {name}"),
            Drop::OracleBlock => {
                let cascade = if kind == "TABLE" { " CASCADE CONSTRAINTS" } else { "" };
                format!(
                    "BEGIN EXECUTE IMMEDIATE 'DROP {kind} {}{cascade}'; EXCEPTION WHEN OTHERS THEN IF SQLCODE != -942 THEN RAISE; END IF; END",
                    name.replace('\'', "''")
                )
            }
        }
    }

    /// The comment statements of a table and its columns (COMMENT ON, or
    /// SQL Server's extended properties).
    fn comment_statements(&self, table: &ModelTable) -> Vec<String> {
        let mut out = Vec::new();
        let table_comment = table.comment.as_deref().filter(|c| !c.is_empty());
        let columns: Vec<(&str, &str)> = table
            .draft
            .iter()
            .flat_map(|d| d.columns.iter())
            .filter_map(|c| {
                c.comment
                    .as_deref()
                    .filter(|x| !x.is_empty())
                    .map(|x| (c.name.as_str(), x))
            })
            .collect();
        match self.d.comments {
            Comments::On => {
                let name = self.qualified(table);
                if let Some(c) = table_comment {
                    out.push(format!("COMMENT ON TABLE {name} IS {}", self.literal(c)));
                }
                for (column, c) in columns {
                    out.push(format!(
                        "COMMENT ON COLUMN {name}.{} IS {}",
                        self.q(column),
                        self.literal(c)
                    ));
                }
            }
            Comments::Extended => {
                if let Some(c) = table_comment {
                    out.push(self.extended(c, &table.schema, "TABLE", &table.table, None));
                }
                for (column, c) in columns {
                    out.push(self.extended(c, &table.schema, "TABLE", &table.table, Some(column)));
                }
            }
            _ => {}
        }
        out
    }

    fn extended(&self, value: &str, schema: &str, kind: &str, name: &str, column: Option<&str>) -> String {
        let n = |s: &str| format!("N'{}'", s.replace('\'', "''"));
        let mut sql = format!(
            "EXEC sys.sp_addextendedproperty @name = N'MS_Description', @value = {}, @level0type = N'SCHEMA', @level0name = {}, @level1type = N'{kind}', @level1name = {}",
            n(value),
            n(schema),
            n(name)
        );
        if let Some(c) = column {
            sql.push_str(&format!(", @level2type = N'COLUMN', @level2name = {}", n(c)));
        }
        sql
    }

    fn view_comment(&self, view: &ModelView) -> Option<String> {
        let c = view.comment.as_deref().filter(|c| !c.is_empty())?;
        match self.d.comments {
            Comments::On => Some(format!(
                "COMMENT ON {} {} IS {}",
                if self.d.id == "oracle" { "TABLE" } else { "VIEW" },
                self.qualified_name(&view.schema, &view.name),
                self.literal(c)
            )),
            Comments::Extended => Some(self.extended(c, &view.schema, "VIEW", &view.name, None)),
            _ => None,
        }
    }

    /// `CREATE VIEW` of a view; its query is written as typed, trimmed.
    fn view_create(&self, view: &ModelView) -> String {
        format!(
            "CREATE VIEW {} AS {}",
            self.qualified_name(&view.schema, &view.name),
            view.sql.trim()
        )
    }

    fn check_draft(&self, table: &ModelTable, draft: &DraftTable) -> Result<(), String> {
        validate_draft_for_ddl(table, draft)?;
        if self.d.id == "mysql" {
            mysql_text_key(table, draft)?;
        }
        if self.d.key == Key::OnColumn && !draft.columns.iter().any(|c| c.primary_key) {
            return Err(format!(
                "No {} a tabela {}.{} precisa de uma chave primária",
                self.label(),
                table.schema,
                table.table
            ));
        }
        Ok(())
    }

    /// The notes as SQL comments, for the top of a script.
    fn notes_header(&self) -> String {
        if self.notes.is_empty() {
            return String::new();
        }
        let mut out = format!("-- {}: o que o banco não tem ficou de fora\n", self.label());
        for n in &self.notes {
            out.push_str(&format!("-- * {n}\n"));
        }
        out.push('\n');
        out
    }
}

/// Cube Store's `INDEX n (cols)` after the column list.
fn parts_suffix_index(g: &Gen<'_>, after: &mut Vec<String>, index: &super::cube_model::TableIndex, _table: &ModelTable) {
    after.push(format!(
        " INDEX {} ({})",
        g.q(&index.name),
        g.columns_list(index.columns.iter().map(|c| c.name.as_str()))
    ));
}

/// The views in dependency order (what a view reads comes first), keeping
/// the definition order among independent ones. Dependencies outside
/// `views` are ignored. A cycle is an error naming the views left.
fn view_order<'a>(views: &[&'a ModelView]) -> Result<Vec<&'a ModelView>, String> {
    let ids: HashSet<&str> = views.iter().map(|v| v.id.as_str()).collect();
    let mut done: HashSet<&str> = HashSet::new();
    let mut left: Vec<&ModelView> = views.to_vec();
    let mut out = Vec::with_capacity(views.len());
    while !left.is_empty() {
        let ready = left.iter().position(|v| {
            v.depends_on_views
                .iter()
                .all(|d| !ids.contains(d.as_str()) || done.contains(d.as_str()))
        });
        match ready {
            Some(i) => {
                let v = left.remove(i);
                done.insert(v.id.as_str());
                out.push(v);
            }
            None => {
                let mut names: Vec<&str> = left.iter().map(|v| v.name.as_str()).collect();
                names.sort();
                return Err(format!(
                    "As views {} dependem umas das outras",
                    names.join(", ")
                ));
            }
        }
    }
    Ok(out)
}

/// The parent (the "one" side) and child aliases of a reference, with the
/// column pairs as (child, parent).
fn reference_ends(j: &ModelJoin) -> (&str, &str, Vec<(&str, &str)>) {
    let parent_is_from = j.relationship == "one_to_many";
    let mut pairs = vec![if parent_is_from {
        (j.to.column.as_str(), j.from.column.as_str())
    } else {
        (j.from.column.as_str(), j.to.column.as_str())
    }];
    for p in &j.pairs {
        pairs.push(if parent_is_from {
            (p.to.as_str(), p.from.as_str())
        } else {
            (p.from.as_str(), p.to.as_str())
        });
    }
    if parent_is_from {
        (j.from.alias.as_str(), j.to.alias.as_str(), pairs)
    } else {
        (j.to.alias.as_str(), j.from.alias.as_str(), pairs)
    }
}

fn fk_name(j: &ModelJoin, child: &str, parent: &str) -> String {
    j.name
        .clone()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("fk_{child}_{parent}"))
}

fn mysql_text_key(table: &ModelTable, draft: &DraftTable) -> Result<(), String> {
    if let Some(c) = draft
        .columns
        .iter()
        .find(|c| c.primary_key && c.kind == "text")
    {
        return Err(format!(
            "No MySQL a chave primária {}.{}.{} não pode ser text: use integer, bigint ou outro tipo",
            table.schema, table.table, c.name
        ));
    }
    Ok(())
}

fn comments_on(def: &Definition) -> bool {
    def.settings
        .as_ref()
        .and_then(|s| s.sql_generation.as_ref())
        .and_then(|g| g.comments)
        .unwrap_or(true)
}

/// The tables in an order that creates a parent before its children, for
/// a dialect whose foreign keys sit inside the CREATE TABLE; the
/// definition order otherwise, and among independent tables.
fn creation_order<'a>(d: &Dialect, def: &'a Definition, tables: Vec<&'a ModelTable>) -> Vec<&'a ModelTable> {
    if d.fk != Fk::Inline {
        return tables;
    }
    let aliases: HashSet<&str> = tables.iter().map(|t| t.alias.as_str()).collect();
    let parents_of = |alias: &str| -> Vec<&str> {
        def.joins
            .iter()
            .map(reference_ends)
            .filter(|(p, c, _)| *c == alias && *p != alias && aliases.contains(p))
            .map(|(p, _, _)| p)
            .collect()
    };
    let mut done: HashSet<&str> = HashSet::new();
    let mut left = tables;
    let mut out = Vec::with_capacity(left.len());
    while !left.is_empty() {
        let i = left
            .iter()
            .position(|t| parents_of(&t.alias).iter().all(|p| done.contains(p)))
            .unwrap_or(0);
        let t = left.remove(i);
        done.insert(t.alias.as_str());
        out.push(t);
    }
    out
}

/// What "Create in database" runs: each draft's own scripts around its
/// CREATE TABLE, then the indexes, the foreign keys whose child is a draft,
/// and the comments.
pub fn create_statements(def: &Definition, db_type: &str) -> Result<Vec<Statement>, String> {
    create_plan(def, db_type).map(|(statements, _)| statements)
}

/// [`create_statements`] with the notes of what the database leaves out.
pub fn create_plan(def: &Definition, db_type: &str) -> Result<(Vec<Statement>, Vec<String>), String> {
    let mut g = Gen::new(db_type, comments_on(def))?;
    if let Some(why) = g.d.no_create {
        return Err(why.to_string());
    }
    let mut out = Vec::new();
    let drafts: Vec<&ModelTable> = def.tables.iter().filter(|t| t.draft.is_some()).collect();
    for table in creation_order(g.d, def, drafts) {
        let Some(draft) = &table.draft else { continue };
        g.check_draft(table, draft)?;
        let mut group: Vec<String> = Vec::new();
        if let Some(s) = draft.script_before.as_deref().filter(|s| !s.trim().is_empty()) {
            group.push(s.to_string());
        }
        group.extend(g.before_create(table, draft));
        let create_at = group.len();
        let (create, after) = g.draft_create(def, table, draft);
        group.push(create);
        group.extend(after);
        if let Some(s) = draft.script_after.as_deref().filter(|s| !s.trim().is_empty()) {
            group.push(s.to_string());
        }
        group.extend(g.index_statements(table, draft));
        if g.comments {
            group.extend(g.comment_statements(table));
        }
        let name = format!("{}.{}", table.schema, table.table);
        out.extend(group.into_iter().enumerate().map(|(i, sql)| Statement {
            table: Some(name.clone()),
            sql,
            creates: i == create_at,
        }));
    }
    let drafts: HashSet<&str> = def
        .tables
        .iter()
        .filter(|t| t.draft.is_some())
        .map(|t| t.alias.as_str())
        .collect();
    for j in &def.joins {
        let (_, child, _) = reference_ends(j);
        if !drafts.contains(child) {
            continue;
        }
        if let Some((_, sql)) = g.fk_statement(def, j) {
            out.push(Statement {
                table: None,
                sql,
                creates: false,
            });
        }
    }
    Ok((out, g.notes))
}

fn optional(text: Option<&String>) -> Option<&str> {
    text.map(|s| s.as_str()).filter(|s| !s.trim().is_empty())
}

/// The whole script of the model, as Redgate's "Generate SQL": the drop
/// part first when asked, then the creates, each statement ending in `;`,
/// under a note of what the database leaves out.
pub fn script(
    def: &Definition,
    columns: &DatabaseStructure,
    db_type: &str,
    opts: &ScriptOptions,
) -> Result<String, String> {
    let mut g = Gen::new(db_type, opts.comments)?;
    validate(def)?;
    let included: Vec<&ModelTable> = def
        .tables
        .iter()
        .filter(|t| !opts.only_new || t.draft.is_some())
        .filter(|t| opts.only.as_ref().is_none_or(|only| only.contains(&t.alias)))
        .collect();
    let included = creation_order(g.d, def, included);
    let aliases: HashSet<&str> = included.iter().map(|t| t.alias.as_str()).collect();
    let joins: Vec<&ModelJoin> = def
        .joins
        .iter()
        .filter(|j| {
            let (parent, child, _) = reference_ends(j);
            aliases.contains(parent) && aliases.contains(child)
        })
        .collect();
    let views: Vec<&ModelView> = def
        .views
        .iter()
        .filter(|v| opts.only.as_ref().is_none_or(|only| only.contains(&format!("view:{}", v.id))))
        .collect();
    let views = view_order(&views)?;
    if !views.is_empty() && !g.d.views {
        g.note(format!("O {} não tem views: ficaram de fora", g.label()));
    }
    let views: Vec<&ModelView> = if g.d.views { views } else { Vec::new() };
    let scripts = def.settings.as_ref().and_then(|s| s.scripts.as_ref());
    let mut out: Vec<String> = Vec::new();
    let push = |out: &mut Vec<String>, sql: &str| {
        out.push(format!("{};\n", sql.trim_end().trim_end_matches(';')))
    };

    if opts.drop {
        if let Some(s) = optional(scripts.and_then(|s| s.drop_before.as_ref())) {
            push(&mut out, s);
        }
        for v in views.iter().rev() {
            let sql = g.drop_table("VIEW", g.qualified_name(&v.schema, &v.name));
            push(&mut out, &sql);
        }
        for j in &joins {
            if let Some(sql) = g.drop_fk(def, j) {
                push(&mut out, &sql);
            }
        }
        for table in included.iter().rev() {
            let sql = g.drop_table("TABLE", g.qualified(table));
            push(&mut out, &sql);
        }
        if let Some(s) = optional(scripts.and_then(|s| s.drop_after.as_ref())) {
            push(&mut out, s);
        }
    }
    if let Some(s) = optional(scripts.and_then(|s| s.create_before.as_ref())) {
        push(&mut out, s);
    }
    let mut after: Vec<String> = Vec::new();
    for table in &included {
        match &table.draft {
            Some(draft) => {
                g.check_draft(table, draft)?;
                if let Some(s) = optional(draft.script_before.as_ref()) {
                    push(&mut out, s);
                }
                for sql in g.before_create(table, draft) {
                    push(&mut out, &sql);
                }
                let (create, following) = g.draft_create(def, table, draft);
                push(&mut out, &create);
                for sql in following {
                    push(&mut out, &sql);
                }
                if let Some(s) = optional(draft.script_after.as_ref()) {
                    push(&mut out, s);
                }
                after.extend(g.index_statements(table, draft));
            }
            None => {
                let cols = columns
                    .get(&table.schema)
                    .and_then(|t| t.get(&table.table))
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let key = (def.primary.as_deref() == Some(table.alias.as_str()))
                    .then_some(def.primary_key.as_deref())
                    .flatten();
                let sql = g.existing_create(table, cols, key);
                push(&mut out, &sql);
            }
        }
        if opts.comments {
            after.extend(g.comment_statements(table));
        }
    }
    for sql in &after {
        push(&mut out, sql);
    }
    for j in &joins {
        if let Some((_, sql)) = g.fk_statement(def, j) {
            push(&mut out, &sql);
        }
    }
    for v in &views {
        if let Some(s) = optional(v.script_before.as_ref()) {
            push(&mut out, s);
        }
        push(&mut out, &g.view_create(v));
        if let Some(s) = optional(v.script_after.as_ref()) {
            push(&mut out, s);
        }
        if opts.comments {
            if let Some(sql) = g.view_comment(v) {
                push(&mut out, &sql);
            }
        }
    }
    if let Some(s) = optional(scripts.and_then(|s| s.create_after.as_ref())) {
        push(&mut out, s);
    }
    Ok(format!("{}{}", g.notes_header(), out.concat()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rbac::cube_model::{
        ColumnPair, End, IndexColumn, ModelScripts, Settings, TableCheck, TableIndex, UniqueKey,
    };

    fn col(name: &str, kind: &str) -> DraftColumn {
        DraftColumn {
            name: name.into(),
            kind: kind.into(),
            ..Default::default()
        }
    }

    fn items() -> ModelTable {
        ModelTable {
            alias: "items".into(),
            schema: "shop".into(),
            table: "items".into(),
            draft: Some(DraftTable {
                columns: vec![
                    DraftColumn {
                        primary_key: true,
                        auto_increment: true,
                        ..col("id", "integer")
                    },
                    col("sku", "varchar(32)"),
                    DraftColumn {
                        nullable: true,
                        default: Some("0".into()),
                        check: Some("price >= 0".into()),
                        ..col("price", "numeric(10,2)")
                    },
                ],
                unique_keys: vec![UniqueKey {
                    name: "uk_sku".into(),
                    columns: vec!["sku".into()],
                }],
                indexes: vec![TableIndex {
                    name: "ix_price".into(),
                    columns: vec![IndexColumn {
                        name: "price".into(),
                        desc: true,
                    }],
                    unique: false,
                }],
                checks: vec![TableCheck {
                    name: "ck_x".into(),
                    expression: "price < 1000".into(),
                }],
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn existing(alias: &str) -> ModelTable {
        ModelTable {
            alias: alias.into(),
            schema: "shop".into(),
            table: alias.into(),
            ..Default::default()
        }
    }

    fn orders_customers() -> Definition {
        Definition {
            data_source: "pg".into(),
            tables: vec![
                existing("customers"),
                ModelTable {
                    draft: Some(DraftTable {
                        columns: vec![
                            DraftColumn {
                                primary_key: true,
                                ..col("id", "integer")
                            },
                            col("customer_id", "integer"),
                        ],
                        ..Default::default()
                    }),
                    ..existing("orders")
                },
            ],
            joins: vec![ModelJoin {
                from: End {
                    alias: "orders".into(),
                    column: "customer_id".into(),
                },
                to: End {
                    alias: "customers".into(),
                    column: "id".into(),
                },
                relationship: "many_to_one".into(),
                join: "left".into(),
                on_delete: Some("cascade".into()),
                on_update: Some("none".into()),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn structure() -> DatabaseStructure {
        let mut tables = std::collections::BTreeMap::new();
        tables.insert(
            "customers".to_string(),
            vec![cubedriver::SchemaColumn {
                name: "id".into(),
                type_: "integer".into(),
                attributes: vec![],
                foreign_keys: vec![],
            }],
        );
        DatabaseStructure::from([("shop".to_string(), tables)])
    }

    fn with(tables: Vec<ModelTable>) -> Definition {
        Definition {
            data_source: "pg".into(),
            tables,
            ..Default::default()
        }
    }

    /// Two designed tables with every part a table can have, a reference
    /// between them and a commented view: what each dialect is tried on.
    fn rich() -> Definition {
        let customers = ModelTable {
            alias: "customers".into(),
            schema: "shop".into(),
            table: "customers".into(),
            comment: Some("Who buys".into()),
            draft: Some(DraftTable {
                columns: vec![
                    DraftColumn { primary_key: true, auto_increment: true, ..col("id", "integer") },
                    DraftColumn { comment: Some("Full name".into()), ..col("name", "text") },
                    DraftColumn { nullable: true, ..col("email", "text") },
                ],
                unique_keys: vec![UniqueKey { name: "uk_email".into(), columns: vec!["email".into()] }],
                ..Default::default()
            }),
            ..Default::default()
        };
        let orders = ModelTable {
            alias: "orders".into(),
            schema: "shop".into(),
            table: "orders".into(),
            draft: Some(DraftTable {
                columns: vec![
                    DraftColumn { primary_key: true, ..col("id", "bigint") },
                    col("customer_id", "integer"),
                    DraftColumn {
                        default: Some("0".into()),
                        check: Some("total >= 0".into()),
                        ..col("total", "numeric")
                    },
                    DraftColumn { nullable: true, ..col("created_at", "timestamp") },
                ],
                indexes: vec![TableIndex {
                    name: "ix_created".into(),
                    columns: vec![IndexColumn { name: "created_at".into(), desc: true }],
                    unique: false,
                }],
                checks: vec![TableCheck { name: "ck_total".into(), expression: "total < 1000000".into() }],
                ..Default::default()
            }),
            ..Default::default()
        };
        Definition {
            data_source: "x".into(),
            // The child first: dialects whose foreign keys sit inside the
            // CREATE TABLE create the parent before it.
            tables: vec![orders, customers],
            joins: vec![ModelJoin {
                from: End { alias: "orders".into(), column: "customer_id".into() },
                to: End { alias: "customers".into(), column: "id".into() },
                relationship: "many_to_one".into(),
                join: "left".into(),
                on_delete: Some("cascade".into()),
                ..Default::default()
            }],
            views: vec![ModelView {
                comment: Some("Big orders".into()),
                ..view("v1", "SELECT id FROM shop.orders WHERE total > 100")
            }],
            ..Default::default()
        }
    }

    #[test]
    fn every_dialect_writes_the_rich_model() {
        let opts = ScriptOptions { drop: true, comments: true, ..Default::default() };
        // (db type, fragments the script has, fragments it must not have)
        let cases: &[(&str, &[&str], &[&str])] = &[
            ("postgres", &["\"id\" integer GENERATED BY DEFAULT AS IDENTITY NOT NULL", "COMMENT ON TABLE", "ON DELETE CASCADE", "CREATE INDEX"], &["-- "]),
            ("mysql", &["`id` int AUTO_INCREMENT NOT NULL", " COMMENT='Who buys'", "DROP FOREIGN KEY"], &["COMMENT ON"]),
            ("redshift", &["IDENTITY(1,1)", "COMMENT ON TABLE", "não tem CHECK", "não tem índices"], &["CREATE INDEX", "CHECK ("]),
            ("materialize", &["CREATE INDEX \"ix_created\"", "não tem chave primária", "não tem chaves estrangeiras"], &["PRIMARY KEY", "FOREIGN KEY"]),
            ("questdb", &["CREATE TABLE \"orders\"", "\"total\" double"], &["\"shop\".", "NOT NULL"]),
            ("crate", &["timestamp with time zone", "PRIMARY KEY"], &["FOREIGN KEY", "CREATE INDEX"]),
            ("vertica", &["IDENTITY(1,1)", "FOREIGN KEY", "não aceita ON DELETE CASCADE"], &["ON DELETE"]),
            ("clickhouse", &["ENGINE = MergeTree ORDER BY (`id`)", "`created_at` Nullable(DateTime64(3))", "CONSTRAINT `chk_orders_total` CHECK (total >= 0)", "ADD INDEX `ix_created` (`created_at`) TYPE minmax", " COMMENT 'Who buys'", "`total` Decimal(18,4) DEFAULT 0"], &["FOREIGN KEY", "NOT NULL", "UNIQUE"]),
            ("bigquery", &["PRIMARY KEY (`id`) NOT ENFORCED", "REFERENCES `shop`.`customers` (`id`) NOT ENFORCED", "OPTIONS(description='Full name')", "`id` INT64"], &["CHECK", "CASCADE"]),
            ("snowflake", &["AUTOINCREMENT", "TIMESTAMP_NTZ", "COMMENT ON COLUMN", "ON DELETE CASCADE"], &["CHECK", "CREATE INDEX"]),
            ("mssql", &["IDENTITY(1,1)", "sp_addextendedproperty", "datetime2", "ON DELETE CASCADE"], &["COMMENT ON"]),
            ("oracle", &["GENERATED BY DEFAULT AS IDENTITY", "DEFAULT 0 NOT NULL", "BEGIN EXECUTE IMMEDIATE 'DROP TABLE", "COMMENT ON TABLE \"shop\".\"v1\"", "ON DELETE CASCADE"], &["IF EXISTS"]),
            ("sqlite", &["\"id\" INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL", "FOREIGN KEY (\"customer_id\") REFERENCES \"customers\" (\"id\") ON DELETE CASCADE", "CREATE INDEX \"shop\".\"ix_created\" ON \"orders\"", "DEFAULT (0)"], &["ALTER TABLE", "COMMENT"]),
            ("duckdb", &["CREATE SEQUENCE IF NOT EXISTS \"shop\".\"seq_customers_id\"", "DEFAULT nextval(", "FOREIGN KEY", "não aceita ON DELETE CASCADE"], &["ALTER TABLE"]),
            ("databricks-jdbc", &["USING DELTA", "ALTER TABLE `shop`.`orders` ADD CONSTRAINT `ck_total` CHECK", "GENERATED BY DEFAULT AS IDENTITY", "delta.feature.allowColumnDefaults", "COMMENT 'Full name'"], &["UNIQUE ("]),
            ("hive", &["STORED AS ORC", "PRIMARY KEY (`id`) DISABLE NOVALIDATE", "FOREIGN KEY (`customer_id`) REFERENCES `shop`.`customers` (`id`) DISABLE NOVALIDATE"], &["NOT NULL", "DEFAULT"]),
            ("trino", &["\"name\" VARCHAR NOT NULL COMMENT 'Full name'", "não tem chave primária"], &["PRIMARY KEY", "DEFAULT"]),
            ("athena", &["TBLPROPERTIES ('table_type' = 'ICEBERG')", "LOCATION 's3://<bucket>/<path>/shop/orders/'"], &["PRIMARY KEY"]),
            ("dremio", &["CREATE TABLE \"shop\".\"orders\""], &["PRIMARY KEY"]),
            ("firebolt", &["PRIMARY INDEX \"id\""], &["PRIMARY KEY", "FOREIGN KEY"]),
            ("cubestore", &[" INDEX \"ix_created\" (\"created_at\")", "DROP TABLE \"shop\"", "não tem chave primária"], &["CREATE VIEW", "IF EXISTS", "UNIQUE KEY"]),
            ("ksql", &["\"id\" BIGINT PRIMARY KEY", "WITH (KAFKA_TOPIC = 'orders', PARTITIONS = 1, VALUE_FORMAT = 'JSON')"], &["\"shop\".", "CREATE VIEW"]),
        ];
        for (db, has, lacks) in cases {
            let sql = script(&rich(), &structure(), db, &opts).unwrap_or_else(|e| panic!("{db}: {e}"));
            for h in *has {
                assert!(sql.contains(h), "{db}: no {h}\n{sql}");
            }
            let statements: String = sql.lines().filter(|l| !l.starts_with("--")).collect::<Vec<_>>().join("\n");
            for l in *lacks {
                assert!(!statements.contains(l), "{db}: has {l}\n{sql}");
            }
            let d = dialects::of(db).unwrap();
            if d.no_create.is_none() {
                let (statements, notes) = create_plan(&rich(), db).unwrap_or_else(|e| panic!("{db}: {e}"));
                assert!(statements.iter().filter(|s| s.creates).count() == 2, "{db}");
                assert!(statements.iter().all(|s| !s.sql.starts_with("--")), "{db}");
                assert_eq!(notes.is_empty(), !sql.starts_with("--"), "{db}");
            }
        }
        // Every connector is covered.
        for d in dialects::DIALECTS.iter().filter(|d| d.no_sql.is_none()) {
            assert!(cases.iter().any(|(db, _, _)| d.db_types.contains(db)), "{}", d.id);
        }
    }

    #[test]
    fn inline_foreign_keys_create_the_parent_first() {
        for db in ["sqlite", "duckdb"] {
            let statements = create_statements(&rich(), db).unwrap();
            let creates: Vec<&str> = statements.iter().filter(|s| s.creates).filter_map(|s| s.table.as_deref()).collect();
            assert_eq!(creates, ["shop.customers", "shop.orders"], "{db}");
        }
        let statements = create_statements(&rich(), "postgres").unwrap();
        let creates: Vec<&str> = statements.iter().filter(|s| s.creates).filter_map(|s| s.table.as_deref()).collect();
        assert_eq!(creates, ["shop.orders", "shop.customers"]);
    }

    fn all(statements: &[Statement]) -> String {
        statements
            .iter()
            .map(|s| s.sql.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn postgres_table_with_keys_indexes_checks() {
        let sql = all(&create_statements(&with(vec![items()]), "postgres").unwrap());
        for part in [
            r#""id" integer GENERATED BY DEFAULT AS IDENTITY NOT NULL"#,
            r#""sku" varchar(32) NOT NULL"#,
            r#""price" numeric(10,2) DEFAULT 0 CHECK (price >= 0)"#,
            r#"PRIMARY KEY ("id")"#,
            r#"CONSTRAINT "uk_sku" UNIQUE ("sku")"#,
            r#"CONSTRAINT "ck_x" CHECK (price < 1000)"#,
            r#"CREATE INDEX "ix_price" ON "shop"."items" ("price" DESC)"#,
        ] {
            assert!(sql.contains(part), "{part} not in {sql}");
        }
    }

    #[test]
    fn mysql_unsigned_auto_increment() {
        let mut t = items();
        let draft = t.draft.as_mut().unwrap();
        draft.columns[0].kind = "int".into();
        draft.columns[0].unsigned = true;
        draft.primary_key_name = Some("pk_items".into());
        let sql = all(&create_statements(&with(vec![t]), "mysql").unwrap());
        assert!(sql.contains("`id` int UNSIGNED AUTO_INCREMENT NOT NULL"), "{sql}");
        assert!(sql.contains("CONSTRAINT `pk_items` PRIMARY KEY (`id`)"), "{sql}");
    }

    #[test]
    fn fk_with_actions() {
        let statements = create_statements(&orders_customers(), "postgres").unwrap();
        let fk = statements.last().unwrap();
        assert_eq!(fk.table, None);
        assert_eq!(
            fk.sql,
            r#"ALTER TABLE "shop"."orders" ADD CONSTRAINT "fk_orders_customers" FOREIGN KEY ("customer_id") REFERENCES "shop"."customers" ("id") ON DELETE CASCADE"#
        );
        let mut def = orders_customers();
        def.joins[0].name = Some("fk_named".into());
        def.joins[0].pairs = vec![ColumnPair {
            from: "customer_id".into(),
            to: "id".into(),
        }];
        let sql = all(&create_statements(&def, "postgres").unwrap());
        assert!(sql.contains(r#"CONSTRAINT "fk_named" FOREIGN KEY ("customer_id", "customer_id")"#), "{sql}");
    }

    #[test]
    fn script_options() {
        let mut def = orders_customers();
        def.tables.push(items());
        def.settings = Some(Settings {
            scripts: Some(ModelScripts {
                create_before: Some("-- create begins".into()),
                create_after: Some("-- create ends".into()),
                drop_before: Some("-- drop begins".into()),
                drop_after: Some("-- drop ends".into()),
            }),
            ..Default::default()
        });
        let full = script(&def, &structure(), "postgres", &ScriptOptions::default()).unwrap();
        assert!(full.starts_with("-- create begins;\n"), "{full}");
        assert!(full.ends_with("-- create ends;\n"), "{full}");
        assert!(full.contains(r#"CREATE TABLE "shop"."customers" ("id" integer);"#), "{full}");
        assert!(full.contains("FOREIGN KEY"), "{full}");
        assert!(!full.contains("DROP"), "{full}");

        let only_new = script(
            &def,
            &structure(),
            "postgres",
            &ScriptOptions {
                only_new: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!only_new.contains(r#""shop"."customers" ("#), "{only_new}");
        // The parent is left out, so is the reference.
        assert!(!only_new.contains("FOREIGN KEY"), "{only_new}");

        let only = script(
            &def,
            &structure(),
            "postgres",
            &ScriptOptions {
                only: Some(vec!["items".into()]),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(only.contains(r#"CREATE TABLE "shop"."items""#), "{only}");
        assert!(!only.contains(r#"CREATE TABLE "shop"."orders""#), "{only}");

        let dropping = script(
            &def,
            &structure(),
            "postgres",
            &ScriptOptions {
                drop: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(dropping.starts_with("-- drop begins;\n"), "{dropping}");
        let drop_at = dropping.find(r#"DROP TABLE IF EXISTS "shop"."orders""#).unwrap();
        let fk_drop = dropping.find(r#"DROP CONSTRAINT "fk_orders_customers""#).unwrap();
        let create_at = dropping.find("CREATE TABLE").unwrap();
        assert!(fk_drop < drop_at && drop_at < create_at, "{dropping}");
    }

    #[test]
    fn comments_in_each_dialect() {
        let mut t = items();
        t.comment = Some("the items".into());
        t.draft.as_mut().unwrap().columns[1].comment = Some("stock's unit".into());
        let pg = all(&create_statements(&with(vec![t.clone()]), "postgres").unwrap());
        assert!(pg.contains(r#"COMMENT ON TABLE "shop"."items" IS 'the items'"#), "{pg}");
        assert!(pg.contains(r#"COMMENT ON COLUMN "shop"."items"."sku" IS 'stock''s unit'"#), "{pg}");
        let my = all(&create_statements(&with(vec![t]), "mysql").unwrap());
        assert!(my.contains("`sku` varchar(32) NOT NULL COMMENT 'stock''s unit'"), "{my}");
        assert!(my.contains(") COMMENT='the items'"), "{my}");
    }

    #[test]
    fn create_statements_tags_tables() {
        let statements = create_statements(&orders_customers(), "postgres").unwrap();
        assert_eq!(statements[0].table.as_deref(), Some("shop.orders"));
        assert!(statements[0].creates);
        assert!(statements[1..].iter().all(|s| s.table.is_none()));
    }

    #[test]
    fn each_draft_runs_as_a_group_created_by_its_create_table() {
        let mut def = orders_customers();
        let mut t = items();
        t.comment = Some("the items".into());
        t.draft.as_mut().unwrap().script_after = Some("-- after items".into());
        def.tables.push(t);
        let statements = create_statements(&def, "postgres").unwrap();
        let items: Vec<&Statement> = statements
            .iter()
            .filter(|s| s.table.as_deref() == Some("shop.items"))
            .collect();
        // CREATE, its script, its index and its comment, in that order.
        assert!(items[0].sql.starts_with("CREATE TABLE"), "{items:?}");
        assert_eq!(items[1].sql, "-- after items");
        assert!(items[2].sql.starts_with("CREATE INDEX"), "{items:?}");
        assert!(items[3].sql.starts_with("COMMENT ON TABLE"), "{items:?}");
        assert_eq!(items.iter().filter(|s| s.creates).count(), 1);
        assert!(items[0].creates);
        // The group comes whole, before any foreign key.
        let first_fk = statements.iter().position(|s| s.sql.contains("FOREIGN KEY")).unwrap();
        let last_item = statements.iter().rposition(|s| s.table.as_deref() == Some("shop.items")).unwrap();
        assert!(last_item < first_fk);
    }

    #[test]
    fn old_portable_types_still_create() {
        let def = with(vec![ModelTable {
            draft: Some(DraftTable {
                columns: vec![col("n", "numeric")],
                ..Default::default()
            }),
            ..existing("t")
        }]);
        let sql = all(&create_statements(&def, "postgres").unwrap());
        assert!(sql.contains(r#""n" numeric(18,4) NOT NULL"#), "{sql}");
    }

    #[test]
    fn script_refuses_other_databases() {
        assert_eq!(
            script(&orders_customers(), &structure(), "nope", &ScriptOptions::default())
                .unwrap_err(),
            "Criar tabelas não é suportado para nope"
        );
        assert!(script(&orders_customers(), &structure(), "pinot", &ScriptOptions::default())
            .unwrap_err()
            .contains("table config"));
    }

    fn view(id: &str, sql: &str) -> ModelView {
        ModelView {
            id: id.into(),
            name: id.into(),
            schema: "shop".into(),
            sql: sql.into(),
            ..Default::default()
        }
    }

    fn gen(def: &Definition, opts: &ScriptOptions) -> Result<String, String> {
        script(def, &structure(), "postgres", opts)
    }

    #[test]
    fn views_after_foreign_keys() {
        let mut def = orders_customers();
        def.settings = Some(Settings {
            scripts: Some(ModelScripts {
                create_after: Some("-- end".into()),
                ..Default::default()
            }),
            ..Default::default()
        });
        let mut v = view("v_orders", "select 1;  \n");
        v.script_before = Some("-- before".into());
        v.script_after = Some("-- after".into());
        def.views = vec![v];
        let sql = gen(&def, &ScriptOptions::default()).unwrap();
        let fk = sql.find("FOREIGN KEY").unwrap();
        let before = sql.find("-- before;\n").unwrap();
        let create = sql.find("CREATE VIEW \"shop\".\"v_orders\" AS select 1;\n").unwrap();
        let after = sql.find("-- after;\n").unwrap();
        let end = sql.find("-- end;").unwrap();
        assert!(fk < before && before < create && create < after && after < end, "{sql}");
    }

    #[test]
    fn views_in_dependency_order() {
        let mut def = with(vec![]);
        let mut v2 = view("v2", "select 2");
        v2.depends_on_views = vec!["v1".into()];
        def.views = vec![v2, view("v1", "select 1")];
        let sql = gen(&def, &ScriptOptions::default()).unwrap();
        assert!(sql.find("\"v1\"").unwrap() < sql.find("\"v2\"").unwrap(), "{sql}");
    }

    #[test]
    fn view_cycle_is_refused() {
        let mut def = with(vec![]);
        let mut v1 = view("v1", "select 1");
        v1.depends_on_views = vec!["v2".into()];
        let mut v2 = view("v2", "select 2");
        v2.depends_on_views = vec!["v1".into()];
        def.views = vec![v1, v2];
        assert_eq!(
            gen(&def, &ScriptOptions::default()),
            Err("As views v1, v2 dependem umas das outras".to_string())
        );
    }

    #[test]
    fn drop_views_first() {
        let mut def = orders_customers();
        let mut v2 = view("v2", "select 2");
        v2.depends_on_views = vec!["v1".into()];
        def.views = vec![view("v1", "select 1"), v2];
        let sql = gen(&def, &ScriptOptions { drop: true, ..Default::default() }).unwrap();
        let d2 = sql.find("DROP VIEW IF EXISTS \"shop\".\"v2\"").unwrap();
        let d1 = sql.find("DROP VIEW IF EXISTS \"shop\".\"v1\"").unwrap();
        let fk = sql.find("DROP CONSTRAINT").unwrap();
        assert!(d2 < d1 && d1 < fk, "{sql}");
    }

    #[test]
    fn view_comment() {
        let mut def = with(vec![]);
        let mut v = view("v1", "select 1");
        v.comment = Some("it's a view".into());
        def.views = vec![v];
        let opts = ScriptOptions { comments: true, ..Default::default() };
        let pg = gen(&def, &opts).unwrap();
        assert!(pg.contains("COMMENT ON VIEW \"shop\".\"v1\" IS 'it''s a view';"), "{pg}");
        assert!(pg.find("CREATE VIEW").unwrap() < pg.find("COMMENT ON VIEW").unwrap(), "{pg}");
        let my = script(&def, &structure(), "mysql", &opts).unwrap();
        assert!(!my.contains("COMMENT"), "{my}");
        assert!(my.contains("CREATE VIEW `shop`.`v1` AS select 1;"), "{my}");
    }

    #[test]
    fn only_selects_views() {
        let mut def = orders_customers();
        def.views = vec![view("v1", "select 1"), view("v2", "select 2")];
        let sql = gen(&def, &ScriptOptions { only: Some(vec!["view:v1".into()]), ..Default::default() }).unwrap();
        assert!(sql.contains("\"v1\"") && !sql.contains("\"v2\"") && !sql.contains("CREATE TABLE"), "{sql}");
        let new = gen(&def, &ScriptOptions { only_new: true, ..Default::default() }).unwrap();
        assert!(new.contains("\"v1\"") && new.contains("\"v2\""), "{new}");
    }

    #[test]
    fn only_view_id_does_not_pull_in_a_table_aliased_like_it() {
        let mut def = orders_customers();
        // A view whose id equals a table's alias.
        def.views = vec![view("orders", "select 1")];
        def.views[0].name = "report".into();
        let sql = gen(&def, &ScriptOptions { only: Some(vec!["view:orders".into()]), ..Default::default() }).unwrap();
        assert!(!sql.contains("CREATE TABLE"), "{sql}");
        assert!(sql.contains("select 1"), "{sql}");
        let table = gen(&def, &ScriptOptions { only: Some(vec!["orders".into()]), ..Default::default() }).unwrap();
        assert!(table.contains("CREATE TABLE") && !table.contains("select 1"), "{table}");
    }
}
