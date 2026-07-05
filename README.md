# zed-csv-toolkit

A Zed extension for delimiter-separated values — CSV, TSV, SSV (semicolon),
and PSV (pipe) — that goes beyond syntax highlighting:

- **Rainbow columns**: each column is highlighted in a cycling color, via the
  [rainbow-csv-tree-sitter](https://github.com/coroa/rainbow-csv-tree-sitter)
  grammar (pinned by commit; Zed fetches and builds it itself).
- **`csv-ls`, a lightweight language server** (in this repo, ~500 lines of
  dependency-light Rust):
  - *Diagnostics*: rows whose field count differs from the header row
    (ragged rows), unclosed quotes, and text after a closing quote — the
    errors that silently corrupt CSVs.
  - *Hover*: hovering any cell shows its column name (from the header row),
    column index, and row position — invaluable in wide files where the
    header is scrolled away.
  - Handles RFC 4180 quoting, including multi-line quoted fields, and is
    lenient: malformed input is recovered from and reported, never fatal.

## Design decisions

**Why no GUI table editor?** Zed's extension API (WASM, `zed_extension_api`)
only lets extensions contribute languages (tree-sitter grammars + queries),
language servers, debuggers, themes, icon themes, snippets, and agent/MCP
servers. There is no API for custom panes or views, so a VSCode-style
spreadsheet editor is not possible in any Zed extension today. If Zed ever
grows such an API, this repo is the natural place to add it.

**Relationship to the rainbow grammar.** `coroa/rainbow-csv-tree-sitter` is
a standalone tree-sitter *grammar* repository (a fork of
`Kalmaegi/rainbow-csv-tree-sitter`), not a Zed extension. Zed clones it at
the pinned commit and compiles it to WASM itself; nothing from it is vendored
here or linked into `csv-ls`. The entire coupling surface is the grammar
names in `extension.toml` plus the seven node names (`first`…`seventh`)
referenced by `languages/*/highlights.scm`.

**Why one extension, not several?** The grammar and the language server are
complementary and share the language definitions (`languages/*/config.toml`);
Zed has no extension-bundle concept, and splitting would only multiply
maintenance. The LSP is still independently usable from any LSP client.

**Why a hand-rolled parser?** The `csv` crate doesn't expose per-field source
spans in LSP coordinates (line + UTF-16 column). The scanner in
`crates/csv-ls/src/parse.rs` is ~200 lines, has no dependencies, and is
fully unit-tested — easier to audit than to wrap.

**Delimiter selection** follows the Zed language id (`CSV` → `,`, `TSV` →
tab, `SSV` → `;`, `PSV` → `|`), falling back to content sniffing for unknown
language ids. For a semicolon-delimited `.csv` file, assign it to the SSV
language in Zed (language selector in the status bar).

## Development

Everything is driven by [pixi](https://pixi.sh); Rust comes from your system
`rustup` (standard `~/.rustup` / `~/.cargo` locations):

```sh
pixi run bootstrap-rust  # idempotent: stable toolchain + wasm32-wasip1 target
pixi run test            # unit tests
pixi run lint            # rustfmt --check + clippy -D warnings
pixi run build           # csv-ls (native) + extension (wasm)
pixi run ci              # all of the above
```

### Trying it in Zed

1. `pixi run build-lsp` and put `target/release/csv-ls` on your `PATH`
   (or set it explicitly in Zed settings, see below).
2. In Zed: command palette → `zed: install dev extension` → select this
   repo's root. Zed compiles the extension itself, so it needs a rustup
   Rust with the `wasm32-wasip1` target — exactly what
   `pixi run bootstrap-rust` ensures.
3. Open a `.csv`/`.tsv`/`.psv` file.

Binary resolution order for the language server:

1. Zed settings override:
   ```json
   { "lsp": { "csv-ls": { "binary": { "path": "/abs/path/to/csv-ls" } } } }
   ```
2. `csv-ls` found on `PATH`.
3. Download from this repo's GitHub releases (`csv-ls-<target>.tar.gz`,
   published by CI on `v*` tags). This only works while the repo is public.

CI (GitHub Actions) runs `pixi run ci` on Linux and macOS and uploads the
extension WASM plus `csv-ls` binaries for six targets — Linux x86_64/aarch64,
macOS arm64/x86_64 (cross-compiled on the same runners), and Windows
x86_64/aarch64 (cargo-driven; the bash-based pixi tasks make Windows a
compile-target rather than a pixi dev platform) — so you can grab a binary
from the latest Actions run instead of building locally.

## Inline table view (Jupyter kernel built into csv-ls)

Zed has no extension API for custom panes, but its REPL natively renders
Jupyter `application/vnd.dataresource+json` output as an inline table
(`crates/repl/src/outputs/table.rs` — its highest-ranked output type).
`csv-ls` doubles as a tiny Jupyter kernel that exploits this: "executing"
CSV text replies with exactly that MIME type, so Zed draws a real table
below your selection. Read-only — Zed's table widget has no editing/sorting
hooks — but it's a genuine table GUI without forking Zed.

Setup: **none**. The kernel is built into `csv-ls` itself (`csv-ls kernel`,
implementing the Jupyter wire protocol in Rust — no Python, no ipykernel).
Whenever the language server starts (i.e. the first time you open a CSV
file), it installs/refreshes four kernelspecs (`csv`, `tsv`, `ssv`, `psv`)
in the standard Jupyter location (`~/Library/Jupyter/kernels` on macOS,
`~/.local/share/jupyter/kernels` on Linux, `%APPDATA%\jupyter\kernels` on
Windows), pointing at its own binary — so the specs heal themselves when
the binary moves, e.g. across extension updates. Kernelspecs it didn't
generate (no `"metadata": {"generated_by": "csv-ls"}`) are never touched —
except specs left behind by this repo's retired Python kernel (argv
pointing at `csv_kernel.py`), which are recognized and migrated.
To opt out, set `CSV_LS_NO_KERNELSPECS=1` in the server's environment, or
remove the spec directories with `jupyter kernelspec remove csv tsv ssv psv`.

Usage: open a CSV file in Zed, select the rows you want (include the header;
`cmd-a` for the whole file), then run `repl: run` (`ctrl-shift-enter`). The
table appears inline; `repl: clear outputs` removes it. Zed auto-matches the
kernel because each kernelspec's `language` equals the Zed language name; if
you have several kernels per language, pin it in Zed settings:

```json
{ "jupyter": { "kernel_selections": { "csv": "csv" } } }
```

### One-keystroke "preview"

Zed has no preview pane API for extensions (markdown/SVG previews are
built-in features), so the closest thing to "preview this CSV" is a
keybinding that chains select-all + run + deselect via
`workspace::SendKeystrokes`:

```json
[
  {
    "context": "Editor && extension == csv",
    "bindings": {
      "ctrl-alt-p": ["workspace::SendKeystrokes", "cmd-a ctrl-shift-enter escape"]
    }
  }
]
```

(on Linux use `ctrl-a`; duplicate the block with `extension == tsv` etc.
as needed). Extensions cannot ship keybindings, so this stays a
copy-paste snippet.

### Markdown preview of a table

The REPL table and Zed's markdown preview render differently (the preview
wraps text, for one), so both views are worth having. Instead of the manual
chain (open table → copy → new buffer → paste → set language → preview),
`csv-ls markdown <file>` renders the file as a GFM pipe table directly —
same escaping as the table widget's copy button — and `--temp` writes it to
a stable `<stem>.md` in the system temp dir and prints the path. Wire it up
as a Zed task plus a keybinding (`zed` here is Zed's CLI, `cli: install`
from the command palette on macOS):

```json
// tasks.json
{
  "label": "csv: markdown preview",
  "command": "zed \"$(csv-ls markdown --temp \"$ZED_FILE\")\"",
  "reveal": "never",
  "hide": "always"
}
```

```json
// keymap.json
{
  "context": "Editor && extension == csv",
  "bindings": {
    "ctrl-alt-m": ["task::Spawn", { "task_name": "csv: markdown preview" }]
  }
}
```

The task needs `csv-ls` and `zed` findable from the task shell; if it
seems to do nothing, set `"reveal": "always"` and `"hide": "never"`
temporarily to see the command's error, and check `command -v csv-ls` in
Zed's terminal (a GUI-launched Zed may have a shorter PATH than your
shell — hardcode the absolute path in the task if so).

One keystroke opens the markdown buffer; your usual `markdown: open
preview` key does the rest (a task cannot press it for you — the buffer
opens asynchronously, so a `SendKeystrokes` chain would fire too early).
Re-running the task rewrites the same temp file and Zed reloads the open
buffer, so the preview stays one keystroke away as the CSV evolves.

### What the table view can(not) do

The rendering is Zed's own widget (`crates/repl/src/outputs/table.rs`),
which the kernel cannot influence beyond the data it sends: columns
autosize to their widest cell, long cells make the table scroll
horizontally (no wrapping), and there is no sorting, column resizing, or
other interaction — only copy, which yields a markdown table (or use
`csv-ls markdown`, above, which produces the same thing without the mouse).

Note on flags: the REPL is generally available and needs **no** feature
flag or environment variable. What *is* gated (dev builds via
`LOCAL_NOTEBOOK_DEV=1`, or the staff `notebooks` feature flag) is Zed's
separate native `.ipynb` notebook UI. That UI is hardcoded to `.ipynb`
files and has no extension hook, so a CSV cannot be opened "as a notebook";
this kernel-through-the-REPL route is the only table view available to an
extension today.

## Roadmap ideas (deliberately not yet included)

- Completions of values already present in the current column.
- Code actions: normalize quoting, trim whitespace, transpose header case.
- Document symbols for header navigation.

## License

MIT. The rainbow grammar is fetched by Zed from its own repository and is
not vendored here.
