#!/usr/bin/env bash
# Build browser/social derivatives from locally retained high-quality masters.
# Masters are intentionally not required to live in Git.
set -euo pipefail

usage() {
  echo "usage: $0 GUI_MASTER TUI_MASTER [GIF_START_SECONDS] [GIF_DURATION_SECONDS]" >&2
  exit 2
}

[ "$#" -ge 2 ] && [ "$#" -le 4 ] || usage
gui_master=$1
tui_master=$2
gif_start=${3:-0}
gif_duration=${4:-10}
root=$(cd "$(dirname "$0")/.." && pwd)
video_dir="$root/videos"
demo_dir="$root/demo"
mkdir -p "$video_dir" "$demo_dir"

scale="scale='min(2560,iw)':-2:flags=lanczos"

encode_pair() {
  input=$1
  stem=$2
  ffmpeg -hide_banner -y -i "$input" -an -vf "$scale" \
    -c:v libx264 -preset slow -crf 18 -profile:v high -pix_fmt yuv420p \
    -movflags +faststart "$video_dir/$stem.mp4"
  ffmpeg -hide_banner -y -i "$input" -an -vf "$scale" \
    -c:v libvpx-vp9 -row-mt 1 -crf 30 -b:v 0 -pix_fmt yuv420p \
    "$video_dir/$stem.webm"
}

encode_pair "$gui_master" alien-gui-walkthrough
encode_pair "$tui_master" alien-tui-walkthrough

palette=$(mktemp "${TMPDIR:-/tmp}/alien-palette.XXXXXX.png")
trap 'rm -f "$palette"' EXIT
gif_filter="fps=15,scale=1280:-2:flags=lanczos"
ffmpeg -hide_banner -y -ss "$gif_start" -t "$gif_duration" -i "$gui_master" \
  -vf "$gif_filter,palettegen=max_colors=192:stats_mode=diff" "$palette"
ffmpeg -hide_banner -y -ss "$gif_start" -t "$gif_duration" -i "$gui_master" \
  -i "$palette" -lavfi "$gif_filter [x]; [x][1:v] paletteuse=dither=sierra2_4a:diff_mode=rectangle" \
  "$demo_dir/alien-demo.gif"
