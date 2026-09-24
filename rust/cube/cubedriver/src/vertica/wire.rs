//! A minimal client for Vertica's frontend/backend protocol.
//!
//! Vertica's protocol is derived from PostgreSQL's v3 protocol, but
//! `tokio-postgres` cannot drive it: Vertica numbers its types with its own
//! OIDs (6 = `int`, 9 = `varchar`, 16 = `numeric`, which is PostgreSQL's
//! `bool`), so every extended-protocol statement makes `tokio-postgres`
//! resolve "unknown" OIDs through `pg_catalog.pg_type`, which Vertica does not
//! have, and binary results would be decoded with PostgreSQL's layouts. The
//! newer Vertica protocol versions (3.5+) also change `RowDescription` and
//! `ParameterDescription`.
//!
//! What is implemented is what the driver needs, following `vertica-python`:
//!
//! * start-up asking for protocol **3.8** (the newest one Vertica 9.2 speaks,
//!   and the last before complex types changed `RowDescription` again), with
//!   `autocommit=on`. Servers that answer with 3.0 get PostgreSQL's
//!   `RowDescription` layout, newer ones Vertica's (see
//!   [`parse_row_description`]);
//! * optional TLS (`SSLRequest`, then rustls);
//! * password authentication: cleartext, MD5 and Vertica's salted SHA-512 /
//!   MD5 hashes;
//! * the simple query protocol with text results. Parameters are interpolated
//!   client-side, as `vertica-python` does by default.
//!
//! Not implemented, each failing with a named error: Kerberos/GSS, OAuth,
//! TOTP and `crypt` authentication, and `COPY … FROM LOCAL`/`STDIN`.

use std::collections::HashMap;
use std::sync::Arc;

use bytes::{Buf, BufMut, BytesMut};
use md5::{Digest as _, Md5};
use sha2::Sha512;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::config::SslConfig;
use crate::error::{DriverError, Result};

/// Fixed protocol version of the start-up packet (servers before protocol
/// 3.7 use it as is).
pub const FIXED_PROTOCOL_VERSION: i32 = 3 << 16 | 5;
/// Protocol version requested through the `protocol_version` parameter.
pub const REQUESTED_PROTOCOL_VERSION: u32 = 3 << 16 | 8;
/// First protocol version with Vertica's own `RowDescription` layout.
const VERTICA_ROW_DESCRIPTION_VERSION: u32 = 3 << 16 | 5;
/// `SSLRequest` code.
const SSL_REQUEST_CODE: i32 = 80_877_103;

/// Connection parameters.
#[derive(Debug, Clone)]
pub struct ConnectOptions {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: Option<String>,
    pub database: Option<String>,
    pub ssl: Option<SslConfig>,
}

trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

/// Column of a result set: its name and Vertica type OID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub type_oid: u32,
    pub type_modifier: i32,
}

/// One result set of a simple query.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResultSet {
    pub fields: Vec<Field>,
    /// Text values, `None` for `NULL`.
    pub rows: Vec<Vec<Option<String>>>,
    /// `CommandComplete` tag.
    pub command: String,
}

/// An open, authenticated connection.
pub struct Connection {
    stream: Box<dyn Stream>,
    buf: BytesMut,
    /// `ParameterStatus` values reported by the server.
    pub parameters: HashMap<String, String>,
    /// Set while a request is in flight; a connection dropped half-way
    /// through a query is not reusable.
    pub broken: bool,
    /// Negotiated protocol version (`protocol_version` parameter status;
    /// servers that do not report it use the fixed 3.5).
    pub protocol_version: u32,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("parameters", &self.parameters)
            .field("broken", &self.broken)
            .finish()
    }
}

/// A server `ErrorResponse`.
fn error_response(body: &[u8]) -> DriverError {
    let mut message = None;
    let mut code = None;
    for field in body.split(|b| *b == 0) {
        let Some((&kind, value)) = field.split_first() else {
            continue;
        };
        let value = String::from_utf8_lossy(value).into_owned();
        match kind {
            b'M' => message = Some(value),
            b'C' => code = Some(value),
            _ => {}
        }
    }
    DriverError::Database {
        message: message.unwrap_or_else(|| "Unknown Vertica error".to_string()),
        code,
    }
}

fn read_cstr(buf: &mut &[u8]) -> Result<String> {
    let end = buf
        .iter()
        .position(|b| *b == 0)
        .ok_or_else(|| protocol_error("unterminated string"))?;
    let s = String::from_utf8_lossy(&buf[..end]).into_owned();
    buf.advance(end + 1);
    Ok(s)
}

fn protocol_error(what: &str) -> DriverError {
    DriverError::Query(format!("Vertica protocol error: {what}"))
}

fn need(buf: &[u8], n: usize) -> Result<()> {
    if buf.len() < n {
        Err(protocol_error("truncated message"))
    } else {
        Ok(())
    }
}

/// Parses a `RowDescription` of protocol `version`.
pub fn parse_row_description(body: &[u8], version: u32) -> Result<Vec<Field>> {
    if version >= VERTICA_ROW_DESCRIPTION_VERSION {
        parse_vertica_row_description(body)
    } else {
        parse_pg_row_description(body)
    }
}

/// Vertica's layout (protocol 3.5+, without complex types): a pool of
/// user-defined types, then per field its name, a 64-bit table OID (followed
/// by schema and table names when not 0), the attribute number,
/// `is_non_native`, the type OID (an index into the pool for non-native
/// types), size, nullability, identity, type modifier and format.
pub fn parse_vertica_row_description(mut body: &[u8]) -> Result<Vec<Field>> {
    need(body, 2)?;
    let count = body.get_u16();
    if count == 0 {
        return Ok(Vec::new());
    }
    need(body, 4)?;
    let pool_size = body.get_u32();
    let mut user_types = Vec::with_capacity(pool_size as usize);
    for _ in 0..pool_size {
        need(body, 4)?;
        let base_oid = body.get_u32();
        let _name = read_cstr(&mut body)?;
        user_types.push(base_oid);
    }
    let mut fields = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let name = read_cstr(&mut body)?;
        need(body, 8)?;
        let table_oid = body.get_u64();
        if table_oid != 0 {
            let _schema = read_cstr(&mut body)?;
            let _table = read_cstr(&mut body)?;
        }
        need(body, 2 + 1 + 4 + 2 + 2 + 2 + 4 + 2)?;
        let _attribute_number = body.get_u16();
        let non_native = body.get_u8();
        let type_ref = body.get_u32();
        let _size = body.get_i16();
        let _null_ok = body.get_u16();
        let _identity = body.get_u16();
        let type_modifier = body.get_i32();
        let _format = body.get_u16();
        let type_oid = if non_native == 1 {
            *user_types
                .get(type_ref as usize)
                .ok_or_else(|| protocol_error("unknown user-defined type"))?
        } else {
            type_ref
        };
        fields.push(Field {
            name,
            type_oid,
            type_modifier,
        });
    }
    Ok(fields)
}

/// PostgreSQL's layout, which Vertica uses for protocol 3.0.
pub fn parse_pg_row_description(mut body: &[u8]) -> Result<Vec<Field>> {
    need(body, 2)?;
    let count = body.get_i16();
    let mut fields = Vec::with_capacity(count.max(0) as usize);
    for _ in 0..count {
        let name = read_cstr(&mut body)?;
        need(body, 18)?;
        let _table_oid = body.get_i32();
        let _column = body.get_i16();
        let type_oid = body.get_u32();
        let _type_size = body.get_i16();
        let type_modifier = body.get_i32();
        let _format = body.get_i16();
        fields.push(Field {
            name,
            type_oid,
            type_modifier,
        });
    }
    Ok(fields)
}

/// Parses a `DataRow`.
pub fn parse_data_row(mut body: &[u8]) -> Result<Vec<Option<String>>> {
    need(body, 2)?;
    let count = body.get_i16();
    let mut values = Vec::with_capacity(count.max(0) as usize);
    for _ in 0..count {
        need(body, 4)?;
        let len = body.get_i32();
        if len < 0 {
            values.push(None);
            continue;
        }
        let len = len as usize;
        need(body, len)?;
        values.push(Some(String::from_utf8_lossy(&body[..len]).into_owned()));
        body.advance(len);
    }
    Ok(values)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `MD5(MD5(password + user) + salt)`, prefixed with `md5`.
pub fn md5_password(password: &str, user: &str, salt: &[u8]) -> String {
    let inner = hex(&Md5::digest(
        [password.as_bytes(), user.as_bytes()].concat(),
    ));
    let outer = hex(&Md5::digest([inner.as_bytes(), salt].concat()));
    format!("md5{outer}")
}

/// `SHA512(SHA512(password + user_salt) + salt)`, prefixed with `sha512`.
pub fn sha512_password(password: &str, user_salt: &[u8], salt: &[u8]) -> String {
    let inner = hex(&Sha512::digest([password.as_bytes(), user_salt].concat()));
    let outer = hex(&Sha512::digest([inner.as_bytes(), salt].concat()));
    format!("sha512{outer}")
}

/// Builds the start-up message.
pub fn startup_message(opts: &ConnectOptions) -> Vec<u8> {
    let mut body = BytesMut::new();
    body.put_i32(FIXED_PROTOCOL_VERSION);
    // `pack('!16sxIx', b'protocol_version', PROTOCOL_VERSION)`: the value is
    // the raw 32-bit version, as `vertica-python` sends it.
    body.put_slice(b"protocol_version\0");
    body.put_u32(REQUESTED_PROTOCOL_VERSION);
    body.put_u8(0);
    let mut param = |k: &str, v: &str| {
        body.put_slice(k.as_bytes());
        body.put_u8(0);
        body.put_slice(v.as_bytes());
        body.put_u8(0);
    };
    param("user", &opts.user);
    if let Some(db) = opts.database.as_deref().filter(|d| !d.is_empty()) {
        param("database", db);
    }
    param("autocommit", "on");
    param("binary_data_protocol", "0");
    param("client_type", "cubejs");
    body.put_u8(0);
    let mut msg = Vec::with_capacity(body.len() + 4);
    msg.extend_from_slice(&((body.len() + 4) as i32).to_be_bytes());
    msg.extend_from_slice(&body);
    msg
}

fn frontend_message(tag: u8, payload: &[u8]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(payload.len() + 5);
    msg.push(tag);
    msg.extend_from_slice(&((payload.len() + 4) as i32).to_be_bytes());
    msg.extend_from_slice(payload);
    msg
}

fn cstr_payload(s: &str) -> Vec<u8> {
    let mut v = s.as_bytes().to_vec();
    v.push(0);
    v
}

impl Connection {
    /// Connects, negotiates TLS when configured and authenticates.
    pub async fn connect(opts: &ConnectOptions) -> Result<Self> {
        let tcp = TcpStream::connect((opts.host.as_str(), opts.port))
            .await
            .map_err(|e| {
                DriverError::Query(format!(
                    "Unable to connect to Vertica at {}:{}: {e}",
                    opts.host, opts.port
                ))
            })?;
        let _ = tcp.set_nodelay(true);

        let stream: Box<dyn Stream> = match &opts.ssl {
            None => Box::new(tcp),
            Some(ssl) => Box::new(Self::start_tls(tcp, opts, ssl).await?),
        };

        let mut conn = Self {
            stream,
            buf: BytesMut::with_capacity(8192),
            parameters: HashMap::new(),
            broken: true,
            protocol_version: FIXED_PROTOCOL_VERSION as u32,
        };
        conn.write(&startup_message(opts)).await?;
        conn.authenticate(opts).await?;
        conn.broken = false;
        Ok(conn)
    }

    async fn start_tls(
        mut tcp: TcpStream,
        opts: &ConnectOptions,
        ssl: &SslConfig,
    ) -> Result<tokio_rustls::client::TlsStream<TcpStream>> {
        let mut request = Vec::with_capacity(8);
        request.extend_from_slice(&8i32.to_be_bytes());
        request.extend_from_slice(&SSL_REQUEST_CODE.to_be_bytes());
        tcp.write_all(&request).await?;
        let answer = tcp.read_u8().await?;
        if answer != b'S' {
            return Err(DriverError::Connection {
                pool_name: "vertica".to_string(),
                message: "SSL requested but disabled on the server".to_string(),
            });
        }
        let config = crate::postgres::client_config(ssl)?;
        let server_name = ssl.servername.clone().unwrap_or_else(|| opts.host.clone());
        let server_name =
            rustls::pki_types::ServerName::try_from(server_name.clone()).map_err(|e| {
                DriverError::Config(format!("Invalid TLS server name {server_name}: {e}"))
            })?;
        tokio_rustls::TlsConnector::from(Arc::new(config))
            .connect(server_name, tcp)
            .await
            .map_err(|e| DriverError::Connection {
                pool_name: "vertica".to_string(),
                message: format!("TLS handshake failed: {e}"),
            })
    }

    async fn authenticate(&mut self, opts: &ConnectOptions) -> Result<()> {
        let password = opts.password.clone().unwrap_or_default();
        loop {
            let (tag, body) = self.read_message().await?;
            match tag {
                b'R' => {
                    let mut b = &body[..];
                    need(b, 4)?;
                    let code = b.get_i32();
                    let reply = match code {
                        0 => continue,
                        3 => password.clone(),
                        5 | 65_541 => {
                            need(b, 4)?;
                            md5_password(&password, &opts.user, &b[..4])
                        }
                        65_536 | 66_048 => {
                            need(b, 8)?;
                            let salt = b[..4].to_vec();
                            b.advance(4);
                            let user_salt_len = b.get_i32();
                            if user_salt_len != 16 {
                                return Err(protocol_error(&format!(
                                    "wrong user salt size {user_salt_len}"
                                )));
                            }
                            need(b, 16)?;
                            sha512_password(&password, &b[..16], &salt)
                        }
                        other => {
                            let method = match other {
                                2 => "Kerberos V5",
                                4 => "crypt",
                                6 => "SCM credential",
                                7 | 8 => "GSS/Kerberos",
                                9..=11 => "password change",
                                12 => "OAuth",
                                14 => "TOTP",
                                _ => "unknown",
                            };
                            return Err(DriverError::NotImplemented(format!(
                                "Vertica authentication method {other} ({method}) is not supported \
                                 by the Rust driver; use password authentication"
                            )));
                        }
                    };
                    self.write(&frontend_message(b'p', &cstr_payload(&reply)))
                        .await?;
                }
                b'S' => self.parameter_status(&body)?,
                b'K' | b'N' => {}
                b'E' => return Err(error_response(&body)),
                b'Z' => return Ok(()),
                _ => {}
            }
        }
    }

    fn parameter_status(&mut self, body: &[u8]) -> Result<()> {
        let mut b = body;
        let key = read_cstr(&mut b)?;
        let value = read_cstr(&mut b)?;
        if key == "protocol_version" {
            if let Ok(v) = value.parse::<u32>() {
                self.protocol_version = v;
            }
        }
        self.parameters.insert(key, value);
        Ok(())
    }

    async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.stream.write_all(bytes).await?;
        self.stream.flush().await?;
        Ok(())
    }

    async fn read_message(&mut self) -> Result<(u8, BytesMut)> {
        loop {
            if self.buf.len() >= 5 {
                let len = i32::from_be_bytes([self.buf[1], self.buf[2], self.buf[3], self.buf[4]]);
                if len < 4 {
                    return Err(protocol_error("invalid message length"));
                }
                let total = len as usize + 1;
                if self.buf.len() >= total {
                    let mut msg = self.buf.split_to(total);
                    let tag = msg[0];
                    msg.advance(5);
                    return Ok((tag, msg));
                }
                self.buf.reserve(total - self.buf.len());
            }
            let n = self.stream.read_buf(&mut self.buf).await?;
            if n == 0 {
                return Err(protocol_error("connection closed by the server"));
            }
        }
    }

    /// Runs `sql` with the simple query protocol and returns every result set.
    pub async fn simple_query(&mut self, sql: &str) -> Result<Vec<ResultSet>> {
        self.broken = true;
        self.write(&frontend_message(b'Q', &cstr_payload(sql)))
            .await?;
        let mut results = Vec::new();
        let mut current: Option<ResultSet> = None;
        let mut error = None;
        loop {
            let (tag, body) = self.read_message().await?;
            match tag {
                b'T' => {
                    current = Some(ResultSet {
                        fields: parse_row_description(&body, self.protocol_version)?,
                        ..Default::default()
                    })
                }
                b'D' => {
                    let row = parse_data_row(&body)?;
                    current
                        .get_or_insert_with(ResultSet::default)
                        .rows
                        .push(row);
                }
                b'C' => {
                    let mut b = &body[..];
                    let mut result = current.take().unwrap_or_default();
                    result.command = read_cstr(&mut b).unwrap_or_default();
                    results.push(result);
                }
                b'I' => results.push(current.take().unwrap_or_default()),
                b'E' => {
                    // Keep the first error; the server still ends with `Z`.
                    if error.is_none() {
                        error = Some(error_response(&body));
                    }
                    current = None;
                }
                b'S' => self.parameter_status(&body)?,
                // `COPY … FROM STDIN` / `LOCAL`: refused, the server then
                // reports the failure and ends with `Z`.
                b'G' => {
                    self.write(&frontend_message(
                        b'f',
                        &cstr_payload("COPY FROM STDIN is not supported by the Cube driver"),
                    ))
                    .await?;
                    error.get_or_insert_with(|| {
                        DriverError::NotImplemented(
                            "COPY ... FROM STDIN/LOCAL is not supported by the Vertica driver"
                                .to_string(),
                        )
                    });
                }
                b'Z' => break,
                // Notices, `BackendKeyData`, copy-out data and Vertica
                // specific messages carry nothing the driver needs.
                _ => {}
            }
        }
        self.broken = false;
        match error {
            Some(e) => Err(e),
            None => Ok(results),
        }
    }

    /// Sends `Terminate`.
    pub async fn close(mut self) {
        let _ = self.write(&frontend_message(b'X', &[])).await;
        let _ = self.stream.shutdown().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_message_layout() {
        let msg = startup_message(&ConnectOptions {
            host: "h".into(),
            port: 5433,
            user: "dbadmin".into(),
            password: None,
            database: Some("docker".into()),
            ssl: None,
        });
        let len = i32::from_be_bytes([msg[0], msg[1], msg[2], msg[3]]) as usize;
        assert_eq!(len, msg.len());
        assert_eq!(&msg[4..8], &[0, 3, 0, 5]);
        assert_eq!(&msg[8..30], b"protocol_version\0\0\x03\0\x08\0");
        assert_eq!(
            &msg[30..],
            &b"user\0dbadmin\0database\0docker\0autocommit\0on\0binary_data_protocol\x000\0client_type\0cubejs\0\0"[..]
        );
    }

    #[test]
    fn password_hashes() {
        // PostgreSQL's well-known MD5 scheme.
        assert_eq!(
            md5_password("secret", "pw", &[1, 2, 3, 4]),
            format!(
                "md5{}",
                hex(&Md5::digest(
                    [
                        hex(&Md5::digest(b"secretpw")).as_bytes(),
                        &[1u8, 2, 3, 4][..]
                    ]
                    .concat()
                ))
            )
        );
        let h = sha512_password("secret", &[7; 16], &[1, 2, 3, 4]);
        assert!(h.starts_with("sha512"));
        assert_eq!(h.len(), 6 + 128);
    }

    #[test]
    fn parses_row_description_and_data_row() {
        // Captured from Vertica 9.2: `SELECT 1 AS n, 'x'::varchar AS s`.
        let t = b"\x00\x02n\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x06\x00\x08\xff\xff\xff\xff\x00\x00s\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\t\xff\xff\x00\x00\x00\x05\x00\x00";
        let fields = parse_row_description(t, 3 << 16).unwrap();
        assert_eq!(
            fields,
            vec![
                Field {
                    name: "n".into(),
                    type_oid: 6,
                    type_modifier: -1
                },
                Field {
                    name: "s".into(),
                    type_oid: 9,
                    type_modifier: 5
                },
            ]
        );
        // Captured from Vertica 9.2 at protocol 3.8: `SELECT 1 AS n`.
        let t = b"\x00\x01\x00\x00\x00\x00n\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x06\x00\x08\x00\x01\x00\x00\xff\xff\xff\xff\x00\x00";
        assert_eq!(
            parse_row_description(t, 3 << 16 | 8).unwrap(),
            vec![Field {
                name: "n".into(),
                type_oid: 6,
                type_modifier: -1
            }]
        );
        // A column of a table, and a user-defined type from the pool.
        let mut t = vec![0, 1, 0, 0, 0, 1, 0, 0, 0, 9];
        t.extend_from_slice(b"GEOMETRY\0c\0");
        t.extend_from_slice(&7u64.to_be_bytes());
        t.extend_from_slice(b"public\0tab\0");
        t.extend_from_slice(&[0, 1, 1, 0, 0, 0, 0, 0, 8, 0, 1, 0, 0, 0, 0, 0, 3, 0, 0]);
        assert_eq!(
            parse_row_description(&t, 3 << 16 | 8).unwrap(),
            vec![Field {
                name: "c".into(),
                type_oid: 9,
                type_modifier: 3
            }]
        );
        assert!(parse_row_description(&t[..20], 3 << 16 | 8).is_err());

        let d = b"\x00\x03\x00\x00\x00\x011\xff\xff\xff\xff\x00\x00\x00\x00";
        assert_eq!(
            parse_data_row(d).unwrap(),
            vec![Some("1".into()), None, Some(String::new())]
        );
        assert!(parse_data_row(b"\x00\x01\x00\x00\x00\x05ab").is_err());
    }

    #[test]
    fn error_fields() {
        let e =
            error_response(b"SERROR\0C42703\0MColumn \"nope\" does not exist\0Fparse_expr.c\0\0");
        match e {
            DriverError::Database { message, code } => {
                assert_eq!(message, "Column \"nope\" does not exist");
                assert_eq!(code.as_deref(), Some("42703"));
            }
            other => panic!("{other:?}"),
        }
    }
}
