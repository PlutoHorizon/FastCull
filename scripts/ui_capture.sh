#!/usr/bin/env bash
# Deterministic UI screenshot capture for FastCull.
#
# Usage: scripts/ui_capture.sh <out-dir> <WxH> <fixture-dir> [binary] [default|features]
#
# Produces, under <out-dir>:
#   main/00..04.png    main picker window: fit loupe / 100% zoom / grid / last photo / labelled
#                      (in "features" mode: main/00..12.png, which also covers comparison
#                      and the exposure before/after pair the pixel regression needs)
#   raw/00-loaded.png .. 03-saved.png   RAW editor window (only when an ARW/NEF/DNG fixture exists)
#   main/result.txt, raw/result.txt     smoke receipts
#   logs/                              application logs
#
# The fixture directory is copied into <out-dir>/photos before every run, because the
# smoke controller writes ratings, colour labels, flags and orientation back into the
# files it opens. Copying keeps every capture identical.
#
# Set FASTCULL_UI_BIN to override the binary (defaults to target/release/fastcull).
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${1:?usage: ui_capture.sh <out-dir> <WxH> <fixture-dir> [binary] [default|features]}"
SIZE="${2:-1280x850}"
FIXTURES="${3:?fixture dir}"
BIN="${4:-${FASTCULL_UI_BIN:-$ROOT/target/release/fastcull}}"
MODE="${5:-default}"

if [ ! -x "$BIN" ]; then
  echo "ui_capture: binary is not executable: $BIN" >&2
  exit 2
fi

# Prepare the fixture set when it is missing. The file names control the sort
# order, and the smoke controller navigates to the LAST photo before the
# exposure step, so that one has to carry real mid-tones: the pixel regression
# in scripts/verify_feature_images.py samples its canvas and needs more than a
# hundred in-range pixels to compare.
if [ ! -d "$FIXTURES" ]; then
  mkdir -p "$FIXTURES"
  cp "$ROOT/tests/fixtures/preview-3x2.jpg" "$FIXTURES/b-green.jpg" 2>/dev/null || true
  for photo in "$ROOT"/.agent-temp/*/big.jpg "$ROOT"/.agent-temp/*/small.jpg; do
    [ -f "$photo" ] && cp "$photo" "$FIXTURES/a-balloons.jpg" && break
  done
  for arw in "$ROOT"/.agent-temp/*/sample.arw; do
    [ -f "$arw" ] && cp "$arw" "$FIXTURES/c-tree.arw" && break
  done
  for dng in "$ROOT"/.agent-temp/*/sample.dng; do
    [ -f "$dng" ] && cp "$dng" "$FIXTURES/z-temple.dng" && break
  done
  [ -f "$FIXTURES/z-temple.dng" ] || \
    cp "$ROOT/tests/fixtures/asym-5120x2048.jpg" "$FIXTURES/z-asym.jpg"
  # The RAW editor window needs the native bridge library next to the binary.
  for lib in "$ROOT"/.agent-temp/*/libfastcull_raw.dylib; do
    if [ -f "$lib" ]; then
      mkdir -p "$ROOT/target/release" "$ROOT/target/debug"
      cp "$lib" "$ROOT/target/release/" 2>/dev/null || true
      cp "$lib" "$ROOT/target/debug/" 2>/dev/null || true
      break
    fi
  done
fi

if [ ! -d "$FIXTURES" ]; then
  echo "ui_capture: fixture directory not found: $FIXTURES" >&2
  exit 2
fi

hex() {
  printf '%s' "$1" | od -An -tx1 | tr -d ' \n'
}

rm -rf "$OUT"
mkdir -p "$OUT/main" "$OUT/raw" "$OUT/logs" "$OUT/photos"
cp "$FIXTURES"/* "$OUT/photos/" 2>/dev/null || true
if [ -z "$(ls -A "$OUT/photos" 2>/dev/null)" ]; then
  echo "ui_capture: no fixtures copied from $FIXTURES" >&2
  exit 2
fi

# ---- main picker window ---------------------------------------------------
if [ "$MODE" = "features" ]; then
  export FASTCULL_FEATURE_SMOKE=1
else
  unset FASTCULL_FEATURE_SMOKE
fi

FASTCULL_SMOKE_DIR="$OUT/main" \
FASTCULL_SMOKE_SIZE="$SIZE" \
FASTCULL_LOG_DIR="$OUT/logs" \
"$BIN" "$OUT/photos" >"$OUT/main-run.log" 2>&1 &
MAIN_PID=$!

for _ in $(seq 1 240); do
  [ -f "$OUT/main/result.txt" ] && break
  sleep 1
done
kill "$MAIN_PID" 2>/dev/null || true
wait "$MAIN_PID" 2>/dev/null || true

if [ ! -f "$OUT/main/result.txt" ]; then
  echo "ui_capture: the main window smoke run did not finish" >&2
  tail -20 "$OUT/logs"/*.log 2>/dev/null >&2
  exit 3
fi

# ---- RAW editor window ----------------------------------------------------
RAW_SRC=""
for cand in "$OUT/photos"/*.ARW "$OUT/photos"/*.arw "$OUT/photos"/*.NEF "$OUT/photos"/*.nef \
            "$OUT/photos"/*.DNG "$OUT/photos"/*.dng; do
  if [ -f "$cand" ]; then RAW_SRC="$cand"; break; fi
done

if [ -n "$RAW_SRC" ]; then
  # The request file and the receipt must live in the same directory.
  RECEIPT="$OUT/raw-receipt.json"
  printf 'FASTCULL_RAW_WINDOW_V1\n%s\n%s\n-\n-\n' "$(hex "$RAW_SRC")" "$(hex "$RECEIPT")" \
    > "$OUT/raw-request.txt"
  FASTCULL_RAW_EDIT_SMOKE_DIR="$OUT/raw" \
  FASTCULL_LOG_DIR="$OUT/logs" \
  "$BIN" --raw-editor "$OUT/raw-request.txt" >"$OUT/raw-run.log" 2>&1 &
  RAW_PID=$!
  for _ in $(seq 1 180); do
    [ -f "$OUT/raw/result.txt" ] && break
    [ -f "$OUT/raw/error.txt" ] && break
    sleep 1
  done
  kill "$RAW_PID" 2>/dev/null || true
  wait "$RAW_PID" 2>/dev/null || true
fi

MAIN_PNG=$(find "$OUT/main" -name '*.png' 2>/dev/null | wc -l | tr -d ' ')
RAW_PNG=$(find "$OUT/raw" -name '*.png' 2>/dev/null | wc -l | tr -d ' ')
echo "main_png=$MAIN_PNG raw_png=$RAW_PNG out=$OUT"

if [ "$MAIN_PNG" -lt 5 ]; then
  echo "ui_capture: expected at least 5 main window screenshots, got $MAIN_PNG" >&2
  exit 4
fi
