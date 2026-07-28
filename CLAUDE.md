# CLAUDE.md

Zed extension for CSV/TSV/SSV/PSV: rainbow grammar + `csv-ls`, one binary
that is LSP, Jupyter kernel, kernelspec installer, and markdown renderer.
Internals and design rationale: [MAINTAINER.md](MAINTAINER.md). User docs:
[README.md](README.md) — keep it concise; depth goes in MAINTAINER.md.

## Commands

- `pixi run ci` — fmt-check, clippy `-D warnings`, tests, native + wasm
  builds. Must pass before committing.
- `pixi run -e kernel test-kernel` — jupyter_client integration test
  against `target/release/csv-ls`. Run it for any kernel/kernelspec change.
- If `cargo` is missing from a raw shell: `export PATH="$HOME/.cargo/bin:$PATH"`.
- Release: bump version in `extension.toml` + both `Cargo.toml`s,
  `cargo update -p csv-ls -p zed-csv-toolkit --offline`, commit, tag `v*`,
  push; CI publishes the release assets the extension auto-downloads.

## Invariants (violating these breaks users silently)

- `kernel_info_reply` needs `language_info.name` and `.version`: Zed's
  runtimelib deserializes strictly.
- Never publish on iopub before a subscriber has joined (PUB/SUB slow
  joiner): the kernel waits for `SocketEvent::Accepted` before processing.
  Kernel stderr lands in Zed's log — keep the `log()` calls.
- `install-kernelspecs` may overwrite only specs marked
  `metadata.generated_by == "csv-ls"` or legacy specs whose argv references
  `csv_kernel.py`; foreign specs are untouchable. The same rule governs
  `uninstall-kernelspecs`.
- The LSP installs kernelspecs only when the client opts in with
  `initializationOptions.install_kernelspecs` — Zed's guidelines forbid
  modifying the environment outside the one Zed designates, and a Jupyter
  data directory is outside it whether or not it already exists (registry
  review: zed-industries/extensions#6990). Never make it unattended again.
- `table.rs` unit tests and `csv-kernel/test_kernel.py` assert the same
  semantics (typing, uniquify, padding, nulls) — change both together.
- Tests run in parallel threads: never mutate env vars in a test without
  the `ENV_LOCK` in `table.rs`; prefer passing values as parameters.

## Style

- Dependency-light; no clap — subcommand parsing is hand-rolled in
  `main.rs`. Match existing comment style: state constraints, don't
  narrate code.
