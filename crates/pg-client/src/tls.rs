//! TLS com rustls, nos modos do libpq.
//!
//! `Prefer` e `Require` cifram sem conferir o certificado, como o libpq faz;
//! `VerifyFull` confere o certificado contra as autoridades do sistema e o
//! nome do servidor.

use std::sync::Arc;

use pgstudio_core::connection::SslMode;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use tokio_postgres_rustls::MakeRustlsConnect;

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Conector TLS para o modo pedido. Em `Disable` ele nunca é usado.
pub fn connector(mode: SslMode) -> Result<MakeRustlsConnect, String> {
    let provider = provider();
    let builder = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?;
    let config = match mode {
        SslMode::VerifyFull => {
            let mut roots = RootCertStore::empty();
            let found = rustls_native_certs::load_native_certs();
            let (added, _) = roots.add_parsable_certificates(found.certs);
            if added == 0 {
                return Err(
                    "No trusted root certificates were found on this system, so the server \
                     certificate cannot be verified."
                        .into(),
                );
            }
            builder.with_root_certificates(roots).with_no_client_auth()
        }
        SslMode::Disable | SslMode::Prefer | SslMode::Require => builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerify(
                provider.signature_verification_algorithms,
            )))
            .with_no_client_auth(),
    };
    Ok(MakeRustlsConnect::new(config))
}

/// Aceita qualquer certificado, mas ainda confere as assinaturas do
/// handshake.
#[derive(Debug)]
struct NoVerify(WebPkiSupportedAlgorithms);

impl ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mode_builds_a_connector() {
        for mode in [SslMode::Disable, SslMode::Prefer, SslMode::Require] {
            assert!(connector(mode).is_ok(), "{mode:?}");
        }
        // VerifyFull depende dos certificados do sistema; basta não entrar
        // em pânico.
        let _ = connector(SslMode::VerifyFull);
    }
}
