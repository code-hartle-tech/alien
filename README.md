# Alien

**Fan, lighting, turbo and telemetry control for Acer Predator and Nitro
laptops on Linux — no vendor software, no Windows.**

A clean-room implementation of Acer's gaming WMI protocol, with a CLI, a
terminal UI and a desktop app. Every constant in it was verified against real
firmware; where a control is accepted by the firmware but has no observable
effect on a model, this software says so instead of implying it works.

Named after the other franchise.

```
alien fan max                 # both fans to maximum
alien status                  # temperatures, RPM, turbo, backlight
alien rgb '#00aec7'           # keyboard colour
alien profile apply silent    # hand the fans back to the EC
alien doctor                  # is this machine supported?
```

---

## Why bother

On the reference machine — a Helios 300 PH315-53 — **the stock fan curve costs
about a third of the CPU.** Same benchmark, same session, fans the only
variable:

| | 7-zip (MIPS) | GPU idle |
|---|---|---|
| EC automatic curve | 14,492 / 11,603 (~26.1k) | 86–89 °C |
| fans forced to maximum | **20,961 / 17,708 (~38.7k)** | **81 °C** |

**≈ +48%.** Published figures for an unthrottled i7-10750H are 35–45k, so the
stock curve was holding the chip in thermal throttle and every other tuning
knob on the machine was measuring a throttled processor.

That is the headline feature. Everything else is convenience.

## Install

The privileged helper, `alien-daemon`, must come from a native package — it
needs root and the out-of-tree `acpi_call` module, which no sandbox can
provide. The frontends can come from anywhere.

| | |
|---|---|
| Arch | `packaging/arch/PKGBUILD` |
| Debian / Ubuntu | `packaging/debian` → `alien-predator.deb` |
| NixOS | `services.alien.enable = true;` (flake included) |
| Flatpak | `tech.hartle.Alien` — GUI only, daemon from a native package |
| Snap | `alien-predator` — frontends only |
| Docker | CLI only, needs `--privileged` |
| Anything else | static binaries from the release page |

Then:

```sh
sudo systemctl enable --now alien-daemon
sudo gpasswd -a "$USER" alien
```

**Log out and back in.** Group changes do not reach an already-running desktop
session, and the GUI will refuse to start until they do.

## How it works

One WMI GUID (`7A4DDFE7-5B5D-40B4-8595-4408E0CC7F56`) exposes everything. It
maps to an ACPI method — on our reference machine `\_SB.PCI0.WMID.WMBH`,
declared in **SSDT12**, not the DSDT. Anyone grepping only the DSDT concludes
the interface does not exist.

`alien-daemon` owns that interface and serves everything else over a Unix
socket at `/run/alien/alien.sock`, mode `0660`, group `alien`. That is not only
about privilege: **`/proc/acpi/call` is a single global kernel buffer and
write-then-read is not atomic**, so two processes using it directly interleave
— one writes, the other writes, the first reads the *other's* answer, with no
error anywhere. A single owner behind a mutex is the only way concurrent use is
safe.

The daemon is not a general-purpose firmware proxy. Every request is checked
against an allowlist of the functions Alien actually uses, and the misc-setting
sub-index that writes persistent CMOS is refused outright.

Full protocol notes, including the traps: [`docs/protocol.md`](docs/protocol.md).
Feature-by-feature comparison against PredatorSense, facer and Linuwu-Sense:
[`docs/parity.md`](docs/parity.md).

## What works, and what does not

Being specific about this is the point of the project.

**Verified working** — fan max / auto / per-fan manual duty, fan RPM and
temperature telemetry, four-zone keyboard colour (each zone independently),
backlight effects, brightness and speed, turbo flag get/set.

Lighting is verified *by looking at the keyboard*, not by readback. That
distinction cost this project a long detour: an eight-byte effect payload
(the real one is sixteen) was accepted, stored and read back perfectly while
lighting nothing at all.

**Accepted by firmware, no observable effect on PH315-53:**

- *CPU overclock.* PredatorSense gates it on `Feature.ini OverclockSupport
  CPU`, which is `0` for this model — the firmware write is genuinely inert.
  What the vendor calls "CPU turbo" on Intel machines is Intel XTU power limits
  (PL1/PL2), not this interface. GPU overclock does go through function 22.

**Not achievable at all on this model**, documented so nobody re-runs it: the
GPU power limit is VBIOS-locked (`nvidia-smi -pl` returns "not supported"), CPU
undervolt needs MSR `0x150` which firmware can lock, and RAPL limits already
sit far above what the chassis sustains — the EC owns the envelope.

## Other models

The function numbers are believed common across the Acer gaming WMI interface,
but the fan bitmask, the sensor ids and the turbo values are worth confirming
per model. `alien doctor` prints everything a bug report needs. Reports
welcome — especially from machines where CPU overclock *does* work, since that
is the one place our reference machine goes quiet.

## Safety

Every call Alien makes is one the vendor's own software issues. A non-zero
status byte means "firmware rejected", not "damaged", and the fans can always
be handed back with `alien fan auto`.

Two genuine hazards exist, and Alien refuses both: misc-setting **sub-index 6**
writes a byte that survives power cycles, and raw EC register writes, which
this project never does — everything goes through the firmware's own
mutex-guarded methods.

## Licence

GPL-2.0-or-later. Not affiliated with Acer; see [NOTICE](NOTICE) for the
interoperability position and credit to the prior reverse-engineering work this
builds on.
