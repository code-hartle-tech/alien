# Fedora / RHEL / openSUSE spec for Alien.
#
# Built from the vendored source tarball produced by
# `code/tools/mkrelease.sh --vendor`, because a package build must not reach
# the network and Fedora's own builders have none.

%global forgeurl https://github.com/code-hartle-tech/alien
%global debug_package %{nil}

Name:           alien-predator
Version:        0.6.0
Release:        1%{?dist}
Summary:        Fan, lighting, cooling and telemetry control for Acer Predator systems

License:        GPL-2.0-or-later
URL:            https://alien.hartle.tech
Source0:        alien-%{version}.tar.gz

ExclusiveArch:  x86_64

BuildRequires:  cargo >= 1.88
BuildRequires:  rust >= 1.88
BuildRequires:  pkgconfig
BuildRequires:  systemd-rpm-macros
BuildRequires:  pkgconfig(egl)
BuildRequires:  pkgconfig(gl)
BuildRequires:  pkgconfig(wayland-client)
BuildRequires:  pkgconfig(x11)
BuildRequires:  pkgconfig(xcursor)
BuildRequires:  pkgconfig(xi)
BuildRequires:  pkgconfig(xkbcommon)
BuildRequires:  pkgconfig(xrandr)

# acpi_call is NOT in Fedora proper — it lives in RPM Fusion as akmod-acpi_call.
# A hard Requires would make this package uninstallable on a stock Fedora, so it
# is a Recommends and the daemon reports the missing module by name at startup
# instead of failing obscurely.
Recommends:     akmod-acpi_call
Recommends:     mesa-libGL
Recommends:     libxkbcommon
Suggests:       xorg-x11-drv-nvidia-cuda-libs

%description
Fan control, keyboard lighting, a real temperature curve, guarded performance
controls and honest telemetry on Acer gaming laptops — with no vendor software
and no Windows.

An independent, from-scratch interoperability implementation of the Acer gaming
WMI protocol, verified against real firmware. Where a control is accepted by the
firmware but has no observable effect on a given model, this software says so
rather than implying it works.

Ships a CLI (alien), a terminal UI (alien-tui), a desktop app (alien-gui), the
cooling-curve controller (alien-cooling) and the privileged helper
(alien-daemon) that owns the firmware interface so every frontend stays
unprivileged.

%prep
%autosetup -n alien-%{version}

%build
cd code
cargo build --release --locked --workspace --offline

%check
cd code
# Pure-logic tests only. Anything touching firmware needs root and real
# hardware, neither of which a package build may assume.
cargo test --release --locked --workspace --offline

%install
install -Dm755 code/target/release/alien         %{buildroot}%{_bindir}/alien
install -Dm755 code/target/release/alien-tui     %{buildroot}%{_bindir}/alien-tui
install -Dm755 code/target/release/alien-gui     %{buildroot}%{_bindir}/alien-gui
install -Dm755 code/target/release/alien-daemon  %{buildroot}%{_bindir}/alien-daemon
install -Dm755 code/target/release/alien-cooling %{buildroot}%{_bindir}/alien-cooling
install -Dm755 code/tools/alien-launch           %{buildroot}%{_bindir}/alien-launch

install -Dm644 packaging/systemd/alien-daemon.service \
  %{buildroot}%{_unitdir}/alien-daemon.service
install -Dm644 packaging/systemd/alien-cooling.service \
  %{buildroot}%{_unitdir}/alien-cooling.service
install -Dm755 packaging/systemd/alien-sleep.sh \
  %{buildroot}%{_prefix}/lib/systemd/system-sleep/alien

install -Dm644 packaging/shared/alien.sysusers \
  %{buildroot}%{_sysusersdir}/alien.conf
install -Dm644 packaging/shared/60-alien.rules \
  %{buildroot}%{_udevrulesdir}/60-alien.rules
install -Dm644 packaging/shared/alien.modules-load.conf \
  %{buildroot}%{_prefix}/lib/modules-load.d/alien.conf
install -Dm644 packaging/shared/tech.hartle.Alien.desktop \
  %{buildroot}%{_datadir}/applications/tech.hartle.Alien.desktop
install -Dm644 assets/tech.hartle.Alien.svg \
  %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/tech.hartle.Alien.svg

install -Dm644 NOTICE  %{buildroot}%{_datadir}/doc/%{name}/NOTICE
install -Dm644 code/alien-gui/assets/fonts/Archivo-OFL.txt \
  %{buildroot}%{_datadir}/licenses/%{name}/Archivo-OFL.txt
install -Dm644 code/alien-gui/assets/fonts/IBMPlex-OFL.txt \
  %{buildroot}%{_datadir}/licenses/%{name}/IBMPlex-OFL.txt

%pre
%sysusers_create_compat %{_sysusersdir}/alien.conf

# Installed but NOT started. The daemon writes to firmware, and a package
# install is not the moment to start doing that unasked.
%post
%systemd_post alien-daemon.service alien-cooling.service
if [ $1 -eq 1 ]; then
  cat <<'EOF'

  Alien is installed but not running yet.

    sudo systemctl enable --now alien-daemon
    sudo usermod -aG alien "$USER"

  Then LOG OUT AND BACK IN. Group membership does not reach a session that is
  already running, and the GUI will refuse to start until it does.

  The kernel module acpi_call is required and is not in Fedora proper:
    sudo dnf install akmod-acpi_call        # needs RPM Fusion free

  Optional: sudo systemctl enable --now alien-cooling   (temperature curve)

EOF
fi

%preun
%systemd_preun alien-daemon.service alien-cooling.service

%postun
%systemd_postun_with_restart alien-daemon.service

%files
%license LICENSE
%doc README.md
%{_bindir}/alien
%{_bindir}/alien-tui
%{_bindir}/alien-gui
%{_bindir}/alien-daemon
%{_bindir}/alien-cooling
%{_bindir}/alien-launch
%{_unitdir}/alien-daemon.service
%{_unitdir}/alien-cooling.service
%{_prefix}/lib/systemd/system-sleep/alien
%{_sysusersdir}/alien.conf
%{_udevrulesdir}/60-alien.rules
%{_prefix}/lib/modules-load.d/alien.conf
%{_datadir}/applications/tech.hartle.Alien.desktop
%{_datadir}/icons/hicolor/scalable/apps/tech.hartle.Alien.svg
%{_datadir}/doc/%{name}/NOTICE
%{_datadir}/licenses/%{name}/Archivo-OFL.txt
%{_datadir}/licenses/%{name}/IBMPlex-OFL.txt

%changelog
* Sun Aug 16 2026 HARTLE.TECH <contact@hartle.tech> - 0.6.0-1
- First RPM packaging. Ships the cooling-curve controller and the suspend hook
  alongside the CLI, TUI, GUI and daemon.
