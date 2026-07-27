# Changelog

## 0.3.0

Preparation for the Zed extension registry.

### Changed

- The kernelspec install that runs at language-server start now writes
  only where Jupyter already exists — an explicit `$JUPYTER_DATA_DIR`, or
  an existing platform data directory. On a machine with no Jupyter,
  `csv-ls` no longer creates one; it logs the skip and the command to run.
  `csv-ls install-kernelspecs` is unaffected.
- `lsp.csv-ls.binary.arguments` and `.env` are now passed through however
  the binary was resolved. Previously both were dropped, which left the
  documented `CSV_LS_NO_KERNELSPECS` opt-out with no way to be set.
- `markdown --temp` writes to `<temp>/csv-ls-<user>/<digest>/<stem>.md`
  instead of `<temp>/<stem>.md`. Two same-named CSVs in different
  directories no longer overwrite each other's preview, the output is no
  longer at a predictable path in a world-writable directory, and users
  sharing a machine no longer share a private root they cannot both own.

### Added

- `csv-ls uninstall-kernelspecs` removes the kernelspecs `csv-ls`
  installed, leaving any it did not install alone. Run it before removing
  the extension.
- `csv-ls --help` and `csv-ls --version`.

### Fixed

- Two columns could resolve to the same name when a header already
  contained the disambiguating suffix (`a (2)`, `a`, `a`), which dropped a
  column from every row of the inline table.
- Columns containing `inf`, `-Infinity`, or `NaN` typed as numbers and
  rendered as blank cells, since JSON cannot represent those values. They
  stay string columns and render verbatim.
- The kernel reported a hardcoded `implementation_version` of `0.2.0`.
- An `execute_request` with an empty selection published no output at all,
  indistinguishable from the cold-start kernel-discovery race.

## 0.2.3 and earlier

See the [GitHub releases](https://github.com/ickc/zed-csv-toolkit/releases).
