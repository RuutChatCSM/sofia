#!/bin/bash

# Set "chatgpt.cliExecutable": "/Users/<USERNAME>/code/sofia/scripts/debug-sofia.sh" in VSCode settings to always get the 
# latest sofia-rs binary when debugging Sofia Extension.


set -euo pipefail

SOFIA_RS_DIR=$(realpath "$(dirname "$0")/../sofia-rs")
(cd "$SOFIA_RS_DIR" && cargo run --quiet --bin sofia -- "$@")