# Alien

<p align="center">
  <img src="assets/tech.hartle.Alien.svg" width="240" alt="Alien project mark">
</p>

<p align="center">
  <strong>Acer Predator hardware control for Linux.</strong><br>
  Fans, lighting, telemetry and firmware features — no Windows, no vendor binaries.
</p>

<p align="center">
  <a href="https://github.com/code-hartle-tech/alien/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/code-hartle-tech/alien/actions/workflows/ci.yml/badge.svg"></a>
  <img alt="Alien 0.5.0" src="https://img.shields.io/badge/release-0.5.0-3fe86c">
  <a href="LICENSE"><img alt="GPL-2.0-or-later" src="https://img.shields.io/badge/license-GPL--2.0--or--later-3fe86c"></a>
  <img alt="Rust 1.88 or newer" src="https://img.shields.io/badge/Rust-1.88%2B-53d8ff?logo=rust&logoColor=white">
  <img alt="Linux x86-64" src="https://img.shields.io/badge/platform-Linux%20x86--64-ffb000?logo=linux&logoColor=white">
</p>

<p align="center">
  <a href="docs/media/README.md"><img src="docs/media/demo/alien-demo.gif" width="960" alt="Alien GUI and terminal UI demonstration"></a>
</p>

## The measurement that started this

The stock firmware fan curve holds this chip in thermal throttle. Measured on a
Predator Helios 300 PH315-53 with a proper A/B/A protocol — four twenty-minute
blocks, first half of each discarded, run order alternated so drift is separable
from effect:

| Fan state | 7-Zip | CPU clock under load | Chassis |
|---|---:|---:|---:|
| Firmware automatic curve | 26 721 MIPS | **1 446 MHz** | 83.4 °C |
| Fans at maximum | **43 232 MIPS** | **2 406 MHz** | 77.7 °C |

**+61.8 % sustained throughput.** Same-condition drift across the repeated
blocks was 9.0 % and 7.5 %, so the effect is roughly seven times the noise.

Both conditions pin the CPU at 92 °C. The difference is the clock underneath it:
the firmware curve never ramps past ~3 500 RPM, even at 95 °C, even under
sustained load.

That is one laptop and one workload. It is also why this project exists.

## What you get

| | |
|---|---|
| **Fan control** | Firmware-auto, maximum, per-fan manual duty, and independent per-fan modes — CPU on the firmware curve while the GPU runs pinned |
| **A real fan curve** | `alien-cooling` — a stepped controller with hysteresis, dwell timers and a critical latch. Quiet at idle, ramps before the chassis saturates |
| **Limiter attribution** | `alien limiters` tells you *which* cap is binding. No other tool on either OS reports this |
| **Crash-safe game profiles** | Apply a profile for a game and always get it back — even if the game is SIGKILLed, or the machine suspends mid-session |
| **Telemetry** | Five sensors, both fan RPMs, live CPU/GPU clocks, RAPL power state |
| **Four-zone lighting** | Per-zone colour and brightness, plus Breathing, Wave, Zoom, Shifting and Neon |
| **Guarded firmware features** | Typed CoolBoost, LCD overdrive, and an exact-target GPU-mode transaction with full readback and rollback |
| **Three frontends** | Desktop app, SSH-friendly TUI, scriptable CLI — one protocol library underneath |

## See it

| Desktop control centre | Keyboard-first terminal UI |
|---|---|
| [![Alien dashboard](docs/media/screenshots/alien-gui-dashboard.png)](docs/media/screenshots/alien-gui-dashboard.png) | [![Alien terminal UI](docs/media/screenshots/alien-tui-rich.png)](docs/media/screenshots/alien-tui-rich.png) |

[Every full-resolution screen and the MP4/WebM walkthroughs →](docs/media/README.md)

## What makes it different

**It tells you when something did nothing.** This firmware is full of calls that
return success and change no hardware. Alien distinguishes *the firmware
accepted it*, *a getter confirmed it*, and *we measured it physically* — and
says which one you have.

Three controls on the reference machine measured **worse than nothing**, and all
three are documented as such rather than shipped as features:

- GPU clock offsets: `855/853/855/830` — no speed-up
- CoolBoost: no sustained cooling benefit at the tested operating point
- Raising the thermal ceiling (TCC offset 8 → 0): **−45 %**, because it hands
  control to the EC's cruder protection

**Failure escalates loudly.** Handing the fans back to the firmware isn't a safe
state here — it's the state that permits 95 °C and 1 446 MHz. Every failure path
in the controller goes to maximum. The kernel agrees: `acer-wmi` maps
`pwm_enable = 0` to `ACER_WMID_FAN_MODE_TURBO`.

**Restore actually survives.** gamemode keeps its restore baseline in RAM, so a
SIGKILL strands your machine. Alien writes the baseline to disk *before* touching
hardware, and recovers on next start.

## Compatibility

Evidence-tiered, deliberately:

| Tier | Count | Meaning |
|---|---:|---|
| **Live-verified** | 1 | Predator PH315-53, BIOS V1.07, exercised on real hardware |
| **Package-mapped** | 18 | PredatorSense plug-ins identify these model strings; 17 still need hardware receipts |
| **Candidate** | 36 | Associated with PredatorSense by Acer's own sources; Alien mapping unverified |

Run `alien doctor` on the actual machine before any setter. Catalog membership
is never a runtime safety bypass. → [full catalog](docs/model-compatibility.md)

## Install

`alien-daemon` runs on the host as root with the out-of-tree `acpi_call` module.
Every frontend stays unprivileged and talks to its group-owned Unix socket.

### NixOS

```nix
{
  inputs.alien.url = "github:code-hartle-tech/alien";

  outputs = { nixpkgs, alien, ... }: {
    nixosConfigurations.my-host = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        alien.nixosModules.default
        { services.alien = { enable = true; users = [ "alice" ]; }; }
      ];
    };
  };
}
```

Rebuild, then log out and back in so the `alien` group reaches your session.

### Anywhere else

```sh
cd code && cargo build --release --locked --workspace
```

Needs Rust 1.88+. You'll also need the `alien` group, the `acpi_call` module,
the hardened systemd unit and correct socket ownership — don't run a frontend as
root instead. The Arch, Debian, Flatpak, Snap and Docker files under
[`packaging/`](packaging/README.md) are maintainer references, not published
binaries yet.

## Use it

```sh
alien doctor                  # is this machine supported, and what's gated
alien status                  # temperatures, RPM, clocks, power
alien limiters watch 60       # which cap is actually binding
alien fan max                 # both fans to maximum
alien fan cpu auto            # one fan to the firmware curve, other untouched
alien fan gpu 70              # manual duty, one fan
alien profile apply silent    # named profile
alien gamesync begin performance   # apply with a crash-safe restore recorded
alien gaming launch-options   # the Steam launch line worth pasting
```

`alien-gui` for the desktop app, `alien-tui` over SSH. `alien --help` for
everything.

## How it fits together

```text
alien-gui ─┐
alien-tui ─┼─► alien-core ─► /run/alien/alien.sock ─► alien-daemon
alien CLI ─┤                                      │
cooling  ──┘                                      ├─► serialized acpi_call
                                                  │    └─► WMI / ACPI ─► SMM / EC
                                                  └─► exact-target NVML transaction
```

The daemon is not a raw firmware proxy — its allowlist accepts only
characterized operations with exact payload and reply shapes. It also has to be
a single owner, because `/proc/acpi/call` is one global kernel buffer whose
write-then-read is not atomic: two callers interleave and each reads the other's
answer, with no error anywhere.

## Safety

- No arbitrary WMI passthrough, no raw EC access
- Misc-setting sub-index 6 is denied — it reaches a byte that survives power cycles
- The GPU transaction is gated to one exact DMI/PCI/BIOS target, checks live
  driver ranges, requires an explicit risk acknowledgement, reads every leg back
  and rolls back in reverse on partial failure
- `alien` group membership is real hardware authority — grant it deliberately

## Open gates

Fan control, telemetry and per-zone colour have hardware confirmation on the
live reference. Two claims stay open until physical optics exist: the full
keyboard effect matrix needs a camera framing the keyboard, and LCD overdrive
needs a 240 fps camera on the panel. A screenshot proves rendering, not light.

## Read further

- [**Ghost in the firmware** — how PredatorSense was reverse-engineered](docs/reverse-engineering-predatorsense.md)
- [Protocol notes: WMI, ACPI, lighting, GPU modes](docs/protocol.md)
- [Feature parity and deliberate limits](docs/parity.md)
- [Model evidence catalog](docs/model-compatibility.md)
- [Media kit](docs/media/README.md) · [Packaging](packaging/README.md) · [Security](SECURITY.md)

## Contributing

Hardware reports are the most valuable thing you can send. Start with `alien
doctor`, keep read-only evidence separate from physical observation, and never
attach Acer installers, firmware or decompiler output.
→ [CONTRIBUTING.md](CONTRIBUTING.md)

## License

[GPL-2.0-or-later](LICENSE). An independent interoperability project, not
affiliated with or endorsed by Acer. No Acer software, artwork, firmware or
binaries are distributed here — see [NOTICE](NOTICE).
