#!/usr/bin/env bash
# run.sh — sources .env and runs a cargo example.
# Usage:
#   echo "Hello world." | ./run.sh
#   echo "Hello world." | ./run.sh --release

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ ! -f "$SCRIPT_DIR/.env" ]]; then
  echo "❌  .env not found. Copy .env.example to .env and fill in the paths."
  exit 1
fi

# Source into this subprocess — exports are inherited by cargo and the binary.
source "$SCRIPT_DIR/.env"

# Verify the key vars got set.
if [[ -z "${MISAKI_VENV:-}" ]]; then
  echo "❌  MISAKI_VENV is not set after sourcing .env"
  exit 1
fi

exec cargo run ${1:+--$1} --example basic
