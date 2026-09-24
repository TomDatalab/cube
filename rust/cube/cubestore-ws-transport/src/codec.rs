use std::io::Cursor;

use arrow::array::RecordBatch;
use arrow::ipc::reader::StreamReader;
use bytes::Bytes;
use cubeshared::codegen::{
    root_as_http_message, BinaryValue, BinaryValueArgs, BoolValue, BoolValueArgs, Float64Value,
    Float64ValueArgs, HttpCommand, HttpMessage, HttpMessageArgs, HttpParameter, HttpParameterArgs,
    HttpParameterValue, HttpQuery, HttpQueryArgs, HttpQueryResultData, HttpTable, HttpTableArgs,
    Int64Value, Int64ValueArgs, NullValue, NullValueArgs, QueryResultFormat, StringValue,
    StringValueArgs,
};
use flatbuffers::FlatBufferBuilder;

use crate::error::TransportError;
use crate::request::{QueryOptions, QueryParameter};
use crate::result::{QueryResult, ResponseFormat, ResultData};

/// Build a binary FlatBuffer payload carrying an `HttpQuery` command with no
/// parameters, no inline tables and the Arrow response format.
pub fn encode_query(message_id: u32, connection_id: &str, sql: &str) -> Bytes {
    encode_query_with_options(message_id, connection_id, sql, &QueryOptions::default())
}

/// Build a binary FlatBuffer payload carrying an `HttpQuery` command.
///
/// Mirrors `WebSocketConnection.query`: the optional `trace_obj`,
/// `inline_tables` and `parameters` fields are only written when non-empty, so
/// a plain query serialises exactly as it did before parameters existed.
pub fn encode_query_with_options(
    message_id: u32,
    connection_id: &str,
    sql: &str,
    options: &QueryOptions,
) -> Bytes {
    let mut builder = FlatBufferBuilder::with_capacity(1024);
    let query_offset = builder.create_string(sql);

    let trace_obj_offset = options
        .trace_obj
        .as_deref()
        .map(|t| builder.create_string(t));

    let inline_tables_offset = if options.inline_tables.is_empty() {
        None
    } else {
        let mut table_offsets = Vec::with_capacity(options.inline_tables.len());
        for table in &options.inline_tables {
            let name = builder.create_string(&table.name);
            let column_offsets: Vec<_> = table
                .columns
                .iter()
                .map(|c| builder.create_string(c))
                .collect();
            let columns = builder.create_vector(&column_offsets);
            let type_offsets: Vec<_> = table
                .types
                .iter()
                .map(|t| builder.create_string(t))
                .collect();
            let types = builder.create_vector(&type_offsets);
            let csv_rows = builder.create_string(&table.csv_rows);
            table_offsets.push(HttpTable::create(
                &mut builder,
                &HttpTableArgs {
                    name: Some(name),
                    columns: Some(columns),
                    types: Some(types),
                    csv_rows: Some(csv_rows),
                },
            ));
        }
        Some(builder.create_vector(&table_offsets))
    };

    let parameters_offset = if options.parameters.is_empty() {
        None
    } else {
        let offsets: Vec<_> = options
            .parameters
            .iter()
            .map(|p| serialize_parameter(&mut builder, p))
            .collect();
        Some(builder.create_vector(&offsets))
    };

    let http_query = HttpQuery::create(
        &mut builder,
        &HttpQueryArgs {
            query: Some(query_offset),
            trace_obj: trace_obj_offset,
            inline_tables: inline_tables_offset,
            parameters: parameters_offset,
            response_format: match options.response_format {
                ResponseFormat::Legacy => QueryResultFormat::Legacy,
                // `Completed` can never be requested; Arrow is the closest.
                ResponseFormat::Arrow | ResponseFormat::Completed => QueryResultFormat::Arrow,
            },
        },
    );

    let connection_id_offset = builder.create_string(connection_id);

    let message = HttpMessage::create(
        &mut builder,
        &HttpMessageArgs {
            message_id,
            command_type: HttpCommand::HttpQuery,
            command: Some(http_query.as_union_value()),
            connection_id: Some(connection_id_offset),
        },
    );
    builder.finish(message, None);
    Bytes::copy_from_slice(builder.finished_data())
}

/// Port of `WebSocketConnection.serializeParameter`.
fn serialize_parameter<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    parameter: &QueryParameter,
) -> flatbuffers::WIPOffset<HttpParameter<'a>> {
    let (value_type, value) = match parameter {
        QueryParameter::Null => (
            HttpParameterValue::NullValue,
            NullValue::create(builder, &NullValueArgs {}).as_union_value(),
        ),
        QueryParameter::Bool(v) => (
            HttpParameterValue::BoolValue,
            BoolValue::create(builder, &BoolValueArgs { v: *v }).as_union_value(),
        ),
        QueryParameter::Int64(v) => (
            HttpParameterValue::Int64Value,
            Int64Value::create(builder, &Int64ValueArgs { v: *v }).as_union_value(),
        ),
        QueryParameter::Float64(v) => (
            HttpParameterValue::Float64Value,
            Float64Value::create(builder, &Float64ValueArgs { v: *v }).as_union_value(),
        ),
        QueryParameter::String(v) => {
            let s = builder.create_string(v);
            (
                HttpParameterValue::StringValue,
                StringValue::create(builder, &StringValueArgs { v: Some(s) }).as_union_value(),
            )
        }
        QueryParameter::Binary(v) => {
            let b = builder.create_vector(v);
            (
                HttpParameterValue::BinaryValue,
                BinaryValue::create(builder, &BinaryValueArgs { v: Some(b) }).as_union_value(),
            )
        }
    };

    HttpParameter::create(
        builder,
        &HttpParameterArgs {
            value_type,
            value: Some(value),
        },
    )
}

/// Decoded response from the server.
pub enum DecodedResponse {
    Ok(QueryResult),
    Error(String),
}

pub struct DecodedFrame {
    pub message_id: u32,
    pub response: DecodedResponse,
}

/// Parse an incoming binary frame, extracting message id and result/error.
pub fn decode_frame(bytes: &[u8]) -> Result<DecodedFrame, TransportError> {
    let msg = root_as_http_message(bytes)
        .map_err(|e| TransportError::Protocol(format!("flatbuffer decode: {e}")))?;
    let message_id = msg.message_id();

    let response = match msg.command_type() {
        HttpCommand::HttpError => {
            let err = msg.command_as_http_error().ok_or_else(|| {
                TransportError::Protocol("HttpError union variant missing".into())
            })?;
            DecodedResponse::Error(err.error().unwrap_or("unknown error").to_string())
        }
        HttpCommand::HttpResultSet => {
            let rs = msg.command_as_http_result_set().ok_or_else(|| {
                TransportError::Protocol("HttpResultSet union variant missing".into())
            })?;

            let columns: Vec<String> = rs
                .columns()
                .map(|cols| cols.iter().map(|s| s.to_string()).collect())
                .unwrap_or_default();

            let mut rows: Vec<Vec<Option<String>>> = Vec::new();
            if let Some(row_vec) = rs.rows() {
                rows.reserve(row_vec.len());
                for row in row_vec.iter() {
                    let mut out: Vec<Option<String>> = Vec::with_capacity(columns.len());
                    if let Some(values) = row.values() {
                        for v in values.iter() {
                            out.push(v.string_value().map(|s| s.to_string()));
                        }
                    }
                    rows.push(out);
                }
            }

            log::debug!(
                "decoded HttpResultSet: {} columns, {} rows",
                columns.len(),
                rows.len()
            );

            DecodedResponse::Ok(QueryResult {
                data: ResultData::Legacy { columns, rows },
            })
        }
        HttpCommand::HttpQueryResult => {
            let qr = msg.command_as_http_query_result().ok_or_else(|| {
                TransportError::Protocol("HttpQueryResult union variant missing".into())
            })?;

            match qr.data_type() {
                HttpQueryResultData::HttpQueryResultArrow => {
                    let arrow = qr.data_as_http_query_result_arrow().ok_or_else(|| {
                        TransportError::Protocol(
                            "HttpQueryResult.data variant is not HttpQueryResultArrow".into(),
                        )
                    })?;

                    let result = decode_arrow_ipc(arrow.data().bytes())?;
                    log::debug!(
                        "decoded HttpQueryResult (Arrow IPC): {} columns, {} rows",
                        result.get_columns().len(),
                        result.row_count()
                    );

                    DecodedResponse::Ok(result)
                }
                HttpQueryResultData::HttpQueryResultCompleted => {
                    // Command completed without a result set (zero columns).
                    log::debug!("decoded HttpQueryResult (Completed): no result set");
                    DecodedResponse::Ok(QueryResult {
                        data: ResultData::Completed,
                    })
                }
                other => {
                    return Err(TransportError::Protocol(format!(
                        "unsupported HttpQueryResult.data variant: {:?}",
                        other.variant_name()
                    )));
                }
            }
        }
        other => {
            return Err(TransportError::Protocol(format!(
                "unexpected command variant: {:?}",
                other.variant_name()
            )));
        }
    };

    Ok(DecodedFrame {
        message_id,
        response,
    })
}

fn decode_arrow_ipc(bytes: &[u8]) -> Result<QueryResult, TransportError> {
    let reader = StreamReader::try_new(Cursor::new(bytes), None)
        .map_err(|e| TransportError::Protocol(format!("arrow IPC open: {e}")))?;

    let schema = reader.schema();
    let batches: Vec<RecordBatch> = reader
        .collect::<Result<_, _>>()
        .map_err(|e| TransportError::Protocol(format!("arrow IPC read batch: {e}")))?;

    Ok(QueryResult {
        data: ResultData::Arrow { schema, batches },
    })
}
