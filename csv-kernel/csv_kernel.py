"""A minimal Jupyter kernel that renders delimiter-separated values as tables.

"Executing" a cell means: parse the submitted text as CSV/TSV/SSV/PSV and
reply with a Tabular Data Resource (application/vnd.dataresource+json),
which Zed's REPL renders as a native inline table
(https://specs.frictionlessdata.io/tabular-data-resource/).

The delimiter comes from the CSV_KERNEL_DELIMITER environment variable
(set per kernelspec by scripts/install-kernel.sh); unset or "sniff" means
guess from the first line.
"""

import csv
import io
import os

from ipykernel.kernelbase import Kernel

MIME = "application/vnd.dataresource+json"
SNIFF_CANDIDATES = [",", "\t", ";", "|"]


def resolve_delimiter(text: str) -> str:
    delim = os.environ.get("CSV_KERNEL_DELIMITER", "sniff")
    if delim != "sniff":
        return delim
    first_line = next((l for l in text.splitlines() if l.strip()), "")
    counts = [(first_line.count(c), -i) for i, c in enumerate(SNIFF_CANDIDATES)]
    return SNIFF_CANDIDATES[-max(counts)[1]]


def uniquify(names: list[str]) -> list[str]:
    """Field names must be unique and non-empty to key the data objects."""
    seen: dict[str, int] = {}
    out = []
    for i, raw in enumerate(names):
        name = raw.strip() or f"column {i + 1}"
        if name in seen:
            seen[name] += 1
            name = f"{name} ({seen[name]})"
        seen.setdefault(name, 1)
        out.append(name)
    return out


def column_type(values: list[str]) -> str:
    """Frictionless field type: integer/number if every non-empty value parses."""
    non_empty = [v for v in values if v != ""]
    if not non_empty:
        return "string"
    try:
        for v in non_empty:
            int(v)
        return "integer"
    except ValueError:
        pass
    try:
        for v in non_empty:
            float(v)
        return "number"
    except ValueError:
        return "string"


def convert(value: str, typ: str):
    if value == "":
        return None
    if typ == "integer":
        return int(value)
    if typ == "number":
        return float(value)
    return value


def to_dataresource(text: str, delimiter: str) -> dict | None:
    rows = [r for r in csv.reader(io.StringIO(text), delimiter=delimiter) if r]
    if not rows:
        return None
    header, *data = rows
    width = max(len(r) for r in rows)
    names = uniquify(header + [""] * (width - len(header)))
    padded = [r + [""] * (width - len(r)) for r in data]
    types = [column_type([r[i] for r in padded]) for i in range(width)]
    return {
        "schema": {
            "fields": [{"name": n, "type": t} for n, t in zip(names, types)],
        },
        "data": [
            {n: convert(v, t) for n, v, t in zip(names, row, types)}
            for row in padded
        ],
    }


class CsvKernel(Kernel):
    implementation = "csv-kernel"
    implementation_version = "0.1.0"
    banner = "csv-kernel: renders delimiter-separated values as tables"
    # "version" is required by strict clients (Zed's runtimelib fails to
    # deserialize kernel_info_reply without language_info.version).
    language_info = {
        "name": os.environ.get("CSV_KERNEL_LANGUAGE", "csv"),
        "version": "rfc4180",
        "mimetype": "text/csv",
        "file_extension": ".csv",
    }

    def do_execute(
        self, code, silent, store_history=True, user_expressions=None,
        allow_stdin=False, *, cell_meta=None, cell_id=None,
    ):
        table = to_dataresource(code, resolve_delimiter(code))
        if table is not None and not silent:
            summary = f"{len(table['data'])} rows × {len(table['schema']['fields'])} columns"
            self.send_response(
                self.iopub_socket,
                "display_data",
                {"data": {MIME: table, "text/plain": summary}, "metadata": {}},
            )
        return {
            "status": "ok",
            "execution_count": self.execution_count,
            "payload": [],
            "user_expressions": {},
        }


if __name__ == "__main__":
    from ipykernel.kernelapp import IPKernelApp

    IPKernelApp.launch_instance(kernel_class=CsvKernel)
