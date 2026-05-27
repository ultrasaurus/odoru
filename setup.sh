#!/usr/bin/env bash
# setup.sh — creates a Python venv with misaki-en installed.
# Run once before building or running ko-odoru.
#
# Usage:
#   ./setup.sh                  # installs to ~/.misaki-g2p/venv
#   ./setup.sh /some/other/path # installs to a custom path
#
# After running, export the printed MISAKI_VENV line or add it to your shell
# profile (.zshrc / .bashrc), then build normally:
#
#   export MISAKI_VENV=~/.misaki-g2p/venv
#   PYO3_PYTHON=$(which python3) cargo build

set -euo pipefail

VENV_PATH="${1:-$HOME/.misaki-g2p/venv}"

# ── 1. Verify we have a Python 3.10+ arm64 interpreter ────────────────────────
PYTHON=$(which python3)

PY_VERSION=$("$PYTHON" -c "import sys; print(f'{sys.version_info.major}.{sys.version_info.minor}')")
PY_ARCH=$("$PYTHON" -c "import platform; print(platform.machine())")

echo "Found Python $PY_VERSION ($PY_ARCH) at $PYTHON"

if [[ "$PY_ARCH" != "arm64" ]]; then
  echo "⚠️  Warning: Python is not arm64. On an M1 Mac this may cause a crash."
  echo "   Install an arm64 Python via: brew install python3"
fi

# ── 2. Create the venv ────────────────────────────────────────────────────────
echo "Creating venv at $VENV_PATH …"
"$PYTHON" -m venv "$VENV_PATH"

# ── 3. Install misaki-en ──────────────────────────────────────────────────────
echo "Installing misaki-en …"
"$VENV_PATH/bin/pip" install --upgrade pip --quiet
"$VENV_PATH/bin/pip" install "misaki[en]" click

# ── 4. Verify the install ─────────────────────────────────────────────────────
echo "Verifying misaki import …"
"$VENV_PATH/bin/python" -c "from misaki.en import G2P; g2p = G2P(); print('misaki OK:', g2p('hello world')[0])"

# ── 5. Print the env var the user needs to set ────────────────────────────────
echo ""
echo "✅  Setup complete. Run this before building:"
echo ""
echo "   export MISAKI_VENV=$VENV_PATH"
echo "   export PYO3_PYTHON=$VENV_PATH/bin/python"
echo ""
echo "Then build with:"
echo "   cargo build"
