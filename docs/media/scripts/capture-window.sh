#!/usr/bin/env bash
# Capture one live Hyprland window by class/title without touching any Alien
# hardware control. Run this script as root on the reference host with
# CAPTURE_USER set to the unprivileged desktop-session user.
set -euo pipefail

usage() {
  echo "usage: CAPTURE_USER=desktop-user $0 OUTPUT.png CLASS_REGEX [TITLE_REGEX]" >&2
  exit 2
}

[ "$#" -ge 2 ] && [ "$#" -le 3 ] || usage
output=$1
class_regex=$2
title_regex=${3:-.*}
capture_user=${CAPTURE_USER:-}
[ -n "$capture_user" ] && [ "$capture_user" != root ] || {
  echo "CAPTURE_USER must name the unprivileged desktop-session user" >&2
  exit 2
}

user_environment=$(systemctl --user --machine="${capture_user}@" show-environment)
xdg_runtime_dir=$(printf '%s\n' "$user_environment" | sed -n 's/^XDG_RUNTIME_DIR=//p')
hyprland_signature=$(printf '%s\n' "$user_environment" | sed -n 's/^HYPRLAND_INSTANCE_SIGNATURE=//p')
wayland_display=$(printf '%s\n' "$user_environment" | sed -n 's/^WAYLAND_DISPLAY=//p')
[ -n "$xdg_runtime_dir" ]
[ -n "$hyprland_signature" ]
[ -n "$wayland_display" ]

desktop() {
  runuser -u "$capture_user" -- env \
    XDG_RUNTIME_DIR="$xdg_runtime_dir" \
    HYPRLAND_INSTANCE_SIGNATURE="$hyprland_signature" \
    WAYLAND_DISPLAY="$wayland_display" \
    "$@"
}

matches=$(desktop hyprctl -j clients | jq \
  --arg class "$class_regex" --arg title "$title_regex" \
  '[.[] | select(.mapped == true and (.class | test($class; "i")) and (.title | test($title; "i")))]')
count=$(jq 'length' <<<"$matches")
[ "$count" -eq 1 ] || {
  echo "expected exactly one matching window, found $count" >&2
  jq -r '.[] | "class=\(.class) title=\(.title) address=\(.address)"' <<<"$matches" >&2
  exit 1
}

geometry=$(jq -r '.[0] | "\(.at[0]),\(.at[1]) \(.size[0])x\(.size[1])"' <<<"$matches")
mkdir -p "$(dirname "$output")"
desktop grim -l 9 -g "$geometry" "$output"
chmod 0644 "$output"
printf '%s\t%s\n' "$output" "$geometry"
