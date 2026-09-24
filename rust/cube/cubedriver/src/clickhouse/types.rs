//! ClickHouse type mapping: port of `ClickhouseTypeToGeneric` and
//! `ClickHouseDriver.toGenericType` from `ClickHouseDriver.ts`.

use crate::types::GenericType;

/// `ClickhouseTypeToGeneric`.
pub fn clickhouse_to_generic(lower_type: &str) -> Option<GenericType> {
    Some(match lower_type {
        "enum" => GenericType::Text,
        "string" => GenericType::Text,
        "datetime" => GenericType::Timestamp,
        "datetime64" => GenericType::Timestamp,
        "date" => GenericType::Date,
        "decimal" => GenericType::Decimal(None),
        // integers
        "int8" => GenericType::Int,
        "int16" => GenericType::Int,
        "int32" => GenericType::Int,
        "int64" => GenericType::Bigint,
        // unsigned int
        "uint8" => GenericType::Int,
        "uint16" => GenericType::Int,
        "uint32" => GenericType::Int,
        "uint64" => GenericType::Bigint,
        // floats
        "float32" => GenericType::Float,
        "float64" => GenericType::Double,
        // We don't support enums
        "enum8" => GenericType::Text,
        "enum16" => GenericType::Text,
        _ => return None,
    })
}

/// `ClickHouseDriver.toGenericType`.
///
/// Handles the parameterised and container types ClickHouse reports:
/// `Nullable(Int64)`, `LowCardinality(Nullable(String))`, `Array(DateTime)`,
/// `Map(String, Int32)`, `Decimal(10, 2)`, `DateTime64(3, 'UTC')`, ...
pub fn to_generic_type(
    column_type: &str,
    precision: Option<i64>,
    scale: Option<i64>,
    precise_decimal: bool,
) -> GenericType {
    let type_ = column_type.trim();
    let lower_type = type_.to_lowercase();

    if let Some(g) = clickhouse_to_generic(&lower_type) {
        return g;
    }

    let Some(args_start) = type_.find('(') else {
        return crate::types::to_generic_type(type_, precision, scale, precise_decimal);
    };

    let name = lower_type[..args_start].trim().to_string();
    let args = match type_.rfind(')') {
        Some(end) if end > args_start => &type_[args_start + 1..end],
        _ => &type_[args_start + 1..],
    };

    match name.as_str() {
        "nullable" | "lowcardinality" => to_generic_type(args, precision, scale, precise_decimal),
        "array" => GenericType::Other(format!(
            "{}[]",
            to_generic_type(args, None, None, precise_decimal)
        )),
        "map" | "tuple" | "nested" => GenericType::Text,
        "decimal" => {
            let mut parts = args.split(',');
            // JS: `Number(argPrecision)` / `Number(argScale)`, `NaN` when absent
            // — and `NaN` is falsy, so it behaves like "no precision".
            let arg_precision = parts.next().and_then(|p| p.trim().parse::<i64>().ok());
            let arg_scale = parts.next().and_then(|p| p.trim().parse::<i64>().ok());
            crate::types::to_generic_type(&name, arg_precision, arg_scale, precise_decimal)
        }
        // Parameterised scalars: DateTime('UTC'), DateTime64(3, 'UTC'),
        // Enum8('Date' = 1), FixedString(16). Their arguments never carry a
        // type, so only the name is mapped.
        _ => clickhouse_to_generic(&name).unwrap_or_else(|| {
            crate::types::to_generic_type(&name, precision, scale, precise_decimal)
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(t: &str) -> GenericType {
        to_generic_type(t, None, None, false)
    }

    #[test]
    fn scalar_types() {
        assert_eq!(g("String"), GenericType::Text);
        assert_eq!(g("Int64"), GenericType::Bigint);
        assert_eq!(g("Int32"), GenericType::Int);
        assert_eq!(g("UInt64"), GenericType::Bigint);
        assert_eq!(g("Float32"), GenericType::Float);
        assert_eq!(g("Float64"), GenericType::Double);
        assert_eq!(g("Date"), GenericType::Date);
        assert_eq!(g("DateTime"), GenericType::Timestamp);
        assert_eq!(g("DateTime64"), GenericType::Timestamp);
        assert_eq!(g("Enum8"), GenericType::Text);
    }

    #[test]
    fn wrapped_types_unwrap() {
        assert_eq!(g("Nullable(Int64)"), GenericType::Bigint);
        assert_eq!(g("Nullable(String)"), GenericType::Text);
        assert_eq!(g("LowCardinality(Nullable(String))"), GenericType::Text);
        assert_eq!(g("Nullable(DateTime('UTC'))"), GenericType::Timestamp);
    }

    #[test]
    fn container_types() {
        assert_eq!(
            g("Array(DateTime)"),
            GenericType::Other("timestamp[]".into())
        );
        assert_eq!(g("Array(String)"), GenericType::Other("text[]".into()));
        assert_eq!(g("Map(String, Int32)"), GenericType::Text);
        assert_eq!(g("Tuple(Int32, String)"), GenericType::Text);
        assert_eq!(g("Nested(a Int32)"), GenericType::Text);
    }

    #[test]
    fn parameterised_scalars() {
        assert_eq!(g("DateTime('UTC')"), GenericType::Timestamp);
        assert_eq!(g("DateTime64(3, 'UTC')"), GenericType::Timestamp);
        assert_eq!(g("Enum8('a' = 1)"), GenericType::Text);
        assert_eq!(
            g("FixedString(16)"),
            GenericType::Other("fixedstring".into())
        );
    }

    #[test]
    fn decimals_carry_precision() {
        assert_eq!(g("Decimal(10, 2)"), GenericType::Decimal(None));
        assert_eq!(
            to_generic_type("Decimal(10, 2)", None, None, true),
            GenericType::Decimal(Some((10, 2)))
        );
        assert_eq!(
            to_generic_type("Decimal(10, 0)", None, None, true),
            GenericType::Decimal(None)
        );
        assert_eq!(g("Decimal64(4)"), GenericType::Other("decimal64".into()));
    }

    #[test]
    fn unknown_types_pass_through() {
        assert_eq!(g("UUID"), GenericType::Other("UUID".into()));
        assert_eq!(g("IPv4"), GenericType::Other("IPv4".into()));
    }
}
