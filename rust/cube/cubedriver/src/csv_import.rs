//! Materialising an export-bucket CSV download into memory.
//!
//! Cube Store imports `TableCsvData` natively (`CREATE TABLE … LOCATION`), so
//! it never needs this. Any other external driver, however, only knows how to
//! insert rows, which is what [`csv_to_memory`] produces: it fetches the
//! unloaded files and parses them with the CSV dialect the unloading driver
//! reported (`csvDelimiter`, `csvNoHeader`, `csvDisableQuoting`,
//! `exportBucketCsvEscapeSymbol`).

use serde_json::Value;

use crate::error::{DriverError, Result};
use crate::type_detection::detect_types_from_tabular;
use crate::types::{Column, QueryResult, Row, TableCsvData, TableMemoryData};

/// Fetches and parses every file of `data`.
///
/// The files are the signed URLs produced by `unload`; a plain filesystem path
/// is read from disk, which keeps local export-bucket mounts working.
pub async fn csv_to_memory(data: &TableCsvData) -> Result<TableMemoryData> {
    let dialect = CsvDialect::from(data);
    let mut header: Option<Vec<String>> = None;
    let mut rows: Vec<Vec<CsvField>> = Vec::new();

    for file in &data.csv_file {
        let content = read_file(file).await?;
        let parsed = parse_csv(&content, &dialect);
        let mut parsed = parsed.into_iter();
        if !data.csv_no_header {
            match parsed.next() {
                Some(first) => {
                    if header.is_none() {
                        header = Some(first.into_iter().map(|f| f.value).collect());
                    }
                }
                // an empty file has no header either
                None => continue,
            }
        }
        rows.extend(parsed);
    }

    // Column names: the reported types win, then the CSV header, then `c0…cN`.
    let width = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let names: Vec<String> = match (&data.types, &header) {
        (Some(types), _) if !types.is_empty() => types.iter().map(|c| c.name.clone()).collect(),
        (_, Some(header)) if !header.is_empty() => header.clone(),
        _ => (0..width).map(|i| format!("c{i}")).collect(),
    };

    let rows: Vec<Row> = rows
        .into_iter()
        .map(|row| {
            (0..names.len())
                .map(|i| match row.get(i) {
                    // An unquoted empty field is SQL NULL; `""` stays a string.
                    Some(field) if field.value.is_empty() && !field.quoted => Value::Null,
                    Some(field) => Value::String(field.value.clone()),
                    None => Value::Null,
                })
                .collect()
        })
        .collect();

    let columns: Vec<Column> = match &data.types {
        Some(types) if !types.is_empty() => types.clone(),
        _ => {
            let untyped = QueryResult::new(
                names
                    .iter()
                    .map(|name| Column::new(name.clone(), "text"))
                    .collect(),
                rows.clone(),
            );
            detect_types_from_tabular(&untyped)?
        }
    };

    Ok(QueryResult::new(columns, rows))
}

/// Reads one unloaded file: an `http(s)` URL is downloaded, anything else is
/// read from the filesystem.
async fn read_file(file: &str) -> Result<String> {
    let bytes: Vec<u8> = if file.starts_with("http://") || file.starts_with("https://") {
        let response = reqwest::get(file)
            .await
            .map_err(|e| DriverError::Connection {
                pool_name: "export bucket".to_string(),
                message: format!("Unable to download {file}: {e}"),
            })?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .map_err(|e| DriverError::Query(format!("Unable to read {file}: {e}")))?;
        if !status.is_success() {
            return Err(DriverError::Query(format!(
                "Unable to download {file}: HTTP {status}"
            )));
        }
        body.to_vec()
    } else {
        std::fs::read(file)
            .map_err(|e| DriverError::Query(format!("Unable to read {file}: {e}")))?
    };

    // gzip magic: Athena, Trino and most warehouses unload `*.csv.gz`.
    // Recognised by content rather than by name, because a signed URL's path
    // does not always keep the extension. Concatenated members (one per
    // writer) are all read.
    let bytes = if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut decoded = Vec::new();
        std::io::Read::read_to_end(&mut flate2::read::MultiGzDecoder::new(&bytes[..]), &mut decoded)
            .map_err(|e| DriverError::Query(format!("Unable to decompress {file}: {e}")))?;
        decoded
    } else {
        bytes
    };

    String::from_utf8(bytes)
        .map_err(|e| DriverError::TypeDetection(format!("{file} is not valid UTF-8: {e}")))
}

/// The CSV dialect of an unloaded file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsvDialect {
    pub delimiter: char,
    /// `csvDisableQuoting` inverted: whether `"` starts a quoted field.
    pub quoting: bool,
    /// `exportBucketCsvEscapeSymbol` (`\` for some warehouses).
    pub escape: Option<char>,
}

impl Default for CsvDialect {
    fn default() -> Self {
        Self {
            delimiter: ',',
            quoting: true,
            escape: None,
        }
    }
}

impl From<&TableCsvData> for CsvDialect {
    fn from(data: &TableCsvData) -> Self {
        Self {
            delimiter: data
                .csv_delimiter
                .as_deref()
                .and_then(parse_delimiter)
                .unwrap_or(','),
            quoting: !data.csv_disable_quoting,
            escape: data
                .export_bucket_csv_escape_symbol
                .as_ref()
                .and_then(|e| e.chars().next()),
        }
    }
}

/// A reported delimiter: one character, or caret notation for a control
/// character (`^A` is `\x01`, Hive's and Athena's default), which is how
/// Cube Store reads the same option.
fn parse_delimiter(delimiter: &str) -> Option<char> {
    let mut chars = delimiter.chars();
    match (chars.next(), chars.next(), chars.next()) {
        (Some('^'), Some(c @ '@'..='_'), None) => char::from_u32(c as u32 - 0x40),
        (first, _, _) => first,
    }
}

/// One parsed field: the text plus whether it was quoted (which is what tells
/// an empty string apart from a `NULL`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CsvField {
    pub value: String,
    pub quoted: bool,
}

impl CsvField {
    fn new(value: impl Into<String>, quoted: bool) -> Self {
        Self {
            value: value.into(),
            quoted,
        }
    }
}

/// Parses `content` into rows of fields (RFC 4180 with the dialect's
/// delimiter, optional quoting and optional escape symbol).
pub fn parse_csv(content: &str, dialect: &CsvDialect) -> Vec<Vec<CsvField>> {
    let mut rows = Vec::new();
    let mut row: Vec<CsvField> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut in_quotes = false;
    let mut chars = content.chars().peekable();
    let mut seen_any = false;

    while let Some(c) = chars.next() {
        seen_any = true;
        if in_quotes {
            if Some(c) == dialect.escape {
                if let Some(next) = chars.next() {
                    field.push(next);
                }
                continue;
            }
            if c == '"' {
                // A doubled quote is an escaped one.
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                }
                continue;
            }
            field.push(c);
            continue;
        }

        match c {
            '"' if dialect.quoting && field.is_empty() => {
                in_quotes = true;
                quoted = true;
            }
            c if c == dialect.delimiter => {
                row.push(CsvField::new(std::mem::take(&mut field), quoted));
                quoted = false;
            }
            '\r' => {
                // swallowed; the following \n ends the record
            }
            '\n' => {
                row.push(CsvField::new(std::mem::take(&mut field), quoted));
                quoted = false;
                rows.push(std::mem::take(&mut row));
            }
            c => field.push(c),
        }
    }

    if seen_any && (!field.is_empty() || quoted || !row.is_empty()) {
        row.push(CsvField::new(field, quoted));
        rows.push(row);
    }

    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::GenericType;

    fn values(rows: &[Vec<CsvField>]) -> Vec<Vec<String>> {
        rows.iter()
            .map(|row| row.iter().map(|f| f.value.clone()).collect())
            .collect()
    }

    #[test]
    fn parses_rfc4180() {
        let rows = parse_csv(
            "id,name\n1,\"a,b\"\n2,\"say \"\"hi\"\"\"\n3,\n",
            &CsvDialect::default(),
        );
        assert_eq!(
            values(&rows),
            vec![
                vec!["id", "name"],
                vec!["1", "a,b"],
                vec!["2", "say \"hi\""],
                vec!["3", ""],
            ]
        );
        // the last field of row 3 was not quoted -> NULL, unlike `""`
        assert!(!rows[3][1].quoted);
        assert!(rows[1][1].quoted);
    }

    #[test]
    fn honours_the_dialect() {
        let dialect = CsvDialect {
            delimiter: '|',
            quoting: false,
            escape: None,
        };
        let rows = parse_csv("a|\"b\"|c\n", &dialect);
        assert_eq!(values(&rows), vec![vec!["a", "\"b\"", "c"]]);

        let dialect = CsvDialect {
            delimiter: ',',
            quoting: true,
            escape: Some('\\'),
        };
        let rows = parse_csv("\"a\\\"b\",c\n", &dialect);
        assert_eq!(values(&rows), vec![vec!["a\"b", "c"]]);
    }

    #[test]
    fn handles_crlf_and_a_missing_trailing_newline() {
        let rows = parse_csv("a,b\r\n1,2", &CsvDialect::default());
        assert_eq!(values(&rows), vec![vec!["a", "b"], vec!["1", "2"]]);
        assert!(parse_csv("", &CsvDialect::default()).is_empty());
    }

    #[tokio::test]
    async fn materialises_a_local_file() {
        let dir = std::env::temp_dir().join("cubedriver-csv-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("part-0.csv");
        std::fs::write(&path, "id,name,amount\n1,a,100\n2,,200\n").unwrap();

        let data = TableCsvData {
            csv_file: vec![path.to_string_lossy().to_string()],
            ..Default::default()
        };
        let memory = csv_to_memory(&data).await.unwrap();
        assert_eq!(
            memory.columns,
            vec![
                Column::new("id", "int"),
                // `detect_types_from_tabular` reports non-numeric text as `string`
                Column::new("name", "string"),
                Column::new("amount", "int"),
            ]
        );
        assert_eq!(
            memory.rows,
            vec![
                vec![Value::from("1"), Value::from("a"), Value::from("100")],
                vec![Value::from("2"), Value::Null, Value::from("200")],
            ]
        );

        // the reported types win over detection, and `csvNoHeader` keeps the
        // first line as data
        std::fs::write(&path, "1,a,100\n").unwrap();
        let data = TableCsvData {
            csv_file: vec![path.to_string_lossy().to_string()],
            csv_no_header: true,
            types: Some(vec![
                Column::new("id", "bigint"),
                Column::new("name", "string"),
                Column::new("amount", "decimal"),
            ]),
            ..Default::default()
        };
        let memory = csv_to_memory(&data).await.unwrap();
        assert_eq!(memory.columns[0].type_, GenericType::Bigint);
        assert_eq!(memory.len(), 1);
        assert_eq!(memory.get_string(0, "name").as_deref(), Some("a"));

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn compressed_exports_are_decompressed() {
        use std::io::Write;

        let dir = std::env::temp_dir().join("cubedriver-csv-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("part-gz-0.csv.gz");
        // Two gzip members, as parallel writers produce, with Athena's `^A`.
        let mut bytes = Vec::new();
        for chunk in ["1\u{1}a\n", "2\u{1}\n"] {
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(chunk.as_bytes()).unwrap();
            bytes.extend(encoder.finish().unwrap());
        }
        std::fs::write(&path, bytes).unwrap();
        let data = TableCsvData {
            csv_file: vec![path.to_string_lossy().to_string()],
            csv_no_header: true,
            csv_delimiter: Some("^A".to_string()),
            csv_disable_quoting: true,
            types: Some(vec![Column::new("id", "int"), Column::new("name", "string")]),
            ..Default::default()
        };
        let memory = csv_to_memory(&data).await.unwrap();
        assert_eq!(memory.len(), 2);
        assert_eq!(memory.get_string(0, "name").as_deref(), Some("a"));
        assert_eq!(memory.rows[1][1], Value::Null);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn caret_delimiters_are_control_characters() {
        assert_eq!(parse_delimiter("^A"), Some('\u{1}'));
        assert_eq!(parse_delimiter("|"), Some('|'));
        assert_eq!(parse_delimiter("^"), Some('^'));
    }
}
