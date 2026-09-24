//! HiveServer2 binary transport over `tokio::net::TcpStream`.
//!
//! * `PLAIN`: the SASL handshake of `TSaslTransport.js` (`START "PLAIN"`,
//!   `OK "<authzid>\0<user>\0<password>"`, expecting `COMPLETE`), after which
//!   every Thrift message travels in a frame prefixed by its 4-byte
//!   big-endian length. This is what HiveServer2 speaks with
//!   `hive.server2.authentication=NONE` (the default) or `LDAP`/`CUSTOM`.
//! * `NOSASL`: raw, unframed Thrift messages
//!   (`hive.server2.authentication=NOSASL`). Replies are decoded as bytes
//!   arrive, since nothing announces their length.
//!
//! Messages are encoded/decoded with the `thrift` crate's binary protocol
//! (see [`super::tcli`]); only the I/O is done here, asynchronously.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::tcli::{decode_reply, DecodeError, Request, Response};
use crate::error::{DriverError, Result};

const SASL_START: u8 = 1;
const SASL_OK: u8 = 2;
const SASL_BAD: u8 = 3;
const SASL_ERROR: u8 = 4;
const SASL_COMPLETE: u8 = 5;
/// `TSaslTransport.receiveSaslMessage` payload limit (100 MiB).
const MAX_SASL_PAYLOAD: u32 = 104_857_600;
/// Refuse absurd frames rather than allocating them.
const MAX_FRAME: u32 = 1 << 30;

/// `auth` of the Node.js driver (`PLAIN` by default).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Auth {
    NoSasl,
    Plain {
        authzid: String,
        username: String,
        password: String,
    },
}

/// One Thrift connection to HiveServer2.
pub struct Connection {
    stream: TcpStream,
    framed: bool,
    sequence: i32,
    buffer: Vec<u8>,
}

fn connection_error(message: impl std::fmt::Display) -> DriverError {
    DriverError::Connection {
        pool_name: "hive".to_string(),
        message: message.to_string(),
    }
}

impl Connection {
    /// Opens the socket and performs the SASL handshake when needed.
    pub async fn connect(host: &str, port: u16, auth: &Auth, timeout: Duration) -> Result<Self> {
        let stream = tokio::time::timeout(timeout, TcpStream::connect((host, port)))
            .await
            .map_err(|_| connection_error(format!("connect to {host}:{port} timed out")))?
            .map_err(|e| connection_error(format!("{host}:{port}: {e}")))?;
        stream.set_nodelay(true).ok();
        let mut connection = Self {
            stream,
            framed: matches!(auth, Auth::Plain { .. }),
            sequence: 0,
            buffer: Vec::new(),
        };
        if let Auth::Plain {
            authzid,
            username,
            password,
        } = auth
        {
            tokio::time::timeout(timeout, connection.sasl_plain(authzid, username, password))
                .await
                .map_err(|_| connection_error("SASL handshake timed out"))??;
        }
        Ok(connection)
    }

    async fn send_sasl(&mut self, status: u8, payload: &[u8]) -> Result<()> {
        let mut message = Vec::with_capacity(5 + payload.len());
        message.push(status);
        message.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        message.extend_from_slice(payload);
        self.stream
            .write_all(&message)
            .await
            .map_err(connection_error)
    }

    async fn sasl_plain(&mut self, authzid: &str, username: &str, password: &str) -> Result<()> {
        self.send_sasl(SASL_START, b"PLAIN").await?;
        // `sasl-plain`: authzid NUL authcid NUL passwd.
        let payload = format!("{authzid}\0{username}\0{password}");
        self.send_sasl(SASL_OK, payload.as_bytes()).await?;

        let mut header = [0u8; 5];
        self.stream
            .read_exact(&mut header)
            .await
            .map_err(|e| connection_error(format!("SASL handshake failed: {e}")))?;
        let status = header[0];
        let size = u32::from_be_bytes([header[1], header[2], header[3], header[4]]);
        if size > MAX_SASL_PAYLOAD {
            return Err(connection_error(format!(
                "Incorrect payload size in SASL message: {size}"
            )));
        }
        let mut payload = vec![0u8; size as usize];
        self.stream
            .read_exact(&mut payload)
            .await
            .map_err(connection_error)?;
        let text = String::from_utf8_lossy(&payload);
        match status {
            SASL_COMPLETE => Ok(()),
            SASL_BAD | SASL_ERROR => Err(connection_error(format!("SASL Error: {text}"))),
            other => Err(connection_error(format!(
                "SASL Failed with status {other}: {text}"
            ))),
        }
    }

    /// Sends `request` and decodes the reply.
    pub async fn call<R: Response>(&mut self, request: &Request<'_>) -> Result<R> {
        self.sequence = self.sequence.wrapping_add(1);
        let method = request.method();
        let bytes = request
            .encode(self.sequence)
            .map_err(|e| DriverError::Query(format!("Unable to encode {method}: {e}")))?;
        let mut out = Vec::with_capacity(bytes.len() + 4);
        if self.framed {
            out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        }
        out.extend_from_slice(&bytes);
        self.stream
            .write_all(&out)
            .await
            .map_err(connection_error)?;
        self.stream.flush().await.map_err(connection_error)?;

        if self.framed {
            let mut len = [0u8; 4];
            self.stream
                .read_exact(&mut len)
                .await
                .map_err(connection_error)?;
            let len = u32::from_be_bytes(len);
            if len > MAX_FRAME {
                return Err(connection_error(format!("Frame of {len} bytes refused")));
            }
            let mut frame = vec![0u8; len as usize];
            self.stream
                .read_exact(&mut frame)
                .await
                .map_err(connection_error)?;
            return match decode_reply::<R>(&frame, method) {
                Ok((response, _)) => Ok(response),
                Err(e) => Err(decode_error(e, method)),
            };
        }

        // Unframed: accumulate until a whole reply decodes.
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            if !self.buffer.is_empty() {
                match decode_reply::<R>(&self.buffer, method) {
                    Ok((response, used)) => {
                        self.buffer.drain(..used);
                        return Ok(response);
                    }
                    Err(DecodeError::Incomplete) => {}
                    Err(e) => {
                        self.buffer.clear();
                        return Err(decode_error(e, method));
                    }
                }
            }
            let n = self
                .stream
                .read(&mut chunk)
                .await
                .map_err(connection_error)?;
            if n == 0 {
                return Err(connection_error(format!(
                    "Connection closed by HiveServer2 while waiting for {method}"
                )));
            }
            self.buffer.extend_from_slice(&chunk[..n]);
        }
    }

    /// Closes the socket.
    pub async fn shutdown(mut self) {
        let _ = self.stream.shutdown().await;
    }
}

fn decode_error(e: DecodeError, method: &str) -> DriverError {
    match e {
        DecodeError::Incomplete => {
            connection_error(format!("Truncated reply from HiveServer2 to {method}"))
        }
        DecodeError::Application(m) => DriverError::Query(m),
        DecodeError::Protocol(m) => connection_error(format!("{method}: {m}")),
    }
}
