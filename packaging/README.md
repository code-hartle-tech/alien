# Packaging Alien

Six targets, one binary set: `alien` (CLI), `alien-tui`, `alien-gui` and
`alien-daemon`.

## The constraint that shapes all of this

`alien-daemon` needs root and the `acpi_call` kernel module. **It cannot live
inside a sandbox.** A flatpak or snap has no path to `/proc/acpi/call` at any
privilege level, and no amount of permission granting changes that — the file
is not in the sandbox's mount namespace, and even bind-mounting it in would
leave the writes unprivileged.

So the split is fixed:

| Component | Where it must be installed |
|---|---|
| `alien-daemon` | **the host**, as a system service |
| `alien`, `alien-tui`, `alien-gui` | anywhere — they only need the socket |

Flatpak and Snap therefore ship the **frontends only**, and both are useless
until the daemon is installed on the host from a native package. Every
sandboxed build says so on first run rather than failing with a bare
permission error.

This is the same shape CoolerControl and similar hardware tools settled on,
for the same reason.

## Targets

| Directory | Produces | Ships |
|---|---|---|
| `arch/` | `PKGBUILD` | everything, incl. daemon |
| `debian/` | `.deb` (Debian, Ubuntu, Mint…) | everything, incl. daemon |
| `nixos/` | flake + NixOS module | everything, incl. daemon |
| `flatpak/` | `tech.hartle.Alien` | GUI only |
| `snap/` | `alien` snap | frontends only |
| `docker/` | image | CLI only, `--privileged` |
| `../code/tools/mkrelease.sh` | source tar.gz + static binaries | — |

## Docker is the odd one

A container can reach `/proc/acpi/call` only with `--privileged` and the host's
`/proc` mounted, which is most of the isolation gone. It is included because it
is genuinely useful for CI and for one-shot `alien fan max` on a headless box,
not because it is a sensible way to run a desktop tool. The Dockerfile says so.

## Group membership takes effect at next login

Adding a user to `alien` does not change an already-running desktop session.
Every package prints this at install time, because the failure mode otherwise
is a GUI that refuses to start with a permission error minutes after the user
"already did that step".
