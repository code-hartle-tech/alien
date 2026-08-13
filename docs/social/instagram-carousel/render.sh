#!/usr/bin/env bash
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
OUT="$HERE/png"
mkdir -p "$OUT"
PROFILE=''
cleanup() {
  if [ -n "$PROFILE" ] && [ -d "$PROFILE" ]; then
    find "$PROFILE" -depth -delete
  fi
}
trap cleanup EXIT INT TERM

if [ -n "${CHROME_BIN:-}" ]; then
  CHROME=$CHROME_BIN
elif [ -x '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' ]; then
  CHROME='/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'
elif command -v google-chrome >/dev/null 2>&1; then
  CHROME=$(command -v google-chrome)
elif command -v chromium >/dev/null 2>&1; then
  CHROME=$(command -v chromium)
else
  echo 'Chrome or Chromium is required to render the carousel' >&2
  exit 1
fi

URL="file://$HERE/carousel.html"
for slide in $(seq 1 10); do
  printf -v number '%02d' "$slide"
  PROFILE=$(mktemp -d "${TMPDIR:-/tmp}/alien-carousel-chrome.XXXXXX")
  screenshot="$OUT/alien-predatorsense-$number.png"
  previous_mtime=0
  if [ -f "$screenshot" ]; then
    previous_mtime=$(stat -f '%m' "$screenshot" 2>/dev/null || stat -c '%Y' "$screenshot")
  fi
  "$CHROME" \
    --headless=new \
    --hide-scrollbars \
    --disable-gpu \
    --disable-extensions \
    --no-first-run \
    --user-data-dir="$PROFILE" \
    --allow-file-access-from-files \
    --run-all-compositor-stages-before-draw \
    --virtual-time-budget=1000 \
    --force-device-scale-factor=1 \
    --window-size=1080,1350 \
    --screenshot="$screenshot" \
    "$URL?slide=$slide" >/dev/null 2>&1 &
  chrome_pid=$!
  rendered=false
  for _ in $(seq 1 200); do
    if [ -s "$screenshot" ]; then
      current_mtime=$(stat -f '%m' "$screenshot" 2>/dev/null || stat -c '%Y' "$screenshot")
      if [ "$current_mtime" -gt "$previous_mtime" ]; then
        rendered=true
        break
      fi
    fi
    if ! kill -0 "$chrome_pid" 2>/dev/null; then
      wait "$chrome_pid"
      rendered=true
      break
    fi
    sleep 0.1
  done
  if [ "$rendered" != true ]; then
    echo "Chrome did not finish slide $slide within 20 seconds" >&2
    kill "$chrome_pid" 2>/dev/null || true
    wait "$chrome_pid" 2>/dev/null || true
    exit 1
  fi
  # Chrome 151 on macOS can leave its headless parent alive after atomically
  # writing a complete screenshot. Stop only this isolated per-slide process.
  if kill -0 "$chrome_pid" 2>/dev/null; then
    kill "$chrome_pid" 2>/dev/null || true
  fi
  wait "$chrome_pid" 2>/dev/null || true
  find "$PROFILE" -depth -delete
  PROFILE=''
done

count=$(find "$OUT" -maxdepth 1 -type f -name 'alien-predatorsense-??.png' | wc -l | tr -d ' ')
if [ "$count" -ne 10 ]; then
  echo "Expected 10 rendered slides, found $count" >&2
  exit 1
fi

if command -v sips >/dev/null 2>&1; then
  for slide in $(seq 1 10); do
    printf -v number '%02d' "$slide"
    file="$OUT/alien-predatorsense-$number.png"
    width=$(sips -g pixelWidth "$file" | awk '/pixelWidth/ {print $2}')
    height=$(sips -g pixelHeight "$file" | awk '/pixelHeight/ {print $2}')
    if [ "$width" != 1080 ] || [ "$height" != 1350 ]; then
      echo "$file is ${width}x${height}; expected 1080x1350" >&2
      exit 1
    fi
  done
fi

echo "Rendered 10 Instagram-ready 1080x1350 PNGs in $OUT"
