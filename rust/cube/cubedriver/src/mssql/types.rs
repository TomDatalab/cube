//! MS SQL type mapping and value conversion.

use base64::Engine;
use serde_json::Value;
use tiberius::{ColumnType, Row};

use crate::error::{DriverError, Result};
use crate::types::GenericType;

/// Port of `GenericTypeToMSSql`.
pub fn generic_to_mssql(generic: &GenericType) -> String {
    match generic {
        GenericType::Boolean => "bit".to_string(),
        GenericType::String => "nvarchar(max)".to_string(),
        GenericType::Text => "nvarchar(max)".to_string(),
        GenericType::Timestamp => "datetime2".to_string(),
        GenericType::Other(other) if other == "uuid" => "uniqueidentifier".to_string(),
        other => other.to_string(),
    }
}

/// Port of `MSSqlToGenericType`.
pub fn mssql_to_generic(db_type: &str) -> Option<GenericType> {
    // The Node map is keyed by the *raw* type name, without lower-casing.
    match db_type {
        "bit" => Some(GenericType::Boolean),
        "uniqueidentifier" => Some(GenericType::Other("uuid".to_string())),
        "datetime2" => Some(GenericType::Timestamp),
        _ => None,
    }
}

/// Port of `MSSqlDriver.toGenericType`.
pub fn to_generic_type(
    db_type: &str,
    precision: Option<i64>,
    scale: Option<i64>,
    precise_decimal: bool,
) -> GenericType {
    mssql_to_generic(db_type).unwrap_or_else(|| {
        crate::types::to_generic_type(db_type, precision, scale, precise_decimal)
    })
}

/// Port of `mapFields`: the intermediate type name a result-set column gets
/// before it is passed through `toGenericType`.
pub fn column_type_name(column_type: ColumnType) -> &'static str {
    match column_type {
        ColumnType::Bit | ColumnType::Bitn => "boolean",
        // integers
        ColumnType::Int1
        | ColumnType::Int2
        | ColumnType::Int4
        | ColumnType::Int8
        | ColumnType::Intn => "int",
        // float (fixed point)
        ColumnType::Money | ColumnType::Money4 | ColumnType::Decimaln | ColumnType::Numericn => {
            "decimal"
        }
        // double
        ColumnType::Float4 | ColumnType::Float8 | ColumnType::Floatn => "double",
        // strings
        ColumnType::BigChar
        | ColumnType::BigVarChar
        | ColumnType::NChar
        | ColumnType::NVarchar
        | ColumnType::Text
        | ColumnType::NText
        | ColumnType::Xml => "text",
        // date and time
        ColumnType::Timen => "time",
        // `sql.Date` maps to `timestamp` in the Node driver, not to `date`
        ColumnType::Daten
        | ColumnType::Datetime
        | ColumnType::Datetime2
        | ColumnType::Datetime4
        | ColumnType::Datetimen
        | ColumnType::DatetimeOffsetn => "timestamp",
        // others (uniqueidentifier, variant, binary, image, udt, geography…)
        _ => "string",
    }
}

/// Converts the cell at `index` into JSON.
///
/// Mirrors the Node driver's conversions: every numeric type is stringified
/// (`sql.valueHandler`) and every `Date` is serialised with `toJSON()` under
/// `useUTC: true`.
pub fn cell_to_value(row: &Row, index: usize, column_type: ColumnType) -> Result<Value> {
    let decode = |e: tiberius::error::Error| DriverError::TypeDetection(e.to_string());

    let value = match column_type {
        ColumnType::Bit | ColumnType::Bitn => row
            .try_get::<bool, _>(index)
            .map_err(decode)?
            .map(Value::Bool),
        // `Intn` / `Floatn` are the nullable (variable length) forms: the
        // payload can be any width, so every candidate is tried in turn.
        ColumnType::Int1
        | ColumnType::Int2
        | ColumnType::Int4
        | ColumnType::Int8
        | ColumnType::Intn => integer_to_string(row, index)
            .map_err(decode)?
            .map(Value::String),
        ColumnType::Float4 | ColumnType::Float8 | ColumnType::Floatn => float_to_string(row, index)
            .map_err(decode)?
            .map(Value::String),
        ColumnType::Money | ColumnType::Money4 | ColumnType::Decimaln | ColumnType::Numericn => row
            .try_get::<tiberius::numeric::Numeric, _>(index)
            .map_err(decode)?
            .map(|v| Value::String(v.to_string())),
        ColumnType::Daten => row
            .try_get::<chrono::NaiveDate, _>(index)
            .map_err(decode)?
            .map(|v| Value::String(format!("{}T00:00:00.000Z", v.format("%Y-%m-%d")))),
        ColumnType::Timen => row
            .try_get::<chrono::NaiveTime, _>(index)
            .map_err(decode)?
            .map(|v| {
                // `mssql` hands back a `Date` pinned to 1970-01-01.
                Value::String(format!("1970-01-01T{}Z", v.format("%H:%M:%S%.3f")))
            }),
        ColumnType::Datetime
        | ColumnType::Datetime2
        | ColumnType::Datetime4
        | ColumnType::Datetimen => row
            .try_get::<chrono::NaiveDateTime, _>(index)
            .map_err(decode)?
            .map(|v| Value::String(v.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())),
        ColumnType::DatetimeOffsetn => row
            .try_get::<chrono::DateTime<chrono::Utc>, _>(index)
            .map_err(decode)?
            .map(|v| Value::String(v.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())),
        ColumnType::Guid => row
            .try_get::<uuid_str::Guid, _>(index)
            .map_err(decode)?
            .map(|v| Value::String(v.0)),
        ColumnType::BigBinary | ColumnType::BigVarBin | ColumnType::Image => row
            .try_get::<&[u8], _>(index)
            .map_err(decode)?
            .map(|v| Value::String(base64::engine::general_purpose::STANDARD.encode(v))),
        // NVARCHAR, VARCHAR, XML, SQL_VARIANT and anything else: as text.
        _ => row
            .try_get::<&str, _>(index)
            .map_err(decode)?
            .map(|v| Value::String(v.to_string())),
    };

    Ok(value.unwrap_or(Value::Null))
}

/// Reads an integer cell of any width and stringifies it.
///
/// A nullable column travels as `intn`, whose payload width follows the
/// declared type, so every width is tried; a reader that matches the payload
/// answers `Ok(None)` for SQL `NULL`, which is how a null is told apart from a
/// type mismatch.
fn integer_to_string(
    row: &Row,
    index: usize,
) -> std::result::Result<Option<String>, tiberius::error::Error> {
    let mut is_null = false;
    let mut last_error = None;

    macro_rules! try_width {
        ($ty:ty) => {
            match row.try_get::<$ty, _>(index) {
                Ok(Some(value)) => return Ok(Some(value.to_string())),
                Ok(None) => is_null = true,
                Err(e) => last_error = Some(e),
            }
        };
    }

    try_width!(i64);
    try_width!(i32);
    try_width!(i16);
    try_width!(u8);

    match (is_null, last_error) {
        (true, _) => Ok(None),
        (false, Some(e)) => Err(e),
        (false, None) => Ok(None),
    }
}

/// Reads a floating point cell of either width and stringifies it.
fn float_to_string(
    row: &Row,
    index: usize,
) -> std::result::Result<Option<String>, tiberius::error::Error> {
    let mut is_null = false;
    let mut last_error = None;

    macro_rules! try_width {
        ($ty:ty) => {
            match row.try_get::<$ty, _>(index) {
                Ok(Some(value)) => return Ok(Some(value.to_string())),
                Ok(None) => is_null = true,
                Err(e) => last_error = Some(e),
            }
        };
    }

    try_width!(f64);
    try_width!(f32);

    match (is_null, last_error) {
        (true, _) => Ok(None),
        (false, Some(e)) => Err(e),
        (false, None) => Ok(None),
    }
}

/// Minimal `FromSql` shim turning a TDS `uniqueidentifier` into its string
/// form, so that the driver does not need a `uuid` dependency.
pub mod uuid_str {
    use tiberius::{ColumnData, FromSql};

    /// A GUID rendered as `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Guid(pub String);

    impl<'a> FromSql<'a> for Guid {
        fn from_sql(value: &'a ColumnData<'static>) -> tiberius::Result<Option<Self>> {
            match value {
                ColumnData::Guid(Some(uuid)) => Ok(Some(Guid(uuid.to_string()))),
                ColumnData::Guid(None) => Ok(None),
                _ => Ok(None),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_to_database_types() {
        assert_eq!(generic_to_mssql(&GenericType::Boolean), "bit");
        assert_eq!(generic_to_mssql(&GenericType::String), "nvarchar(max)");
        assert_eq!(generic_to_mssql(&GenericType::Text), "nvarchar(max)");
        assert_eq!(generic_to_mssql(&GenericType::Timestamp), "datetime2");
        assert_eq!(
            generic_to_mssql(&GenericType::Other("uuid".into())),
            "uniqueidentifier"
        );
        assert_eq!(generic_to_mssql(&GenericType::Bigint), "bigint");
        assert_eq!(
            generic_to_mssql(&GenericType::Decimal(Some((10, 2)))),
            "decimal(10, 2)"
        );
    }

    #[test]
    fn database_to_generic_types() {
        assert_eq!(
            to_generic_type("bit", None, None, false),
            GenericType::Boolean
        );
        assert_eq!(
            to_generic_type("uniqueidentifier", None, None, false),
            GenericType::Other("uuid".into())
        );
        assert_eq!(
            to_generic_type("datetime2", None, None, false),
            GenericType::Timestamp
        );
        assert_eq!(
            to_generic_type("nvarchar", None, None, false),
            GenericType::Text
        );
        assert_eq!(
            to_generic_type("numeric", Some(10), Some(2), true),
            GenericType::Decimal(Some((10, 2)))
        );
        assert_eq!(
            to_generic_type("datetime", None, None, false),
            GenericType::Timestamp
        );
    }

    #[test]
    fn result_column_types() {
        assert_eq!(column_type_name(ColumnType::Bit), "boolean");
        assert_eq!(column_type_name(ColumnType::Int4), "int");
        assert_eq!(column_type_name(ColumnType::Intn), "int");
        assert_eq!(column_type_name(ColumnType::Decimaln), "decimal");
        assert_eq!(column_type_name(ColumnType::Money), "decimal");
        assert_eq!(column_type_name(ColumnType::Float8), "double");
        assert_eq!(column_type_name(ColumnType::NVarchar), "text");
        assert_eq!(column_type_name(ColumnType::Timen), "time");
        assert_eq!(column_type_name(ColumnType::Daten), "timestamp");
        assert_eq!(column_type_name(ColumnType::Datetime2), "timestamp");
        assert_eq!(column_type_name(ColumnType::Guid), "string");

        // and through `toGenericType`
        assert_eq!(
            to_generic_type(column_type_name(ColumnType::Timen), None, None, false),
            GenericType::String
        );
        assert_eq!(
            to_generic_type(column_type_name(ColumnType::Daten), None, None, false),
            GenericType::Timestamp
        );
    }
}
