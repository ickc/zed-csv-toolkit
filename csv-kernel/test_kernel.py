"""Integration test: install the kernelspecs into a temp Jupyter dir, launch
the real csv kernel through jupyter_client, execute CSV text, and check the
Tabular Data Resource output that Zed's REPL would render."""

import json
import os
import queue
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
MIME = "application/vnd.dataresource+json"


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
    with tempfile.TemporaryDirectory() as tmp:
        os.environ["JUPYTER_DATA_DIR"] = tmp
        subprocess.run(
            ["bash", str(REPO / "scripts" / "install-kernel.sh")],
            check=True,
            env=os.environ,
        )
        for name in ("csv", "tsv", "ssv", "psv"):
            spec = json.loads(
                (Path(tmp) / "kernels" / name / "kernel.json").read_text()
            )
            assert spec["language"] == name, spec

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
