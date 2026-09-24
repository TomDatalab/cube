//! Hand-written subset of HiveServer2's `TCLIService.thrift`
//! (`OpenSession`, `CloseSession`, `ExecuteStatement`, `GetOperationStatus`,
//! `GetResultSetMetadata`, `FetchResults`, `CloseOperation`), serialised with
//! the `thrift` crate's binary protocol.
//!
//! Only the fields the driver needs are modelled; every other field is
//! skipped on read, so newer servers (Hive 3/4, Spark Thrift Server, Impala)
//! that add fields stay compatible. Field ids follow the IDL shipped with the
//! Node.js driver (`idl/Hive_2.x/TCLIService_types.js`).

use std::collections::BTreeMap;
use std::io::Cursor;

use base64::Engine;
use serde_json::Value;
use thrift::protocol::{
    TBinaryInputProtocol, TBinaryOutputProtocol, TFieldIdentifier, TInputProtocol, TListIdentifier,
    TMapIdentifier, TMessageIdentifier, TMessageType, TOutputProtocol, TStructIdentifier, TType,
};

/// `TProtocolVersion.HIVE_CLI_SERVICE_PROTOCOL_V6`: first version with
/// column-oriented result sets.
pub const PROTOCOL_V6: i32 = 5;
/// `HIVE_CLI_SERVICE_PROTOCOL_V9` (default of the Hive 2.1.1 IDL).
pub const PROTOCOL_V9: i32 = 8;
/// `HIVE_CLI_SERVICE_PROTOCOL_V10` (Hive 2.2+).
pub const PROTOCOL_V10: i32 = 9;

/// `TStatusCode`.
pub mod status_code {
    pub const SUCCESS: i32 = 0;
    pub const SUCCESS_WITH_INFO: i32 = 1;
    pub const STILL_EXECUTING: i32 = 2;
    pub const ERROR: i32 = 3;
    pub const INVALID_HANDLE: i32 = 4;
}

/// `TOperationState`.
pub mod operation_state {
    pub const INITIALIZED: i32 = 0;
    pub const RUNNING: i32 = 1;
    pub const FINISHED: i32 = 2;
    pub const CANCELED: i32 = 3;
    pub const CLOSED: i32 = 4;
    pub const ERROR: i32 = 5;
    pub const UNKNOWN: i32 = 6;
    pub const PENDING: i32 = 7;
    pub const TIMEDOUT: i32 = 8;
}

/// `TTypeId` → Hive type name.
pub const TYPE_NAMES: &[&str] = &[
    "boolean",
    "tinyint",
    "smallint",
    "int",
    "bigint",
    "float",
    "double",
    "string",
    "timestamp",
    "binary",
    "array",
    "map",
    "struct",
    "uniontype",
    "user_defined",
    "decimal",
    "void",
    "date",
    "varchar",
    "char",
    "interval_year_month",
    "interval_day_time",
    "timestamp with local time zone",
];

pub type ThriftResult<T> = thrift::Result<T>;

/// `THandleIdentifier`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HandleIdentifier {
    pub guid: Vec<u8>,
    pub secret: Vec<u8>,
}

/// `TSessionHandle`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionHandle {
    pub session_id: HandleIdentifier,
}

/// `TOperationHandle`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OperationHandle {
    pub operation_id: HandleIdentifier,
    pub operation_type: i32,
    pub has_result_set: bool,
    pub modified_row_count: Option<f64>,
}

/// `TStatus`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    pub status_code: i32,
    pub info_messages: Vec<String>,
    pub sql_state: Option<String>,
    pub error_code: Option<i32>,
    pub error_message: Option<String>,
}

impl Status {
    pub fn is_error(&self) -> bool {
        self.status_code == status_code::ERROR || self.status_code == status_code::INVALID_HANDLE
    }

    /// `HS2Util.getThriftErrorMessage(status, default)[2]`.
    pub fn message(&self, default_message: &str) -> String {
        thrift_error_message(
            self.error_message.as_deref().filter(|m| !m.is_empty()),
            &self.info_messages,
            default_message,
        )
    }
}

/// `HS2Util.getThriftErrorMessage`, third element.
pub fn thrift_error_message(
    error_message: Option<&str>,
    info_messages: &[String],
    default_message: &str,
) -> String {
    let error_message = error_message.unwrap_or(default_message);
    [
        error_message,
        "-- Error caused from HiveServer2\n\n",
        &info_messages.join("\n"),
    ]
    .join("\n\n")
}

/// `TOpenSessionResp`.
#[derive(Debug, Clone, Default)]
pub struct OpenSessionResp {
    pub status: Status,
    pub server_protocol_version: i32,
    pub session_handle: Option<SessionHandle>,
    pub configuration: BTreeMap<String, String>,
}

/// `TExecuteStatementResp`.
#[derive(Debug, Clone, Default)]
pub struct ExecuteStatementResp {
    pub status: Status,
    pub operation_handle: Option<OperationHandle>,
}

/// `TGetOperationStatusResp`.
#[derive(Debug, Clone, Default)]
pub struct GetOperationStatusResp {
    pub status: Status,
    pub operation_state: Option<i32>,
    pub sql_state: Option<String>,
    pub error_code: Option<i32>,
    pub error_message: Option<String>,
}

/// `TColumnDesc`, reduced to what the driver uses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ColumnDesc {
    pub column_name: String,
    /// Hive type name (`int`, `string`, `decimal`, `array`, ...).
    pub type_name: String,
    pub position: i32,
    pub comment: Option<String>,
}

/// `TGetResultSetMetadataResp`.
#[derive(Debug, Clone, Default)]
pub struct GetResultSetMetadataResp {
    pub status: Status,
    pub columns: Option<Vec<ColumnDesc>>,
}

/// `TRowSet`, decoded into JSON cells.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RowSet {
    /// Row-oriented results (protocol < V6).
    pub rows: Vec<Vec<Value>>,
    /// Column-oriented results (protocol >= V6): one vector per column.
    pub columns: Option<Vec<Vec<Value>>>,
}

impl RowSet {
    /// Number of rows in the block.
    pub fn len(&self) -> usize {
        match &self.columns {
            Some(columns) => columns.first().map(Vec::len).unwrap_or(0),
            None => self.rows.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Rows of the block.
    pub fn into_rows(self) -> Vec<Vec<Value>> {
        match self.columns {
            Some(columns) => {
                let len = columns.first().map(Vec::len).unwrap_or(0);
                let mut iters: Vec<_> = columns.into_iter().map(Vec::into_iter).collect();
                (0..len)
                    .map(|_| {
                        iters
                            .iter_mut()
                            .map(|it| it.next().unwrap_or(Value::Null))
                            .collect()
                    })
                    .collect()
            }
            None => self.rows,
        }
    }
}

/// `TFetchResultsResp`.
#[derive(Debug, Clone, Default)]
pub struct FetchResultsResp {
    pub status: Status,
    pub has_more_rows: Option<bool>,
    pub results: Option<RowSet>,
}

/// A response carrying only a `TStatus` (`TCloseSessionResp`, `TCloseOperationResp`).
#[derive(Debug, Clone, Default)]
pub struct StatusResp {
    pub status: Status,
}

// ----------------------------------------------------------------------
// Encoding
// ----------------------------------------------------------------------

fn field(name: &str, field_type: TType, id: i16) -> TFieldIdentifier {
    TFieldIdentifier::new(name, field_type, id)
}

fn write_handle_identifier(p: &mut dyn TOutputProtocol, h: &HandleIdentifier) -> ThriftResult<()> {
    p.write_struct_begin(&TStructIdentifier::new("THandleIdentifier"))?;
    p.write_field_begin(&field("guid", TType::String, 1))?;
    p.write_bytes(&h.guid)?;
    p.write_field_end()?;
    p.write_field_begin(&field("secret", TType::String, 2))?;
    p.write_bytes(&h.secret)?;
    p.write_field_end()?;
    p.write_field_stop()?;
    p.write_struct_end()
}

fn write_session_handle(p: &mut dyn TOutputProtocol, h: &SessionHandle) -> ThriftResult<()> {
    p.write_struct_begin(&TStructIdentifier::new("TSessionHandle"))?;
    p.write_field_begin(&field("sessionId", TType::Struct, 1))?;
    write_handle_identifier(p, &h.session_id)?;
    p.write_field_end()?;
    p.write_field_stop()?;
    p.write_struct_end()
}

fn write_operation_handle(p: &mut dyn TOutputProtocol, h: &OperationHandle) -> ThriftResult<()> {
    p.write_struct_begin(&TStructIdentifier::new("TOperationHandle"))?;
    p.write_field_begin(&field("operationId", TType::Struct, 1))?;
    write_handle_identifier(p, &h.operation_id)?;
    p.write_field_end()?;
    p.write_field_begin(&field("operationType", TType::I32, 2))?;
    p.write_i32(h.operation_type)?;
    p.write_field_end()?;
    p.write_field_begin(&field("hasResultSet", TType::Bool, 3))?;
    p.write_bool(h.has_result_set)?;
    p.write_field_end()?;
    if let Some(count) = h.modified_row_count {
        p.write_field_begin(&field("modifiedRowCount", TType::Double, 4))?;
        p.write_double(count)?;
        p.write_field_end()?;
    }
    p.write_field_stop()?;
    p.write_struct_end()
}

fn write_string_map(
    p: &mut dyn TOutputProtocol,
    map: &BTreeMap<String, String>,
) -> ThriftResult<()> {
    p.write_map_begin(&TMapIdentifier::new(
        TType::String,
        TType::String,
        map.len() as i32,
    ))?;
    for (k, v) in map {
        p.write_string(k)?;
        p.write_string(v)?;
    }
    p.write_map_end()
}

/// A call: method name plus the request struct (always field 1 of the args).
pub enum Request<'a> {
    OpenSession {
        client_protocol: i32,
        username: &'a str,
        password: &'a str,
        configuration: &'a BTreeMap<String, String>,
    },
    CloseSession {
        session: &'a SessionHandle,
    },
    ExecuteStatement {
        session: &'a SessionHandle,
        statement: &'a str,
        run_async: bool,
    },
    GetOperationStatus {
        operation: &'a OperationHandle,
    },
    GetResultSetMetadata {
        operation: &'a OperationHandle,
    },
    FetchResults {
        operation: &'a OperationHandle,
        max_rows: i64,
    },
    CloseOperation {
        operation: &'a OperationHandle,
    },
}

impl Request<'_> {
    /// The service method name.
    pub fn method(&self) -> &'static str {
        match self {
            Request::OpenSession { .. } => "OpenSession",
            Request::CloseSession { .. } => "CloseSession",
            Request::ExecuteStatement { .. } => "ExecuteStatement",
            Request::GetOperationStatus { .. } => "GetOperationStatus",
            Request::GetResultSetMetadata { .. } => "GetResultSetMetadata",
            Request::FetchResults { .. } => "FetchResults",
            Request::CloseOperation { .. } => "CloseOperation",
        }
    }

    fn write_req(&self, p: &mut dyn TOutputProtocol) -> ThriftResult<()> {
        p.write_struct_begin(&TStructIdentifier::new(format!("T{}Req", self.method())))?;
        match self {
            Request::OpenSession {
                client_protocol,
                username,
                password,
                configuration,
            } => {
                p.write_field_begin(&field("client_protocol", TType::I32, 1))?;
                p.write_i32(*client_protocol)?;
                p.write_field_end()?;
                p.write_field_begin(&field("username", TType::String, 2))?;
                p.write_string(username)?;
                p.write_field_end()?;
                p.write_field_begin(&field("password", TType::String, 3))?;
                p.write_string(password)?;
                p.write_field_end()?;
                if !configuration.is_empty() {
                    p.write_field_begin(&field("configuration", TType::Map, 4))?;
                    write_string_map(p, configuration)?;
                    p.write_field_end()?;
                }
            }
            Request::CloseSession { session } => {
                p.write_field_begin(&field("sessionHandle", TType::Struct, 1))?;
                write_session_handle(p, session)?;
                p.write_field_end()?;
            }
            Request::ExecuteStatement {
                session,
                statement,
                run_async,
            } => {
                p.write_field_begin(&field("sessionHandle", TType::Struct, 1))?;
                write_session_handle(p, session)?;
                p.write_field_end()?;
                p.write_field_begin(&field("statement", TType::String, 2))?;
                p.write_string(statement)?;
                p.write_field_end()?;
                p.write_field_begin(&field("runAsync", TType::Bool, 4))?;
                p.write_bool(*run_async)?;
                p.write_field_end()?;
            }
            Request::GetOperationStatus { operation }
            | Request::GetResultSetMetadata { operation }
            | Request::CloseOperation { operation } => {
                p.write_field_begin(&field("operationHandle", TType::Struct, 1))?;
                write_operation_handle(p, operation)?;
                p.write_field_end()?;
            }
            Request::FetchResults {
                operation,
                max_rows,
            } => {
                p.write_field_begin(&field("operationHandle", TType::Struct, 1))?;
                write_operation_handle(p, operation)?;
                p.write_field_end()?;
                // TFetchOrientation.FETCH_NEXT
                p.write_field_begin(&field("orientation", TType::I32, 2))?;
                p.write_i32(0)?;
                p.write_field_end()?;
                p.write_field_begin(&field("maxRows", TType::I64, 3))?;
                p.write_i64(*max_rows)?;
                p.write_field_end()?;
                // FETCH_TYPE.ROW (1 would be the operation log)
                p.write_field_begin(&field("fetchType", TType::I16, 4))?;
                p.write_i16(0)?;
                p.write_field_end()?;
            }
        }
        p.write_field_stop()?;
        p.write_struct_end()
    }

    /// Serialises the whole `Call` message.
    pub fn encode(&self, sequence_number: i32) -> ThriftResult<Vec<u8>> {
        let mut buf = Vec::new();
        {
            let mut p = TBinaryOutputProtocol::new(&mut buf, true);
            p.write_message_begin(&TMessageIdentifier::new(
                self.method(),
                TMessageType::Call,
                sequence_number,
            ))?;
            p.write_struct_begin(&TStructIdentifier::new(format!("{}_args", self.method())))?;
            p.write_field_begin(&field("req", TType::Struct, 1))?;
            self.write_req(&mut p)?;
            p.write_field_end()?;
            p.write_field_stop()?;
            p.write_struct_end()?;
            p.write_message_end()?;
            p.flush()?;
        }
        Ok(buf)
    }
}

// ----------------------------------------------------------------------
// Decoding
// ----------------------------------------------------------------------

/// Reads a struct, handing every field to `f`; fields `f` does not consume
/// (returns `false`) are skipped.
fn read_fields(
    p: &mut dyn TInputProtocol,
    mut f: impl FnMut(&mut dyn TInputProtocol, i16, TType) -> ThriftResult<bool>,
) -> ThriftResult<()> {
    p.read_struct_begin()?;
    loop {
        let ident = p.read_field_begin()?;
        if ident.field_type == TType::Stop {
            break;
        }
        let id = ident.id.unwrap_or(i16::MIN);
        if !f(p, id, ident.field_type)? {
            p.skip(ident.field_type)?;
        }
        p.read_field_end()?;
    }
    p.read_struct_end()
}

fn read_list<T>(
    p: &mut dyn TInputProtocol,
    mut f: impl FnMut(&mut dyn TInputProtocol) -> ThriftResult<T>,
) -> ThriftResult<Vec<T>> {
    let TListIdentifier { size, .. } = p.read_list_begin()?;
    let mut values = Vec::with_capacity(size.max(0) as usize);
    for _ in 0..size {
        values.push(f(p)?);
    }
    p.read_list_end()?;
    Ok(values)
}

fn read_handle_identifier(p: &mut dyn TInputProtocol) -> ThriftResult<HandleIdentifier> {
    let mut h = HandleIdentifier::default();
    read_fields(p, |p, id, t| match (id, t) {
        (1, TType::String) => {
            h.guid = p.read_bytes()?;
            Ok(true)
        }
        (2, TType::String) => {
            h.secret = p.read_bytes()?;
            Ok(true)
        }
        _ => Ok(false),
    })?;
    Ok(h)
}

fn read_session_handle(p: &mut dyn TInputProtocol) -> ThriftResult<SessionHandle> {
    let mut h = SessionHandle::default();
    read_fields(p, |p, id, t| match (id, t) {
        (1, TType::Struct) => {
            h.session_id = read_handle_identifier(p)?;
            Ok(true)
        }
        _ => Ok(false),
    })?;
    Ok(h)
}

fn read_operation_handle(p: &mut dyn TInputProtocol) -> ThriftResult<OperationHandle> {
    let mut h = OperationHandle::default();
    read_fields(p, |p, id, t| match (id, t) {
        (1, TType::Struct) => {
            h.operation_id = read_handle_identifier(p)?;
            Ok(true)
        }
        (2, TType::I32) => {
            h.operation_type = p.read_i32()?;
            Ok(true)
        }
        (3, TType::Bool) => {
            h.has_result_set = p.read_bool()?;
            Ok(true)
        }
        (4, TType::Double) => {
            h.modified_row_count = Some(p.read_double()?);
            Ok(true)
        }
        _ => Ok(false),
    })?;
    Ok(h)
}

fn read_status(p: &mut dyn TInputProtocol) -> ThriftResult<Status> {
    let mut s = Status::default();
    read_fields(p, |p, id, t| match (id, t) {
        (1, TType::I32) => {
            s.status_code = p.read_i32()?;
            Ok(true)
        }
        (2, TType::List) => {
            s.info_messages = read_list(p, |p| p.read_string())?;
            Ok(true)
        }
        (3, TType::String) => {
            s.sql_state = Some(p.read_string()?);
            Ok(true)
        }
        (4, TType::I32) => {
            s.error_code = Some(p.read_i32()?);
            Ok(true)
        }
        (5, TType::String) => {
            s.error_message = Some(p.read_string()?);
            Ok(true)
        }
        _ => Ok(false),
    })?;
    Ok(s)
}

fn read_string_map(p: &mut dyn TInputProtocol) -> ThriftResult<BTreeMap<String, String>> {
    let ident = p.read_map_begin()?;
    let mut map = BTreeMap::new();
    for _ in 0..ident.size {
        let k = p.read_string()?;
        let v = p.read_string()?;
        map.insert(k, v);
    }
    p.read_map_end()?;
    Ok(map)
}

/// `TTypeDesc` → name of its first entry.
fn read_type_desc(p: &mut dyn TInputProtocol) -> ThriftResult<String> {
    let mut names: Vec<String> = Vec::new();
    read_fields(p, |p, id, t| match (id, t) {
        (1, TType::List) => {
            names = read_list(p, read_type_entry)?;
            Ok(true)
        }
        _ => Ok(false),
    })?;
    Ok(names
        .into_iter()
        .next()
        .unwrap_or_else(|| "string".to_string()))
}

/// `TTypeEntry` union.
fn read_type_entry(p: &mut dyn TInputProtocol) -> ThriftResult<String> {
    let mut name = String::from("string");
    read_fields(p, |p, id, t| match (id, t) {
        (1, TType::Struct) => {
            // TPrimitiveTypeEntry { 1: TTypeId type, 2: optional TTypeQualifiers }
            read_fields(p, |p, id, t| match (id, t) {
                (1, TType::I32) => {
                    let type_id = p.read_i32()?;
                    name = TYPE_NAMES
                        .get(type_id as usize)
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| format!("type_{type_id}"));
                    Ok(true)
                }
                _ => Ok(false),
            })?;
            Ok(true)
        }
        (2..=6, TType::Struct) => {
            name = ["array", "map", "struct", "uniontype", "user_defined"][(id - 2) as usize]
                .to_string();
            Ok(false)
        }
        _ => Ok(false),
    })?;
    Ok(name)
}

fn read_column_desc(p: &mut dyn TInputProtocol) -> ThriftResult<ColumnDesc> {
    let mut c = ColumnDesc::default();
    read_fields(p, |p, id, t| match (id, t) {
        (1, TType::String) => {
            c.column_name = p.read_string()?;
            Ok(true)
        }
        (2, TType::Struct) => {
            c.type_name = read_type_desc(p)?;
            Ok(true)
        }
        (3, TType::I32) => {
            c.position = p.read_i32()?;
            Ok(true)
        }
        (4, TType::String) => {
            c.comment = Some(p.read_string()?);
            Ok(true)
        }
        _ => Ok(false),
    })?;
    Ok(c)
}

fn read_table_schema(p: &mut dyn TInputProtocol) -> ThriftResult<Vec<ColumnDesc>> {
    let mut columns = Vec::new();
    read_fields(p, |p, id, t| match (id, t) {
        (1, TType::List) => {
            columns = read_list(p, read_column_desc)?;
            Ok(true)
        }
        _ => Ok(false),
    })?;
    Ok(columns)
}

/// `i64` values are strings (`i64ToString` in jshs2).
fn i64_value(v: i64) -> Value {
    Value::String(v.to_string())
}

fn double_value(v: f64) -> Value {
    serde_json::Number::from_f64(v)
        .map(Value::Number)
        .unwrap_or_else(|| Value::String(v.to_string()))
}

fn bytes_to_string(bytes: Vec<u8>) -> Value {
    Value::String(match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    })
}

fn binary_value(bytes: Vec<u8>) -> Value {
    Value::String(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// `T*Column { 1: list<T> values, 2: binary nulls }`, nulls applied.
fn read_typed_column(
    p: &mut dyn TInputProtocol,
    read_value: fn(&mut dyn TInputProtocol) -> ThriftResult<Value>,
) -> ThriftResult<Vec<Value>> {
    let mut values = Vec::new();
    let mut nulls = Vec::new();
    read_fields(p, |p, id, t| match (id, t) {
        (1, TType::List) => {
            values = read_list(p, read_value)?;
            Ok(true)
        }
        (2, TType::String) => {
            nulls = p.read_bytes()?;
            Ok(true)
        }
        _ => Ok(false),
    })?;
    for (i, value) in values.iter_mut().enumerate() {
        // `HS2Util.getIsNull`: bit `i % 8` of byte `i / 8`.
        if nulls
            .get(i / 8)
            .map(|b| b & (1 << (i % 8)) != 0)
            .unwrap_or(false)
        {
            *value = Value::Null;
        }
    }
    Ok(values)
}

/// `TColumn` union.
fn read_column(p: &mut dyn TInputProtocol) -> ThriftResult<Vec<Value>> {
    let mut values = Vec::new();
    read_fields(p, |p, id, t| {
        if t != TType::Struct {
            return Ok(false);
        }
        let reader: fn(&mut dyn TInputProtocol) -> ThriftResult<Value> = match id {
            1 => |p| Ok(Value::Bool(p.read_bool()?)),
            2 => |p| Ok(Value::from(p.read_i8()?)),
            3 => |p| Ok(Value::from(p.read_i16()?)),
            4 => |p| Ok(Value::from(p.read_i32()?)),
            5 => |p| Ok(i64_value(p.read_i64()?)),
            6 => |p| Ok(double_value(p.read_double()?)),
            7 => |p| Ok(bytes_to_string(p.read_bytes()?)),
            8 => |p| Ok(binary_value(p.read_bytes()?)),
            _ => return Ok(false),
        };
        values = read_typed_column(p, reader)?;
        Ok(true)
    })?;
    Ok(values)
}

/// `TColumnValue` union of `T*Value { 1: optional value }`.
fn read_column_value(p: &mut dyn TInputProtocol) -> ThriftResult<Value> {
    let mut value = Value::Null;
    read_fields(p, |p, id, t| {
        if t != TType::Struct {
            return Ok(false);
        }
        let mut inner = Value::Null;
        read_fields(p, |p, fid, ft| {
            if fid != 1 {
                return Ok(false);
            }
            inner = match (id, ft) {
                (1, TType::Bool) => Value::Bool(p.read_bool()?),
                (2, TType::I08) => Value::from(p.read_i8()?),
                (3, TType::I16) => Value::from(p.read_i16()?),
                (4, TType::I32) => Value::from(p.read_i32()?),
                (5, TType::I64) => i64_value(p.read_i64()?),
                (6, TType::Double) => double_value(p.read_double()?),
                (7, TType::String) => bytes_to_string(p.read_bytes()?),
                _ => return Ok(false),
            };
            Ok(true)
        })?;
        value = inner;
        Ok(true)
    })?;
    Ok(value)
}

fn read_row(p: &mut dyn TInputProtocol) -> ThriftResult<Vec<Value>> {
    let mut values = Vec::new();
    read_fields(p, |p, id, t| match (id, t) {
        (1, TType::List) => {
            values = read_list(p, read_column_value)?;
            Ok(true)
        }
        _ => Ok(false),
    })?;
    Ok(values)
}

fn read_row_set(p: &mut dyn TInputProtocol) -> ThriftResult<RowSet> {
    let mut set = RowSet::default();
    read_fields(p, |p, id, t| match (id, t) {
        (2, TType::List) => {
            set.rows = read_list(p, read_row)?;
            Ok(true)
        }
        (3, TType::List) => {
            set.columns = Some(read_list(p, read_column)?);
            Ok(true)
        }
        _ => Ok(false),
    })?;
    Ok(set)
}

/// A decoded response struct.
pub trait Response: Default {
    fn read(p: &mut dyn TInputProtocol) -> ThriftResult<Self>;
    fn status(&self) -> &Status;
}

macro_rules! status_response {
    ($ty:ty, |$s:ident, $p:ident, $id:ident, $t:ident| $body:expr) => {
        impl Response for $ty {
            fn read(p: &mut dyn TInputProtocol) -> ThriftResult<Self> {
                let mut $s = <$ty>::default();
                read_fields(p, |$p, $id, $t| match ($id, $t) {
                    (1, TType::Struct) => {
                        $s.status = read_status($p)?;
                        Ok(true)
                    }
                    _ => $body,
                })?;
                Ok($s)
            }

            fn status(&self) -> &Status {
                &self.status
            }
        }
    };
}

status_response!(StatusResp, |_s, _p, _id, _t| Ok(false));

status_response!(OpenSessionResp, |s, p, id, t| match (id, t) {
    (2, TType::I32) => {
        s.server_protocol_version = p.read_i32()?;
        Ok(true)
    }
    (3, TType::Struct) => {
        s.session_handle = Some(read_session_handle(p)?);
        Ok(true)
    }
    (4, TType::Map) => {
        s.configuration = read_string_map(p)?;
        Ok(true)
    }
    _ => Ok(false),
});

status_response!(ExecuteStatementResp, |s, p, id, t| match (id, t) {
    (2, TType::Struct) => {
        s.operation_handle = Some(read_operation_handle(p)?);
        Ok(true)
    }
    _ => Ok(false),
});

status_response!(GetOperationStatusResp, |s, p, id, t| match (id, t) {
    (2, TType::I32) => {
        s.operation_state = Some(p.read_i32()?);
        Ok(true)
    }
    (3, TType::String) => {
        s.sql_state = Some(p.read_string()?);
        Ok(true)
    }
    (4, TType::I32) => {
        s.error_code = Some(p.read_i32()?);
        Ok(true)
    }
    (5, TType::String) => {
        s.error_message = Some(p.read_string()?);
        Ok(true)
    }
    _ => Ok(false),
});

status_response!(GetResultSetMetadataResp, |s, p, id, t| match (id, t) {
    (2, TType::Struct) => {
        s.columns = Some(read_table_schema(p)?);
        Ok(true)
    }
    _ => Ok(false),
});

status_response!(FetchResultsResp, |s, p, id, t| match (id, t) {
    (2, TType::Bool) => {
        s.has_more_rows = Some(p.read_bool()?);
        Ok(true)
    }
    (3, TType::Struct) => {
        s.results = Some(read_row_set(p)?);
        Ok(true)
    }
    _ => Ok(false),
});

/// Why a reply could not be decoded.
#[derive(Debug)]
pub enum DecodeError {
    /// More bytes are needed (unframed transport).
    Incomplete,
    /// The server answered with a `TApplicationException`.
    Application(String),
    /// Malformed or unexpected reply.
    Protocol(String),
}

fn is_eof(e: &thrift::Error) -> bool {
    match e {
        thrift::Error::Transport(t) => t.kind == thrift::TransportErrorKind::EndOfFile,
        _ => false,
    }
}

/// Decodes the reply to `method` from `bytes`. Returns the response and the
/// number of bytes consumed.
pub fn decode_reply<R: Response>(bytes: &[u8], method: &str) -> Result<(R, usize), DecodeError> {
    let mut cursor = Cursor::new(bytes);
    let result = {
        let mut p = TBinaryInputProtocol::new(&mut cursor, true);
        decode_reply_inner::<R>(&mut p, method)
    };
    match result {
        Ok(r) => Ok((r, cursor.position() as usize)),
        Err(DecodeError::Protocol(msg)) if msg == "EOF" => Err(DecodeError::Incomplete),
        Err(e) => Err(e),
    }
}

fn decode_reply_inner<R: Response>(
    p: &mut dyn TInputProtocol,
    method: &str,
) -> Result<R, DecodeError> {
    let map = |e: thrift::Error| {
        if is_eof(&e) {
            DecodeError::Protocol("EOF".to_string())
        } else {
            DecodeError::Protocol(e.to_string())
        }
    };
    let ident = p.read_message_begin().map_err(map)?;
    if ident.message_type == TMessageType::Exception {
        let err = thrift::Error::read_application_error_from_in_protocol(p).map_err(map)?;
        p.read_message_end().map_err(map)?;
        return Err(DecodeError::Application(format!(
            "{method} failed: {}",
            err.message
        )));
    }
    if ident.name != method {
        return Err(DecodeError::Protocol(format!(
            "Unexpected Thrift reply {} to {method}",
            ident.name
        )));
    }
    let mut response = None;
    read_fields(p, |p, id, t| match (id, t) {
        (0, TType::Struct) => {
            response = Some(R::read(p)?);
            Ok(true)
        }
        _ => Ok(false),
    })
    .map_err(map)?;
    p.read_message_end().map_err(map)?;
    response.ok_or_else(|| DecodeError::Protocol(format!("{method} returned no result")))
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Server-side encoders used to fabricate replies in unit tests.
    use super::*;

    pub fn reply(method: &str, seq: i32, write: impl FnOnce(&mut dyn TOutputProtocol)) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut p = TBinaryOutputProtocol::new(&mut buf, true);
            p.write_message_begin(&TMessageIdentifier::new(method, TMessageType::Reply, seq))
                .unwrap();
            p.write_struct_begin(&TStructIdentifier::new("result"))
                .unwrap();
            p.write_field_begin(&field("success", TType::Struct, 0))
                .unwrap();
            p.write_struct_begin(&TStructIdentifier::new("resp"))
                .unwrap();
            write(&mut p);
            p.write_field_stop().unwrap();
            p.write_struct_end().unwrap();
            p.write_field_end().unwrap();
            p.write_field_stop().unwrap();
            p.write_struct_end().unwrap();
            p.write_message_end().unwrap();
        }
        buf
    }

    pub fn status(p: &mut dyn TOutputProtocol, code: i32, message: Option<&str>) {
        p.write_field_begin(&field("status", TType::Struct, 1))
            .unwrap();
        p.write_struct_begin(&TStructIdentifier::new("TStatus"))
            .unwrap();
        p.write_field_begin(&field("statusCode", TType::I32, 1))
            .unwrap();
        p.write_i32(code).unwrap();
        p.write_field_end().unwrap();
        if let Some(m) = message {
            p.write_field_begin(&field("infoMessages", TType::List, 2))
                .unwrap();
            p.write_list_begin(&TListIdentifier::new(TType::String, 1))
                .unwrap();
            p.write_string("*org.apache.hive.service.cli.HiveSQLException:boom")
                .unwrap();
            p.write_list_end().unwrap();
            p.write_field_end().unwrap();
            p.write_field_begin(&field("sqlState", TType::String, 3))
                .unwrap();
            p.write_string("42000").unwrap();
            p.write_field_end().unwrap();
            p.write_field_begin(&field("errorMessage", TType::String, 5))
                .unwrap();
            p.write_string(m).unwrap();
            p.write_field_end().unwrap();
        }
        p.write_field_stop().unwrap();
        p.write_struct_end().unwrap();
        p.write_field_end().unwrap();
    }

    /// A `TColumn` of the given union `kind` (7 = string, 4 = i32, ...).
    pub fn column(
        p: &mut dyn TOutputProtocol,
        kind: i16,
        element: TType,
        len: usize,
        write_value: &dyn Fn(&mut dyn TOutputProtocol, usize),
        nulls: &[u8],
    ) {
        p.write_struct_begin(&TStructIdentifier::new("TColumn"))
            .unwrap();
        p.write_field_begin(&field("v", TType::Struct, kind))
            .unwrap();
        p.write_struct_begin(&TStructIdentifier::new("TXColumn"))
            .unwrap();
        p.write_field_begin(&field("values", TType::List, 1))
            .unwrap();
        p.write_list_begin(&TListIdentifier::new(element, len as i32))
            .unwrap();
        for i in 0..len {
            write_value(p, i);
        }
        p.write_list_end().unwrap();
        p.write_field_end().unwrap();
        p.write_field_begin(&field("nulls", TType::String, 2))
            .unwrap();
        p.write_bytes(nulls).unwrap();
        p.write_field_end().unwrap();
        p.write_field_stop().unwrap();
        p.write_struct_end().unwrap();
        p.write_field_end().unwrap();
        p.write_field_stop().unwrap();
        p.write_struct_end().unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    #[test]
    fn encodes_calls_in_binary_protocol() {
        let config = BTreeMap::new();
        let bytes = Request::OpenSession {
            client_protocol: PROTOCOL_V9,
            username: "anonymous",
            password: "",
            configuration: &config,
        }
        .encode(1)
        .unwrap();
        // strict binary protocol: version 1 | CALL
        assert_eq!(&bytes[..4], &[0x80, 0x01, 0x00, 0x01]);
        assert_eq!(&bytes[4..8], &[0, 0, 0, 11]);
        assert_eq!(&bytes[8..19], b"OpenSession");
        assert_eq!(&bytes[19..23], &[0, 0, 0, 1]);
    }

    #[test]
    fn decodes_open_session() {
        let reply = reply("OpenSession", 1, |p| {
            status(p, status_code::SUCCESS, None);
            p.write_field_begin(&field("serverProtocolVersion", TType::I32, 2))
                .unwrap();
            p.write_i32(PROTOCOL_V10).unwrap();
            p.write_field_end().unwrap();
            p.write_field_begin(&field("sessionHandle", TType::Struct, 3))
                .unwrap();
            write_session_handle(
                p,
                &SessionHandle {
                    session_id: HandleIdentifier {
                        guid: vec![1; 16],
                        secret: vec![2; 16],
                    },
                },
            )
            .unwrap();
            p.write_field_end().unwrap();
            // an unknown field is skipped
            p.write_field_begin(&field("future", TType::String, 42))
                .unwrap();
            p.write_string("x").unwrap();
            p.write_field_end().unwrap();
        });
        let (resp, used): (OpenSessionResp, _) = decode_reply(&reply, "OpenSession").unwrap();
        assert_eq!(used, reply.len());
        assert_eq!(resp.server_protocol_version, PROTOCOL_V10);
        assert_eq!(resp.session_handle.unwrap().session_id.guid, vec![1; 16]);

        // truncated replies ask for more bytes
        assert!(matches!(
            decode_reply::<OpenSessionResp>(&reply[..reply.len() - 3], "OpenSession"),
            Err(DecodeError::Incomplete)
        ));
    }

    #[test]
    fn decodes_columnar_results_with_nulls() {
        let reply = reply("FetchResults", 3, |p| {
            status(p, status_code::SUCCESS, None);
            p.write_field_begin(&field("results", TType::Struct, 3))
                .unwrap();
            p.write_struct_begin(&TStructIdentifier::new("TRowSet"))
                .unwrap();
            p.write_field_begin(&field("startRowOffset", TType::I64, 1))
                .unwrap();
            p.write_i64(0).unwrap();
            p.write_field_end().unwrap();
            p.write_field_begin(&field("rows", TType::List, 2)).unwrap();
            p.write_list_begin(&TListIdentifier::new(TType::Struct, 0))
                .unwrap();
            p.write_list_end().unwrap();
            p.write_field_end().unwrap();
            p.write_field_begin(&field("columns", TType::List, 3))
                .unwrap();
            p.write_list_begin(&TListIdentifier::new(TType::Struct, 4))
                .unwrap();
            column(
                p,
                4,
                TType::I32,
                3,
                &|p, i| p.write_i32(i as i32 * 10).unwrap(),
                &[0b010],
            );
            column(
                p,
                7,
                TType::String,
                3,
                &|p, i| p.write_string(&format!("s{i}")).unwrap(),
                &[0b100],
            );
            column(
                p,
                5,
                TType::I64,
                3,
                &|p, _| p.write_i64(9_007_199_254_740_993).unwrap(),
                &[0],
            );
            column(
                p,
                1,
                TType::Bool,
                3,
                &|p, i| p.write_bool(i == 0).unwrap(),
                &[],
            );
            p.write_list_end().unwrap();
            p.write_field_end().unwrap();
            p.write_field_stop().unwrap();
            p.write_struct_end().unwrap();
            p.write_field_end().unwrap();
        });
        let (resp, _): (FetchResultsResp, _) = decode_reply(&reply, "FetchResults").unwrap();
        let rows = resp.results.unwrap().into_rows();
        assert_eq!(
            rows,
            vec![
                vec![
                    Value::from(0),
                    Value::from("s0"),
                    Value::from("9007199254740993"),
                    Value::Bool(true)
                ],
                vec![
                    Value::Null,
                    Value::from("s1"),
                    Value::from("9007199254740993"),
                    Value::Bool(false)
                ],
                vec![
                    Value::from(20),
                    Value::Null,
                    Value::from("9007199254740993"),
                    Value::Bool(false)
                ],
            ]
        );
    }

    #[test]
    fn decodes_errors() {
        let reply = reply("ExecuteStatement", 2, |p| {
            status(
                p,
                status_code::ERROR,
                Some("Error while compiling statement: FAILED"),
            );
        });
        let (resp, _): (ExecuteStatementResp, _) =
            decode_reply(&reply, "ExecuteStatement").unwrap();
        assert!(resp.status.is_error());
        assert_eq!(resp.status.sql_state.as_deref(), Some("42000"));
        assert_eq!(
            resp.status.message("ExecuteStatement operation fail,... !!"),
            "Error while compiling statement: FAILED\n\n-- Error caused from HiveServer2\n\n\n\n*org.apache.hive.service.cli.HiveSQLException:boom"
        );

        // TApplicationException
        let mut buf = Vec::new();
        {
            let mut p = TBinaryOutputProtocol::new(&mut buf, true);
            p.write_message_begin(&TMessageIdentifier::new(
                "FetchResults",
                TMessageType::Exception,
                1,
            ))
            .unwrap();
            thrift::Error::write_application_error_to_out_protocol(
                &thrift::ApplicationError::new(thrift::ApplicationErrorKind::UnknownMethod, "nope"),
                &mut p,
            )
            .unwrap();
            p.write_message_end().unwrap();
        }
        match decode_reply::<FetchResultsResp>(&buf, "FetchResults") {
            Err(DecodeError::Application(m)) => assert!(m.contains("nope")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn decodes_result_set_metadata() {
        let reply = reply("GetResultSetMetadata", 4, |p| {
            status(p, status_code::SUCCESS, None);
            p.write_field_begin(&field("schema", TType::Struct, 2))
                .unwrap();
            p.write_struct_begin(&TStructIdentifier::new("TTableSchema"))
                .unwrap();
            p.write_field_begin(&field("columns", TType::List, 1))
                .unwrap();
            p.write_list_begin(&TListIdentifier::new(TType::Struct, 2))
                .unwrap();
            for (i, (name, type_id)) in [("t.id", 3), ("t.amount", 15)].iter().enumerate() {
                p.write_struct_begin(&TStructIdentifier::new("TColumnDesc"))
                    .unwrap();
                p.write_field_begin(&field("columnName", TType::String, 1))
                    .unwrap();
                p.write_string(name).unwrap();
                p.write_field_end().unwrap();
                p.write_field_begin(&field("typeDesc", TType::Struct, 2))
                    .unwrap();
                p.write_struct_begin(&TStructIdentifier::new("TTypeDesc"))
                    .unwrap();
                p.write_field_begin(&field("types", TType::List, 1))
                    .unwrap();
                p.write_list_begin(&TListIdentifier::new(TType::Struct, 1))
                    .unwrap();
                p.write_struct_begin(&TStructIdentifier::new("TTypeEntry"))
                    .unwrap();
                p.write_field_begin(&field("primitiveEntry", TType::Struct, 1))
                    .unwrap();
                p.write_struct_begin(&TStructIdentifier::new("TPrimitiveTypeEntry"))
                    .unwrap();
                p.write_field_begin(&field("type", TType::I32, 1)).unwrap();
                p.write_i32(*type_id).unwrap();
                p.write_field_end().unwrap();
                p.write_field_stop().unwrap();
                p.write_struct_end().unwrap();
                p.write_field_end().unwrap();
                p.write_field_stop().unwrap();
                p.write_struct_end().unwrap();
                p.write_list_end().unwrap();
                p.write_field_end().unwrap();
                p.write_field_stop().unwrap();
                p.write_struct_end().unwrap();
                p.write_field_end().unwrap();
                p.write_field_begin(&field("position", TType::I32, 3))
                    .unwrap();
                p.write_i32(i as i32 + 1).unwrap();
                p.write_field_end().unwrap();
                p.write_field_stop().unwrap();
                p.write_struct_end().unwrap();
            }
            p.write_list_end().unwrap();
            p.write_field_end().unwrap();
            p.write_field_stop().unwrap();
            p.write_struct_end().unwrap();
            p.write_field_end().unwrap();
        });
        let (resp, _): (GetResultSetMetadataResp, _) =
            decode_reply(&reply, "GetResultSetMetadata").unwrap();
        let columns = resp.columns.unwrap();
        assert_eq!(columns[0].column_name, "t.id");
        assert_eq!(columns[0].type_name, "int");
        assert_eq!(columns[1].type_name, "decimal");
        assert_eq!(columns[1].position, 2);
    }
}
