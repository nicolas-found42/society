#!/usr/bin/env bash
# Record a demo clip of the observer window and encode it for the README.
#
#   tools/gif.sh village       -> docs/media/village.gif
#   tools/gif.sh feed          -> docs/media/feed.gif
#   tools/gif.sh speed         -> docs/media/speed.gif
#   tools/gif.sh inspector     -> docs/media/inspector.png (a still: a pause has
#                                 nothing to animate, and a PNG is not 2 MB)
#   tools/gif.sh intervene     -> docs/media/intervene.gif (a whisper and a gift
#                                 arriving in the feed while it records)
#   tools/gif.sh --check       -> verify the committed clips: present, 960 wide,
#                                 enough frames to read as motion, under budget
#
# Each scene launches its own village on a scratch save under `.scratch/`, so a
# recording never touches `data/society.json` and never depends on where your
# own village happens to be. The window is captured by id through
# `tools/shot-bin` (ScreenCaptureKit), so nothing is raised, clicked or focused
# while recording: you can keep working.
#
# Requirements: macOS with Screen Recording permission for the terminal, the
# Rust toolchain (`cargo build --release -p society`), `swiftc` for the helper,
# and `ffmpeg` for the encoding step.
#
# The scenes are sized for a README: 960x640 logical points captured at 2x and
# scaled back to 960 px, 8 fps, 4 s. The window is flat colour with thin glyphs,
# so the palette is small and undithered: dithering the text is what makes a UI
# clip megabytes for no visible gain.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
cd "$ROOT"

SCENE="${1:-village}"

# `--check` is the gate for a docs change: it never launches the app, so it can
# run in a pre-merge pass on a machine with no window server.
if [ "$SCENE" = "--check" ]; then
  if ! command -v ffprobe >/dev/null; then
    echo "ffprobe is needed to inspect the clips (brew install ffmpeg)" >&2
    exit 2
  fi
  status=0
  for asset in "$ROOT"/docs/media/*; do
    name="$(basename "$asset")"
    bytes=$(wc -c <"$asset" | tr -d ' ')
    width=$(ffprobe -v error -select_streams v:0 -show_entries stream=width -of csv=p=0 "$asset")
    frames=$(ffprobe -v error -select_streams v:0 -count_frames -show_entries stream=nb_read_frames -of csv=p=0 "$asset")
    if [ "$bytes" -gt 4000000 ]; then
      echo "$name: $bytes bytes is over the 4 MB README budget"
      status=1
    fi
    if [ "$width" != "960" ]; then
      echo "$name: ${width}px wide, the README clips are 960"
      status=1
    fi
    if [ "$name" != "inspector.png" ] && [ "${frames:-0}" -lt 24 ]; then
      echo "$name: ${frames:-0} frames is too few to read as motion"
      status=1
    fi
    echo "$name: ${width}px, ${frames:-1} frame(s), ${bytes} bytes"
  done
  exit "$status"
fi
WIDTH=960
HEIGHT=640
FPS=8
SECONDS=4
PORT="${PORT:-8199}"
MEDIA="$ROOT/docs/media"
WORK="$ROOT/.scratch/gif-$SCENE"
SAVE="$WORK/society.json"
APP="$ROOT/target/release/society"
SHOT="$HERE/shot-bin"

mkdir -p "$MEDIA"

if [ ! -x "$APP" ]; then
  echo "building the app..." >&2
  cargo build --release -p society
fi
if [ ! -x "$SHOT" ]; then
  echo "building the capture helper..." >&2
  swiftc -O "$HERE/shot.swift" -o "$SHOT"
fi
if ! command -v ffmpeg >/dev/null; then
  echo "ffmpeg is needed to encode the clip (brew install ffmpeg)" >&2
  exit 2
fi

case "$SCENE" in
  village)
    # Two turns a second: the ring moves, the feed fills, and the clip is short
    # enough that a GIF stays small.
    APP_ARGS=(--view world --warmup 40 --speed 2)
    FRAMES=$((FPS * SECONDS))
    OUT="$MEDIA/village.gif"
    ;;
  feed)
    # The feed needs history behind it, then plays fast enough to watch lines
    # arrive and the digest change.
    APP_ARGS=(--view feed --warmup 60 --speed 10)
    FRAMES=$((FPS * SECONDS))
    OUT="$MEDIA/feed.gif"
    ;;
  speed)
    # The maximum multiplier, for the "it can fast-forward" claim.
    APP_ARGS=(--view world --warmup 60 --speed 100)
    FRAMES=$((FPS * 4))
    OUT="$MEDIA/speed.gif"
    ;;
  intervene)
    # A whisper and a gift land in the feed while the clip records: the second
    # headline claim is that you are an observer who can be felt, not a god who
    # edits minds. The app is paused, so the only thing that changes is the
    # intervention arriving.
    APP_ARGS=(--view feed --warmup 40 --paused)
    FRAMES=$((FPS * 3))
    OUT="$MEDIA/intervene.gif"
    ;;
  inspector)
    # A still: the factors disclosure is a click a screenshot cannot make, so
    # the app opens it for us.
    APP_ARGS=(--view inspector --select Ada --expand-factors --paused --warmup 40)
    FRAMES=1
    OUT="$MEDIA/inspector.png"
    ;;
  *)
    echo "unknown scene '$SCENE' (village, feed, speed, inspector)" >&2
    exit 2
    ;;
esac

rm -rf "$WORK"
mkdir -p "$WORK/frames"
rm -f "$OUT"

SOCIETY_SAVE_PATH="$SAVE" "$APP" "${APP_ARGS[@]}" --size "${WIDTH}x${HEIGHT}" \
  --port "$PORT" >"$WORK/app.log" 2>&1 &
APP_PID=$!
cleanup() {
  kill "$APP_PID" 2>/dev/null || true
  wait "$APP_PID" 2>/dev/null || true
}
trap cleanup EXIT

# Wait for the window to exist: the helper fails fast when it cannot find one.
for _ in $(seq 1 100); do
  if "$SHOT" "$WORK/frames/probe.png" 0 >/dev/null 2>&1; then
    break
  fi
  sleep 0.2
done
if [ ! -f "$WORK/frames/probe.png" ]; then
  echo "no society window appeared; see $WORK/app.log" >&2
  exit 1
fi
rm -f "$WORK/frames/probe.png"

# The intervention scene needs something to intervene with, timed to land while
# the frames are being taken.
if [ "$SCENE" = intervene ]; then
  (
    sleep 1.2
    curl -s -X POST "localhost:$PORT/inject" -H 'content-type: application/json' \
      -d '{"kind":"whisper","target":"Ada","text":"the granary door was open last night"}' >/dev/null
    curl -s -X POST "localhost:$PORT/inject" -H 'content-type: application/json' \
      -d '{"kind":"gift","target":"Odo","amount":3}' >/dev/null
  ) &
fi

# Frames are captured from one helper process so the interval is the interval.
# A tiny delay lets the app paint its first frame after the window appears.
"$SHOT" "$WORK/frames/frame.png" 0.5 "$FRAMES" $((1000 / FPS))

if [ "$SCENE" = inspector ]; then
  cp "$WORK/frames/frame-0000.png" "$OUT"
else
  ffmpeg -loglevel error -y -framerate "$FPS" -i "$WORK/frames/frame-%04d.png" \
    -vf "scale=${WIDTH}:-1:flags=lanczos,split[palette][frames];[palette]palettegen=max_colors=48:stats_mode=diff[pal];[frames][pal]paletteuse=dither=none:diff_mode=rectangle" \
    -loop 0 "$OUT"
fi

BYTES=$(wc -c <"$OUT" | tr -d ' ')
echo "$OUT  ${BYTES} bytes"
if [ "$BYTES" -gt 4000000 ]; then
  echo "warning: ${BYTES} bytes is larger than a README clip should be" >&2
fi
