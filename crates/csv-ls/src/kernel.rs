//! A Jupyter kernel (protocol v5.3) that renders delimiter-separated values
//! as tables. "Executing" a cell parses the submitted text and replies with
//! a Tabular Data Resource (`application/vnd.dataresource+json`), which
//! Zed's REPL renders as a native inline table.
//!
//! ZeroMQ is spoken over five sockets bound from the Jupyter connection
//! file: shell/control/stdin are ROUTER, iopub is PUB, heartbeat is REP.
//! Everything runs on a single-threaded tokio runtime; there is no shared
//! mutable state across tasks beyond message counters, so no locking.

use std::path::Path;

use bytes::Bytes;
use futures_util::StreamExt;
use hmac::{Hmac, KeyInit, Mac};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Sha256;
use uuid::Uuid;
use zeromq::prelude::*;
use zeromq::{PubSocket, RepSocket, RouterSocket, SocketEvent, ZmqMessage};

use crate::table;
use crate::time;

type Error = Box<dyn std::error::Error + Sync + Send>;

/// Kernel activity goes to stderr, which Zed captures into its own log —
/// `zed: open log` then shows exactly which messages reached the kernel.
fn log(msg: &str) {
    eprintln!("csv-kernel: {msg}");
}

const PROTOCOL_VERSION: &str = "5.3";
const DELIM: &[u8] = b"<IDS|MSG>";

#[derive(Deserialize)]
struct ConnectionFile {
    transport: String,
    ip: String,
    shell_port: u16,
    iopub_port: u16,
    stdin_port: u16,
    control_port: u16,
    hb_port: u16,
    key: String,
    signature_scheme: String,
}

fn endpoint(conn: &ConnectionFile, port: u16) -> String {
    format!("{}://{}:{}", conn.transport, conn.ip, port)
}

/// A parsed, signature-verified incoming message.
struct Incoming {
    /// Identity frames to echo back on a ROUTER reply (empty for iopub,
    /// which has no request driving it).
    identities: Vec<Bytes>,
    header: Value,
    content: Value,
}

impl Incoming {
    fn msg_type(&self) -> &str {
        self.header
            .get("msg_type")
            .and_then(Value::as_str)
            .unwrap_or("")
    }
}

/// HMAC-SHA256 hex digest over header‖parent_header‖metadata‖content, per
/// the Jupyter wire protocol. Empty key means unsigned: empty signature.
fn sign(key: &[u8], parts: [&[u8]; 4]) -> String {
    if key.is_empty() {
        return String::new();
    }
    let mut mac =
        <Hmac<Sha256> as KeyInit>::new_from_slice(key).expect("HMAC accepts keys of any length");
    for p in parts {
        mac.update(p);
    }
    let mut hex = String::with_capacity(64);
    for byte in mac.finalize().into_bytes() {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// Parse the frames of a ROUTER-received message: identity frames, then the
/// `<IDS|MSG>` delimiter, then signature/header/parent_header/metadata/
/// content. Trailing buffer frames are ignored. Returns `None` if the
/// framing is malformed or the signature doesn't verify.
fn parse_message(msg: ZmqMessage, key: &[u8]) -> Option<Incoming> {
    let frames: Vec<Bytes> = msg.into_vec();
    let delim_at = frames.iter().position(|f| f.as_ref() == DELIM)?;
    let identities = frames[..delim_at].to_vec();
    let rest = &frames[delim_at + 1..];
    if rest.len() < 5 {
        return None;
    }
    let (sig, header_b, parent_b, metadata_b, content_b) =
        (&rest[0], &rest[1], &rest[2], &rest[3], &rest[4]);

    if !key.is_empty() {
        let expected = sign(key, [header_b, parent_b, metadata_b, content_b]);
        if expected.as_bytes() != sig.as_ref() {
            return None;
        }
    }

    let header: Value = serde_json::from_slice(header_b).ok()?;
    let content: Value = serde_json::from_slice(content_b).ok()?;
    Some(Incoming {
        identities,
        header,
        content,
    })
}

/// Build a header for an outgoing message: fresh msg_id, shared session,
/// current UTC timestamp.
fn make_header(session: &str, msg_type: &str) -> Value {
    json!({
        "msg_id": Uuid::new_v4().to_string(),
        "session": session,
        "username": "csv-ls",
        "date": time::now(),
        "msg_type": msg_type,
        "version": PROTOCOL_VERSION,
    })
}

/// Assemble the six-plus-N frame wire message: identities, delimiter,
/// signature, header, parent_header, metadata, content.
fn build_message(
    identities: &[Bytes],
    key: &[u8],
    header: &Value,
    parent_header: &Value,
    content: &Value,
) -> ZmqMessage {
    let metadata = json!({});
    let header_b = serde_json::to_vec(header).expect("Value always serializes");
    let parent_b = serde_json::to_vec(parent_header).expect("Value always serializes");
    let metadata_b = serde_json::to_vec(&metadata).expect("Value always serializes");
    let content_b = serde_json::to_vec(content).expect("Value always serializes");
    let sig = sign(key, [&header_b, &parent_b, &metadata_b, &content_b]);

    let mut frames: Vec<Bytes> = identities.to_vec();
    frames.push(Bytes::from_static(DELIM));
    frames.push(Bytes::from(sig.into_bytes()));
    frames.push(Bytes::from(header_b));
    frames.push(Bytes::from(parent_b));
    frames.push(Bytes::from(metadata_b));
    frames.push(Bytes::from(content_b));
    ZmqMessage::try_from(frames).expect("at least one frame is always present")
}

/// Reply to a shell/control request on a ROUTER socket, addressed to the
/// request's own identity frames.
async fn reply(
    socket: &mut RouterSocket,
    key: &[u8],
    session: &str,
    incoming: &Incoming,
    msg_type: &str,
    content: Value,
) -> Result<(), Error> {
    let header = make_header(session, msg_type);
    let msg = build_message(
        &incoming.identities,
        key,
        &header,
        &incoming.header,
        &content,
    );
    socket.send(msg).await?;
    Ok(())
}

/// Publish on iopub. The topic frame (used for PUB/SUB filtering) is the
/// message type, matching ipykernel's convention.
async fn publish(
    iopub: &mut PubSocket,
    key: &[u8],
    session: &str,
    parent_header: &Value,
    msg_type: &str,
    content: Value,
) -> Result<(), Error> {
    log(&format!("iopub publish: {msg_type}"));
    let header = make_header(session, msg_type);
    let topic = [Bytes::from(msg_type.as_bytes().to_vec())];
    let msg = build_message(&topic, key, &header, parent_header, &content);
    iopub.send(msg).await?;
    Ok(())
}

async fn publish_status(
    iopub: &mut PubSocket,
    key: &[u8],
    session: &str,
    parent_header: &Value,
    state: &str,
) -> Result<(), Error> {
    publish(
        iopub,
        key,
        session,
        parent_header,
        "status",
        json!({"execution_state": state}),
    )
    .await
}

fn kernel_info_content() -> Value {
    let language = std::env::var("CSV_KERNEL_LANGUAGE").unwrap_or_else(|_| "csv".to_string());
    json!({
        "status": "ok",
        "protocol_version": PROTOCOL_VERSION,
        "implementation": "csv-kernel",
        "implementation_version": "0.2.0",
        "language_info": {
            "name": language,
            "version": "rfc4180",
            "mimetype": "text/csv",
            "file_extension": ".csv",
        },
        "banner": "csv-kernel (csv-ls): renders delimiter-separated values as tables",
        "help_links": [],
    })
}

/// Handle one shell-channel request, wrapped in the required busy/idle
/// status pair on iopub. Unknown message types still get busy/idle but no
/// reply (matching real kernels' tolerance of unimplemented message types).
async fn handle_shell(
    shell: &mut RouterSocket,
    iopub: &mut PubSocket,
    key: &[u8],
    session: &str,
    execution_count: &mut u64,
    incoming: Incoming,
) -> Result<(), Error> {
    log(&format!("shell request: {}", incoming.msg_type()));
    publish_status(iopub, key, session, &incoming.header, "busy").await?;

    match incoming.msg_type() {
        "kernel_info_request" => {
            reply(
                shell,
                key,
                session,
                &incoming,
                "kernel_info_reply",
                kernel_info_content(),
            )
            .await?;
        }
        "execute_request" => {
            let code = incoming.content["code"].as_str().unwrap_or("");
            let silent = incoming.content["silent"].as_bool().unwrap_or(false);
            let store_history = incoming.content["store_history"].as_bool().unwrap_or(true);

            let delimiter = table::resolve_delimiter(code);
            if let Some(built) = table::build(code, delimiter) {
                if !silent {
                    let data = json!({table::MIME: built.value, "text/plain": built.summary});
                    publish(
                        iopub,
                        key,
                        session,
                        &incoming.header,
                        "display_data",
                        json!({"data": data, "metadata": {}}),
                    )
                    .await?;
                }
            }
            if !silent && store_history {
                *execution_count += 1;
            }
            reply(
                shell,
                key,
                session,
                &incoming,
                "execute_reply",
                json!({
                    "status": "ok",
                    "execution_count": *execution_count,
                    "payload": [],
                    "user_expressions": {},
                }),
            )
            .await?;
        }
        "comm_info_request" => {
            reply(
                shell,
                key,
                session,
                &incoming,
                "comm_info_reply",
                json!({"status": "ok", "comms": {}}),
            )
            .await?;
        }
        _ => {}
    }

    publish_status(iopub, key, session, &incoming.header, "idle").await?;
    Ok(())
}

/// Handle one control-channel request. Returns `true` if the kernel should
/// exit after this message (a shutdown request).
async fn handle_control(
    control: &mut RouterSocket,
    iopub: &mut PubSocket,
    key: &[u8],
    session: &str,
    incoming: Incoming,
) -> Result<bool, Error> {
    log(&format!("control request: {}", incoming.msg_type()));
    publish_status(iopub, key, session, &incoming.header, "busy").await?;

    let mut shutdown = false;
    match incoming.msg_type() {
        "shutdown_request" => {
            let restart = incoming.content["restart"].clone();
            reply(
                control,
                key,
                session,
                &incoming,
                "shutdown_reply",
                json!({"status": "ok", "restart": restart}),
            )
            .await?;
            shutdown = true;
        }
        "interrupt_request" => {
            // Execution is instantaneous; nothing to interrupt.
            reply(
                control,
                key,
                session,
                &incoming,
                "interrupt_reply",
                json!({"status": "ok"}),
            )
            .await?;
        }
        _ => {}
    }

    publish_status(iopub, key, session, &incoming.header, "idle").await?;
    Ok(shutdown)
}

/// Entry point for `csv-ls kernel -f <connection_file>`. Spins up a
/// single-threaded tokio runtime for the lifetime of the kernel process.
pub fn run(connection_file: &Path) -> Result<(), Error> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(run_async(connection_file))
}

async fn run_async(connection_file: &Path) -> Result<(), Error> {
    let text = std::fs::read_to_string(connection_file)?;
    let conn: ConnectionFile = serde_json::from_str(&text)?;
    if !conn.key.is_empty() && conn.signature_scheme != "hmac-sha256" {
        return Err(format!("unsupported signature scheme: {}", conn.signature_scheme).into());
    }
    let key = conn.key.as_bytes().to_vec();
    let session = Uuid::new_v4().to_string();

    let mut shell = RouterSocket::new();
    shell.bind(&endpoint(&conn, conn.shell_port)).await?;
    let mut control = RouterSocket::new();
    control.bind(&endpoint(&conn, conn.control_port)).await?;
    // Bound so clients can connect, but the REPL never uses stdin (no
    // `input()`-style prompts to service); never read from it.
    let mut stdin_sock = RouterSocket::new();
    stdin_sock.bind(&endpoint(&conn, conn.stdin_port)).await?;
    let mut iopub = PubSocket::new();
    let mut iopub_monitor = iopub.monitor();
    iopub.bind(&endpoint(&conn, conn.iopub_port)).await?;
    let mut hb = RepSocket::new();
    hb.bind(&endpoint(&conn, conn.hb_port)).await?;
    log("sockets bound, waiting for an iopub subscriber");

    // PUB/SUB slow joiner: Zed sends kernel_info and the first (queued)
    // execute_request as soon as its sockets connect, without waiting for
    // the iopub subscription handshake to reach this PUB socket — anything
    // published before that is silently dropped, and the first `repl: run`
    // would show no table. Hold off processing (requests queue in the shell
    // socket meanwhile) until a peer actually connects to iopub, plus a
    // beat for its subscription frame; the timeout keeps a subscriber-less
    // client from hanging the kernel forever.
    let accepted = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while let Some(event) = iopub_monitor.next().await {
            if matches!(event, SocketEvent::Accepted(..)) {
                return true;
            }
        }
        false
    })
    .await;
    match accepted {
        Ok(true) => log("iopub subscriber connected"),
        Ok(false) => log("iopub monitor closed without a subscriber"),
        Err(_) => log("no iopub subscriber within 10s; proceeding"),
    }
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let heartbeat = tokio::spawn(async move {
        // A REP socket's send() re-attaches the envelope recv() stripped,
        // so this is a verbatim echo with no extra bookkeeping.
        while let Ok(msg) = hb.recv().await {
            if hb.send(msg).await.is_err() {
                break;
            }
        }
    });

    let mut execution_count: u64 = 0;
    loop {
        tokio::select! {
            msg = control.recv() => {
                let Ok(msg) = msg else { break };
                let Some(incoming) = parse_message(msg, &key) else { continue };
                if handle_control(&mut control, &mut iopub, &key, &session, incoming).await? {
                    break;
                }
            }
            msg = shell.recv() => {
                let Ok(msg) = msg else { break };
                let Some(incoming) = parse_message(msg, &key) else { continue };
                handle_shell(&mut shell, &mut iopub, &key, &session, &mut execution_count, incoming).await?;
            }
        }
    }

    heartbeat.abort();
    drop(stdin_sock);
    Ok(())
}
