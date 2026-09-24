//! rustls based TLS for the Postgres driver (no OpenSSL).

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_postgres::tls::MakeTlsConnect;
use tokio_postgres_rustls::MakeRustlsConnect;

use crate::config::SslConfig;
use crate::error::{DriverError, Result};

/// `MakeTlsConnect` built from [`SslConfig`]; honours
/// `CUBEJS_DB_SSL_SERVERNAME` by overriding the SNI/verification host name.
#[derive(Clone)]
pub struct CubeTls {
    inner: MakeRustlsConnect,
    servername: Option<String>,
}

impl std::fmt::Debug for CubeTls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CubeTls")
            .field("servername", &self.servername)
            .finish()
    }
}

impl CubeTls {
    /// Builds the connector for `ssl`.
    pub fn new(ssl: &SslConfig) -> Result<Self> {
        Ok(Self {
            inner: MakeRustlsConnect::new(client_config(ssl)?),
            servername: ssl.servername.clone(),
        })
    }
}

impl<S> MakeTlsConnect<S> for CubeTls
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    type Stream = <MakeRustlsConnect as MakeTlsConnect<S>>::Stream;
    type TlsConnect = <MakeRustlsConnect as MakeTlsConnect<S>>::TlsConnect;
    type Error = <MakeRustlsConnect as MakeTlsConnect<S>>::Error;

    fn make_tls_connect(
        &mut self,
        domain: &str,
    ) -> std::result::Result<Self::TlsConnect, Self::Error> {
        let domain = self.servername.as_deref().unwrap_or(domain);
        <MakeRustlsConnect as MakeTlsConnect<S>>::make_tls_connect(&mut self.inner, domain)
    }
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Builds the rustls [`ClientConfig`] equivalent of Node's `tls.ConnectionOptions`.
pub fn client_config(ssl: &SslConfig) -> Result<ClientConfig> {
    if ssl.passphrase.is_some() {
        return Err(DriverError::Config(
            "CUBEJS_DB_SSL_PASSPHRASE (encrypted private keys) is not supported by the Rust driver; provide an unencrypted key".to_string(),
        ));
    }
    if ssl.ciphers.is_some() {
        log::warn!(
            "CUBEJS_DB_SSL_CIPHERS is ignored: rustls does not accept OpenSSL cipher strings"
        );
    }

    let provider = provider();
    let builder = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| DriverError::Config(format!("TLS configuration error: {e}")))?;

    let builder = if ssl.reject_unauthorized {
        // Like Node: an explicit `ca` replaces the default trust store.
        let mut roots = RootCertStore::empty();
        match &ssl.ca {
            Some(ca) => {
                for cert in parse_certs(ca, "CUBEJS_DB_SSL_CA")? {
                    roots
                        .add(cert)
                        .map_err(|e| DriverError::Config(format!("Invalid CA certificate: {e}")))?;
                }
            }
            None => roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned()),
        }
        builder.with_root_certificates(roots)
    } else {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoCertificateVerification(provider)))
    };

    let config = match (&ssl.cert, &ssl.key) {
        (Some(cert), Some(key)) => {
            let certs = parse_certs(cert, "CUBEJS_DB_SSL_CERT")?;
            let key = parse_key(key)?;
            builder
                .with_client_auth_cert(certs, key)
                .map_err(|e| DriverError::Config(format!("Invalid client certificate/key: {e}")))?
        }
        (None, None) => builder.with_no_client_auth(),
        _ => {
            return Err(DriverError::Config(
                "CUBEJS_DB_SSL_CERT and CUBEJS_DB_SSL_KEY must be set together".to_string(),
            ))
        }
    };

    Ok(config)
}

fn parse_certs(pem: &str, what: &str) -> Result<Vec<CertificateDer<'static>>> {
    let certs = rustls_pemfile::certs(&mut pem.as_bytes())
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| DriverError::Config(format!("{what} is not a valid PEM certificate: {e}")))?;
    if certs.is_empty() {
        return Err(DriverError::Config(format!(
            "{what} does not contain any certificate"
        )));
    }
    Ok(certs)
}

fn parse_key(pem: &str) -> Result<PrivateKeyDer<'static>> {
    rustls_pemfile::private_key(&mut pem.as_bytes())
        .map_err(|e| DriverError::Config(format!("CUBEJS_DB_SSL_KEY is not a valid PEM key: {e}")))?
        .ok_or_else(|| {
            DriverError::Config("CUBEJS_DB_SSL_KEY does not contain a private key".to_string())
        })
}

/// `rejectUnauthorized: false`: accept any server certificate but still
/// verify handshake signatures.
#[derive(Debug)]
struct NoCertificateVerification(Arc<CryptoProvider>);

impl ServerCertVerifier for NoCertificateVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_permissive_and_strict_configs() {
        let permissive = SslConfig::default();
        assert!(client_config(&permissive).is_ok());
        let strict = SslConfig {
            reject_unauthorized: true,
            ..Default::default()
        };
        assert!(client_config(&strict).is_ok());
        assert!(CubeTls::new(&strict).is_ok());
    }

    #[test]
    fn rejects_unsupported_material() {
        let with_passphrase = SslConfig {
            passphrase: Some("secret".into()),
            ..Default::default()
        };
        assert!(client_config(&with_passphrase).is_err());
        let half_client_auth = SslConfig {
            cert: Some("-----BEGIN CERTIFICATE-----\nAA==\n-----END CERTIFICATE-----\n".into()),
            ..Default::default()
        };
        assert!(client_config(&half_client_auth).is_err());
        let bad_ca = SslConfig {
            reject_unauthorized: true,
            ca: Some(
                "-----BEGIN CERTIFICATE-----\nnot base64!\n-----END CERTIFICATE-----\n".into(),
            ),
            ..Default::default()
        };
        assert!(client_config(&bad_ca).is_err());
    }
}
