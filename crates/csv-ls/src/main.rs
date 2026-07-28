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
use std::path::{Path, PathBuf};

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
use sha2::{Digest, Sha256};

struct Doc {
    parsed: parse::Parsed,
    delim: char,
}

type Error = Box<dyn std::error::Error + Sync + Send>;

const USAGE: &str = "\
csv-ls — language server, Jupyter kernel, and markdown renderer for
delimiter-separated values (CSV/TSV/SSV/PSV).

Usage:
  csv-ls                                speak LSP over stdio (default)
  csv-ls kernel -f <connection_file>    run as a Jupyter kernel
  csv-ls install-kernelspecs            install the csv/tsv/ssv/psv kernelspecs
  csv-ls uninstall-kernelspecs          remove the kernelspecs csv-ls installed
  csv-ls markdown <file> [--temp]       render as a GitHub-flavored markdown
                                        table; --temp writes it to a temp file
                                        and prints that path instead
  csv-ls --help | --version

The LSP writes kernelspecs only when the client asks it to, by sending
`initializationOptions: {\"install_kernelspecs\": true}`; otherwise nothing
outside the editor's own directories is ever touched.

Environment:
  JUPYTER_DATA_DIR          where kernelspecs are read and written
";

/// `csv-ls` with no args speaks LSP over stdio (the original behavior);
/// every other mode is a subcommand. See `USAGE`.
fn main() -> Result<(), Error> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => run_lsp(),
        Some("kernel") => kernel::run(&connection_file_arg(&args[1..])?),
        Some("install-kernelspecs") => kernelspec::install(true),
        Some("uninstall-kernelspecs") => kernelspec::uninstall(),
        Some("markdown") => markdown_command(&args[1..]),
        Some("--help" | "-h" | "help") => {
            print!("{USAGE}");
            Ok(())
        }
        Some("--version" | "-V") => {
            println!("csv-ls {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some(other) => {
            Err(format!("csv-ls: unknown subcommand `{other}`; try `csv-ls --help`").into())
        }
    }
}

/// Render a delimiter-separated file as a GFM pipe table. By default the
/// markdown goes to stdout; `--temp` instead writes it under the system temp
/// dir (see `temp_markdown_path`) and prints that path — a stable path, so
/// re-running updates the same file and Zed reloads the already-open buffer.
/// The delimiter comes from the file extension, falling back to sniffing.
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
        let out = temp_markdown_path(&file)?;
        std::fs::write(&out, md)?;
        println!("{}", out.display());
    } else {
        print!("{md}");
    }
    Ok(())
}

/// Where `--temp` writes:
/// `<temp>/csv-ls-<user>/<digest of the source path>/<stem>.md`.
///
/// Two constraints shape that. The path has to be stable across runs — that
/// is what lets Zed reload an already-open preview buffer instead of opening
/// a second one — but writing a predictable `<stem>.md` straight into the
/// temp dir was wrong on both counts: two `data.csv` files in different
/// directories overwrote each other, and on Linux the temp dir is shared and
/// world-writable, so another local user could pre-plant `/tmp/data.md` as a
/// symlink and redirect the write. A per-source subdirectory fixes the
/// collision and keeps the stem in the filename (so the Zed tab still reads
/// `data.md`), and the directories are created by us, private, and refused
/// if they turn out to be anything but a real directory.
fn temp_markdown_path(file: &Path) -> Result<PathBuf, Error> {
    let root = temp_root();
    create_private_dir(&root)?;

    // Canonicalize so the same file reached by different paths maps to one
    // output; the source is known to exist here, it was just read.
    let source = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let mut digest = Sha256::new();
    digest.update(source.to_string_lossy().as_bytes());
    let dir = root.join(
        digest.finalize()[..8]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
    );
    create_private_dir(&dir)?;

    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("table");
    Ok(dir.join(format!("{stem}.md")))
}

/// The root the per-source directories live under. Its name carries a
/// per-user component because `create_private_dir` insists on owning what it
/// finds: a single shared `/tmp/csv-ls` would be created 0700 by whichever
/// user ran first and then fail for every other user on the machine. Windows
/// and macOS already hand out a per-user temp dir, so the component is
/// redundant there and only Linux/BSD actually need it.
fn temp_root() -> PathBuf {
    let temp = std::env::temp_dir();
    #[cfg(unix)]
    {
        temp.join(format!("csv-ls-{}", user_tag()))
    }
    #[cfg(not(unix))]
    {
        temp.join("csv-ls")
    }
}

/// Something that differs between users of the same machine. `/proc/self` is
/// owned by our real uid, which covers Linux — the platform with the shared
/// temp dir — without a libc dependency; elsewhere fall back to the login
/// name Zed inherits from the user's shell. This only keeps users out of each
/// other's directory: `create_private_dir` is what makes the path safe, so a
/// spoofed or missing value costs nothing.
#[cfg(unix)]
fn user_tag() -> String {
    use std::os::unix::fs::MetadataExt;
    if let Ok(md) = std::fs::metadata("/proc/self") {
        return md.uid().to_string();
    }
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "shared".to_string())
}

/// Create `dir` if it is missing and make sure what is there is a real
/// directory only we can write to. `symlink_metadata` does not follow the
/// final component, so a symlink someone else planted is rejected here
/// rather than silently written through. If the directory exists but belongs
/// to another user, `set_permissions` fails and we stop — the safe outcome.
fn create_private_dir(dir: &Path) -> Result<(), Error> {
    match std::fs::create_dir(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    if !std::fs::symlink_metadata(dir)?.is_dir() {
        return Err(format!("csv-ls: {} is not a directory", dir.display()).into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
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

/// Refresh the kernelspecs behind the inline table view — but only for a
/// client that asked, with `initializationOptions.install_kernelspecs`. A
/// Jupyter data directory is user-managed state outside the directory the
/// editor designates for this extension, so opening a CSV must never write
/// there on its own; opting in is the explicit installation action, and it
/// then re-runs on every start so the specs follow the binary when a release
/// upgrade moves it. Notes go to stderr, which Zed captures (`zed: open
/// log`); stdout is the LSP channel and must stay clean.
fn install_kernelspecs_if_opted_in(init_params: &serde_json::Value) {
    let opted_in = init_params
        .get("initializationOptions")
        .and_then(|opts| opts.get("install_kernelspecs"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if !opted_in {
        // Says why `repl: run` finds no kernel, which is otherwise silent.
        eprintln!(
            "csv-ls: inline table view off; kernelspecs are not installed \
             (set lsp.csv-ls.initialization_options.install_kernelspecs to true, \
             or run `csv-ls install-kernelspecs`)"
        );
        return;
    }
    match kernelspec::install(false) {
        Ok(()) => eprintln!("csv-ls: kernelspecs installed"),
        Err(e) => eprintln!("csv-ls: kernelspec install failed: {e}"),
    }
}

fn run_lsp() -> Result<(), Error> {
    let (connection, io_threads) = Connection::stdio();
    let capabilities = serde_json::to_value(ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        ..ServerCapabilities::default()
    })?;
    let init_params = connection.initialize(capabilities)?;
    install_kernelspecs_if_opted_in(&init_params);
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
