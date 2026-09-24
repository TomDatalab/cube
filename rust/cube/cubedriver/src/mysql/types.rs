//! MySQL type mapping: port of `MySqlNativeToMySqlType` and
//! `MySqlToGenericType` from `MySqlDriver.ts`.

use mysql_async::consts::{ColumnFlags, ColumnType};

use crate::types::GenericType;

/// `MySqlNativeToMySqlType` plus the `mysql.Types[...]` fallback of `mysql2`.
///
/// The Node driver looks the native type up in `MySqlNativeToMySqlType` and
/// otherwise falls back to the constant's own name from
/// `node-mysql2/lib/constants/types.js`, so both tables live here.
pub fn native_to_mysql_type(column_type: ColumnType) -> &'static str {
    match column_type {
        // MySqlNativeToMySqlType
        ColumnType::MYSQL_TYPE_DECIMAL => "decimal",
        ColumnType::MYSQL_TYPE_NEWDECIMAL => "decimal",
        ColumnType::MYSQL_TYPE_TINY => "tinyint",
        ColumnType::MYSQL_TYPE_SHORT => "smallint",
        ColumnType::MYSQL_TYPE_LONG => "int",
        ColumnType::MYSQL_TYPE_INT24 => "mediumint",
        ColumnType::MYSQL_TYPE_LONGLONG => "bigint",
        ColumnType::MYSQL_TYPE_NEWDATE => "datetime",
        ColumnType::MYSQL_TYPE_TIMESTAMP => "timestamp",
        ColumnType::MYSQL_TYPE_DATETIME => "datetime",
        ColumnType::MYSQL_TYPE_TIME => "time",
        ColumnType::MYSQL_TYPE_TINY_BLOB => "tinytext",
        ColumnType::MYSQL_TYPE_MEDIUM_BLOB => "mediumtext",
        ColumnType::MYSQL_TYPE_LONG_BLOB => "longtext",
        ColumnType::MYSQL_TYPE_BLOB => "text",
        ColumnType::MYSQL_TYPE_VAR_STRING => "varchar",
        ColumnType::MYSQL_TYPE_STRING => "varchar",
        // `mysql.Types[field.type]` — the constant's own name, verbatim.
        ColumnType::MYSQL_TYPE_FLOAT => "FLOAT",
        ColumnType::MYSQL_TYPE_DOUBLE => "DOUBLE",
        ColumnType::MYSQL_TYPE_NULL => "NULL",
        ColumnType::MYSQL_TYPE_DATE => "DATE",
        ColumnType::MYSQL_TYPE_YEAR => "YEAR",
        ColumnType::MYSQL_TYPE_VARCHAR => "VARCHAR",
        ColumnType::MYSQL_TYPE_BIT => "BIT",
        ColumnType::MYSQL_TYPE_TIMESTAMP2 => "TIMESTAMP2",
        ColumnType::MYSQL_TYPE_DATETIME2 => "DATETIME2",
        ColumnType::MYSQL_TYPE_TIME2 => "TIME2",
        ColumnType::MYSQL_TYPE_TYPED_ARRAY => "TYPED_ARRAY",
        ColumnType::MYSQL_TYPE_JSON => "JSON",
        ColumnType::MYSQL_TYPE_ENUM => "ENUM",
        ColumnType::MYSQL_TYPE_SET => "SET",
        ColumnType::MYSQL_TYPE_GEOMETRY => "GEOMETRY",
        ColumnType::MYSQL_TYPE_UNKNOWN => "UNKNOWN",
    }
}

/// `MySqlToGenericType`.
pub fn mysql_to_generic(mysql_type_lower: &str) -> Option<GenericType> {
    Some(match mysql_type_lower {
        "mediumtext" => GenericType::Text,
        "longtext" => GenericType::Text,
        "mediumint" => GenericType::Int,
        "smallint" => GenericType::Int,
        "bigint" => GenericType::Int,
        "tinyint" => GenericType::Int,
        "mediumint unsigned" => GenericType::Int,
        "smallint unsigned" => GenericType::Int,
        "bigint unsigned" => GenericType::Int,
        "tinyint unsigned" => GenericType::Int,
        _ => return None,
    })
}

/// `MySqlDriver.toGenericType`:
/// `MySqlToGenericType[lower] || MySqlToGenericType[lower.split('(')[0]] || super`.
pub fn to_generic_type(
    column_type: &str,
    precision: Option<i64>,
    scale: Option<i64>,
    precise_decimal: bool,
) -> GenericType {
    let lower = column_type.to_lowercase();
    if let Some(g) = mysql_to_generic(&lower) {
        return g;
    }
    // `lower.split('(')[0]` keeps the trailing text, e.g. `tinyint(1)` but also
    // `bigint(20) unsigned` → `bigint`. JS splits on the first `(` only.
    let head = lower.split('(').next().unwrap_or(&lower);
    if let Some(g) = mysql_to_generic(head) {
        return g;
    }
    crate::types::to_generic_type(column_type, precision, scale, precise_decimal)
}

/// `GenericTypeToMySql`.
pub fn generic_to_mysql(generic: &GenericType) -> String {
    match generic {
        GenericType::String => "varchar(255) CHARACTER SET utf8mb4".to_string(),
        GenericType::Text => "varchar(255) CHARACTER SET utf8mb4".to_string(),
        GenericType::Decimal(None) => "decimal(38,10)".to_string(),
        other => other.to_string(),
    }
}

/// `true` when the column carries the `UNSIGNED` flag.
pub fn is_unsigned(flags: ColumnFlags) -> bool {
    flags.contains(ColumnFlags::UNSIGNED_FLAG)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_types_map_to_mysql_names() {
        assert_eq!(
            native_to_mysql_type(ColumnType::MYSQL_TYPE_LONGLONG),
            "bigint"
        );
        assert_eq!(native_to_mysql_type(ColumnType::MYSQL_TYPE_TINY), "tinyint");
        assert_eq!(
            native_to_mysql_type(ColumnType::MYSQL_TYPE_NEWDECIMAL),
            "decimal"
        );
        assert_eq!(
            native_to_mysql_type(ColumnType::MYSQL_TYPE_VAR_STRING),
            "varchar"
        );
        assert_eq!(native_to_mysql_type(ColumnType::MYSQL_TYPE_BLOB), "text");
        assert_eq!(native_to_mysql_type(ColumnType::MYSQL_TYPE_DATE), "DATE");
        assert_eq!(native_to_mysql_type(ColumnType::MYSQL_TYPE_YEAR), "YEAR");
    }

    #[test]
    fn generic_type_mapping() {
        let g = |t: &str| to_generic_type(t, None, None, false);
        assert_eq!(g("bigint"), GenericType::Int);
        assert_eq!(g("BIGINT"), GenericType::Int);
        assert_eq!(g("bigint(20)"), GenericType::Int);
        assert_eq!(g("tinyint(1)"), GenericType::Int);
        assert_eq!(g("bigint unsigned"), GenericType::Int);
        assert_eq!(g("mediumtext"), GenericType::Text);
        assert_eq!(g("longtext"), GenericType::Text);
        // falls through to BaseDriver
        assert_eq!(g("varchar"), GenericType::Text);
        assert_eq!(g("int"), GenericType::Int);
        assert_eq!(g("integer"), GenericType::Int);
        assert_eq!(g("datetime"), GenericType::Timestamp);
        assert_eq!(g("timestamp"), GenericType::Timestamp);
        assert_eq!(g("time"), GenericType::String);
        assert_eq!(g("DATE"), GenericType::Date);
        assert_eq!(g("decimal"), GenericType::Decimal(None));
        assert_eq!(g("YEAR"), GenericType::Other("YEAR".into()));
        // `int unsigned` has no entry, so it is passed through unchanged
        assert_eq!(g("int unsigned"), GenericType::Other("int unsigned".into()));
        // precise decimals still work through the base mapping
        assert_eq!(
            to_generic_type("numeric", Some(10), Some(2), true),
            GenericType::Decimal(Some((10, 2)))
        );
    }

    #[test]
    fn generic_to_mysql_types() {
        assert_eq!(
            generic_to_mysql(&GenericType::String),
            "varchar(255) CHARACTER SET utf8mb4"
        );
        assert_eq!(
            generic_to_mysql(&GenericType::Text),
            "varchar(255) CHARACTER SET utf8mb4"
        );
        assert_eq!(
            generic_to_mysql(&GenericType::Decimal(None)),
            "decimal(38,10)"
        );
        assert_eq!(
            generic_to_mysql(&GenericType::Decimal(Some((10, 2)))),
            "decimal(10, 2)"
        );
        assert_eq!(generic_to_mysql(&GenericType::Int), "int");
        assert_eq!(generic_to_mysql(&GenericType::Timestamp), "timestamp");
    }
}
