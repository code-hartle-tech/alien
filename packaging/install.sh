#!/bin/sh
# Alien — one-line installer.
#
#   curl -fsSL https://alien.hartle.tech/install.sh | sh
#
# Written for someone who owns a Predator, uses Linux, and does not want to
# read any of this. It therefore has three rules:
#
#   1. Never half-install. Every step that can fail is checked, and a failure
#      stops with a sentence explaining what to do — not a stack trace.
#   2. Never start writing to firmware on its own. It installs and enables the
#      daemon; it does not change a single fan or light.
#   3. Say the quiet part out loud. Group membership does not reach a session
#      that is already running, and Secure Boot silently refuses unsigned
#      kernel modules. Both are printed, loudly, because both otherwise present
#      as "I installed it and it doesn't work".
#
# POSIX sh on purpose: this has to run under dash on a stock Debian before
# anything of ours is installed.

set -eu

REPO="code-hartle-tech/alien"
API="https://api.github.com/repos/$REPO/releases/latest"
RELEASES="https://github.com/$REPO/releases"

DRY_RUN=0
UNINSTALL=0
ASSUME_YES=0
WANT_COOLING=0

# ── output ───────────────────────────────────────────────────────────────────
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
    B=$(printf '\033[1m'); G=$(printf '\033[32m'); Y=$(printf '\033[33m')
    R=$(printf '\033[31m'); D=$(printf '\033[2m');  N=$(printf '\033[0m')
else
    B=''; G=''; Y=''; R=''; D=''; N=''
fi

say()  { printf '%s\n' "$*"; }
step() { printf '%s==>%s %s\n' "$G$B" "$N$B" "$*$N"; }
warn() { printf '%s !! %s%s\n' "$Y" "$*" "$N" >&2; }
dim()  { printf '%s    %s%s\n' "$D" "$*" "$N"; }
die()  { printf '\n%s xx %s%s\n\n' "$R$B" "$*" "$N" >&2; exit 1; }

usage() {
    cat <<'EOF'
Alien installer

  sh install.sh                 install
  sh install.sh --cooling       install and enable the temperature-curve service
  sh install.sh --uninstall     remove everything this installed
  sh install.sh --dry-run       print what would happen, change nothing
  sh install.sh --yes           do not prompt
  sh install.sh --help          this

Environment:
  ALIEN_VERSION=v0.6.0          install a specific tag instead of the latest
EOF
}

for arg in "$@"; do
    case "$arg" in
        --dry-run)   DRY_RUN=1 ;;
        --uninstall) UNINSTALL=1 ;;
        --yes|-y)    ASSUME_YES=1 ;;
        --cooling)   WANT_COOLING=1 ;;
        --help|-h)   usage; exit 0 ;;
        *)           die "unknown option: $arg  (try --help)" ;;
    esac
done

run() {
    if [ "$DRY_RUN" = 1 ]; then
        printf '%s    would run: %s%s\n' "$D" "$*" "$N"
        return 0
    fi
    "$@"
}

# ── privilege ────────────────────────────────────────────────────────────────
# Deliberately not run as root. The script needs to know which *user* to add to
# the alien group, and under `sudo sh install.sh` that user is root — which is
# exactly the wrong answer and produces a GUI that never gets permission.
if [ "$(id -u)" = 0 ]; then
    if [ -n "${SUDO_USER:-}" ]; then
        TARGET_USER=$SUDO_USER
        SUDO=''
    else
        die "Run this as your normal user, not as root.
    It needs to know which account to grant hardware access to, and run as
    root it would grant it to root."
    fi
else
    TARGET_USER=$(id -un)
    if command -v sudo >/dev/null 2>&1; then
        SUDO=sudo
    elif command -v doas >/dev/null 2>&1; then
        SUDO=doas
    else
        die "Neither sudo nor doas is available, and installing a system service needs one."
    fi
fi

say ""
say "${B}  Alien${N} — hardware control for Acer Predator laptops on Linux"
say "${D}  https://alien.hartle.tech${N}"
say ""

# ── platform ─────────────────────────────────────────────────────────────────
[ "$(uname -s)" = Linux ] || die "Alien is Linux-only. This is $(uname -s)."

ARCH=$(uname -m)
[ "$ARCH" = x86_64 ] || die "Only x86_64 is supported. This machine is $ARCH."

command -v systemctl >/dev/null 2>&1 \
    || die "This installer needs systemd. Install from source instead: $RELEASES"

DISTRO=unknown
DISTRO_NAME=$(uname -o 2>/dev/null || echo Linux)
if [ -r /etc/os-release ]; then
    # shellcheck disable=SC1091
    . /etc/os-release
    DISTRO=${ID:-unknown}
    DISTRO_NAME=${PRETTY_NAME:-$DISTRO}
    DISTRO_LIKE=${ID_LIKE:-}
fi

FAMILY=unknown
case " $DISTRO ${DISTRO_LIKE:-} " in
    *" debian "*|*" ubuntu "*) FAMILY=debian ;;
    *" fedora "*|*" rhel "*|*" centos "*) FAMILY=fedora ;;
    *" arch "*) FAMILY=arch ;;
    *" suse "*|*" opensuse "*) FAMILY=suse ;;
    *" nixos "*) FAMILY=nixos ;;
esac
case "$DISTRO" in
    debian|ubuntu|linuxmint|pop|elementary|zorin|raspbian) FAMILY=debian ;;
    fedora|rhel|centos|rocky|almalinux|nobara) FAMILY=fedora ;;
    arch|manjaro|endeavouros|garuda|cachyos|arcolinux) FAMILY=arch ;;
    opensuse*|sles) FAMILY=suse ;;
    nixos) FAMILY=nixos ;;
esac

step "This machine"
dim "$DISTRO_NAME"

if [ "$FAMILY" = nixos ]; then
    die "NixOS does not install packages this way — and it does not need to.

    Add Alien as a flake input instead:

      inputs.alien.url = \"github:$REPO\";
      modules = [ alien.nixosModules.default
                  { services.alien = { enable = true; users = [ \"$TARGET_USER\" ]; }; } ];

    Then nixos-rebuild, and log out and back in."
fi

[ "$FAMILY" != unknown ] || die "Unrecognised distribution: $DISTRO

    Alien has packages for Debian/Ubuntu/Mint, Fedora and Arch, plus a NixOS
    module. For anything else, build from source: $RELEASES"

# ── is this even an Acer? ────────────────────────────────────────────────────
# A warning, not a wall. Someone may be installing on a machine whose DMI
# strings are unusual, and refusing outright would be wrong — but silently
# installing a hardware tool onto a Dell would be worse.
VENDOR=''
[ -r /sys/class/dmi/id/sys_vendor ] && VENDOR=$(cat /sys/class/dmi/id/sys_vendor 2>/dev/null || true)
MODEL=''
[ -r /sys/class/dmi/id/product_name ] && MODEL=$(cat /sys/class/dmi/id/product_name 2>/dev/null || true)

case "$VENDOR" in
    *[Aa]cer*) dim "${MODEL:-Acer system}" ;;
    '')        warn "Could not read the system vendor. Continuing anyway." ;;
    *)
        warn "This looks like a $VENDOR ${MODEL:-system}, not an Acer."
        dim "Alien talks to Acer's gaming-WMI firmware interface. On non-Acer"
        dim "hardware it will install fine and then find nothing to control."
        if [ "$ASSUME_YES" != 1 ] && [ "$UNINSTALL" != 1 ] && [ -t 0 ]; then
            printf '    Install anyway? [y/N] '
            read -r reply || reply=n
            case "$reply" in [Yy]*) ;; *) die "Stopped." ;; esac
        fi
        ;;
esac

# ── uninstall ────────────────────────────────────────────────────────────────
if [ "$UNINSTALL" = 1 ]; then
    step "Removing Alien"
    run $SUDO systemctl disable --now alien-cooling.service 2>/dev/null || true
    run $SUDO systemctl disable --now alien-daemon.service 2>/dev/null || true
    case "$FAMILY" in
        debian) run $SUDO apt-get remove --yes alien-predator || true ;;
        fedora) run $SUDO dnf remove -y alien-predator || true ;;
        arch)   run $SUDO pacman -Rns --noconfirm alien || true ;;
        suse)   run $SUDO zypper --non-interactive remove alien-predator || true ;;
    esac
    # The group is left in place on purpose: removing a group that another
    # tool may have been given membership of is not this script's call.
    say ""
    say "  Removed. The '${B}alien${N}' group was left in place — remove it with:"
    dim "$SUDO groupdel alien"
    say ""
    exit 0
fi

# ── the kernel module ────────────────────────────────────────────────────────
# This is the part that actually goes wrong. Alien reaches the firmware through
# /proc/acpi/call, which comes from the out-of-tree acpi_call module, and every
# distribution ships it differently — or not at all.
step "Kernel module (acpi_call)"

if [ -e /proc/acpi/call ]; then
    dim "already loaded"
else
    case "$FAMILY" in
        debian)
            dim "installing acpi-call-dkms"
            run $SUDO apt-get update -qq
            run $SUDO apt-get install --yes acpi-call-dkms
            ;;
        arch)
            dim "installing acpi_call-dkms"
            run $SUDO pacman -Sy --needed --noconfirm acpi_call-dkms
            ;;
        fedora)
            # Not in Fedora, and NOT in RPM Fusion either — verified against the
            # free repo metadata. The maintained build lives in a COPR.
            dim "acpi_call is not in Fedora or RPM Fusion; enabling the rhea/acpi_call COPR"
            run $SUDO dnf install -y dnf-plugins-core
            run $SUDO dnf copr enable -y rhea/acpi_call
            run $SUDO dnf install -y akmod-acpi_call
            ;;
        suse)
            warn "openSUSE has no packaged acpi_call."
            dim "Build it yourself first: https://github.com/nix-community/acpi_call"
            die "Cannot continue without the acpi_call module."
            ;;
    esac
    run $SUDO modprobe acpi_call 2>/dev/null || true
fi

# Secure Boot refuses unsigned modules, and DKMS modules are unsigned. This is
# the single most common reason a correct install still does not work, and the
# kernel says nothing useful about it.
if command -v mokutil >/dev/null 2>&1; then
    if mokutil --sb-state 2>/dev/null | grep -qi enabled; then
        say ""
        warn "Secure Boot is ENABLED."
        dim "The acpi_call module is built locally and is therefore unsigned, so"
        dim "the kernel will refuse to load it. Alien cannot reach the firmware"
        dim "until you either:"
        dim "  • turn Secure Boot off in the BIOS  (F2 at boot → Boot → Secure Boot), or"
        dim "  • enrol your own signing key with mokutil and sign the module."
        dim ""
        dim "Everything below still installs; it just will not work until then."
        say ""
    fi
fi

# ── which release ────────────────────────────────────────────────────────────
step "Latest release"

fetch() {
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$1"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO- "$1"
    else
        die "Neither curl nor wget is installed."
    fi
}
fetch_to() {
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL -o "$2" "$1"
    else
        wget -qO "$2" "$1"
    fi
}

if [ -n "${ALIEN_VERSION:-}" ]; then
    TAG=$ALIEN_VERSION
else
    # The API is the good path. The redirect is the fallback for anywhere the
    # API is rate-limited — a shared IP behind NAT hits 60/hour easily.
    TAG=$(fetch "$API" 2>/dev/null \
          | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
          | head -1) || TAG=''
    if [ -z "$TAG" ]; then
        TAG=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "$RELEASES/latest" 2>/dev/null \
              | sed -n 's#.*/tag/##p') || TAG=''
    fi
fi
[ -n "$TAG" ] || die "Could not work out the latest release. Check your connection, or set ALIEN_VERSION."

VERSION=${TAG#v}
dim "$TAG"

# ── download ─────────────────────────────────────────────────────────────────
step "Downloading"

TMP=$(mktemp -d)
cleanup() { rm -rf "$TMP"; }
trap cleanup EXIT INT TERM

case "$FAMILY" in
    debian) PKG="alien-predator_${VERSION}_amd64.deb" ;;
    fedora) PKG="alien-predator-${VERSION}-1.fc42.x86_64.rpm" ;;
    arch)   PKG="alien-${VERSION}-1-x86_64.pkg.tar.zst" ;;
    suse)   PKG="alien-predator-${VERSION}-1.x86_64.rpm" ;;
esac

BASE="$RELEASES/download/$TAG"

# Asset names carry a distro tag that this script cannot always predict (the
# .fc42 in an rpm, for one), so the exact filename is resolved from the release
# rather than guessed. The guess above is only the fallback.
ASSETS=$(fetch "$API" 2>/dev/null | sed -n 's/.*"browser_download_url"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' || true)
if [ -n "$ASSETS" ]; then
    case "$FAMILY" in
        debian) MATCH=$(printf '%s\n' "$ASSETS" | grep -E '\.deb$'          | head -1) ;;
        fedora) MATCH=$(printf '%s\n' "$ASSETS" | grep -E '\.rpm$'          | head -1) ;;
        arch)   MATCH=$(printf '%s\n' "$ASSETS" | grep -E '\.pkg\.tar\.zst$' | head -1) ;;
        suse)   MATCH=$(printf '%s\n' "$ASSETS" | grep -E '\.rpm$'          | head -1) ;;
    esac
    [ -n "${MATCH:-}" ] && { PKG=$(basename "$MATCH"); BASE=$(dirname "$MATCH"); }
fi

dim "$PKG"
fetch_to "$BASE/$PKG" "$TMP/$PKG" \
    || die "Could not download $PKG.
    There may be no package for your distribution in $TAG.
    Look at $RELEASES/tag/$TAG"

# ── verify ───────────────────────────────────────────────────────────────────
# A checksum from the same release is not a signature and does not defend
# against a compromised release. It does catch a truncated download, a mirror
# serving stale bytes and a corrupted transfer, which is what actually happens.
step "Verifying"
if fetch_to "$BASE/SHA256SUMS-$VERSION" "$TMP/SHA256SUMS" 2>/dev/null; then
    expected=$(sed -n "s#.*[[:space:]]\./\{0,1\}$PKG\$#&#p" "$TMP/SHA256SUMS" | awk '{print $1}' | head -1)
    if [ -n "$expected" ]; then
        actual=$(sha256sum "$TMP/$PKG" | awk '{print $1}')
        [ "$expected" = "$actual" ] || die "Checksum mismatch for $PKG.
    expected $expected
    got      $actual
    Do not install this. Report it at https://github.com/$REPO/issues"
        dim "sha256 ok"
    else
        warn "No checksum listed for $PKG; continuing."
    fi
else
    warn "No SHA256SUMS in this release; continuing unverified."
fi

# ── install ──────────────────────────────────────────────────────────────────
step "Installing"
case "$FAMILY" in
    debian) run $SUDO apt-get install --yes "$TMP/$PKG" ;;
    fedora) run $SUDO dnf install -y "$TMP/$PKG" ;;
    arch)   run $SUDO pacman -U --noconfirm "$TMP/$PKG" ;;
    suse)   run $SUDO zypper --non-interactive install --allow-unsigned-rpm "$TMP/$PKG" ;;
esac

# ── wire it up ───────────────────────────────────────────────────────────────
step "Enabling the service"
run $SUDO systemctl daemon-reload
run $SUDO systemctl enable --now alien-daemon.service

if [ "$WANT_COOLING" = 1 ]; then
    dim "and the temperature curve"
    run $SUDO systemctl enable --now alien-cooling.service
fi

step "Granting hardware access to $TARGET_USER"
run $SUDO usermod -aG alien "$TARGET_USER"

# ── did it actually work? ────────────────────────────────────────────────────
if [ "$DRY_RUN" = 0 ]; then
    step "Checking"
    sleep 2
    if systemctl is-active --quiet alien-daemon.service; then
        dim "alien-daemon is running"
    else
        warn "alien-daemon did not start."
        dim "See why with:  $SUDO systemctl status alien-daemon --no-pager -l"
        dim "Most often this is the acpi_call module — see the Secure Boot note above."
    fi
fi

# ── the one thing left ───────────────────────────────────────────────────────
say ""
say "  ${G}${B}Alien is installed.${N}"
say ""
say "  ${B}${Y}One thing left: log out and back in.${N}"
dim "Group membership does not reach a session that is already running, so"
dim "until you do, every Alien command will say it cannot reach the daemon."
say ""
say "  Then try:"
dim "alien doctor        what your machine supports"
dim "alien status        temperatures, fan RPM, clocks"
dim "alien fan max       both fans to maximum"
dim "alien-gui           the desktop app"
say ""
if [ "$WANT_COOLING" != 1 ]; then
    say "  Want a real temperature curve instead of max-or-nothing?"
    dim "$SUDO systemctl enable --now alien-cooling"
    say ""
fi
say "  ${D}Laptop not fully supported? Run 'alien doctor' and open a report:${N}"
dim "https://github.com/$REPO/issues/new?template=hardware-support.yml"
say ""
