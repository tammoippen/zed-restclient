use std::collections::HashMap;
use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

mod codelens;
mod exchange;
mod http_client;
mod parser;

#[derive(Debug)]
struct Backend {
    client: Client,
    document_map: RwLock<HashMap<Url, String>>,
    // Per-document cache of named request/response exchanges, keyed by
    // request name. Populated when a named request is sent.
    response_cache: RwLock<HashMap<Url, exchange::ExchangeCache>>,
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                code_lens_provider: Some(CodeLensOptions {
                    resolve_provider: Some(false),
                }),
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec!["zed-restclient::send_request".to_string()],
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "REST Client Sidecar initialized.")
            .await;
    }

    async fn execute_command(
        &self,
        params: ExecuteCommandParams,
    ) -> Result<Option<serde_json::Value>> {
        if params.command == "zed-restclient::send_request"
            && let Err(e) = self.handle_send_request(params.arguments).await
        {
            self.client
                .log_message(MessageType::ERROR, format!("Error: {}", e))
                .await;
        }
        Ok(None)
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = params.text_document.text;

        self.document_map.write().await.insert(uri.clone(), text);

        self.client
            .log_message(MessageType::INFO, format!("Opened file: {}", uri))
            .await;

        self.publish_request_var_diagnostics(uri).await;
    }

    async fn did_change(&self, mut params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        if let Some(change) = params.content_changes.pop() {
            self.document_map
                .write()
                .await
                .insert(uri.clone(), change.text);
            self.publish_request_var_diagnostics(uri).await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        self.document_map
            .write()
            .await
            .remove(&params.text_document.uri);
        self.response_cache
            .write()
            .await
            .remove(&params.text_document.uri);
    }

    async fn code_lens(&self, params: CodeLensParams) -> Result<Option<Vec<CodeLens>>> {
        let uri = params.text_document.uri;

        let mut lenses = Vec::new();

        if let Some(text) = self.document_map.read().await.get(&uri) {
            let start_lines = codelens::find_request_starts(text);

            for marker in start_lines {
                let position_start = Position {
                    line: marker.display_line as u32,
                    character: 0,
                };
                // Make the range span to character 100 so Zed realizes it covers text
                let position_end = Position {
                    line: marker.display_line as u32,
                    character: 100,
                };

                lenses.push(CodeLens {
                    range: Range {
                        start: position_start,
                        end: position_end,
                    },
                    command: Some(Command {
                        title: "▶ Send Request".to_string(),
                        command: "zed-restclient::send_request".to_string(),
                        arguments: Some(vec![
                            serde_json::Value::String(uri.to_string()),
                            serde_json::Value::Number(serde_json::Number::from(marker.block_index)),
                        ]),
                    }),
                    data: None,
                });
            }
        }

        Ok(Some(lenses))
    }
}

/// Convert a byte offset into an LSP `Position` (line + UTF-16 character).
fn offset_to_position(text: &str, offset: usize) -> Position {
    let offset = offset.min(text.len());
    let mut line = 0u32;
    let mut line_start = 0usize;
    for (idx, ch) in text.char_indices() {
        if idx >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            line_start = idx + ch.len_utf8();
        }
    }
    let character = text[line_start..offset].encode_utf16().count() as u32;
    Position { line, character }
}

/// Capture the headers and body of a built reqwest request as an
/// ExchangeMessage, so `{{name.request....}}` references resolve to exactly
/// what was sent (after variable substitution).
fn capture_request_message(req: &reqwest::Request) -> exchange::ExchangeMessage {
    let headers = req
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or_default().to_string()))
        .collect();
    let body = req
        .body()
        .and_then(|b| b.as_bytes())
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .unwrap_or_default();
    exchange::ExchangeMessage { headers, body }
}

impl Backend {
    /// Recompute and publish diagnostics for unresolved request-variable
    /// references in the given document.
    async fn publish_request_var_diagnostics(&self, uri: Url) {
        let text = match self.document_map.read().await.get(&uri) {
            Some(t) => t.clone(),
            None => return,
        };
        let cache = self
            .response_cache
            .read()
            .await
            .get(&uri)
            .cloned()
            .unwrap_or_default();

        let diagnostics = exchange::collect_unresolved_refs(&text, &cache)
            .into_iter()
            .map(|r| Diagnostic {
                range: Range {
                    start: offset_to_position(&text, r.start),
                    end: offset_to_position(&text, r.end),
                },
                severity: Some(DiagnosticSeverity::WARNING),
                source: Some("zed-restclient".to_string()),
                message: r.message,
                ..Default::default()
            })
            .collect();

        self.client
            .publish_diagnostics(uri, diagnostics, None)
            .await;
    }

    async fn handle_send_request(&self, args: Vec<serde_json::Value>) -> anyhow::Result<()> {
        if args.len() < 2 {
            anyhow::bail!("Invalid arguments for send_request. Expected URI and block index.");
        }

        let uri_str = args[0]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Expected URI as first argument"))?;
        let block_idx = args[1]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("Expected block index as second argument"))?
            as usize;

        let uri = Url::parse(uri_str)?;
        let text = {
            let map = self.document_map.read().await;
            map.get(&uri)
                .ok_or_else(|| anyhow::anyhow!("Document not found in memory"))?
                .clone()
        };

        let http_file = parser::parse_http_file(&text);
        let req = match http_file.requests.get(block_idx) {
            Some(r) => r,
            None => {
                let err_msg = format!("Request block not found at index {}", block_idx);
                self.client.log_message(MessageType::ERROR, &err_msg).await;
                return Err(anyhow::anyhow!(err_msg));
            }
        };

        self.client
            .log_message(
                MessageType::INFO,
                format!("Sending {} request to {}", req.method, req.url),
            )
            .await;

        let http_client = reqwest::Client::new();
        let request_cache = {
            let cache = self.response_cache.read().await;
            cache.get(&uri).cloned().unwrap_or_default()
        };
        let reqwest_req = match http_client::build_request(
            &http_client,
            req,
            &http_file.variables,
            &request_cache,
        ) {
            Ok(r) => r,
            Err(e) => {
                let err_msg = format!("Failed to build request: {}", e);
                self.client.log_message(MessageType::ERROR, &err_msg).await;
                return Err(e);
            }
        };

        // Capture the request exactly as sent (post-resolution) so that
        // `{{name.request....}}` references can resolve later.
        let request_message = capture_request_message(&reqwest_req);
        let request_name = req.name.map(|n| n.to_string());

        let response = match http_client.execute(reqwest_req).await {
            Ok(res) => res,
            Err(e) => {
                let err_msg = format!("HTTP Request failed: {}", e);
                self.client.log_message(MessageType::ERROR, &err_msg).await;
                return Err(anyhow::anyhow!(err_msg));
            }
        };

        let status = response.status();
        let headers = response.headers().clone();
        let body = response.text().await.unwrap_or_default();

        let mut response_text = format!("HTTP/1.1 {}\n", status);
        for (name, value) in headers.iter() {
            let v = value.to_str().unwrap_or("[invalid header value]");
            response_text.push_str(&format!("{}: {}\n", name, v));
        }
        response_text.push('\n');
        response_text.push_str(&body);

        // Cache the exchange under its name for request-variable references.
        if let Some(name) = request_name {
            let response_message = exchange::ExchangeMessage {
                headers: headers
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or_default().to_string()))
                    .collect(),
                body: body.clone(),
            };
            {
                let mut cache = self.response_cache.write().await;
                cache.entry(uri.clone()).or_default().insert(
                    name,
                    exchange::Exchange {
                        request: request_message,
                        response: response_message,
                    },
                );
            }
            // The cache changed, so references that were unresolved may now
            // resolve (and vice versa) — refresh diagnostics.
            self.publish_request_var_diagnostics(uri.clone()).await;
        }

        self.client
            .log_message(
                MessageType::INFO,
                format!(
                    "Received response for {}, length: {}",
                    uri_str,
                    response_text.len()
                ),
            )
            .await;

        let temp_dir = std::env::temp_dir();
        let file_path = temp_dir.join("zed_restclient_response.http");
        if let Err(e) = tokio::fs::write(&file_path, &response_text).await {
            self.client
                .log_message(
                    MessageType::ERROR,
                    format!("Failed to write temp file: {}", e),
                )
                .await;
            return Err(e.into());
        }

        if let Ok(url) = Url::from_file_path(&file_path) {
            let result = self
                .client
                .show_document(ShowDocumentParams {
                    uri: url,
                    external: Some(false),
                    take_focus: Some(true),
                    selection: None,
                })
                .await;

            if result.is_err() {
                // Fallback for older Zed versions or if window/showDocument is not supported
                let path_str = file_path.to_string_lossy();
                let opened = ["zeditor", "zed", "zed-preview", "zed-nightly"]
                    .iter()
                    .any(|cmd| {
                        std::process::Command::new(cmd)
                            .arg(path_str.as_ref())
                            .spawn()
                            .is_ok()
                    });

                if !opened {
                    #[cfg(target_os = "macos")]
                    let _ = std::process::Command::new("open")
                        .arg(path_str.as_ref())
                        .spawn();
                    #[cfg(target_os = "linux")]
                    let _ = std::process::Command::new("xdg-open")
                        .arg(path_str.as_ref())
                        .spawn();
                    #[cfg(target_os = "windows")]
                    let _ = std::process::Command::new("cmd")
                        .args(["/C", "start", path_str.as_ref()])
                        .spawn();
                }
            }
        }

        Ok(())
    }
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(|client| Backend {
        client,
        document_map: RwLock::new(HashMap::new()),
        response_cache: RwLock::new(HashMap::new()),
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_to_position_first_line() {
        let text = "GET {{x}}";
        let pos = offset_to_position(text, 4);
        assert_eq!(pos.line, 0);
        assert_eq!(pos.character, 4);
    }

    #[test]
    fn offset_to_position_later_line() {
        let text = "line0\nline1\nGET {{x}}";
        let offset = text.find("{{").unwrap();
        let pos = offset_to_position(text, offset);
        assert_eq!(pos.line, 2);
        assert_eq!(pos.character, 4);
    }

    #[test]
    fn offset_to_position_counts_utf16_units() {
        // 'é' is one char but lives before the offset on the same line.
        let text = "é{{x}}";
        let offset = text.find("{{").unwrap();
        let pos = offset_to_position(text, offset);
        assert_eq!(pos.line, 0);
        assert_eq!(pos.character, 1); // one UTF-16 unit for 'é'
    }

    #[test]
    fn offset_to_position_clamps_past_end() {
        let text = "abc";
        let pos = offset_to_position(text, 999);
        assert_eq!(pos.line, 0);
        assert_eq!(pos.character, 3);
    }
}
