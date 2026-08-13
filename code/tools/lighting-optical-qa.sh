#!/usr/bin/env bash
# Drive the complete PH315-53 Covini lighting matrix while an external camera
# records the physical keyboard. This is deliberately not a unit test: PECM
# readback cannot prove emitted light.
set -euo pipefail

usage() {
  cat <<'EOF'
usage: lighting-optical-qa.sh \
  --i-understand-this-will-change-lighting-and-my-camera-is-recording \
  --i-confirm-the-physical-keyboard-is-in-frame \
  --camera-id RECORDING_ID --camera-fps FPS --camera-settings DESCRIPTION \
  [--dwell SECONDS]

Or: lighting-optical-qa.sh --preflight-only

The full run emits 293 timestamped cases and restores the exact exposed
pre-state. It never opens a camera: an independent camera must continuously
record the physical keyboard with fixed focus, exposure and white balance.
--preflight-only validates the exact host/daemon/getter route without changing
lighting.
EOF
}

mutation_acknowledged=0
frame_acknowledged=0
preflight_only=0
camera_id=
camera_fps=
camera_settings=
dwell=${ALIEN_OPTICAL_DWELL:-3}

while (($# > 0)); do
  case $1 in
    --i-understand-this-will-change-lighting-and-my-camera-is-recording)
      mutation_acknowledged=1
      shift
      ;;
    --i-confirm-the-physical-keyboard-is-in-frame)
      frame_acknowledged=1
      shift
      ;;
    --preflight-only)
      preflight_only=1
      shift
      ;;
    --camera-id)
      (($# >= 2)) || { echo "--camera-id requires a value" >&2; exit 2; }
      camera_id=$2
      shift 2
      ;;
    --camera-fps)
      (($# >= 2)) || { echo "--camera-fps requires a value" >&2; exit 2; }
      camera_fps=$2
      shift 2
      ;;
    --camera-settings)
      (($# >= 2)) || { echo "--camera-settings requires a value" >&2; exit 2; }
      camera_settings=$2
      shift 2
      ;;
    --dwell)
      (($# >= 2)) || { echo "--dwell requires a value" >&2; exit 2; }
      dwell=$2
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if (( preflight_only == 0 && (mutation_acknowledged == 0 || frame_acknowledged == 0) )); then
  echo "both explicit hardware/camera acknowledgements are required" >&2
  usage >&2
  exit 2
fi
if (( preflight_only == 0 )) &&
   [[ -z $camera_id || -z $camera_fps || -z $camera_settings ]]; then
  echo "--camera-id, --camera-fps and --camera-settings are required for an optical run" >&2
  exit 2
fi
if [[ $camera_id == *[$'\t\r\n']* || $camera_settings == *[$'\t\r\n']* ]]; then
  echo "camera metadata must be one line without tab characters" >&2
  exit 2
fi
if (( preflight_only == 0 )) &&
   { [[ ! $camera_fps =~ ^[0-9]+$ ]] || (( camera_fps < 60 || camera_fps > 1000 )); }; then
  echo "--camera-fps must be a whole number between 60 and 1000" >&2
  exit 2
fi

alien_bin=${ALIEN_BIN:-alien}
case $dwell in
  ''|*[!0-9]*)
    echo "ALIEN_OPTICAL_DWELL must be a whole number of seconds" >&2
    exit 2
    ;;
esac
if (( dwell < 2 )); then
  echo "ALIEN_OPTICAL_DWELL must be at least 2 seconds for camera evidence" >&2
  exit 2
fi
command -v "$alien_bin" >/dev/null 2>&1 || {
  echo "Alien CLI not found: $alien_bin" >&2
  exit 1
}

required_socket=/run/alien/alien.sock
if [[ ${ALIEN_SOCKET+x} && $ALIEN_SOCKET != "$required_socket" ]]; then
  echo "refusing non-production ALIEN_SOCKET=$ALIEN_SOCKET; expected $required_socket" >&2
  exit 1
fi
export ALIEN_SOCKET=$required_socket
if [[ ${ALIEN_INTERFACE_LOCK+x} ]]; then
  echo "refusing inherited ALIEN_INTERFACE_LOCK" >&2
  exit 1
fi
unset ALIEN_INTERFACE_LOCK
export ALIEN_REQUIRE_SOCKET=1
if (( EUID == 0 )); then
  echo "run optical QA as the unprivileged desktop user, never root" >&2
  exit 1
fi
if [[ -r /proc/acpi/call || -w /proc/acpi/call ]]; then
  echo "desktop user unexpectedly has direct /proc/acpi/call access; refusing ambiguous transport" >&2
  exit 1
fi
if [[ ! -S $required_socket ]]; then
  echo "production Alien daemon socket is unavailable: $required_socket" >&2
  exit 1
fi
command -v flock >/dev/null 2>&1 || {
  echo "flock is required to exclude a second optical runner" >&2
  exit 1
}
command -v pgrep >/dev/null 2>&1 || {
  echo "pgrep is required to exclude active Alien frontends" >&2
  exit 1
}
runtime_dir=${XDG_RUNTIME_DIR:-/run/user/$EUID}
if [[ ! -d $runtime_dir || ! -O $runtime_dir ]]; then
  echo "trusted per-user runtime directory is unavailable: $runtime_dir" >&2
  exit 1
fi
qa_lock=$runtime_dir/alien-lighting-optical.lock
umask 077
exec 9>"$qa_lock"
if ! flock -n 9; then
  echo "another lighting optical runner holds $qa_lock" >&2
  exit 1
fi

read_dmi() {
  local path=/sys/class/dmi/id/$1
  [[ -r $path ]] || return 1
  tr -d '\r\n' <"$path"
}
dmi_vendor=$(read_dmi sys_vendor)
dmi_product=$(read_dmi product_name)
dmi_board_vendor=$(read_dmi board_vendor)
dmi_board=$(read_dmi board_name)
dmi_bios=$(read_dmi bios_version)
if [[ $dmi_vendor != Acer || $dmi_product != 'Predator PH315-53' ||
      $dmi_board_vendor != CML || $dmi_board != QX50_CMS || $dmi_bios != V1.07 ]]; then
  printf 'exact-target guard failed: vendor=%q product=%q board_vendor=%q board=%q bios=%q\n' \
    "$dmi_vendor" "$dmi_product" "$dmi_board_vendor" "$dmi_board" "$dmi_bios" >&2
  exit 1
fi

alien_cli_path=$(command -v "$alien_bin")
alien_cli_realpath=$(readlink -f -- "$alien_cli_path" 2>/dev/null || printf '%s' "$alien_cli_path")
alien_version=$("$alien_bin" --version)

# Capture every state the typed CLI can observe before redirecting its
# per-user lighting memory. The static enable mask has no firmware getter, so
# the saved mask is the best-known value and is labelled as such throughout.
initial_status=$("$alien_bin" rgb status)
initial_effect=$(awk '/^effect[[:space:]]/{print $2; exit}' <<<"$initial_status")
initial_speed=$(awk '/^speed[[:space:]]/{print $2; exit}' <<<"$initial_status")
initial_brightness=$(awk '/^brightness[[:space:]]/{print $2; exit}' <<<"$initial_status")
initial_colour=$(awk '/^colour[[:space:]]/{print $2; exit}' <<<"$initial_status")
initial_direction=$(awk '/^direction[[:space:]]/{print $2; exit}' <<<"$initial_status")
read -r -a initial_zones <<<"$(awk '/^zones[[:space:]]/{print $2, $3, $4, $5; exit}' <<<"$initial_status")"
read -r -a initial_mask <<<"$(awk '/^saved mask[[:space:]]/{print $3, $4, $5, $6; exit}' <<<"$initial_status")"

case $initial_effect in
  static|breath|neon|wave|shifting|zoom) ;;
  *)
    echo "cannot identify the initial backlight effect from alien rgb status" >&2
    exit 1
    ;;
esac
[[ $initial_speed =~ ^[0-9]+$ ]] || {
  echo "cannot identify initial speed" >&2
  exit 1
}
[[ $initial_brightness =~ ^(0|25|50|75|100)$ ]] || {
  echo "initial brightness is not an exact Covini step: $initial_brightness" >&2
  exit 1
}
(( ${#initial_mask[@]} == 4 )) || {
  echo "cannot capture the four-state saved mask" >&2
  exit 1
}
for state in "${initial_mask[@]}"; do
  [[ $state == on || $state == off ]] || {
    echo "cannot capture initial saved mask state: $state" >&2
    exit 1
  }
done
if [[ $initial_effect == static ]]; then
  (( ${#initial_zones[@]} == 4 )) || {
    echo "cannot capture four initial static zones" >&2
    exit 1
  }
  for colour in "${initial_zones[@]}"; do
    [[ $colour =~ ^#[0-9a-fA-F]{6}$ ]] || {
      echo "cannot capture initial static colour: $colour" >&2
      exit 1
    }
  done
elif [[ $initial_effect == breath || $initial_effect == zoom || $initial_effect == shifting ]]; then
  [[ $initial_colour =~ ^#[0-9a-fA-F]{6}$ ]] || {
    echo "cannot capture initial dynamic colour" >&2
    exit 1
  }
fi
if [[ $initial_effect == static ]]; then
  (( initial_speed == 0 )) || {
    echo "initial Static getter returned non-zero speed $initial_speed" >&2
    exit 1
  }
elif (( initial_speed < 1 || initial_speed > 9 )); then
  echo "initial animated getter speed is outside 1..9: $initial_speed" >&2
  exit 1
fi
case $initial_direction in
  '') initial_direction_arg= ;;
  left-to-right) initial_direction_arg=ltr ;;
  right-to-left) initial_direction_arg=rtl ;;
  *)
    echo "cannot capture initial direction: $initial_direction" >&2
    exit 1
    ;;
esac

printf 'PREFLIGHT alien=%s cli=%s socket=%s euid=%s host=%s target=%s/%s/%s+%s bios=%s\nINITIAL:\n%s\n' \
  "$alien_version" "$alien_cli_realpath" "$required_socket" "$EUID" "$(hostname)" \
  "$dmi_vendor" "$dmi_product" "$dmi_board_vendor" "$dmi_board" "$dmi_bios" "$initial_status"
if (( preflight_only == 1 )); then
  echo "PREFLIGHT_COMPLETE no_mutation_sent=1 optics_not_performed=1"
  exit 0
fi

if active_frontends=$(pgrep -af '(^|/)(\.alien-gui-wrapped|alien-gui|alien-tui)([[:space:]]|$)'); then
  echo "another Alien GUI/TUI is running; no mutation was sent" >&2
  printf '%s\n' "$active_frontends" >&2
  exit 1
else
  pgrep_rc=$?
  if (( pgrep_rc != 1 )); then
    echo "cannot prove that Alien GUI/TUI frontends are absent; no mutation was sent" >&2
    exit 1
  fi
fi

qa_tmp=$(mktemp -d "${TMPDIR:-/tmp}/alien-lighting-optical.XXXXXX")
original_lighting_was_set=${ALIEN_LIGHTING+x}
original_lighting=${ALIEN_LIGHTING-}
export ALIEN_LIGHTING="$qa_tmp/lighting.toml"
restored=0
matrix_complete=0
case_count=0
expected_cases=293

effect() {
  local name=$1 speed=$2 brightness=$3 direction=${4:-} colour=${5:-}
  local args=(rgb effect "$name" "$speed" "$brightness")
  [[ -z $colour ]] || args+=("$colour")
  [[ -z $direction ]] || args+=("$direction")
  "$alien_bin" "${args[@]}"
}

apply_mask() {
  local mask=$1 zone bit state
  for zone in 1 2 3 4; do
    bit=$((1 << (zone - 1)))
    state=off
    (( mask & bit )) && state=on
    "$alien_bin" rgb zone "$zone" "$state" >/dev/null
  done
}

apply_mask_words() {
  local zone state failed=0
  for zone in 1 2 3 4; do
    state=${initial_mask[zone - 1]}
    "$alien_bin" rgb zone "$zone" "$state" >/dev/null || failed=1
  done
  return "$failed"
}

restore_initial() {
  local original_rc=$? final_rc restore_rc=0 cleanup_rc=0 restored_status
  trap - EXIT
  # A second SSH HUP or interrupt must not kill the best-effort restore half
  # way through. Signals are ignored only for this bounded cleanup section.
  trap '' HUP INT TERM PIPE
  set +e
  if (( restored == 0 )); then
    restored=1
    if [[ $initial_effect == static ]]; then
      "$alien_bin" rgb zones "${initial_zones[@]}" >>"$qa_tmp/restore.log" 2>&1 || restore_rc=1
      apply_mask_words || restore_rc=1
      effect static 0 "$initial_brightness" >>"$qa_tmp/restore.log" 2>&1 || restore_rc=1
    else
      apply_mask_words || restore_rc=1
      effect "$initial_effect" "$initial_speed" "$initial_brightness" \
        "$initial_direction_arg" "$initial_colour" >>"$qa_tmp/restore.log" 2>&1 || restore_rc=1
    fi
    restored_status=$("$alien_bin" rgb status 2>>"$qa_tmp/restore.log") || {
      restored_status='RESTORE STATUS READ FAILED'
      restore_rc=1
    }
    echo "RESTORE_BEGIN best_known_initial_state=1"
    printf '%s\n' "$restored_status"
    if [[ $restored_status != "$initial_status" ]]; then
      echo "URGENT: restored readback differs from the captured initial state" >&2
      printf 'INITIAL:\n%s\nRESTORED:\n%s\n' "$initial_status" "$restored_status" >&2
      restore_rc=1
    else
      echo "RESTORE_CONFIRMED exposed_state_matches_initial=1"
    fi
  fi
  if [[ $original_lighting_was_set ]]; then
    export ALIEN_LIGHTING="$original_lighting"
  else
    unset ALIEN_LIGHTING
  fi
  case $qa_tmp in
    */alien-lighting-optical.*) rm -rf -- "$qa_tmp" || cleanup_rc=1 ;;
    *)
      echo "refusing to remove unexpected temporary path: $qa_tmp" >&2
      cleanup_rc=1
      ;;
  esac
  if (( matrix_complete == 1 && original_rc == 0 && restore_rc == 0 && cleanup_rc == 0 )); then
    echo "END complete_matrix=1 cases=$case_count restore_confirmed=1"
  else
    echo "END complete_matrix=0 restore_confirmed=$((restore_rc == 0))" >&2
  fi
  final_rc=$original_rc
  if (( final_rc == 0 && (restore_rc != 0 || cleanup_rc != 0) )); then
    final_rc=1
  fi
  exit "$final_rc"
}
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 141' PIPE
trap restore_initial EXIT

mark() {
  ((case_count += 1))
  printf 'OPTICAL %s case=%s expected=%s\n' \
    "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" "$2"
  sleep "$dwell"
}

assert_off_preserves() {
  local case_name=$1 before=$2 after old_brightness expected
  after=$("$alien_bin" rgb status)
  old_brightness=$(awk '/^brightness[[:space:]]/{print $0; exit}' <<<"$before")
  expected=${before/"$old_brightness"/brightness 0}
  printf 'READBACK case=%s\n%s\n' "$case_name" "$after"
  if [[ $after != "$expected" ]]; then
    printf 'Off readback mismatch for %s\nBEFORE:\n%s\nEXPECTED AFTER:\n%s\nACTUAL AFTER:\n%s\n' \
      "$case_name" "$before" "$expected" "$after" >&2
    return 1
  fi
}

assert_dynamic_status() {
  local case_name=$1 expected_effect=$2 expected_speed=$3 expected_brightness=$4
  local expected_direction=$5 expected_colour=$6 status got_effect got_speed
  local got_brightness got_direction got_colour
  status=$("$alien_bin" rgb status)
  got_effect=$(awk '/^effect[[:space:]]/{print $2; exit}' <<<"$status")
  got_speed=$(awk '/^speed[[:space:]]/{print $2; exit}' <<<"$status")
  got_brightness=$(awk '/^brightness[[:space:]]/{print $2; exit}' <<<"$status")
  got_direction=$(awk '/^direction[[:space:]]/{print $2; exit}' <<<"$status")
  got_colour=$(awk '/^colour[[:space:]]/{print $2; exit}' <<<"$status")
  printf 'READBACK case=%s\n%s\n' "$case_name" "$status"
  if [[ $got_effect != "$expected_effect" || $got_speed != "$expected_speed" ||
        $got_brightness != "$expected_brightness" ||
        $got_direction != "$expected_direction" || $got_colour != "$expected_colour" ]]; then
    printf 'Dynamic readback mismatch for %s: expected effect=%s speed=%s brightness=%s direction=%s colour=%s\n' \
      "$case_name" "$expected_effect" "$expected_speed" "$expected_brightness" \
      "${expected_direction:--}" "${expected_colour:--}" >&2
    return 1
  fi
}

printf 'BEGIN alien=%s dwell=%ss camera_id=%s camera_fps=%s camera_settings=%s\nINITIAL:\n%s\n' \
  "$alien_version" "$dwell" "$camera_id" "$camera_fps" "$camera_settings" "$initial_status"
echo "NOTICE saved zone mask has no firmware getter; camera observation is authoritative"
echo "NOTICE do not use another Alien frontend until RESTORE_CONFIRMED is printed"

# Static colour identity, one incremental colour edge, and all sixteen masks.
"$alien_bin" rgb zones '#ff0000' '#00ff00' '#0000ff' '#ffffff'
effect static 0 100
mark static-primary-zones 'left-to-right red green blue white'
"$alien_bin" rgb zone 2 '#ff00ff'
mark static-zone-2-colour-edge 'only zone 2 changes from green to magenta'
"$alien_bin" rgb zone 2 '#00ff00'
mark static-zone-2-colour-restore 'zone 2 returns to green; other zones stay unchanged'
for mask in 0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15; do
  apply_mask "$mask"
  mark "static-mask-$mask" "only zone bits $(printf '0x%02x' "$mask") lit"
done

# Every exact Covini brightness step, including physical Off at zero.
apply_mask 15
for brightness in 0 25 50 75 100; do
  effect static 0 "$brightness"
  if (( brightness == 0 )); then
    mark static-brightness-0 'all zones physically dark'
  else
    mark "static-brightness-$brightness" "all zones at ${brightness}%"
  fi
done
static_before_off=$("$alien_bin" rgb status)
"$alien_bin" rgb off
assert_off_preserves static-off "$static_before_off"
mark static-off 'all zones physically dark; readback retains Static at brightness 0'

# Brightness zero can prove darkness but cannot reveal speed/direction/motion.
# Exercise it once per animation, with separate truthful labels.
for mode in breath zoom neon wave shifting; do
  case $mode in
    breath|zoom)
      effect "$mode" 5 0 '' '#ff0000'
      assert_dynamic_status "${mode}-brightness-0" "$mode" 5 0 '' '#ff0000'
      ;;
    neon)
      effect neon 5 0
      assert_dynamic_status neon-brightness-0 neon 5 0 '' ''
      ;;
    wave)
      effect wave 5 0 ltr
      assert_dynamic_status wave-brightness-0 wave 5 0 left-to-right ''
      ;;
    shifting)
      effect shifting 5 0 ltr '#ff0000'
      assert_dynamic_status shifting-brightness-0 shifting 5 0 left-to-right '#ff0000'
      ;;
  esac
  mark "${mode}-brightness-0" 'physically dark; animation motion is not observable at zero'
done

# Full observable Cartesian matrix: every animation, all speeds 1..9, all four
# non-zero brightness steps, and both directions where the OEM exposes them.
for brightness in 25 50 75 100; do
  for speed in 1 2 3 4 5 6 7 8 9; do
    effect breath "$speed" "$brightness" '' '#ff0000'
    mark "breath-s${speed}-b${brightness}" 'red breathing pattern'

    effect zoom "$speed" "$brightness" '' '#ff0000'
    mark "zoom-s${speed}-b${brightness}" 'red zoom pattern'

    effect neon "$speed" "$brightness"
    mark "neon-s${speed}-b${brightness}" 'firmware-palette neon pattern'

    for direction in ltr rtl; do
      effect wave "$speed" "$brightness" "$direction"
      mark "wave-${direction}-s${speed}-b${brightness}" \
        "firmware-palette wave moving $direction"

      effect shifting "$speed" "$brightness" "$direction" '#ff0000'
      mark "shifting-${direction}-s${speed}-b${brightness}" \
        "red shifting pattern moving $direction"
    done
  done
done

# Shared Pattern-colour identity, including the previously untested green path.
for colour_name in red green blue; do
  case $colour_name in
    red) colour='#ff0000' ;;
    green) colour='#00ff00' ;;
    blue) colour='#0000ff' ;;
  esac
  for mode in breath zoom shifting; do
    if [[ $mode == shifting ]]; then
      effect "$mode" 5 75 ltr "$colour"
    else
      effect "$mode" 5 75 '' "$colour"
    fi
    mark "${mode}-colour-${colour_name}" "$colour_name user-colour pattern"
  done
done

# Prove dynamic Off preserves the active record instead of falling back to
# Static or fabricating a new effect.
effect breath 5 75 '' '#0000ff'
mark dynamic-off-before 'blue breathing at speed 5 and brightness 75'
dynamic_before_off=$("$alien_bin" rgb status)
"$alien_bin" rgb off
assert_off_preserves dynamic-off "$dynamic_before_off"
mark dynamic-off-after 'physically dark; readback retains Breath at brightness 0'

if (( case_count != expected_cases )); then
  echo "internal matrix count mismatch: expected $expected_cases, got $case_count" >&2
  exit 1
fi
matrix_complete=1
