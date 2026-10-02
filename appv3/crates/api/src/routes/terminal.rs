//! `/api/terminal` — port of `app/api/routes/terminal.py` (single-use
//! tickets + PTY over WebSocket).
//!
//! v3 adds a `{"type": "busy", "busy": bool}` frame, sent whenever a command
//! starts or stops running in the shell, so the client can keep a terminal
//! with a running command open.

use crate::error::{ApiError, ApiResult};
use crate::schema::Body;
use crate::util::*;
use crate::AppState;
use appv3_agent::manager;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const TICKET_TTL: Duration = Duration::from_secs(30);
const MAX_PENDING_TICKETS: usize = 32;
/// How often the socket checks whether a command is running in the shell.
const BUSY_POLL: Duration = Duration::from_millis(500);

struct Ticket {
    workspace: String,
    expiry: Instant,
    rows: i64,
    cols: i64,
}

fn tickets() -> &'static Mutex<HashMap<String, Ticket>> {
    static T: OnceLock<Mutex<HashMap<String, Ticket>>> = OnceLock::new();
    T.get_or_init(Default::default)
}

fn prune(map: &mut HashMap<String, Ticket>) {
    let now = Instant::now();
    map.retain(|_, t| now <= t.expiry);
}

pub fn router() -> Router<AppState> {
    Router::new().route("/ticket", post(issue_ticket)).route("/ws", get(terminal_ws))
}

fn int_field(b: &mut Body, k: &str, default: i64, ge: i64, le: i64) -> i64 {
    let v = b.opt_int(k).unwrap_or(default);
    if b.obj.contains_key(k) && b.errs.is_empty() {
        let input = b.obj[k].clone();
        if v < ge {
            b.errs.push(crate::error::verr_ctx(
                "greater_than_equal",
                &[json!("body"), json!(k)],
                &format!("Input should be greater than or equal to {ge}"),
                input,
                json!({"ge": ge}),
            ));
        } else if v > le {
            b.errs.push(crate::error::verr_ctx("less_than_equal", &[json!("body"), json!(k)], &format!("Input should be less than or equal to {le}"), input, json!({"le": le})));
        }
    }
    v
}

async fn issue_ticket(raw: Bytes) -> ApiResult<Response> {
    let v = body_value(&raw)?;
    let mut b = Body::new(&v)?;
    let workspace = b.str_min1("workspace");
    let rows = int_field(&mut b, "rows", 24, 1, 1000);
    let cols = int_field(&mut b, "cols", 80, 1, 4000);
    // TicketRequest allows extras (plain BaseModel): ignore unknown keys.
    let allowed: Vec<&str> = b.obj.keys().map(String::as_str).collect();
    b.finish(&allowed)?;
    let resolved = manager::validate_workspace(&workspace, true).map_err(ApiError::bad_request)?;
    let mut map = tickets().lock().unwrap();
    prune(&mut map);
    if map.len() >= MAX_PENDING_TICKETS {
        return Err(ApiError::new(429, "Too many pending tickets."));
    }
    let t = crate::util::token_urlsafe();
    map.insert(t.clone(), Ticket { workspace: resolved, expiry: Instant::now() + TICKET_TTL, rows, cols });
    Ok(json(json!({"ticket": t, "expires_in": TICKET_TTL.as_secs_f64()})))
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// `_redeem_ticket` — pop on first use; constant-time comparison.
fn redeem(ticket: &str) -> Option<Ticket> {
    let mut map = tickets().lock().unwrap();
    prune(&mut map);
    let mut matched = None;
    for k in map.keys() {
        if ct_eq(k.as_bytes(), ticket.as_bytes()) {
            matched = Some(k.clone());
        }
    }
    let t = map.remove(&matched?)?;
    (Instant::now() <= t.expiry).then_some(t)
}

async fn terminal_ws(q: Qs, ws: Result<WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>) -> Response {
    // A websocket-only route never matches plain HTTP in Starlette.
    let Ok(ws) = ws else { return json_code(404, json!({"detail": "Not Found"})) };
    let ticket = q.opt("ticket").unwrap_or_default();
    let redeemed = if ticket.is_empty() { None } else { redeem(&ticket) };
    let Some(t) = redeemed else {
        // Closing before accept → the handshake is answered with HTTP 403.
        return crate::middleware::ws_reject();
    };
    ws.on_upgrade(move |socket| run_socket(socket, t))
}

async fn send_json(tx: &mut futures::stream::SplitSink<WebSocket, Message>, v: Value) -> bool {
    tx.send(Message::Text(v.to_string().into())).await.is_ok()
}

/// Incremental UTF-8 decoding (`codecs.getincrementaldecoder("utf-8")(errors="replace")`).
struct Utf8Decoder {
    pending: Vec<u8>,
}

impl Utf8Decoder {
    fn decode(&mut self, chunk: &[u8], last: bool) -> String {
        self.pending.extend_from_slice(chunk);
        let mut out = String::new();
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(s) => {
                    out.push_str(s);
                    self.pending.clear();
                    return out;
                }
                Err(e) => {
                    let good = e.valid_up_to();
                    out.push_str(std::str::from_utf8(&self.pending[..good]).unwrap());
                    match e.error_len() {
                        Some(bad) => {
                            out.push('\u{FFFD}');
                            self.pending.drain(..good + bad);
                        }
                        None => {
                            self.pending.drain(..good);
                            if last {
                                out.push('\u{FFFD}');
                                self.pending.clear();
                            }
                            return out;
                        }
                    }
                }
            }
        }
    }
}

async fn run_socket(socket: WebSocket, t: Ticket) {
    let (mut tx, mut rx) = socket.split();
    let ws_path = t.workspace.clone();
    let session = match tokio::task::spawn_blocking(move || appv3_terminal::create_session(&ws_path, t.rows, t.cols)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            tracing::warn!("terminal_spawn_failed workspace={} err={}", t.workspace, e);
            let _ = send_json(&mut tx, json!({"type": "output", "data": format!("\r\n{e}\r\n")})).await;
            let _ = tx.send(Message::Close(Some(CloseFrame { code: 4429, reason: "".into() }))).await;
            return;
        }
        Err(e) => {
            tracing::warn!("terminal_spawn_failed workspace={} err={}", t.workspace, e);
            let _ = tx.send(Message::Close(Some(CloseFrame { code: 4429, reason: "".into() }))).await;
            return;
        }
    };
    tracing::info!("terminal_ws_connected session_id={} workspace={}", session.session_id, t.workspace);
    let s_out = session.clone();
    let to_ws = async move {
        let mut dec = Utf8Decoder { pending: vec![] };
        let mut busy = false;
        let mut poll = tokio::time::interval(BUSY_POLL);
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            // `read` is cancel-safe (a tokio mutex, then `mpsc::recv`).
            tokio::select! {
                chunk = s_out.read() => match chunk {
                    Some(chunk) => {
                        let text = dec.decode(&chunk, false);
                        if !send_json(&mut tx, json!({"type": "output", "data": text})).await {
                            return tx;
                        }
                    }
                    None => {
                        let tail = dec.decode(&[], true);
                        if !tail.is_empty() {
                            let _ = send_json(&mut tx, json!({"type": "output", "data": tail})).await;
                        }
                        let _ = send_json(&mut tx, json!({"type": "exit"})).await;
                        return tx;
                    }
                },
                _ = poll.tick() => {
                    let now = s_out.busy();
                    if now != busy {
                        busy = now;
                        if !send_json(&mut tx, json!({"type": "busy", "busy": now})).await {
                            return tx;
                        }
                    }
                }
            }
        }
    };
    let s_in = session.clone();
    let to_pty = async move {
        while let Some(Ok(msg)) = rx.next().await {
            let text = match msg {
                Message::Text(t) => t.to_string(),
                Message::Binary(b) => String::from_utf8_lossy(&b).to_string(),
                Message::Close(_) => break,
                _ => continue,
            };
            let Ok(v) = serde_json::from_str::<Value>(&text) else { break };
            match v.get("type").and_then(|t| t.as_str()) {
                Some("input") => {
                    if let Some(d) = v.get("data").and_then(|d| d.as_str()).filter(|d| !d.is_empty()) {
                        if let Err(e) = s_in.write(d.as_bytes().to_vec()).await {
                            tracing::warn!("terminal_ws_error session_id={} err={}", s_in.session_id, e);
                            break;
                        }
                    }
                }
                Some("resize") => {
                    let is_int = |x: Option<&Value>| x.and_then(|n| n.as_i64());
                    if let (Some(r), Some(c)) = (is_int(v.get("rows")), is_int(v.get("cols"))) {
                        s_in.resize(r, c);
                    }
                }
                _ => {}
            }
        }
    };
    let sink = tokio::select! {
        tx = to_ws => Some(tx),
        _ = to_pty => None,
    };
    session.close().await;
    if let Some(mut tx) = sink {
        let _ = tx.send(Message::Close(None)).await;
    }
}
