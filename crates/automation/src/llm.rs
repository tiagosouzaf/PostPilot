//! Provedores de IA por chave de API, além do Claude Code.
//!
//! Todos falam o mesmo protocolo: o `chat/completions` da OpenAI, que a
//! OpenRouter, a Z.ai (GLM) e quase todo serviço e servidor local (Ollama,
//! LM Studio...) também atendem. Um provedor é então só uma URL base, um
//! modelo e uma chave; os que têm nome aqui ([`ProviderKind`]) só poupam o
//! usuário de digitar a URL.
//!
//! A chave não fica no [`Provider`]: quem guarda é a interface (cofre do
//! sistema), que monta a [`Connection`] na hora de usar.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use ureq::Agent;
use ureq::tls::{Certificate, RootCerts, TlsConfig, TlsProvider};

/// Maior resposta de erro ou lista de modelos lida.
const MAX_SMALL: u64 = 4 * 1024 * 1024;

/// Os provedores com URL conhecida.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    /// OpenAI (os modelos do ChatGPT), com chave de platform.openai.com.
    OpenAi,
    /// OpenRouter: uma chave para modelos de vários fornecedores.
    OpenRouter,
    /// GLM pela Z.ai, com a chave da assinatura GLM Coding Plan.
    Glm,
    /// Qualquer outro serviço compatível com a API da OpenAI.
    #[default]
    #[serde(other)]
    Custom,
}

impl ProviderKind {
    /// Na ordem do menu "Add".
    pub const ALL: [ProviderKind; 4] = [
        ProviderKind::OpenAi,
        ProviderKind::OpenRouter,
        ProviderKind::Glm,
        ProviderKind::Custom,
    ];

    /// Nome no menu e nome inicial do provedor (nomes de produto, sem
    /// tradução; o `Custom` é traduzido por quem exibe).
    pub fn label(self) -> &'static str {
        match self {
            ProviderKind::OpenAi => "OpenAI (ChatGPT)",
            ProviderKind::OpenRouter => "OpenRouter",
            ProviderKind::Glm => "GLM (Z.ai)",
            ProviderKind::Custom => "OpenAI-compatible",
        }
    }

    /// URL base inicial.
    pub fn base_url(self) -> &'static str {
        match self {
            ProviderKind::OpenAi => "https://api.openai.com/v1",
            ProviderKind::OpenRouter => "https://openrouter.ai/api/v1",
            // O endereço da assinatura (GLM Coding Plan); a chave avulsa,
            // paga por uso, usa .../api/paas/v4.
            ProviderKind::Glm => "https://api.z.ai/api/coding/paas/v4",
            ProviderKind::Custom => "http://localhost:11434/v1",
        }
    }

    /// Modelo inicial; o usuário troca (o botão de listar modelos ajuda).
    pub fn model(self) -> &'static str {
        match self {
            ProviderKind::OpenAi => "gpt-5",
            ProviderKind::OpenRouter => "openai/gpt-5",
            ProviderKind::Glm => "glm-4.6",
            ProviderKind::Custom => "",
        }
    }
}

/// Um provedor configurado (sem a chave).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Provider {
    /// Identificador estável: é a chave no cofre e a escolha salva.
    pub id: String,
    pub kind: ProviderKind,
    /// Nome que o usuário vê.
    pub name: String,
    pub base_url: String,
    pub model: String,
}

impl Provider {
    /// Um provedor novo do tipo `kind`, com o `id` dado.
    pub fn new(kind: ProviderKind, id: impl Into<String>) -> Provider {
        Provider {
            id: id.into(),
            kind,
            name: kind.label().to_string(),
            base_url: kind.base_url().to_string(),
            model: kind.model().to_string(),
        }
    }

    /// O provedor pronto para chamar, com a chave.
    pub fn connect(&self, api_key: &str) -> Connection {
        Connection {
            base_url: self.base_url.trim().trim_end_matches('/').to_string(),
            api_key: api_key.trim().to_string(),
            model: self.model.trim().to_string(),
        }
    }
}

/// URL, modelo e chave de uma chamada.
#[derive(Clone, PartialEq, Eq)]
pub struct Connection {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

impl std::fmt::Debug for Connection {
    /// Sem a chave: isto pode parar num log.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl Connection {
    /// O servidor, para dizer ao usuário para onde os dados vão.
    pub fn host(&self) -> &str {
        let rest = self
            .base_url
            .split_once("://")
            .map_or(self.base_url.as_str(), |(_, r)| r);
        rest.split('/').next().unwrap_or(rest)
    }
}

/// Uma ferramenta que o modelo pediu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolRequest {
    pub id: String,
    pub name: String,
    /// Os argumentos como o modelo escreveu (JSON em texto).
    pub arguments: String,
}

/// A resposta de uma rodada.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Answer {
    pub content: String,
    pub tool_calls: Vec<ToolRequest>,
}

/// A rodada foi interrompida pelo usuário.
pub const CANCELLED: &str = "cancelled";

fn agent() -> Agent {
    // As raízes do sistema, como o resto do app (proxy corporativo com
    // autoridade própria instalada funciona).
    let found = rustls_native_certs::load_native_certs();
    let certs: Vec<Certificate<'static>> = found
        .certs
        .iter()
        .map(|c| Certificate::from_der(c.as_ref()).to_owned())
        .collect();
    let tls = TlsConfig::builder()
        .provider(TlsProvider::Rustls)
        .root_certs(RootCerts::new_with_certs(&certs))
        .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .build();
    Agent::config_builder()
        .tls_config(tls)
        .user_agent(concat!("PostPilot/", env!("CARGO_PKG_VERSION")))
        .timeout_connect(Some(Duration::from_secs(20)))
        // Modelos que "pensam" demoram para começar a responder.
        .timeout_recv_response(Some(Duration::from_secs(300)))
        .timeout_recv_body(Some(Duration::from_secs(900)))
        .http_status_as_error(false)
        .build()
        .into()
}

/// A mensagem de erro de uma resposta que não deu certo.
fn http_error(status: u16, body: &str) -> String {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            let e = &v["error"];
            e["message"]
                .as_str()
                .or_else(|| e.as_str())
                .or_else(|| v["message"].as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| body.trim().chars().take(300).collect());
    let hint = match status {
        401 | 403 => " (check the API key)",
        404 => " (check the base URL and the model)",
        429 => " (rate limit or no credit)",
        _ => "",
    };
    if detail.is_empty() {
        format!("HTTP {status}{hint}")
    } else {
        format!("HTTP {status}{hint}: {detail}")
    }
}

/// Junta os pedaços de um `chat/completions` em stream.
#[derive(Default)]
struct Assembler {
    answer: Answer,
}

impl Assembler {
    /// Um objeto do stream (ou a resposta inteira, sem stream). Devolve o
    /// texto novo.
    fn push(&mut self, chunk: &Value) -> Result<String, String> {
        if let Some(e) = chunk.get("error").filter(|e| !e.is_null()) {
            return Err(e["message"]
                .as_str()
                .map_or_else(|| e.to_string(), str::to_string));
        }
        let choice = &chunk["choices"][0];
        // `delta` no stream, `message` na resposta inteira.
        let part = if choice["delta"].is_object() {
            &choice["delta"]
        } else {
            &choice["message"]
        };
        let text = part["content"].as_str().unwrap_or_default().to_string();
        self.answer.content.push_str(&text);
        for (n, call) in part["tool_calls"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let index = call["index"].as_u64().map_or(n, |i| i as usize);
            while self.answer.tool_calls.len() <= index {
                self.answer.tool_calls.push(ToolRequest {
                    id: String::new(),
                    name: String::new(),
                    arguments: String::new(),
                });
            }
            let slot = &mut self.answer.tool_calls[index];
            if let Some(id) = call["id"].as_str().filter(|s| !s.is_empty()) {
                slot.id = id.to_string();
            }
            let f = &call["function"];
            if let Some(name) = f["name"].as_str() {
                slot.name.push_str(name);
            }
            match &f["arguments"] {
                Value::String(a) => slot.arguments.push_str(a),
                Value::Null => {}
                // Alguns servidores mandam o objeto pronto.
                other => slot.arguments = other.to_string(),
            }
        }
        Ok(text)
    }

    fn finish(mut self) -> Answer {
        self.answer.tool_calls.retain(|c| !c.name.is_empty());
        for (i, call) in self.answer.tool_calls.iter_mut().enumerate() {
            if call.id.is_empty() {
                call.id = format!("call_{i}");
            }
            if call.arguments.trim().is_empty() {
                call.arguments = "{}".into();
            }
        }
        self.answer
    }
}

/// Uma rodada de conversa: manda `messages` (e as `tools`, no formato da
/// OpenAI) e devolve a resposta. O texto chega aos poucos em `on_text`.
/// `cancel` interrompe; o erro então é [`CANCELLED`].
pub fn chat(
    conn: &Connection,
    messages: &[Value],
    tools: &[Value],
    cancel: &AtomicBool,
    on_text: &mut dyn FnMut(&str),
) -> Result<Answer, String> {
    if conn.model.is_empty() {
        return Err("no model is set for this provider".into());
    }
    let mut body = json!({
        "model": conn.model,
        "messages": messages,
        "stream": true,
    });
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools.to_vec());
    }
    let mut request = agent()
        .post(format!("{}/chat/completions", conn.base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream")
        // A OpenRouter mostra de onde veio a chamada no painel dela.
        .header("X-Title", "PostPilot");
    if !conn.api_key.is_empty() {
        request = request.header("Authorization", format!("Bearer {}", conn.api_key));
    }
    let mut resp = request
        .send(body.to_string())
        .map_err(|e| format!("could not reach {}: {e}", conn.host()))?;
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        let text = resp
            .body_mut()
            .with_config()
            .limit(MAX_SMALL)
            .read_to_string()
            .unwrap_or_default();
        return Err(http_error(status, &text));
    }
    let streaming = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|t| t.contains("event-stream"));
    let mut assembler = Assembler::default();
    if !streaming {
        let text = resp
            .body_mut()
            .with_config()
            .limit(MAX_SMALL)
            .read_to_string()
            .map_err(|e| format!("could not read the answer: {e}"))?;
        let value: Value =
            serde_json::from_str(&text).map_err(|e| format!("unexpected answer: {e}"))?;
        let new = assembler.push(&value)?;
        if !new.is_empty() {
            on_text(&new);
        }
        return Ok(assembler.finish());
    }
    let reader = BufReader::new(resp.body_mut().as_reader());
    for line in reader.lines() {
        if cancel.load(Ordering::SeqCst) {
            return Err(CANCELLED.into());
        }
        let line = line.map_err(|e| format!("the answer was cut short: {e}"))?;
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data == "[DONE]" {
            break;
        }
        let Ok(chunk) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        let new = assembler.push(&chunk)?;
        if !new.is_empty() {
            on_text(&new);
        }
    }
    if cancel.load(Ordering::SeqCst) {
        return Err(CANCELLED.into());
    }
    Ok(assembler.finish())
}

/// Uma pergunta só, sem ferramentas: devolve o texto da resposta.
pub fn ask(conn: &Connection, system: &str, user: &str) -> Result<String, String> {
    let messages = [
        json!({ "role": "system", "content": system }),
        json!({ "role": "user", "content": user }),
    ];
    let answer = chat(conn, &messages, &[], &AtomicBool::new(false), &mut |_| {})?;
    if answer.content.trim().is_empty() {
        return Err("the model answered with nothing".into());
    }
    Ok(answer.content)
}

/// Os modelos que a chave enxerga (`GET /models`), em ordem alfabética.
pub fn list_models(conn: &Connection) -> Result<Vec<String>, String> {
    let mut request = agent().get(format!("{}/models", conn.base_url));
    if !conn.api_key.is_empty() {
        request = request.header("Authorization", format!("Bearer {}", conn.api_key));
    }
    let mut resp = request
        .call()
        .map_err(|e| format!("could not reach {}: {e}", conn.host()))?;
    let status = resp.status().as_u16();
    let text = resp
        .body_mut()
        .with_config()
        .limit(MAX_SMALL)
        .read_to_string()
        .map_err(|e| format!("could not read the answer: {e}"))?;
    if !(200..300).contains(&status) {
        return Err(http_error(status, &text));
    }
    let value: Value =
        serde_json::from_str(&text).map_err(|e| format!("unexpected answer: {e}"))?;
    let list = value["data"]
        .as_array()
        .or_else(|| value["models"].as_array())
        .or_else(|| value.as_array());
    let mut models: Vec<String> = list
        .into_iter()
        .flatten()
        .filter_map(|m| m["id"].as_str().or_else(|| m["name"].as_str()))
        .map(str::to_string)
        .collect();
    models.sort();
    models.dedup();
    Ok(models)
}

/// A IA em uso: o Claude Code instalado ou um provedor por chave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Backend {
    /// O `claude` da máquina, com o login dele.
    ClaudeCode { program: PathBuf },
    /// Um provedor compatível com a OpenAI.
    Api { name: String, conn: Connection },
}

impl Backend {
    /// Nome para a interface.
    pub fn label(&self) -> &str {
        match self {
            Backend::ClaudeCode { .. } => "Claude Code",
            Backend::Api { name, .. } => name,
        }
    }

    /// Uma pergunta só, sem ferramentas. `cwd` é onde o `claude` roda.
    pub fn ask(&self, system: &str, user: &str, cwd: &Path) -> Result<String, String> {
        match self {
            Backend::ClaudeCode { program } => crate::agent::ask(program, system, user, cwd),
            Backend::Api { conn, .. } => ask(conn, system, user),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Um servidor HTTP de mentira: responde cada pedido com o próximo
    /// corpo de `replies` (`(status, content-type, corpo)`) e guarda o que
    /// recebeu.
    pub(crate) fn fake_server(
        replies: Vec<(u16, &'static str, String)>,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for (status, kind, body) in replies {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                // Cabeçalhos e, pelo Content-Length, o corpo.
                let request = loop {
                    let n = stream.read(&mut chunk).unwrap();
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).into_owned();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length = text[..end]
                            .lines()
                            .find_map(|l| {
                                let (k, v) = l.split_once(':')?;
                                k.eq_ignore_ascii_case("content-length")
                                    .then(|| v.trim().parse::<usize>().ok())?
                            })
                            .unwrap_or(0);
                        if buf.len() >= end + 4 + length || n == 0 {
                            break text;
                        }
                    }
                    if n == 0 {
                        break text;
                    }
                };
                seen.push(request);
                let head = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(head.as_bytes()).unwrap();
                stream.write_all(body.as_bytes()).unwrap();
                let _ = stream.flush();
            }
            seen
        });
        (url, handle)
    }

    pub(crate) fn sse(chunks: &[Value]) -> String {
        let mut out = String::new();
        for c in chunks {
            out.push_str(&format!("data: {c}\n\n"));
        }
        out.push_str("data: [DONE]\n\n");
        out
    }

    fn conn(url: &str) -> Connection {
        Provider {
            base_url: format!("{url}/"),
            model: " m1 ".into(),
            ..Provider::default()
        }
        .connect(" sk-test ")
    }

    #[test]
    fn streams_text_and_tool_calls() {
        let body = sse(&[
            json!({"choices":[{"delta":{"role":"assistant","content":"Olá"}}]}),
            json!({"choices":[{"delta":{"content":", mundo"}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"describe_table","arguments":"{\"ta"}}]}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"ble\":\"t\"}"}}]}}]}),
            json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}),
        ]);
        let (url, server) = fake_server(vec![(200, "text/event-stream", body)]);
        let mut pieces = Vec::new();
        let answer = chat(
            &conn(&url),
            &[json!({"role":"user","content":"oi"})],
            &[json!({"type":"function","function":{"name":"describe_table"}})],
            &AtomicBool::new(false),
            &mut |t| pieces.push(t.to_string()),
        )
        .unwrap();
        assert_eq!(pieces, ["Olá", ", mundo"]);
        assert_eq!(answer.content, "Olá, mundo");
        assert_eq!(
            answer.tool_calls,
            [ToolRequest {
                id: "c1".into(),
                name: "describe_table".into(),
                arguments: "{\"table\":\"t\"}".into()
            }]
        );
        let seen = server.join().unwrap();
        let request = &seen[0];
        assert!(
            request.starts_with("POST /v1/chat/completions "),
            "{request}"
        );
        assert!(
            request
                .to_lowercase()
                .contains("authorization: bearer sk-test\r\n"),
            "{request}"
        );
        let sent: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(sent["model"], "m1");
        assert_eq!(sent["tools"][0]["function"]["name"], "describe_table");
    }

    #[test]
    fn whole_json_answers_and_errors() {
        let whole = json!({"choices":[{"message":{"role":"assistant","content":"pronto"}}]});
        let (url, server) = fake_server(vec![
            (200, "application/json", whole.to_string()),
            (
                401,
                "application/json",
                json!({"error":{"message":"bad key"}}).to_string(),
            ),
            (
                200,
                "application/json",
                json!({"data":[{"id":"b"},{"id":"a"}]}).to_string(),
            ),
        ]);
        let c = conn(&url);
        assert_eq!(ask(&c, "s", "u").unwrap(), "pronto");
        let e = ask(&c, "s", "u").unwrap_err();
        assert_eq!(e, "HTTP 401 (check the API key): bad key");
        assert_eq!(list_models(&c).unwrap(), ["a", "b"]);
        let seen = server.join().unwrap();
        assert!(seen[2].starts_with("GET /v1/models "), "{}", seen[2]);
    }

    #[test]
    fn providers_and_hosts() {
        let p = Provider::new(ProviderKind::Glm, "glm-1");
        assert_eq!(p.name, "GLM (Z.ai)");
        let c = p.connect("k");
        assert_eq!(c.host(), "api.z.ai");
        assert!(!format!("{c:?}").contains("api_key"));
        let text = serde_json::to_string(&p).unwrap();
        assert!(text.contains("\"kind\":\"glm\""), "{text}");
        assert_eq!(serde_json::from_str::<Provider>(&text).unwrap(), p);
        // Um tipo de uma versão futura vira "Custom".
        let p: Provider = serde_json::from_str(r#"{"id":"x","kind":"novo"}"#).unwrap();
        assert_eq!(p.kind, ProviderKind::Custom);
    }
}
