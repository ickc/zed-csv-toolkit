# Maintainer notes

User-facing docs are in [README.md](README.md); this file covers
development, releasing, internals, and design rationale.

## Development

Everything is driven by [pixi](https://pixi.sh); Rust comes from your
system `rustup` (standard `~/.rustup` / `~/.cargo` locations):

```sh
pixi run bootstrap-rust        # idempotent: stable toolchain + wasm32-wasip1
pixi run test                  # unit tests
pixi run lint                  # rustfmt --check + clippy -D warnings
pixi run build                 # csv-ls (native) + extension (wasm)
pixi run ci                    # all of the above
pixi run -e kernel test-kernel # jupyter_client integration test (builds csv-ls first)
```

### Trying it in Zed

1. `pixi run build-lsp` and put `target/release/csv-ls` on your PATH (or
   set `lsp.csv-ls.binary.path` in Zed settings). PATH/settings win over
   the GitHub-releases download, so a stale local binary shadows releases.
2. Command palette → `zed: install dev extension` → select this repo's
   root. Zed compiles the WASM itself, so it needs a rustup Rust with the
   `wasm32-wasip1` target — exactly what `pixi run bootstrap-rust` ensures.
3. Open a `.csv`/`.tsv`/`.psv` file.

## Repo layout

- `src/lib.rs` — the Zed extension (WASM). Resolves the csv-ls binary
  (settings → PATH → GitHub releases, cached per version) and maintains a
  versionless alias at `extensions/work/csv-toolkit/bin/csv-ls` for use
  from user tasks.
- `crates/csv-ls` — one dependency-light binary, four modes:
  - no args: the LSP (`main.rs`, `parse.rs`, `analysis.rs`). Full-document
    sync; hand-rolled RFC 4180 scanner because the `csv` crate doesn't
    expose per-field spans in LSP coordinates (line + UTF-16 column).
  - `kernel -f <connection_file>`: Jupyter kernel (`kernel.rs`,
    `table.rs`, `time.rs`).
  - `install-kernelspecs`: writes the four kernelspecs (`kernelspec.rs`);
    also invoked best-effort on every LSP start.
  - `markdown <file> [--temp]`: GFM pipe-table renderer.
- `languages/`, `extension.toml` — Zed language definitions and the
  pinned rainbow grammar commits.
- `csv-kernel/test_kernel.py` — integration test driving the built binary
  through `jupyter_client` (kernelspec install, no-clobber, migration,
  execution over zmq).

## How the inline table view works

Zed has no extension API for custom panes, but its REPL natively renders
Jupyter `application/vnd.dataresource+json` output as an inline table
(`crates/repl/src/outputs/table.rs` — its highest-ranked output type).
The kernel "executes" CSV text by replying with exactly that MIME type.

Protocol notes (hard-won; keep these invariants):

- `kernel_info_reply` must include `language_info.name` **and**
  `.version` — Zed's runtimelib deserializes strictly and a missing field
  kills the kernel silently.
- **PUB/SUB slow joiner**: Zed sends `kernel_info` and the queued first
  `execute_request` the moment its sockets connect, without waiting for
  the iopub subscription handshake; anything published before a
  subscriber joins is dropped by zeromq. The kernel therefore waits for a
  `SocketEvent::Accepted` on the iopub monitor (10s timeout) before
  processing requests. Kernel stderr is captured into Zed's log
  (`zed: open log`), and the kernel logs every request/publish — the
  first thing to check when the table doesn't appear.
- Kernelspec install is idempotent and self-healing: specs are rewritten
  when the binary path changes; only specs with
  `metadata.generated_by == "csv-ls"` (or legacy argv referencing the
  retired `csv_kernel.py`) are ever overwritten.
- Zed auto-matches kernels because each kernelspec's `language` equals
  the Zed language name.

What the table cannot do is set by Zed's widget: autosized columns, no
wrapping, no sorting, no interaction beyond copy-as-markdown. The
`.ipynb` notebook UI (`LOCAL_NOTEBOOK_DEV=1`) is hardcoded to `.ipynb`
and not extension-accessible, and markdown/SVG previews are built-in
features with no extension hook — the kernel-through-the-REPL route is
the only table view available to an extension today.

## Releasing

1. Bump `version` in `extension.toml`, `Cargo.toml`, and
   `crates/csv-ls/Cargo.toml`; refresh the lockfile:
   `cargo update -p csv-ls -p zed-csv-toolkit --offline`.
2. Commit, tag `v<version>`, push branch and tag.
3. CI builds six targets (Linux/macOS/Windows × x64/aarch64) and, on the
   tag, publishes `csv-ls-<target>.tar.gz` release assets.
4. The extension checks the latest GitHub release whenever it (re)loads,
   so release-path users pick the new binary up on their next Zed
   restart; the LSP then refreshes the kernelspecs to the new path.

## Design decisions

- **Why one extension, not several?** The grammar and the language server
  share the language definitions; Zed has no extension-bundle concept.
  The LSP is still independently usable from any LSP client.
- **Relationship to the rainbow grammar.** `coroa/rainbow-csv-tree-sitter`
  is a standalone grammar repo; Zed clones and compiles it itself. The
  coupling surface is the grammar names in `extension.toml` plus the seven
  node names (`first`…`seventh`) in `languages/*/highlights.scm`.
- **Delimiter selection** follows the Zed language id (`CSV` → `,`,
  `TSV` → tab, `SSV` → `;`, `PSV` → `|`), falling back to content
  sniffing for unknown ids.
- **Why is the kernel in Rust, inside csv-ls?** The WASM extension is
  sandboxed and cannot write kernelspecs or ship a Python runtime; the
  native LSP binary Zed already downloads can do both, so it doubles as
  the kernel (same pure-Rust `zeromq` stack as Zed's runtimelib) —
  zero-setup for users.

## Roadmap ideas (deliberately not yet included)

- Completions of values already present in the current column.
- Code actions: normalize quoting, trim whitespace, transpose header case.
- Document symbols for header navigation.
