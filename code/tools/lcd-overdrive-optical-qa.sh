#!/usr/bin/env bash
# Run a camera-recorded A/B/A LCD-overdrive trial on the physical Acer eDP
# panel. This script never opens or records a camera. It only getter-checks and
# toggles Alien's LCD-overdrive endpoint while a separate camera records the
# standalone pursuit pattern.
set -Eeuo pipefail

usage() {
  cat <<'EOF'
usage: lcd-overdrive-optical-qa.sh \
  --i-confirm-a-physical-camera-is-recording-edp-at-240fps-or-faster \
  --i-understand-this-will-toggle-lcd-overdrive \
  --camera-id RECORDING_ID --camera-settings DESCRIPTION \
  [--dwell SECONDS] [--arm-seconds SECONDS] [--log PATH]

Or: lcd-overdrive-optical-qa.sh --preflight-only [--log PATH]

Runs the getter-confirmed sequence Off -> On -> Off, then restores the exact
pre-run getter state even if interrupted. The default dwell is 15 seconds per
phase and the minimum is 10 seconds. The camera must look at the physical eDP
panel; a screenshot, screen recording, remote desktop, or capture-card feed is
not evidence of LCD response.

--preflight-only proves the exact host, display, daemon route, and live getter
without arming restoration, sleeping, or sending any mutation.

Before starting, open lcd-overdrive-pursuit.html on eDP-1, put it fullscreen,
confirm its browser rAF callback cadence is near 144 Hz, and start the
independent camera. The rAF number is a pacing diagnostic, not a physical
panel-refresh measurement.

Environment overrides:
  ALIEN_BIN                    Alien CLI to invoke (default: alien)
  ALIEN_LCD_OPTICAL_DWELL      default dwell when --dwell is omitted
  ALIEN_LCD_OPTICAL_ARM_DELAY  default arming delay when --arm-seconds omitted
  ALIEN_LCD_QA_LOG_DIR         default output directory for the TSV receipt
EOF
}

camera_acknowledged=0
mutation_acknowledged=0
preflight_only=0
camera_id=
camera_settings=
dwell=${ALIEN_LCD_OPTICAL_DWELL:-15}
arm_seconds=${ALIEN_LCD_OPTICAL_ARM_DELAY:-5}
log_path=

while (($# > 0)); do
  case $1 in
    --i-confirm-a-physical-camera-is-recording-edp-at-240fps-or-faster)
      camera_acknowledged=1
      shift
      ;;
    --i-understand-this-will-toggle-lcd-overdrive)
      mutation_acknowledged=1
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
    --arm-seconds)
      (($# >= 2)) || { echo "--arm-seconds requires a value" >&2; exit 2; }
      arm_seconds=$2
      shift 2
      ;;
    --log)
      (($# >= 2)) || { echo "--log requires a path" >&2; exit 2; }
      log_path=$2
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

if (( preflight_only == 0 )) &&
   (( camera_acknowledged == 0 || mutation_acknowledged == 0 )); then
  echo "both explicit acknowledgements are required; no firmware request was sent" >&2
  usage >&2
  exit 2
fi
if (( preflight_only == 0 )) && [[ -z $camera_id || -z $camera_settings ]]; then
  echo "--camera-id and --camera-settings are required for an optical run" >&2
  exit 2
fi
if [[ $camera_id == *[$'\t\r\n']* || $camera_settings == *[$'\t\r\n']* ]]; then
  echo "camera metadata must be one line without tab characters" >&2
  exit 2
fi

case $dwell in
  ''|*[!0-9]*)
    echo "dwell must be a whole number of seconds" >&2
    exit 2
    ;;
esac
if (( dwell < 10 || dwell > 600 )); then
  echo "dwell must be between 10 and 600 seconds" >&2
  exit 2
fi

case $arm_seconds in
  ''|*[!0-9]*)
    echo "arm-seconds must be a whole number of seconds" >&2
    exit 2
    ;;
esac
if (( arm_seconds < 3 || arm_seconds > 60 )); then
  echo "arm-seconds must be between 3 and 60 seconds" >&2
  exit 2
fi

alien_bin=${ALIEN_BIN:-alien}
command -v "$alien_bin" >/dev/null 2>&1 || {
  echo "Alien CLI not found: $alien_bin" >&2
  exit 1
}

# This receipt is intended to prove the real, privileged host-daemon route.
# Never inherit a QA relay or mock socket through Alien's test override.
required_socket=/run/alien/alien.sock
if [[ ${ALIEN_SOCKET+x} && $ALIEN_SOCKET != "$required_socket" ]]; then
  echo "refusing non-production ALIEN_SOCKET=$ALIEN_SOCKET; expected $required_socket" >&2
  exit 1
fi
export ALIEN_SOCKET=$required_socket
if [[ ${ALIEN_INTERFACE_LOCK+x} ]]; then
  echo "refusing inherited ALIEN_INTERFACE_LOCK; optical QA must not redirect direct-access locking" >&2
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
runtime_dir=${XDG_RUNTIME_DIR:-/run/user/$EUID}
if [[ ! -d $runtime_dir || ! -O $runtime_dir ]]; then
  echo "trusted per-user runtime directory is unavailable: $runtime_dir" >&2
  exit 1
fi
qa_lock=$runtime_dir/alien-lcd-overdrive-optical.lock
umask 077
exec 9>"$qa_lock"
if ! flock -n 9; then
  echo "another LCD-overdrive optical runner holds $qa_lock" >&2
  exit 1
fi

alien_cli_path=$(command -v "$alien_bin")
alien_cli_realpath=$(readlink -f -- "$alien_cli_path" 2>/dev/null || printf '%s' "$alien_cli_path")
alien_version=$("$alien_bin" --version 2>&1) || {
  echo "cannot identify Alien CLI: $alien_cli_path" >&2
  exit 1
}
host_name=$(hostname)
kernel_identity=$(uname -srvmo)

read_dmi() {
  local name=$1 path=/sys/class/dmi/id/$1
  [[ -r $path ]] || {
    echo "required DMI field is unreadable: $path" >&2
    return 1
  }
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

shopt -s nullglob
edp_candidates=(/sys/class/drm/card*-eDP-*)
shopt -u nullglob
edp_connector=
for candidate in "${edp_candidates[@]}"; do
  if [[ -r $candidate/status && $(<"$candidate/status") == connected &&
        -r $candidate/enabled && $(<"$candidate/enabled") == enabled ]]; then
    if [[ -n $edp_connector ]]; then
      echo "more than one enabled eDP connector is present; refusing ambiguous target" >&2
      exit 1
    fi
    edp_connector=$candidate
  fi
done
if [[ -z $edp_connector ]]; then
  echo "no connected and enabled physical eDP connector was found" >&2
  exit 1
fi
edp_sysfs_name=${edp_connector##*/}
edp_name=${edp_sysfs_name#*-}

command -v hyprctl >/dev/null 2>&1 || {
  echo "hyprctl is required to prove the active physical-panel mode" >&2
  exit 1
}
command -v jq >/dev/null 2>&1 || {
  echo "jq is required to prove the active physical-panel mode" >&2
  exit 1
}
command -v pgrep >/dev/null 2>&1 || {
  echo "pgrep is required to exclude active Alien frontends" >&2
  exit 1
}
hypr_monitors=$(hyprctl monitors -j 2>&1) || {
  echo "cannot query the live Hyprland monitor state: $hypr_monitors" >&2
  exit 1
}
hypr_edp_count=$(jq --arg name "$edp_name" '[.[] | select(.name == $name)] | length' <<<"$hypr_monitors")
if [[ $hypr_edp_count != 1 ]]; then
  echo "Hyprland does not expose exactly one $edp_name monitor" >&2
  exit 1
fi
hypr_edp_record=$(jq -c --arg name "$edp_name" '.[] | select(.name == $name)' <<<"$hypr_monitors")
read -r edp_width edp_height edp_refresh edp_disabled < <(
  jq -r '[.width, .height, .refreshRate, (.disabled // false)] | @tsv' <<<"$hypr_edp_record"
)
if [[ $edp_width != 1920 || $edp_height != 1080 || $edp_disabled != false ]] ||
   ! awk -v hz="$edp_refresh" 'BEGIN { exit !(hz >= 140 && hz <= 146) }'; then
  printf 'physical eDP mode guard failed: %sx%s@%s disabled=%s\n' \
    "$edp_width" "$edp_height" "$edp_refresh" "$edp_disabled" >&2
  exit 1
fi

shopt -s nullglob
backlight_candidates=(/sys/class/backlight/*)
shopt -u nullglob
if (( ${#backlight_candidates[@]} != 1 )); then
  echo "expected exactly one physical-panel backlight provider, found ${#backlight_candidates[@]}" >&2
  exit 1
fi
panel_backlight=${backlight_candidates[0]}
read_panel_brightness() {
  panel_brightness=$(<"$panel_backlight/brightness")
  panel_actual_brightness=$(<"$panel_backlight/actual_brightness")
  panel_max_brightness=$(<"$panel_backlight/max_brightness")
  [[ $panel_brightness =~ ^[0-9]+$ && $panel_actual_brightness =~ ^[0-9]+$ &&
     $panel_max_brightness =~ ^[1-9][0-9]*$ ]]
}
if ! read_panel_brightness; then
  echo "cannot read exact physical-panel backlight state from $panel_backlight" >&2
  exit 1
fi
initial_panel_brightness=$panel_brightness
initial_panel_actual_brightness=$panel_actual_brightness
initial_panel_max_brightness=$panel_max_brightness

script_dir=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
pattern_path=$script_dir/lcd-overdrive-pursuit.html
if [[ ! -r $pattern_path ]]; then
  echo "pursuit pattern is missing or unreadable: $pattern_path" >&2
  exit 1
fi

query_output=
query_state_result=
query_state() {
  if ! query_output=$("$alien_bin" lcd-overdrive status 2>&1); then
    query_state_result=error
    return 1
  fi
  case $query_output in
    "LCD overdrive on ("*) query_state_result=on ;;
    "LCD overdrive off ("*) query_state_result=off ;;
    *"unsupported by the live firmware getter"*) query_state_result=unsupported ;;
    *) query_state_result=unrecognized; return 1 ;;
  esac
}

# The only support authority is the live getter. Do this before creating a
# restoration transaction or sending any mutation.
if ! query_state; then
  echo "cannot parse the LCD-overdrive getter; no mutation was sent" >&2
  printf '%s\n' "$query_output" >&2
  exit 1
fi
if [[ $query_state_result == unsupported ]]; then
  echo "LCD overdrive is unsupported by the live firmware getter; no mutation was sent" >&2
  exit 1
fi
initial_state=$query_state_result

if [[ -z $log_path ]]; then
  default_state_root=${XDG_STATE_HOME:-${HOME:-/tmp}/.local/state}
  log_dir=${ALIEN_LCD_QA_LOG_DIR:-$default_state_root/alien/qa}
  mkdir -p -- "$log_dir"
  log_path=$log_dir/lcd-overdrive-optical-$(date -u +%Y%m%dT%H%M%SZ)-$$.tsv
else
  log_dir=$(dirname -- "$log_path")
  mkdir -p -- "$log_dir"
fi

if [[ -e $log_path ]]; then
  echo "refusing to overwrite existing receipt: $log_path" >&2
  exit 1
fi
umask 077
if ! (set -o noclobber; : >"$log_path") 2>/dev/null; then
  echo "cannot create receipt: $log_path" >&2
  exit 1
fi
exec 3>>"$log_path"

utc_now() {
  local value
  value=$(date -u +%Y-%m-%dT%H:%M:%S.%NZ)
  if [[ $value == *N* ]]; then
    value=$(date -u +%Y-%m-%dT%H:%M:%S.000000000Z)
  fi
  printf '%s' "$value"
}

epoch_ns_now() {
  local value
  value=$(date +%s%N)
  if [[ $value == *N* ]]; then
    value=$(date +%s)000000000
  fi
  printf '%s' "$value"
}

monotonic_now() {
  if [[ -r /proc/uptime ]]; then
    awk '{print $1; exit}' /proc/uptime
  else
    printf 'unavailable'
  fi
}

emit() {
  local event=$1 requested=${2:--} observed=${3:--} detail=${4:-}
  detail=${detail//$'\t'/ }
  detail=${detail//$'\r'/}
  detail=${detail//$'\n'/ | }
  local line
  printf -v line '%s\t%s\t%s\t%s\t%s\t%s\t%s' \
    "$(utc_now)" "$(epoch_ns_now)" "$(monotonic_now)" \
    "$event" "$requested" "$observed" "$detail"
  printf '%s\n' "$line"
  printf '%s\n' "$line" >&3
}

pattern_hash() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum -- "$pattern_path" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 -- "$pattern_path" | awk '{print $1}'
  else
    printf 'unavailable'
  fi
}

printf '# Alien LCD-overdrive physical optical A/B/A receipt v1\n' >&3
printf '# pattern=%s\n' "$pattern_path" >&3
printf '# pattern_sha256=%s\n' "$(pattern_hash)" >&3
printf '# dwell_seconds=%s\n' "$dwell" >&3
printf '# arm_seconds=%s\n' "$arm_seconds" >&3
printf '# initial_getter_state=%s\n' "$initial_state" >&3
printf '# alien_cli=%s\n' "$alien_cli_realpath" >&3
printf '# alien_version=%s\n' "$alien_version" >&3
printf '# alien_socket=%s\n' "$required_socket" >&3
printf '# alien_require_socket=1\n' >&3
printf '# euid=%s\n' "$EUID" >&3
printf '# direct_acpi_user_access=none\n' >&3
printf '# runner_lock=%s\n' "$qa_lock" >&3
printf '# alien_socket_stat=%s\n' "$(stat -Lc 'device=%d inode=%i mode=%a owner=%u:%g' -- "$required_socket")" >&3
printf '# host=%s\n' "$host_name" >&3
printf '# kernel=%s\n' "$kernel_identity" >&3
printf '# dmi_vendor=%s\n' "$dmi_vendor" >&3
printf '# dmi_product=%s\n' "$dmi_product" >&3
printf '# dmi_board_vendor=%s\n' "$dmi_board_vendor" >&3
printf '# dmi_board=%s\n' "$dmi_board" >&3
printf '# dmi_bios=%s\n' "$dmi_bios" >&3
printf '# edp_connector=%s\n' "$edp_connector" >&3
printf '# edp_mode=%sx%s@%s\n' "$edp_width" "$edp_height" "$edp_refresh" >&3
printf '# hypr_edp_json=%s\n' "$hypr_edp_record" >&3
printf '# panel_backlight=%s\n' "$panel_backlight" >&3
printf '# panel_brightness=%s/%s actual=%s\n' \
  "$initial_panel_brightness" "$initial_panel_max_brightness" "$initial_panel_actual_brightness" >&3
printf '# camera_id=%s\n' "${camera_id:-- preflight only -}" >&3
printf '# camera_settings=%s\n' "${camera_settings:-- preflight only -}" >&3
printf 'utc_iso8601_ns\tepoch_ns\tmonotonic_uptime_s\tevent\trequested\tobserved\tdetail\n' >&3

set_output=
set_lcd_state() {
  local desired=$1
  set_output=$("$alien_bin" lcd-overdrive "$desired" 2>&1)
}

restore_lcd_state() {
  local desired=$1 attempt
  # The daemon admits typed mutations no faster than one per 100 ms. An
  # interrupted setter may have landed immediately before EXIT cleanup, so
  # wait beyond that gate before the first restore attempt. Retry only the
  # explicit rate-limit response; every other failure is returned verbatim.
  sleep 0.2
  for attempt in 1 2 3 4 5; do
    if set_lcd_state "$desired"; then
      emit restore_setter_attempt "$desired" - "attempt=$attempt output=$set_output"
      return 0
    fi
    emit restore_setter_attempt_failed "$desired" error "attempt=$attempt output=$set_output"
    if [[ $set_output != *"typed feature mutations must be at least"* ]]; then
      return 1
    fi
    sleep 0.2
  done
  return 1
}

restore_armed=0
finish() {
  local rc=$?
  trap - EXIT
  # Do not let a second interrupt or SSH HUP terminate the bounded restoration
  # after the first one has already moved us onto the exit path.
  trap '' HUP INT TERM PIPE
  set +e

  if (( restore_armed == 1 )); then
    emit restore_request "$initial_state" - "restore exact pre-run getter state if needed"
    if query_state && [[ $query_state_result == "$initial_state" ]]; then
      emit restore_already_at_initial "$initial_state" "$query_state_result" "$query_output"
    elif restore_lcd_state "$initial_state"; then
      emit restore_setter_return "$initial_state" - "$set_output"
    else
      emit restore_setter_failed "$initial_state" error "$set_output"
      rc=1
    fi

    if query_state; then
      emit restore_getter "$initial_state" "$query_state_result" "$query_output"
      if [[ $query_state_result != "$initial_state" ]]; then
        printf 'URGENT: LCD overdrive restore mismatch; expected %s, getter says %s\n' \
          "$initial_state" "$query_state_result" >&2
        rc=1
      fi
    else
      emit restore_getter_failed "$initial_state" error "$query_output"
      printf 'URGENT: could not confirm the restored LCD-overdrive state\n' >&2
      rc=1
    fi
  fi

  emit run_end - - "exit_status=$rc receipt=$log_path camera_review=pending"
  exec 3>&-
  if (( rc != 0 )); then
    printf 'LCD-overdrive optical QA did not complete cleanly; receipt: %s\n' "$log_path" >&2
  else
    printf 'LCD-overdrive command sequence complete; camera-footage review remains pending; receipt: %s\n' "$log_path"
  fi
  exit "$rc"
}
trap finish EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 141' PIPE

check_panel_brightness() {
  local event=$1 phase=${2:--}
  if ! read_panel_brightness; then
    emit panel_brightness_read_failed "$initial_panel_brightness" error \
      "event=$event phase=$phase path=$panel_backlight"
    return 1
  fi
  emit panel_brightness_check "$initial_panel_brightness" "$panel_brightness" \
    "event=$event phase=$phase max=$panel_max_brightness actual=$panel_actual_brightness"
  if [[ $panel_brightness != "$initial_panel_brightness" ||
        $panel_actual_brightness != "$initial_panel_actual_brightness" ||
        $panel_max_brightness != "$initial_panel_max_brightness" ]]; then
    emit panel_brightness_changed \
      "$initial_panel_brightness/$initial_panel_max_brightness actual=$initial_panel_actual_brightness" \
      "$panel_brightness/$panel_max_brightness actual=$panel_actual_brightness" \
      "event=$event phase=$phase"
    return 1
  fi
}

if (( preflight_only == 1 )); then
  emit preflight_getter - "$initial_state" "$query_output"
  emit target_identity - "$initial_state" \
    "cli=$alien_cli_realpath version=$alien_version socket=$required_socket host=$host_name dmi=$dmi_product bios=$dmi_bios edp=${edp_width}x${edp_height}@${edp_refresh}"
  emit preflight_complete - "$initial_state" \
    "no mutation armed or sent; camera and optical review not performed"
  emit run_end - - "exit_status=0 receipt=$log_path preflight_only=1"
  exec 3>&-
  trap - EXIT
  printf 'LCD-overdrive exact-target preflight complete; no mutation sent; receipt: %s\n' "$log_path"
  exit 0
fi

run_phase() {
  local phase=$1 desired=$2
  if ! check_panel_brightness phase_begin "$phase"; then
    return 1
  fi
  emit phase_request "$desired" - "phase=$phase"
  if ! set_lcd_state "$desired"; then
    emit phase_setter_failed "$desired" error "phase=$phase output=$set_output"
    return 1
  fi
  emit phase_setter_return "$desired" - "phase=$phase output=$set_output"

  if ! query_state; then
    emit phase_getter_failed "$desired" error "phase=$phase output=$query_output"
    return 1
  fi
  emit phase_getter "$desired" "$query_state_result" "phase=$phase output=$query_output"
  if [[ $query_state_result != "$desired" ]]; then
    emit phase_mismatch "$desired" "$query_state_result" "phase=$phase"
    return 1
  fi

  emit phase_dwell_begin "$desired" "$query_state_result" "phase=$phase seconds=$dwell"
  sleep "$dwell"
  if ! check_panel_brightness phase_dwell_end "$phase"; then
    return 1
  fi
  if ! query_state; then
    emit phase_dwell_getter_failed "$desired" error "phase=$phase output=$query_output"
    return 1
  fi
  emit phase_dwell_end "$desired" "$query_state_result" "phase=$phase seconds=$dwell output=$query_output"
  if [[ $query_state_result != "$desired" ]]; then
    emit phase_dwell_mismatch "$desired" "$query_state_result" "phase=$phase"
    return 1
  fi
}

printf 'Pattern: %s\n' "$pattern_path"
printf 'Receipt: %s\n' "$log_path"
printf 'Initial getter state: %s\n' "$initial_state"
printf 'Camera acknowledgement accepted. Keep the physical eDP panel and timestamp overlay in frame.\n'
printf 'Do not use any other Alien frontend until the automatic restore is confirmed.\n'
emit preflight_getter - "$initial_state" "$query_output"
emit target_identity - "$initial_state" \
  "cli=$alien_cli_realpath version=$alien_version socket=$required_socket host=$host_name dmi=$dmi_product bios=$dmi_bios edp=${edp_width}x${edp_height}@${edp_refresh}"
emit camera_sync_arm - "$initial_state" "sequence starts after ${arm_seconds}s; camera asserted active; rAF cadence is not a panel measurement"
sleep "$arm_seconds"

if active_frontends=$(pgrep -af '(^|/)(\.alien-gui-wrapped|alien-gui|alien-tui)([[:space:]]|$)'); then
  emit pre_mutation_frontend_present "$initial_state" - "$active_frontends"
  echo "another Alien GUI/TUI is running; no mutation was sent" >&2
  printf '%s\n' "$active_frontends" >&2
  exit 1
else
  pgrep_rc=$?
  if (( pgrep_rc != 1 )); then
    emit pre_mutation_frontend_check_failed "$initial_state" error "pgrep exit_status=$pgrep_rc"
    echo "cannot prove that Alien GUI/TUI frontends are absent; no mutation was sent" >&2
    exit 1
  fi
fi
emit pre_mutation_frontend_check "$initial_state" none \
  "no alien-gui/alien-tui process found; other one-shot CLI clients remain an operator-controlled boundary"
if ! check_panel_brightness pre_mutation -; then
  echo "physical-panel brightness changed during setup; no mutation was sent" >&2
  exit 1
fi

# Setup and camera arming can take up to a minute. Re-read at the mutation
# boundary so another frontend cannot silently turn the captured pre-state
# stale and then have that stale state restored over its legitimate change.
if ! query_state; then
  emit pre_mutation_getter_failed "$initial_state" error "$query_output"
  echo "cannot re-read LCD overdrive at the mutation boundary; no mutation was sent" >&2
  exit 1
fi
emit pre_mutation_getter "$initial_state" "$query_state_result" "$query_output"
if [[ $query_state_result != "$initial_state" ]]; then
  emit pre_mutation_state_changed "$initial_state" "$query_state_result" \
    "another client changed state during setup; no mutation was sent"
  echo "LCD-overdrive state changed during setup; no mutation was sent" >&2
  exit 1
fi

# Arm restoration immediately before the first possible mutation. From here,
# every exit path writes and getter-confirms the exact initial state.
restore_armed=1
run_phase A_off off
run_phase B_on on
run_phase A2_off off
if ! query_state; then
  emit aba_final_getter_failed off error "$query_output"
  exit 1
fi
emit aba_final_getter off "$query_state_result" "$query_output"
if [[ $query_state_result != off ]]; then
  emit aba_final_mismatch off "$query_state_result" "Off->On->Off final state mismatch"
  exit 1
fi
emit aba_complete off "$query_state_result" \
  "Off->On->Off command sequence complete; footage review pending; automatic initial-state restore follows"
