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
        with CPU/GPU policy unchanged) measured roughly +48% sustained CPU
        throughput on a Helios 300 PH315-53. It is loud. That is the trade.
      '';
    };
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
