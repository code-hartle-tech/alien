#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)

screenshots=(
  alien-gui-splash-contacting.png
  alien-gui-dashboard.png
  alien-gui-fans.png
  alien-gui-lighting.png
  alien-gui-performance-initial.png
  alien-gui-performance.png
  alien-gui-about.png
  alien-gui-model-catalog.png
  alien-gui-first-run.png
  alien-gui-link-lost.png
  alien-gui-performance-narrow.png
  alien-tui-rich.png
  alien-tui-tight.png
  alien-tui-confirmation.png
)
videos=(
  alien-gui-walkthrough.mp4
  alien-gui-walkthrough.webm
  alien-tui-walkthrough.mp4
  alien-tui-walkthrough.webm
)

command -v ffprobe >/dev/null || {
  echo "ffprobe is required" >&2
  exit 1
}

image_dimensions() {
  if command -v magick >/dev/null; then
    magick identify -format '%w\t%h' "$1"
  elif command -v identify >/dev/null; then
    identify -format '%w\t%h' "$1"
  elif command -v sips >/dev/null; then
    width=$(sips -g pixelWidth "$1" | awk '/pixelWidth/{print $2}')
    height=$(sips -g pixelHeight "$1" | awk '/pixelHeight/{print $2}')
    printf '%s\t%s\n' "$width" "$height"
  else
    echo "magick, identify, or sips is required" >&2
    exit 1
  fi
}

sha256_file() {
  if command -v sha256sum >/dev/null; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

printf 'kind\tfile\twidth\theight\tduration_seconds\tframe_rate\tbytes\tsha256\n'
for name in "${screenshots[@]}"; do
  file="$root/screenshots/$name"
  [ -s "$file" ] || { echo "missing: $file" >&2; exit 1; }
  IFS=$'\t' read -r width height < <(image_dimensions "$file")
  case "$name" in
    alien-gui-performance-narrow.png) min_width=960; min_height=720 ;;
    alien-tui-rich.png|alien-tui-confirmation.png) min_width=1200; min_height=800 ;;
    alien-tui-tight.png) min_width=900; min_height=650 ;;
    *) min_width=1800; min_height=1000 ;;
  esac
  if [ "$width" -lt "$min_width" ] || [ "$height" -lt "$min_height" ]; then
    echo "undersized: $file is ${width}x${height}; need at least ${min_width}x${min_height}" >&2
    exit 1
  fi
  printf 'png\t%s\t%s\t%s\t-\t-\t%s\t%s\n' \
    "${file#"$root/"}" "$width" "$height" "$(wc -c <"$file" | tr -d ' ')" "$(sha256_file "$file")"
done

for name in "${videos[@]}"; do
  file="$root/videos/$name"
  [ -s "$file" ] || { echo "missing: $file" >&2; exit 1; }
  bytes=$(wc -c <"$file" | tr -d ' ')
  if [ "$bytes" -ge 104857600 ]; then
    echo "oversized: $file is $bytes bytes; tracked videos must remain below 100MiB" >&2
    exit 1
  fi
  width=$(ffprobe -v error -select_streams v:0 -show_entries stream=width -of csv=p=0 "$file")
  height=$(ffprobe -v error -select_streams v:0 -show_entries stream=height -of csv=p=0 "$file")
  duration=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$file")
  frame_rate=$(ffprobe -v error -select_streams v:0 -show_entries stream=avg_frame_rate -of csv=p=0 "$file")
  printf 'video\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "${file#"$root/"}" "$width" "$height" "$duration" "$frame_rate" \
    "$bytes" "$(sha256_file "$file")"
done

gif="$root/demo/alien-demo.gif"
[ -s "$gif" ] || { echo "missing: $gif" >&2; exit 1; }
gif_bytes=$(wc -c <"$gif" | tr -d ' ')
if [ "$gif_bytes" -ge 15728640 ]; then
  echo "oversized: $gif is $gif_bytes bytes; README GIF must remain below 15MiB" >&2
  exit 1
fi
IFS=$'\t' read -r width height < <(image_dimensions "$gif")
duration=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$gif")
printf 'gif\t%s\t%s\t%s\t%s\t-\t%s\t%s\n' \
  "${gif#"$root/"}" "$width" "$height" "$duration" \
  "$gif_bytes" "$(sha256_file "$gif")"
