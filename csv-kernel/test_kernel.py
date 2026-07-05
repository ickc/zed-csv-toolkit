"""Integration test: install the kernelspecs into a temp Jupyter dir via
`csv-ls install-kernelspecs`, launch the real csv-ls kernel through
jupyter_client, execute CSV/TSV text, and check the Tabular Data Resource
output that Zed's REPL would render."""

import json
import os
import queue
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
MIME = "application/vnd.dataresource+json"


def csv_ls_binary() -> Path:
    path = REPO / "target" / "release" / "csv-ls"
    if not path.is_file():
        raise SystemExit(
            f"{path} not found — run `pixi run build-lsp` (or `cargo build "
            "--release -p csv-ls`) before this test."
        )
    return path


def run_kernel_case(kernel_name: str, text: str):
    from jupyter_client.manager import KernelManager

    km = KernelManager(kernel_name=kernel_name)
    km.start_kernel()
    kc = km.client()
    kc.start_channels()
    try:
        kc.wait_for_ready(timeout=60)
        # Zed's runtimelib deserializes kernel_info_reply strictly; these
        # fields are required (missing language_info.version broke Zed).
        kc.kernel_info()
        info = kc.get_shell_msg(timeout=30)["content"]
        assert info["protocol_version"], info
        for field in ("name", "version"):
            assert info["language_info"].get(field), (field, info)
        # Drain this request's iopub busy/idle before executing.
        while True:
            msg = kc.get_iopub_msg(timeout=30)
            if (
                msg["msg_type"] == "status"
                and msg["content"]["execution_state"] == "idle"
            ):
                break
        kc.execute(text)
        outputs = []
        while True:
            try:
                msg = kc.get_iopub_msg(timeout=30)
            except queue.Empty:
                raise AssertionError("timed out waiting for iopub")
            if msg["msg_type"] == "display_data":
                outputs.append(msg["content"]["data"])
            if (
                msg["msg_type"] == "status"
                and msg["content"]["execution_state"] == "idle"
            ):
                break
        return outputs
    finally:
        kc.stop_channels()
        km.shutdown_kernel(now=True)


def main() -> None:
    binary = csv_ls_binary()
    with tempfile.TemporaryDirectory() as tmp:
        os.environ["JUPYTER_DATA_DIR"] = tmp
        kernels_dir = Path(tmp) / "kernels"

        # No-clobber check: a foreign kernel.json (no csv-ls marker) must
        # survive `install-kernelspecs` untouched.
        foreign_dir = kernels_dir / "csv"
        foreign_dir.mkdir(parents=True)
        foreign_spec = {"argv": ["/usr/bin/true"], "display_name": "Not ours",
                         "language": "csv"}
        (foreign_dir / "kernel.json").write_text(json.dumps(foreign_spec))

        subprocess.run(
            [str(binary), "install-kernelspecs"],
            check=True,
            env=os.environ,
        )

        unchanged = json.loads((foreign_dir / "kernel.json").read_text())
        assert unchanged == foreign_spec, unchanged
        print("no-clobber: foreign csv kernelspec left untouched")

        # Remove the foreign spec and install for real so the rest of the
        # test (and jupyter_client's kernel lookup) sees csv-ls's own specs.
        (foreign_dir / "kernel.json").unlink()
        subprocess.run(
            [str(binary), "install-kernelspecs"],
            check=True,
            env=os.environ,
        )

        for name in ("csv", "tsv", "ssv", "psv"):
            spec = json.loads((kernels_dir / name / "kernel.json").read_text())
            assert spec["language"] == name, spec
            assert Path(spec["argv"][0]).is_absolute(), spec

        outputs = run_kernel_case("csv", "name,age\nalice,30\nbob,\n")
        assert len(outputs) == 1, outputs
        table = outputs[0][MIME]
        assert table["schema"]["fields"] == [
            {"name": "name", "type": "string"},
            {"name": "age", "type": "integer"},
        ], table["schema"]
        assert table["data"] == [
            {"name": "alice", "age": 30},
            {"name": "bob", "age": None},
        ], table["data"]
        assert "2 rows" in outputs[0]["text/plain"]
        print("csv kernel: table output OK")

        outputs = run_kernel_case("tsv", "a\tb\n1.5\t2\n")
        table = outputs[0][MIME]
        assert table["schema"]["fields"][0]["type"] == "number", table
        assert table["data"] == [{"a": 1.5, "b": 2}], table["data"]
        print("tsv kernel: delimiter + number typing OK")

    print("all kernel tests passed")


if __name__ == "__main__":
    sys.exit(main())
