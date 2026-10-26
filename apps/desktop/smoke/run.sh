#!/usr/bin/env bash
# Launch the real desktop app under a virtual display and drive it with smoke/smoke.py.
# Needs: tauri-driver and WebKitWebDriver (Linux) on PATH, xvfb-run, a built binary with the
# frontend embedded (cargo build -p analyst-desktop --features custom-protocol after pnpm build),
# and a repo with a venv (e.g. created by `analyst init`).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
APP="${APP:-$ROOT/target/debug/analyst-desktop}"
REPO="${1:?usage: run.sh <repo path> [out dir]}"
OUT="${2:-$HERE/out}"
export ANALYST_HOME="${ANALYST_HOME:-$OUT/home}"
export ANALYST_TOOLING_PYTHON="${ANALYST_TOOLING_PYTHON:-$ROOT/python/kernel/.venv/bin/python}"
export WEBKIT_DISABLE_COMPOSITING_MODE=1
export WEBKIT_DISABLE_DMABUF_RENDERER=1
mkdir -p "$OUT" "$ANALYST_HOME"
xvfb-run -a -s "-screen 0 1600x1000x24" bash -c '
  tauri-driver --port 4444 > "'"$OUT"'/tauri-driver.log" 2>&1 &
  DRV=$!
  sleep 1.5
  python3 "'"$HERE"'/smoke.py" --app "'"$APP"'" --repo "'"$REPO"'" --out "'"$OUT"'"
  RC=$?
  kill $DRV 2>/dev/null || true
  exit $RC
'
