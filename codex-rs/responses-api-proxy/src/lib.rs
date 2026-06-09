use std::fs::File;
use std::fs::{self};
use std::io::Cursor;
use std::io::Read;
use std::io::Write;
use std::net::SocketAddr;
use std::net::TcpListener;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use clap::Parser;
use reqwest::Url;
use reqwest::blocking::Client;
use reqwest::header::AUTHORIZATION;
use reqwest::header::HOST;
use reqwest::header::HeaderMap;
use reqwest::header::HeaderName;
use reqwest::header::HeaderValue;
use serde::Serialize;
use serde_json::Value;
use tiny_http::Header;
use tiny_http::Method;
use tiny_http::Request;
use tiny_http::Response;
use tiny_http::Server;
use tiny_http::StatusCode;

mod dump;
mod read_api_key;
use dump::ExchangeDumper;
use read_api_key::read_auth_header_from_stdin;

/// CLI arguments for the proxy.
#[derive(Debug, Clone, Parser)]
#[command(name = "responses-api-proxy", about = "Minimal OpenAI responses proxy")]
pub struct Args {
    /// Port to listen on. If not set, an ephemeral port is used.
    #[arg(long)]
    pub port: Option<u16>,

    /// Path to a JSON file to write startup info (single line). Includes {"port": <u16>}.
    #[arg(long, value_name = "FILE")]
    pub server_info: Option<PathBuf>,

    /// Enable HTTP shutdown endpoint at GET /shutdown
    #[arg(long)]
    pub http_shutdown: bool,

    /// Absolute URL the proxy should forward requests to (defaults to OpenAI).
    #[arg(long, default_value = "https://api.openai.com/v1/responses")]
    pub upstream_url: String,

    /// Directory where request/response dumps should be written as JSON.
    #[arg(long, value_name = "DIR")]
    pub dump_dir: Option<PathBuf>,

    /// Serve a deterministic no-secret Responses API mock for the local Teams smoke test.
    #[arg(long)]
    pub mock_teams_smoke: bool,
}

#[derive(Serialize)]
struct ServerInfo {
    port: u16,
    pid: u32,
}

struct ForwardConfig {
    upstream_url: Url,
    host_header: HeaderValue,
}

#[derive(Default)]
struct MockTeamsSmokeState {
    team_id: Option<String>,
    member_id: Option<String>,
    next_response: u64,
    message_list_checks: u64,
    member_to_lead_completed: bool,
}

impl MockTeamsSmokeState {
    fn next_sse_response(&mut self, body: &Value) -> Result<Vec<u8>> {
        if let Some(output) = function_output_text(body, "mock-status") {
            let verdict = if self.member_to_lead_completed {
                "TEAMS_SMOKE_PASS"
            } else {
                "TEAMS_SMOKE_FAIL"
            };
            return self.assistant_response(&format!(
                "{verdict} team_id={} member_id={} member_to_lead_completed={} status_output_bytes={}",
                self.team_id.as_deref().unwrap_or("unknown"),
                self.member_id.as_deref().unwrap_or("unknown"),
                self.member_to_lead_completed,
                output.len()
            ));
        }

        if function_output_text(body, "mock-event-list").is_some() {
            return self.function_call_response(
                "mock-status",
                "team_status",
                serde_json::json!({
                    "team_id": self.required_team_id()?,
                }),
            );
        }

        if let Some(output) = latest_function_output_text_with_prefix(body, "mock-message-list-") {
            if contains_member_to_lead_message(&output) {
                self.member_to_lead_completed = true;
            }
            if !self.member_to_lead_completed && self.message_list_checks < 60 {
                return self.message_list_response();
            }
            return self.function_call_response(
                "mock-event-list",
                "team_event_list",
                serde_json::json!({
                    "team_id": self.required_team_id()?,
                }),
            );
        }

        if function_output_text(body, "mock-lead-to-member").is_some() {
            return self.message_list_response();
        }

        if let Some(output) = function_output_text(body, "mock-spawn-member") {
            self.member_id = parse_json_path_string(&output, &["member", "id"]);
            return self.function_call_response(
                "mock-lead-to-member",
                "team_send",
                serde_json::json!({
                    "team_id": self.required_team_id()?,
                    "target": "member",
                    "member_id": self.required_member_id()?,
                    "delivery_mode": "queue",
                    "message": "Lead-to-member smoke message from local mock Responses provider.",
                }),
            );
        }

        if let Some(output) = function_output_text(body, "mock-create-team") {
            self.team_id = parse_json_path_string(&output, &["team", "id"]);
            return self.function_call_response(
                "mock-spawn-member",
                "team_spawn_member",
                serde_json::json!({
                    "team_id": self.required_team_id()?,
                    "name": "mock-member",
                    "profile": "No-secret local Teams smoke teammate",
                    "capabilities": ["teams-smoke"],
                    "permissions": ["team_send"],
                    "message": "Send one acknowledgement to the team lead with team_send, then stop.",
                }),
            );
        }

        if let Some(output) = function_output_text(body, "mock-member-to-lead") {
            self.member_to_lead_completed = member_to_lead_send_succeeded(&output);
            return self.assistant_response(&format!(
                "TEAMS_SMOKE_MEMBER_DONE member_to_lead_sent={}",
                self.member_to_lead_completed
            ));
        }

        let texts = collect_text_values(body);
        if contains_text(
            &texts,
            "Lead-to-member smoke message from local mock Responses provider.",
        ) || contains_text(
            &texts,
            "Send one acknowledgement to the team lead with team_send",
        ) {
            return self.function_call_response(
                "mock-member-to-lead",
                "team_send",
                serde_json::json!({
                    "to": "team-lead",
                    "summary": "smoke acknowledgement",
                    "message": "Member-to-lead smoke acknowledgement from local mock Responses provider.",
                }),
            );
        }

        self.function_call_response(
            "mock-create-team",
            "create_team",
            serde_json::json!({
                "team_name": "local tmux teams smoke",
            }),
        )
    }

    fn message_list_response(&mut self) -> Result<Vec<u8>> {
        self.message_list_checks += 1;
        let call_id = format!("mock-message-list-{}", self.message_list_checks);
        self.function_call_response(
            &call_id,
            "team_message_list",
            serde_json::json!({
                "team_id": self.required_team_id()?,
                "target": "all",
            }),
        )
    }

    fn function_call_response(
        &mut self,
        call_id: &str,
        name: &str,
        arguments: Value,
    ) -> Result<Vec<u8>> {
        let response_id = self.next_response_id();
        let arguments = serde_json::to_string(&arguments)?;
        Ok(sse(vec![
            event_response_created(&response_id),
            serde_json::json!({
                "type": "response.output_item.done",
                "item": {
                    "type": "function_call",
                    "call_id": call_id,
                    "name": name,
                    "arguments": arguments,
                }
            }),
            event_response_completed(&response_id),
        ]))
    }

    fn assistant_response(&mut self, text: &str) -> Result<Vec<u8>> {
        let response_id = self.next_response_id();
        let message_id = format!("msg-{}", self.next_response);
        Ok(sse(vec![
            event_response_created(&response_id),
            serde_json::json!({
                "type": "response.output_item.done",
                "item": {
                    "type": "message",
                    "role": "assistant",
                    "id": message_id,
                    "content": [{"type": "output_text", "text": text}],
                }
            }),
            event_response_completed(&response_id),
        ]))
    }

    fn next_response_id(&mut self) -> String {
        self.next_response += 1;
        format!("mock-resp-{}", self.next_response)
    }

    fn required_team_id(&self) -> Result<&str> {
        self.team_id
            .as_deref()
            .ok_or_else(|| anyhow!("mock Teams smoke is missing team_id"))
    }

    fn required_member_id(&self) -> Result<&str> {
        self.member_id
            .as_deref()
            .ok_or_else(|| anyhow!("mock Teams smoke is missing member_id"))
    }
}

/// Entry point for the library main, for parity with other crates.
pub fn run_main(args: Args) -> Result<()> {
    if args.mock_teams_smoke {
        return run_mock_teams_smoke(args);
    }

    let auth_header = read_auth_header_from_stdin()?;

    let upstream_url = Url::parse(&args.upstream_url).context("parsing --upstream-url")?;
    let host = match (upstream_url.host_str(), upstream_url.port()) {
        (Some(host), Some(port)) => format!("{host}:{port}"),
        (Some(host), None) => host.to_string(),
        _ => return Err(anyhow!("upstream URL must include a host")),
    };
    let host_header =
        HeaderValue::from_str(&host).context("constructing Host header from upstream URL")?;

    let forward_config = Arc::new(ForwardConfig {
        upstream_url,
        host_header,
    });
    let dump_dir = args
        .dump_dir
        .map(ExchangeDumper::new)
        .transpose()
        .context("creating --dump-dir")?
        .map(Arc::new);

    let (listener, bound_addr) = bind_listener(args.port)?;
    if let Some(path) = args.server_info.as_ref() {
        write_server_info(path, bound_addr.port())?;
    }
    let server = Server::from_listener(listener, None)
        .map_err(|err| anyhow!("creating HTTP server: {err}"))?;
    let client = Arc::new(
        Client::builder()
            // Disable reqwest's 30s default so long-lived response streams keep flowing.
            .timeout(None::<Duration>)
            .build()
            .context("building reqwest client")?,
    );

    eprintln!("responses-api-proxy listening on {bound_addr}");

    let http_shutdown = args.http_shutdown;
    for request in server.incoming_requests() {
        let client = client.clone();
        let forward_config = forward_config.clone();
        let dump_dir = dump_dir.clone();
        std::thread::spawn(move || {
            if http_shutdown && request.method() == &Method::Get && request.url() == "/shutdown" {
                let _ = request.respond(Response::new_empty(StatusCode(200)));
                std::process::exit(0);
            }

            if let Err(e) = forward_request(
                &client,
                auth_header,
                &forward_config,
                dump_dir.as_deref(),
                request,
            ) {
                eprintln!("forwarding error: {e}");
            }
        });
    }

    Err(anyhow!("server stopped unexpectedly"))
}

fn run_mock_teams_smoke(args: Args) -> Result<()> {
    let dump_dir = args
        .dump_dir
        .map(ExchangeDumper::new)
        .transpose()
        .context("creating --dump-dir")?
        .map(Arc::new);
    let state = Arc::new(Mutex::new(MockTeamsSmokeState::default()));

    let (listener, bound_addr) = bind_listener(args.port)?;
    if let Some(path) = args.server_info.as_ref() {
        write_server_info(path, bound_addr.port())?;
    }
    let server = Server::from_listener(listener, None)
        .map_err(|err| anyhow!("creating HTTP server: {err}"))?;

    eprintln!("responses-api-proxy mock Teams smoke listening on {bound_addr}");

    let http_shutdown = args.http_shutdown;
    for request in server.incoming_requests() {
        let dump_dir = dump_dir.clone();
        let state = state.clone();
        std::thread::spawn(move || {
            if http_shutdown && request.method() == &Method::Get && request.url() == "/shutdown" {
                let _ = request.respond(Response::new_empty(StatusCode(200)));
                std::process::exit(0);
            }

            if let Err(e) = mock_teams_smoke_request(state, dump_dir.as_deref(), request) {
                eprintln!("mock Teams smoke error: {e}");
            }
        });
    }

    Err(anyhow!("server stopped unexpectedly"))
}

fn mock_teams_smoke_request(
    state: Arc<Mutex<MockTeamsSmokeState>>,
    dump_dir: Option<&ExchangeDumper>,
    mut req: Request,
) -> Result<()> {
    let method = req.method().clone();
    let url_path = req.url().to_string();

    let mut body = Vec::new();
    req.as_reader().read_to_end(&mut body)?;
    let exchange_dump = dump_dir.and_then(|dump_dir| {
        dump_dir
            .dump_request(&method, &url_path, req.headers(), &body)
            .map_err(|err| {
                eprintln!("responses-api-proxy failed to dump mock request: {err}");
                err
            })
            .ok()
    });

    if method == Method::Get && url_path.starts_with("/v1/models") {
        let body = mock_models_body()?;
        respond_with_body(
            req,
            StatusCode(200),
            "application/json",
            body,
            exchange_dump,
        )?;
        return Ok(());
    }

    if method == Method::Post && url_path == "/v1/responses" {
        let body_json: Value = serde_json::from_slice(&body)
            .context("parsing mock Teams smoke /v1/responses request body")?;
        if should_pause_for_member_reply(&state, &body_json) {
            std::thread::sleep(Duration::from_millis(500));
        }
        let response = state
            .lock()
            .map_err(|err| anyhow!("mock Teams smoke state lock poisoned: {err}"))?
            .next_sse_response(&body_json)?;
        respond_with_body(
            req,
            StatusCode(200),
            "text/event-stream",
            response,
            exchange_dump,
        )?;
        return Ok(());
    }

    respond_with_body(
        req,
        StatusCode(403),
        "text/plain; charset=utf-8",
        b"mock Teams smoke only serves GET /v1/models and POST /v1/responses\n".to_vec(),
        exchange_dump,
    )?;
    Ok(())
}

fn bind_listener(port: Option<u16>) -> Result<(TcpListener, SocketAddr)> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port.unwrap_or(0)));
    let listener = TcpListener::bind(addr).with_context(|| format!("failed to bind {addr}"))?;
    let bound = listener.local_addr().context("failed to read local_addr")?;
    Ok((listener, bound))
}

fn write_server_info(path: &Path, port: u16) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }

    let info = ServerInfo {
        port,
        pid: std::process::id(),
    };
    let mut data = serde_json::to_string(&info)?;
    data.push('\n');
    let mut f = File::create(path)?;
    f.write_all(data.as_bytes())?;
    Ok(())
}

fn respond_with_body(
    req: Request,
    status: StatusCode,
    content_type: &str,
    body: Vec<u8>,
    exchange_dump: Option<dump::ExchangeDump>,
) -> Result<()> {
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        HeaderName::from_static("content-type"),
        HeaderValue::from_str(content_type).context("constructing mock Content-Type header")?,
    );
    let tiny_header = Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes())
        .map_err(|_| anyhow!("constructing tiny_http Content-Type header"))?;
    let content_length = body.len();
    let response_body: Box<dyn Read + Send> = if let Some(exchange_dump) = exchange_dump {
        Box::new(exchange_dump.tee_response_body(status.0, &response_headers, Cursor::new(body)))
    } else {
        Box::new(Cursor::new(body))
    };
    let response = Response::new(
        status,
        vec![tiny_header],
        response_body,
        Some(content_length),
        None,
    );
    let _ = req.respond(response);
    Ok(())
}

fn mock_models_body() -> Result<Vec<u8>> {
    serde_json::to_vec(&serde_json::json!({
        "models": [{
            "slug": "teams-smoke-model",
            "display_name": "Teams Smoke Model",
            "description": "Local no-secret model for Codex Teams smoke validation",
            "default_reasoning_level": "none",
            "supported_reasoning_levels": [],
            "shell_type": "shell_command",
            "visibility": "list",
            "supported_in_api": true,
            "priority": 0,
            "additional_speed_tiers": [],
            "service_tiers": [],
            "default_service_tier": null,
            "availability_nux": null,
            "upgrade": null,
            "base_instructions": "Local Teams smoke model.",
            "supports_reasoning_summaries": false,
            "default_reasoning_summary": "none",
            "support_verbosity": false,
            "default_verbosity": null,
            "apply_patch_tool_type": null,
            "web_search_tool_type": "text",
            "truncation_policy": {"mode": "bytes", "limit": 100000},
            "supports_parallel_tool_calls": false,
            "supports_image_detail_original": false,
            "context_window": 100000,
            "max_context_window": 100000,
            "auto_compact_token_limit": 90000,
            "effective_context_window_percent": 95,
            "experimental_supported_tools": [],
            "input_modalities": ["text"],
            "supports_search_tool": false,
            "auto_review_model_override": null,
            "tool_mode": null,
            "multi_agent_version": null,
        }]
    }))
    .context("serializing mock models response")
}

fn sse(events: Vec<Value>) -> Vec<u8> {
    let mut out = String::new();
    for event in events {
        let kind = event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("message");
        out.push_str("event: ");
        out.push_str(kind);
        out.push('\n');
        out.push_str("data: ");
        out.push_str(&event.to_string());
        out.push_str("\n\n");
    }
    out.into_bytes()
}

fn event_response_created(id: &str) -> Value {
    serde_json::json!({
        "type": "response.created",
        "response": {"id": id},
    })
}

fn event_response_completed(id: &str) -> Value {
    serde_json::json!({
        "type": "response.completed",
        "response": {
            "id": id,
            "usage": {
                "input_tokens": 0,
                "input_tokens_details": null,
                "output_tokens": 0,
                "output_tokens_details": null,
                "total_tokens": 0,
            },
        },
    })
}

fn function_output_text(body: &Value, call_id: &str) -> Option<String> {
    body.get("input")?
        .as_array()?
        .iter()
        .rev()
        .find(|item| {
            item.get("type").and_then(Value::as_str) == Some("function_call_output")
                && item.get("call_id").and_then(Value::as_str) == Some(call_id)
        })
        .and_then(output_item_text)
}

fn latest_function_output_text_with_prefix(body: &Value, call_id_prefix: &str) -> Option<String> {
    body.get("input")?
        .as_array()?
        .iter()
        .rev()
        .find(|item| {
            item.get("type").and_then(Value::as_str) == Some("function_call_output")
                && item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .is_some_and(|call_id| call_id.starts_with(call_id_prefix))
        })
        .and_then(output_item_text)
}

fn output_item_text(item: &Value) -> Option<String> {
    let output = item.get("output")?;
    match output {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => items.iter().find_map(|item| {
            if item.get("type").and_then(Value::as_str) == Some("input_text") {
                item.get("text").and_then(Value::as_str).map(str::to_string)
            } else {
                None
            }
        }),
        Value::Object(obj) => obj
            .get("content")
            .and_then(Value::as_str)
            .map(str::to_string),
        Value::Null | Value::Bool(_) | Value::Number(_) => None,
    }
}

fn parse_json_path_string(text: &str, path: &[&str]) -> Option<String> {
    let mut value = serde_json::from_str::<Value>(text).ok()?;
    for key in path {
        value = value.get(*key)?.clone();
    }
    value.as_str().map(str::to_string)
}

fn collect_text_values(value: &Value) -> Vec<String> {
    let mut texts = Vec::new();
    collect_text_values_inner(value, &mut texts);
    texts
}

fn collect_text_values_inner(value: &Value, texts: &mut Vec<String>) {
    match value {
        Value::String(text) => texts.push(text.clone()),
        Value::Array(items) => {
            for item in items {
                collect_text_values_inner(item, texts);
            }
        }
        Value::Object(obj) => {
            for value in obj.values() {
                collect_text_values_inner(value, texts);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn contains_text(texts: &[String], needle: &str) -> bool {
    texts.iter().any(|text| text.contains(needle))
}

fn should_pause_for_member_reply(state: &Arc<Mutex<MockTeamsSmokeState>>, body: &Value) -> bool {
    if !latest_function_output_call_id_starts_with(body, "mock-message-list-") {
        return false;
    }

    match state.lock() {
        Ok(state) => !state.member_to_lead_completed,
        Err(err) => {
            eprintln!("mock Teams smoke state lock poisoned: {err}");
            false
        }
    }
}

fn latest_function_output_call_id_starts_with(body: &Value, call_id_prefix: &str) -> bool {
    body.get("input")
        .and_then(Value::as_array)
        .and_then(|input| {
            input.iter().rev().find_map(|item| {
                if item.get("type").and_then(Value::as_str) == Some("function_call_output") {
                    item.get("call_id").and_then(Value::as_str)
                } else {
                    None
                }
            })
        })
        .is_some_and(|call_id| call_id.starts_with(call_id_prefix))
}

fn member_to_lead_send_succeeded(output: &str) -> bool {
    serde_json::from_str::<Value>(output)
        .ok()
        .is_some_and(|value| {
            value
                .get("message")
                .is_some_and(member_to_lead_message_matches)
        })
}

fn member_to_lead_message_matches(message: &Value) -> bool {
    let content_matches = message
        .get("content")
        .and_then(Value::as_str)
        .is_some_and(|content| content.contains("Member-to-lead smoke acknowledgement"));
    let target_matches = message
        .get("target")
        .and_then(Value::as_object)
        .is_some_and(|target| target.contains_key("lead"));
    content_matches && target_matches
}

fn contains_member_to_lead_message(output: &str) -> bool {
    serde_json::from_str::<Value>(output)
        .ok()
        .is_some_and(|value| {
            value
                .get("messages")
                .and_then(Value::as_array)
                .is_some_and(|messages| {
                    messages.iter().any(|message| {
                        let content_matches = message
                            .get("content")
                            .and_then(Value::as_str)
                            .is_some_and(|content| {
                                content.contains("Member-to-lead smoke acknowledgement")
                            });
                        let target_matches = message
                            .get("target")
                            .and_then(Value::as_object)
                            .is_some_and(|target| target.contains_key("lead"));
                        content_matches && target_matches
                    })
                })
        })
}

fn forward_request(
    client: &Client,
    auth_header: &'static str,
    config: &ForwardConfig,
    dump_dir: Option<&ExchangeDumper>,
    mut req: Request,
) -> Result<()> {
    // Only allow POST /v1/responses exactly, no query string.
    let method = req.method().clone();
    let url_path = req.url().to_string();
    let allow = method == Method::Post && url_path == "/v1/responses";

    if !allow {
        let resp = Response::new_empty(StatusCode(403));
        let _ = req.respond(resp);
        return Ok(());
    }

    // Read request body
    let mut body = Vec::new();
    let reader = req.as_reader();
    reader.read_to_end(&mut body)?;

    let exchange_dump = dump_dir.and_then(|dump_dir| {
        dump_dir
            .dump_request(&method, &url_path, req.headers(), &body)
            .map_err(|err| {
                eprintln!("responses-api-proxy failed to dump request: {err}");
                err
            })
            .ok()
    });

    // Build headers for upstream, forwarding everything from the incoming
    // request except Authorization (we replace it below).
    let mut headers = HeaderMap::new();
    for header in req.headers() {
        let name_ascii = header.field.as_str();
        let lower = name_ascii.to_ascii_lowercase();
        if lower.as_str() == "authorization" || lower.as_str() == "host" {
            continue;
        }

        let header_name = match HeaderName::from_bytes(lower.as_bytes()) {
            Ok(name) => name,
            Err(_) => continue,
        };
        if let Ok(value) = HeaderValue::from_bytes(header.value.as_bytes()) {
            headers.append(header_name, value);
        }
    }

    // As part of our effort to to keep `auth_header` secret, we use a
    // combination of `from_static()` and `set_sensitive(true)`.
    let mut auth_header_value = HeaderValue::from_static(auth_header);
    auth_header_value.set_sensitive(true);
    headers.insert(AUTHORIZATION, auth_header_value);

    headers.insert(HOST, config.host_header.clone());

    let upstream_resp = client
        .post(config.upstream_url.clone())
        .headers(headers)
        .body(body)
        .send()
        .context("forwarding request to upstream")?;

    // We have to create an adapter between a `reqwest::blocking::Response`
    // and a `tiny_http::Response`. Fortunately, `reqwest::blocking::Response`
    // implements `Read`, so we can use it directly as the body of the
    // `tiny_http::Response`.
    let status = upstream_resp.status();
    let mut response_headers = Vec::new();
    for (name, value) in upstream_resp.headers().iter() {
        // Skip headers that tiny_http manages itself.
        if matches!(
            name.as_str(),
            "content-length" | "transfer-encoding" | "connection" | "trailer" | "upgrade"
        ) {
            continue;
        }

        if let Ok(header) = Header::from_bytes(name.as_str().as_bytes(), value.as_bytes()) {
            response_headers.push(header);
        }
    }

    let content_length = upstream_resp.content_length().and_then(|len| {
        if len <= usize::MAX as u64 {
            Some(len as usize)
        } else {
            None
        }
    });

    let response_body: Box<dyn Read + Send> = if let Some(exchange_dump) = exchange_dump {
        let headers = upstream_resp.headers().clone();
        Box::new(exchange_dump.tee_response_body(status.as_u16(), &headers, upstream_resp))
    } else {
        Box::new(upstream_resp)
    };

    let response = Response::new(
        StatusCode(status.as_u16()),
        response_headers,
        response_body,
        content_length,
        None,
    );

    let _ = req.respond(response);
    Ok(())
}
