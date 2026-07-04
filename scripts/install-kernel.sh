#!/usr/bin/env bash
# Install Jupyter kernelspecs (csv/tsv/ssv/psv) pointing at csv_kernel.py,
# using the python of the current pixi environment. Zed's REPL discovers
# kernels from the standard Jupyter data directories.
set -euo pipefail

python_bin="$(command -v python)"
repo_root="${PIXI_PROJECT_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
kernel_dir_src="$repo_root/csv-kernel"

if [ -n "${JUPYTER_DATA_DIR:-}" ]; then
    data_dir="$JUPYTER_DATA_DIR"
elif [ "$(uname -s)" = "Darwin" ]; then
    data_dir="$HOME/Library/Jupyter"
else
    data_dir="${XDG_DATA_HOME:-$HOME/.local/share}/jupyter"
fi

install_spec() {
    local name="$1" delimiter="$2" display="$3"
    local dir="$data_dir/kernels/$name"
    mkdir -p "$dir"
    python - "$dir/kernel.json" <<EOF
import json, sys
json.dump({
    "argv": ["$python_bin", "$kernel_dir_src/csv_kernel.py", "-f", "{connection_file}"],
    "display_name": "$display",
    "language": "$name",
    "env": {
        "CSV_KERNEL_DELIMITER": "$delimiter",
        "CSV_KERNEL_LANGUAGE": "$name",
    },
}, open(sys.argv[1], "w"), indent=2)
EOF
    echo "installed $dir/kernel.json"
}

install_spec csv ','  "CSV Table"
install_spec tsv '	' "TSV Table"
install_spec ssv ';'  "SSV Table"
install_spec psv '|'  "PSV Table"
