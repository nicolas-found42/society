#!/usr/bin/env bash
# Screenshot the society window without touching focus, the pointer, or z-order.
#
#   tools/shot.sh                 -> /tmp/society.png
#   tools/shot.sh out.png         -> out.png
#   tools/shot.sh out.png 5       -> wait 5s first (let a turn finish)
#
# macOS captures windows per-window (ScreenCaptureKit), so the shot is correct
# even while the window is fully covered by other apps. Nothing is raised and
# nothing is clicked, which is what makes it safe to observe while you work.
#
# Requires Screen Recording permission for the terminal that runs it.

set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="$HERE/shot-bin"

if [ ! -x "$BIN" ]; then
  echo "building capture helper..." >&2
  swiftc -O "$HERE/shot.swift" -o "$BIN"
fi

exec "$BIN" "${1:-/tmp/society.png}" "${2:-0}"
