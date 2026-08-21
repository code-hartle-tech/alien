# Packaging Alien

Six targets, one binary set: `alien` (CLI), `alien-tui`, `alien-gui` and
`alien-daemon`.

## The constraint that shapes all of this

`alien-daemon` needs root and the `acpi_call` kernel module. On the exact
PH315-53 GPU-mode path it also needs the host NVIDIA device nodes and the
driver's `CAP_SYS_ADMIN` authorization for clock-offset setters. **It cannot
live inside a sandbox.** A flatpak or snap has no path to `/proc/acpi/call` at
any privilege level, and no amount of permission granting changes that — the
file is not in the sandbox's mount namespace, and even bind-mounting it in
would leave the writes unprivileged.

So the split is fixed:

| Component | Where it must be installed |
|---|---|
| `alien-daemon` | **the host**, as a system service |
| `alien`, `alien-tui`, `alien-gui` | anywhere — they only need the socket |

Flatpak and Snap therefore ship the **frontends only**, and both are useless
until the daemon is installed on the host from a native package. Every
sandboxed build says so on first run rather than failing with a bare
permission error.

The guarded PH315-53 OEM GPU modes are optional. They appear only when the
exact PCI/DMI/BIOS checks pass and the host NVIDIA driver exports the current
NVML P0 clock-offset API. Arch users need `nvidia-utils`; Debian/Ubuntu driver
packages carry `libnvidia-ml.so.1` under versioned package names. Missing NVML
leaves fan, lighting, telemetry and firmware features working and reports GPU
mode unavailable rather than substituting another clock mechanism.

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
| `../code/tools/mkrelease.sh` | committed source tarball; optional vendoring/native binaries | — |

These are maintainer recipes, not advertised binary downloads. Start from a
clean committed tree and create their source input with:

```sh
code/tools/mkrelease.sh --vendor
```

That writes `dist/alien-<version>.tar.gz`. The vendored archive is the input
expected by the offline Debian rules and is also suitable for the Flatpak
manifest. For an Arch test build, copy the archive beside `PKGBUILD`, then run
`makepkg` in a clean chroot. The `SKIP` checksum is intentional only for this
maintainer handoff; a published package repository must pin the uploaded
release artifact's checksum before presenting the recipe to users.

## What actually builds in CI

`.github/workflows/release.yml` builds and attaches these on every tag:

| Target | State |
|---|---|
| Source tarball (vendored) | ✅ |
| Linux x86_64 binaries | ✅ |
| `.deb` | ✅ |
| `.rpm` | ✅ |
| Arch `.pkg.tar.zst` | ✅ |
| Container image (GHCR) | ✅ |
| Flatpak bundle | ✅ |
| Snap | ✅ |

### The snap builds now — and why it took five rounds

Fixed. It produces a ~3.3 MB frontends-only snap.

Three of the five attempts chased the wrong thing, for one reason worth
recording: **the test was never running the fix.** The release workflow's
dispatch input was `required: true`, so the only way to exercise it was against
an existing tag — and a tag checks out its own tree. Dispatching `v0.6.0` built
a manifest that still said `plugin: rust` and `version: '0.5.0'`, failed with
the errors those fixes had already removed, and read as "the fix did not work".
The input is optional now: empty builds the dispatched ref.

The two real bugs:

1. **`snapcraft --destructive-mode` was not run as root.** Destructive mode
   installs the part's `build-packages` with apt onto the runner itself, which
   needs root. Without it snapcraft reported:

       Cannot install all requested build packages: build-essential,
       ca-certificates, curl, gcc, git, libxkbcommon-dev, make, pkg-config

   That message lists every package and never mentions permissions, so it
   reads as a package-availability problem. Every one of those packages exists
   in noble; none of them was ever the issue. It is also why adding
   `apt-get update` changed nothing — the update ran under sudo and the
   install that mattered did not.

2. **`source: ../..` did not point at the repository.** Snapcraft resolves a
   relative source against the directory it is *invoked from*, not the one the
   manifest lives in. CI stages the manifest to `snap/snapcraft.yaml` and runs
   from the repository root, so `../..` pointed two levels above it and
   `source-subdir: code` then looked for a directory that was not there:

       cd: .../parts/alien/src/code: No such file or directory

   Now `source: .` with no `source-subdir`, because the install steps reach
   repository-root paths through `$CRAFT_PART_SRC`; aiming that at `code/`
   would fix the cargo build and break all nine of them.

The earlier two fixes were real and still stand: `plugin: nil` (the rust plugin
demands rustup or a `rust-deps` part, and this part drives cargo itself), and
`--destructive-mode` on `ubuntu-24.04` to sidestep snapcraft's own LXD instance
failing on core24 with `PermissionError: '/snap/core24/<rev>/dev/urandom'`.

Keep the value in perspective even so: the snap ships the **frontends only**,
because the daemon cannot live in a sandbox. Anyone who can use the snap has
already installed a native package to get the daemon, and that package contains
the frontends too.

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
