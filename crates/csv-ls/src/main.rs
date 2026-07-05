//! csv-ls: a lightweight language server for delimiter-separated values.
//!
//! Speaks LSP over stdio. Full-document sync only (CSV files are edited as a
//! whole; incremental sync would add complexity for little gain). The
//! delimiter is chosen from the document's language id (csv/tsv/ssv/psv) and
//! sniffed from content as a fallback.

mod analysis;
mod kernel;
mod kernelspec;
mod parse;
mod table;
mod time;

use std::collections::HashMap;
use std::path::PathBuf;

use lsp_server::{Connection, Message, Notification, Request, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, Notification as _,
    PublishDiagnostics,
};
use lsp_types::request::{HoverRequest, Request as _};
use lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams, Hover,
    HoverContents, HoverParams, HoverProviderCapability, MarkupContent, MarkupKind,
    PublishDiagnosticsParams, ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind,
    Uri,
};

struct Doc {
    parsed: parse::Parsed,
    delim: char,
}

type Error = Box<dyn std::error::Error + Sync + Send>;

/// `csv-ls` with no args speaks LSP over stdio (the original behavior).
/// `csv-ls kernel -f <connection_file>` runs as a Jupyter kernel instead.
/// `csv-ls install-kernelspecs` writes the kernelspecs and exits.
/// `csv-ls markdown <file> [--temp]` renders the file as a markdown table.
fn main() -> Result<(), Error> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => run_lsp(),
        Some("kernel") => kernel::run(&connection_file_arg(&args[1..])?),
        Some("install-kernelspecs") => kernelspec::install(true),
        Some("markdown") => markdown_command(&args[1..]),
        Some(other) => Err(format!("csv-ls: unknown subcommand `{other}`").into()),
    }
}

/// Render a delimiter-separated file as a GFM pipe table. By default the
/// markdown goes to stdout; `--temp` instead writes `<stem>.md` in the
/// system temp dir and prints its path — a stable path, so re-running
/// updates the same file (and Zed reloads the already-open buffer). The
/// delimiter comes from the file extension, falling back to sniffing.
fn markdown_command(args: &[String]) -> Result<(), Error> {
    let mut temp = false;
    let mut file: Option<PathBuf> = None;
    for arg in args {
        match arg.as_str() {
            "--temp" => temp = true,
            other if !other.starts_with('-') && file.is_none() => {
                file = Some(PathBuf::from(other));
            }
            other => return Err(format!("csv-ls markdown: unexpected argument `{other}`").into()),
        }
    }
    let file = file.ok_or("csv-ls markdown: requires a file path")?;
    let text = std::fs::read_to_string(&file)?;
    let delim = file
        .extension()
        .and_then(|e| e.to_str())
        .and_then(|e| parse::delimiter_for_language_id(&e.to_lowercase()))
        .unwrap_or_else(|| parse::sniff_delimiter(&text));
    let md = table::markdown(&text, delim)
        .ok_or_else(|| format!("csv-ls markdown: {} is empty", file.display()))?;
    if temp {
        let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("table");
        let out = std::env::temp_dir().join(format!("{stem}.md"));
        std::fs::write(&out, md)?;
        println!("{}", out.display());
    } else {
        print!("{md}");
    }
    Ok(())
}

/// Hand-rolled parsing for `-f <path>` / `--connection-file <path>`.
fn connection_file_arg(args: &[String]) -> Result<PathBuf, Error> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == "-f" || args[i] == "--connection-file" {
            let path = args
                .get(i + 1)
                .ok_or("csv-ls kernel: missing path after -f")?;
            return Ok(PathBuf::from(path));
        }
        i += 1;
    }
    Err("csv-ls kernel: requires -f <connection_file>".into())
}

fn run_lsp() -> Result<(), Error> {
    let (connection, io_threads) = Connection::stdio();
    let capabilities = serde_json::to_value(ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        ..ServerCapabilities::default()
    })?;
    connection.initialize(capabilities)?;
    // Best-effort: a REPL table view is a nice-to-have, never a hard
    // requirement for the language server to function.
    if std::env::var_os("CSV_LS_NO_KERNELSPECS")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
    {
        // Opted out.
    } else {
        match kernelspec::install(false) {
            Ok(()) => eprintln!("csv-ls: kernelspecs installed"),
            Err(e) => eprintln!("csv-ls: kernelspec install skipped: {e}"),
        }
    }
    // Take the connection by value so it (and its channel senders) is dropped
    // before joining the I/O threads; otherwise the writer thread never exits.
    main_loop(connection)?;
    io_threads.join()?;
    Ok(())
}

fn main_loop(connection: Connection) -> Result<(), Error> {
    // Keyed by the URI string: `Uri` has interior mutability (clippy's
    // mutable_key_type) and the string form is all we need.
    let mut docs: HashMap<String, Doc> = HashMap::new();

    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req)? {
                    return Ok(());
                }
                handle_request(&connection, &docs, req)?;
            }
            Message::Notification(not) => handle_notification(&connection, &mut docs, not)?,
            Message::Response(_) => {}
        }
    }
    Ok(())
}

fn handle_request(
    connection: &Connection,
    docs: &HashMap<String, Doc>,
    req: Request,
) -> Result<(), Error> {
    let response = match req.method.as_str() {
        HoverRequest::METHOD => {
            let (id, params) = req.extract::<HoverParams>(HoverRequest::METHOD)?;
            let pos = params.text_document_position_params;
            let hover = docs.get(pos.text_document.uri.as_str()).and_then(|doc| {
                analysis::hover(&doc.parsed, pos.position.line, pos.position.character).map(
                    |info| Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: info.markdown,
                        }),
                        range: Some(info.range),
                    },
                )
            });
            Response::new_ok(id, hover)
        }
        _ => Response::new_err(
            req.id,
            lsp_server::ErrorCode::MethodNotFound as i32,
            format!("unhandled method: {}", req.method),
        ),
    };
    connection.sender.send(Message::Response(response))?;
    Ok(())
}

fn handle_notification(
    connection: &Connection,
    docs: &mut HashMap<String, Doc>,
    not: Notification,
) -> Result<(), Error> {
    match not.method.as_str() {
        DidOpenTextDocument::METHOD => {
            let params = not.extract::<DidOpenTextDocumentParams>(DidOpenTextDocument::METHOD)?;
            let d = params.text_document;
            let delim = parse::delimiter_for_language_id(&d.language_id)
                .unwrap_or_else(|| parse::sniff_delimiter(&d.text));
            let doc = Doc {
                parsed: parse::parse(&d.text, delim),
                delim,
            };
            publish_diagnostics(connection, &d.uri, &doc, Some(d.version))?;
            docs.insert(d.uri.as_str().to_owned(), doc);
        }
        DidChangeTextDocument::METHOD => {
            let params =
                not.extract::<DidChangeTextDocumentParams>(DidChangeTextDocument::METHOD)?;
            // Full sync: the last change carries the whole document.
            let Some(change) = params.content_changes.into_iter().next_back() else {
                return Ok(());
            };
            let uri = params.text_document.uri;
            if let Some(doc) = docs.get_mut(uri.as_str()) {
                doc.parsed = parse::parse(&change.text, doc.delim);
                let doc = &docs[uri.as_str()];
                publish_diagnostics(connection, &uri, doc, Some(params.text_document.version))?;
            }
        }
        DidCloseTextDocument::METHOD => {
            let params = not.extract::<DidCloseTextDocumentParams>(DidCloseTextDocument::METHOD)?;
            docs.remove(params.text_document.uri.as_str());
            // Clear diagnostics for the closed file.
            let clear = PublishDiagnosticsParams {
                uri: params.text_document.uri,
                diagnostics: Vec::new(),
                version: None,
            };
            connection
                .sender
                .send(Message::Notification(Notification::new(
                    PublishDiagnostics::METHOD.into(),
                    clear,
                )))?;
        }
        _ => {}
    }
    Ok(())
}

fn publish_diagnostics(
    connection: &Connection,
    uri: &Uri,
    doc: &Doc,
    version: Option<i32>,
) -> Result<(), Error> {
    let params = PublishDiagnosticsParams {
        uri: uri.clone(),
        diagnostics: analysis::diagnostics(&doc.parsed),
        version,
    };
    connection
        .sender
        .send(Message::Notification(Notification::new(
            PublishDiagnostics::METHOD.into(),
            params,
        )))?;
    Ok(())
}
