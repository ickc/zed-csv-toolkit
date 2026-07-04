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

## Roadmap ideas (deliberately not yet included)

- Completions of values already present in the current column.
- Code actions: normalize quoting, trim whitespace, transpose header case.
- Document symbols for header navigation.
- A read-only table *view* via Zed's REPL: Zed natively renders
  `application/vnd.dataresource+json` Jupyter output as a table
  (`crates/repl/src/outputs/table.rs`), so a small Jupyter kernel that
  parses CSV and emits that MIME type would display tables inline —
  no Zed fork required.

## License

MIT. The rainbow grammar is fetched by Zed from its own repository and is
not vendored here.
