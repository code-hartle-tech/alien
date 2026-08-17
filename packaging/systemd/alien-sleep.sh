#!/bin/sh
# systemd-sleep hook: re-assert fan state across suspend.
#
# Installed as /usr/lib/systemd/system-sleep/alien.
#
# ── Why this exists ─────────────────────────────────────────────────────────
# Firmware forgets. The EC comes back from S3 on its own automatic curve, which
# on this hardware is the state that permits 95 C and 1446 MHz. Anything Alien
# had set is silently gone, and nothing tells userspace about it.
#
# No other Linux tool in this space handles it. `PrepareForSleep` appears zero
# times in gamemode's entire tree; system76-power has no sleep handling;
# asusctl's is gated behind an AC-state change so a same-power-source resume
# reapplies nothing. thinkfan does it properly, and this follows its pattern.
#
# ── Why a sleep hook and not a D-Bus inhibitor ──────────────────────────────
# Fewer moving parts, and it works without adding a D-Bus dependency to a
# project whose entire tree is serde plus toml.
#
# The constraint that shapes it: systemd freezes `user.slice` while these hooks
# run. So this must talk to alien-daemon directly and must never route through
# anything living in a user session.
#
# ── Why `post` restores to maximum ──────────────────────────────────────────
# Because it cannot know what was set. There is no fan-mode getter in this
# firmware, so "put it back how it was" is not an available operation. If a
# lease is outstanding, `gamesync recover` knows the baseline and is preferred;
# otherwise maximum is the deliberate default, matching `acer-wmi`, which maps
# pwm_enable=0 to ACER_WMID_FAN_MODE_TURBO.
#
# Best-effort throughout: a failure here must never block suspend or resume.

set -u

ALIEN=/usr/bin/alien
[ -x "$ALIEN" ] || ALIEN=$(command -v alien 2>/dev/null) || exit 0

case "$1" in
    pre)
        # Hand the fans back before going down. Leaving them pinned in manual
        # across a suspend risks the EC and Alien disagreeing on resume, and
        # the machine is about to be idle anyway.
        "$ALIEN" fan auto >/dev/null 2>&1 || true
        ;;
    post)
        # The daemon may not have finished re-initialising the moment this
        # runs; a short bounded retry is cheaper than a race.
        i=0
        while [ "$i" -lt 10 ]; do
            if "$ALIEN" status >/dev/null 2>&1; then
                break
            fi
            i=$((i + 1))
            sleep 1
        done

        # An outstanding lease knows what to restore. Prefer it.
        if "$ALIEN" gamesync status 2>/dev/null | grep -q '^holder'; then
            "$ALIEN" gamesync recover >/dev/null 2>&1 || true
            exit 0
        fi

        # Otherwise re-assert the safe state. The EC has reverted to its own
        # curve and nobody else is going to notice.
        "$ALIEN" fan max >/dev/null 2>&1 || true
        ;;
esac

exit 0
