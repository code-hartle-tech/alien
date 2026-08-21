# NixOS module for Alien.
#
# Everything the other packages do in postinst scripts is declarative here: the
# group, the kernel module, the service and its hardening. Enabling this module
# and adding yourself to `alien` is the whole installation.
{ config, lib, pkgs, ... }:

let
  cfg = config.services.alien;
in
{
  options.services.alien = {
    enable = lib.mkEnableOption "Alien — Acer Predator hardware control";

    package = lib.mkOption {
      type = lib.types.package;
      description = "The alien package to use.";
    };

    users = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      example = [ "alice" ];
      description = ''
        Users to add to the `alien` group, who may then control fans, keyboard
        lighting and privileged firmware features. On the exact supported
        PH315-53 target this also includes explicitly acknowledged, unsupported
        NVIDIA P0 clock offsets. This is a real capability — grant it as
        deliberately as any other hardware-control group.

        Note that on a rebuild the change applies to new logins only; an
        already-running desktop session keeps the groups it started with.
      '';
    };

    profileOnBoot = lib.mkOption {
      type = lib.types.nullOr (lib.types.enum [ "silent" "performance" "turbo" ]);
      default = null;
      example = "performance";
      description = ''
        Apply a profile once at boot.

        Worth considering on a machine that lives on AC: the stock EC fan curve
        holds the CPU in thermal throttle, and `performance` (fans at maximum,
        with CPU/GPU policy unchanged) measured **+61.8%** sustained CPU
        throughput on a Helios 300 PH315-53 — 26,721 to 43,232 MIPS across a
        4x20-minute ABBA run with the first half of each block discarded, against
        same-condition drift of 9.0% and 7.5%. It is loud. That is the trade.

        `services.alien.cooling.enable` is usually the better answer: it buys
        most of the same headroom without running the fans flat out all day.
      '';
    };

    cooling.enable = lib.mkEnableOption ''
      the temperature-driven fan curve.

      The alternative to `profileOnBoot = "performance"`, which pins both fans
      at maximum and leaves them there. This runs a real curve instead —
      stepped thresholds with asymmetric up/down hysteresis, dwell timers, a
      critical latch and boot-into-failsafe — so the machine is quiet at idle
      and ramps before the chassis saturates.

      Deliberately not a PID: the plant has roughly a 10:1 gain variation
      across the duty range, integer-degree sensors destroy the derivative
      term, and a 92 degree thermal pin guarantees integral windup
    '';
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];

    # acpi_call is out of tree and not autoloaded.
    boot.extraModulePackages = [ config.boot.kernelPackages.acpi_call ];
    boot.kernelModules = [ "acpi_call" ];

    users.groups.alien = { };
    users.users = lib.genAttrs cfg.users (_: { extraGroups = [ "alien" ]; });

    systemd.services.alien-daemon = {
      description = "Alien — Acer Predator hardware control daemon";
      wantedBy = [ "multi-user.target" ];
      after = [ "systemd-modules-load.service" ];
      wants = [ "systemd-modules-load.service" ];

      serviceConfig = {
        Type = "exec";
        ExecStart = "${cfg.package}/bin/alien-daemon";
        Restart = "on-failure";
        RestartSec = 2;

        RuntimeDirectory = "alien";
        RuntimeDirectoryMode = "0755";

        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        PrivateNetwork = true;
        RestrictAddressFamilies = [ "AF_UNIX" ];
        RestrictNamespaces = true;
        RestrictRealtime = true;
        RestrictSUIDSGID = true;
        LockPersonality = true;
        MemoryDenyWriteExecute = true;
        SystemCallArchitectures = "native";
        # @chown must come back after ~@privileged strips it: giving the socket
        # to group `alien` is the daemon's one privileged syscall class, and
        # without it seccomp raises SIGSYS and the process core-dumps at startup
        # rather than degrading gracefully. CAP_SYS_ADMIN below is a driver
        # authorization check, not permission for another syscall class.
        SystemCallFilter = [ "@system-service" "~@privileged @resources @obsolete" "@chown" ];
        # NVIDIA's open kernel driver authorizes NVML clock-offset setters with
        # capable(CAP_SYS_ADMIN). Keep the bounding set to only that driver
        # requirement plus CAP_CHOWN for the group-owned Unix socket; all
        # other service sandboxing remains in force.
        CapabilityBoundingSet = [ "CAP_CHOWN" "CAP_SYS_ADMIN" ];

        # NOT set, deliberately: ProtectKernelTunables would make
        # /proc/acpi/call read-only, which is the entire job.
      };
    };

    # The curve controller. Strictly downstream of the broker: an ordinary
    # socket client with no firmware access of its own, which is why it can be
    # locked down much harder than the daemon it talks to.
    systemd.services.alien-cooling = lib.mkIf cfg.cooling.enable {
      description = "Alien — temperature-driven fan curve controller";
      wantedBy = [ "multi-user.target" ];
      after = [ "alien-daemon.service" ];
      # `requires`, not `wants`: a curve controller that cannot reach the
      # daemon has nothing useful to do.
      requires = [ "alien-daemon.service" ];

      serviceConfig = {
        Type = "exec";
        ExecStart = "${cfg.package}/bin/alien-cooling";
        Restart = "on-failure";
        RestartSec = 2;

        # The restore layer that survives everything. The in-process signal
        # handler covers SIGTERM and SIGINT, but the release profile builds
        # with `panic = "abort"`, so a panic runs no Rust cleanup at all — and
        # SIGKILL never does. Without this line either case leaves the fans
        # pinned in manual forever, because this hardware has no EC watchdog
        # to wind them back.
        ExecStopPost = [ "-${cfg.package}/bin/alien fan max" ];

        # No root, no capabilities, no kernel module: group `alien` is the
        # entire privilege boundary, exactly as for the GUI and TUI.
        DynamicUser = true;
        SupplementaryGroups = [ "alien" ];

        # Never let Device::open fall back to /proc/acpi/call. If this service
        # won the boot race and took the lock, alien-daemon would then fail to
        # start with InterfaceBusy — and the fallback needs root anyway, which
        # DynamicUser denies, so the failure would be confusing rather than
        # loud.
        Environment = [ "ALIEN_REQUIRE_SOCKET=1" ];

        # Being reaped mid-flight is the one failure with no recovery path:
        # the fans stay wherever they were last set. The binary re-reads this
        # at startup and refuses to run if it did not take.
        OOMScoreAdjust = -1000;

        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        PrivateNetwork = true;
        PrivateDevices = true;
        RestrictAddressFamilies = [ "AF_UNIX" ];
        RestrictNamespaces = true;
        RestrictRealtime = true;
        RestrictSUIDSGID = true;
        LockPersonality = true;
        MemoryDenyWriteExecute = true;
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectControlGroups = true;
        SystemCallArchitectures = "native";
        SystemCallFilter = [ "@system-service" "~@privileged @resources @obsolete @debug @mount" ];
      };
    };

    systemd.services.alien-profile = lib.mkIf (cfg.profileOnBoot != null) {
      description = "Apply the Alien '${cfg.profileOnBoot}' profile at boot";
      wantedBy = [ "multi-user.target" ];
      after = [ "alien-daemon.service" ];
      requires = [ "alien-daemon.service" ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        ExecStart = "${cfg.package}/bin/alien profile apply ${cfg.profileOnBoot}";
        # Hand the fans back on the way down, so a machine that comes up
        # without this unit is not left screaming.
        ExecStop = "${cfg.package}/bin/alien fan auto";
      };
    };
  };
}
