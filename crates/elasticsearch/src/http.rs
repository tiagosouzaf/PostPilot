//! O cliente HTTP do Elasticsearch: um `ureq::Agent` com o endereço, o
//! TLS e a credencial da conexão.

use std::sync::Arc;
use std::time::Duration;

use pgstudio_core::connection::{ConnectionSettings, SslMode};
use ureq::Agent;
use ureq::tls::{Certificate, RootCerts, TlsConfig, TlsProvider};

use crate::json::Json;

/// Quanto esperar por um servidor que não responde.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Maior resposta lida (uma busca enorme ainda cabe; além disso é melhor
/// paginar).
const MAX_BODY: u64 = 256 * 1024 * 1024;

/// O que deu errado antes de haver uma resposta HTTP.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HttpError {
    /// Rede: não conectou, caiu, não respondeu.
    Network(String),
    /// TLS: certificado, ou o servidor fala outro protocolo.
    Tls(String),
}

impl HttpError {
    pub fn message(&self) -> &str {
        match self {
            HttpError::Network(m) | HttpError::Tls(m) => m,
        }
    }
}

/// Uma resposta do servidor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    pub content_type: String,
    pub body: String,
}

impl Reply {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn json(&self) -> Option<Json> {
        Json::parse(&self.body).ok()
    }

    /// O erro como o servidor explica: `tipo: motivo` do `error` da
    /// resposta, ou o corpo quando não é JSON.
    pub fn error_text(&self) -> String {
        let json = self.json();
        let error = json.as_ref().and_then(|j| j.get("error"));
        let reason = error.and_then(|e| match e {
            Json::String(s) => Some(s.clone()),
            _ => {
                // A causa de dentro costuma dizer mais que a de fora.
                let cause = e
                    .get("root_cause")
                    .and_then(Json::as_array)
                    .and_then(|c| c.first())
                    .unwrap_or(e);
                let reason = cause.get("reason").and_then(Json::as_str)?;
                Some(match cause.get("type").and_then(Json::as_str) {
                    Some(ty) => format!("{ty}: {reason}"),
                    None => reason.to_string(),
                })
            }
        });
        match reason {
            Some(r) => r,
            None if self.body.trim().is_empty() => format!("HTTP {}", self.status),
            None => self.body.trim().to_string(),
        }
    }
}

/// O alfabeto do Base64 (RFC 4648), para o `Authorization: Basic`.
fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(TABLE[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// O caminho como vai na URL: o que não pode ir cru (espaço, acentos,
/// aspas) vai em `%XX`; o resto fica como foi escrito.
pub fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.bytes() {
        let keep = b.is_ascii_alphanumeric() || b"/?&=%-._~*,:;+!$'()@[]".contains(&b);
        if keep {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Fala com um Elasticsearch.
#[derive(Clone)]
pub struct EsClient {
    agent: Agent,
    /// `http://host:porta`, sem a barra do fim.
    base: String,
    authorization: Option<String>,
}

impl EsClient {
    /// `password` com login é a senha (`Basic`); sem login é uma chave de
    /// API (`ApiKey`), ou nada num servidor sem segurança.
    pub fn new(settings: &ConnectionSettings, password: &str) -> EsClient {
        let scheme = match settings.ssl {
            SslMode::Disable => "http",
            _ => "https",
        };
        let host = if settings.host.contains(':') && !settings.host.starts_with('[') {
            format!("[{}]", settings.host)
        } else {
            settings.host.clone()
        };
        let base = format!("{scheme}://{host}:{}", settings.port);
        let user = settings.user.trim();
        let authorization = if !user.is_empty() {
            Some(format!(
                "Basic {}",
                base64(format!("{user}:{password}").as_bytes())
            ))
        } else if !password.is_empty() {
            Some(format!("ApiKey {password}"))
        } else {
            None
        };
        let mut tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()));
        if settings.ssl == SslMode::VerifyFull {
            // As raízes do sistema, como nos outros motores.
            let found = rustls_native_certs::load_native_certs();
            let certs: Vec<Certificate<'static>> = found
                .certs
                .iter()
                .map(|c| Certificate::from_der(c.as_ref()).to_owned())
                .collect();
            tls = tls.root_certs(RootCerts::new_with_certs(&certs));
        } else {
            tls = tls.disable_verification(true);
        }
        let agent = Agent::config_builder()
            .tls_config(tls.build())
            .user_agent("PostPilot")
            // Direto ao servidor, como as outras conexões de banco: o
            // proxy do ambiente é para a internet.
            .proxy(None)
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .http_status_as_error(false)
            .max_redirects(0)
            .build()
            .into();
        EsClient {
            agent,
            base,
            authorization,
        }
    }

    /// Manda uma requisição. `body` é o corpo e o `Content-Type`. Um
    /// `GET` com corpo vai como está: o Elasticsearch aceita.
    pub fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<(&str, &str)>,
    ) -> Result<Reply, HttpError> {
        let url = format!("{}{}", self.base, encode_path(path));
        let mut builder = ureq::http::Request::builder()
            .method(method)
            .uri(&url)
            .header("Accept", "application/json, text/plain");
        if let Some(authorization) = &self.authorization {
            builder = builder.header("Authorization", authorization);
        }
        let invalid = |e: ureq::http::Error| HttpError::Network(e.to_string());
        let result = match body {
            Some((text, content_type)) => self.agent.run(
                builder
                    .header("Content-Type", content_type)
                    .body(text.to_string())
                    .map_err(invalid)?,
            ),
            None => self.agent.run(builder.body(()).map_err(invalid)?),
        };
        let mut response = result.map_err(transport_error)?;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_BODY)
            .read_to_string()
            .map_err(transport_error)?;
        Ok(Reply {
            status,
            content_type,
            body,
        })
    }

    /// Um `GET` que tem de dar certo e responder JSON.
    pub fn get_json(&self, path: &str) -> Result<Json, String> {
        let reply = self
            .request("GET", path, None)
            .map_err(|e| e.message().to_string())?;
        if !reply.ok() {
            return Err(reply.error_text());
        }
        Json::parse(&reply.body)
    }
}

fn transport_error(e: ureq::Error) -> HttpError {
    let text = e.to_string();
    let lower = text.to_lowercase();
    match e {
        ureq::Error::Tls(_) | ureq::Error::Rustls(_) => HttpError::Tls(text),
        _ if lower.contains("certificate") || lower.contains("tls") => HttpError::Tls(text),
        _ => HttpError::Network(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_and_paths() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(
            base64(b"elastic:PostPilot!2026"),
            "ZWxhc3RpYzpQb3N0UGlsb3QhMjAyNg=="
        );
        assert_eq!(
            encode_path("/meu índice/_search?q=nome:ana&size=1"),
            "/meu%20%C3%ADndice/_search?q=nome:ana&size=1"
        );
        assert_eq!(
            encode_path("/logs-*,-logs-old/_count"),
            "/logs-*,-logs-old/_count"
        );
    }

    #[test]
    fn errors_come_from_the_reply() {
        let reply = |body: &str| Reply {
            status: 404,
            content_type: "application/json".into(),
            body: body.into(),
        };
        let r = reply(
            r#"{"error":{"root_cause":[{"type":"index_not_found_exception","reason":"no such index [x]"}],"type":"outer","reason":"outer reason"},"status":404}"#,
        );
        assert_eq!(
            r.error_text(),
            "index_not_found_exception: no such index [x]"
        );
        assert_eq!(
            reply(r#"{"error":"Incorrect HTTP method"}"#).error_text(),
            "Incorrect HTTP method"
        );
        assert_eq!(reply("").error_text(), "HTTP 404");
        assert_eq!(reply("not json").error_text(), "not json");
        assert!(!r.ok());
    }
}
