# Feature parity

What PredatorSense, facer and Linuwu-Sense do, and where Alien stands against
each. Written to be checkable rather than asserted: where Alien does not have
something, it says so, and where something cannot work on a given machine it
says why.

Legend: **yes** · **no** · *n/a* (hardware cannot) · ~partial~

## Core hardware control

| | PredatorSense (Win) | facer | Linuwu-Sense | **Alien** |
|---|---|---|---|---|
| Fan: automatic / maximum | yes | ~model-gated~ | yes | **yes** |
| Fan: manual per-fan duty | yes | no | yes | **yes**, confirmed by firmware readback |
| Fan: custom curve | yes | no | ~fan table~ | **no** — see below |
| Fan RPM telemetry | yes | no | yes | **yes** |
| CPU / GPU / board temperature | yes | no | ~partial~ | **yes**, all three, ground-truthed |
| Turbo / OC flags | yes | yes | yes | **yes**, and honest about inertness |
| GPU overclock (MHz offsets) | yes | no | no | **no** — see below |
| CPU "turbo" (Intel XTU) | yes | no | no | **no** — see below |
| Keyboard: 4-zone static | yes | yes | yes | **yes**, each zone independently |
| Keyboard: effects | yes | yes | yes | **yes**, all 7 |
| Keyboard: per-key colour | yes, on per-key models | no | no | **yes**, on per-key models |
| Backlight timeout | yes | no | yes | **no** — not exposed on this model |
| Battery charge limiter | yes | no | yes | **no** — sub-index rejected here |
| USB charging while off | yes | no | yes | **no** — not exposed on this model |
| LCD overdrive | yes | no | yes | **no** — not exposed on this model |
| Boot animation / sound | yes | no | yes | **no** — cosmetic firmware setting |

## Where Alien goes further

| | PredatorSense | facer | Linuwu-Sense | **Alien** |
|---|---|---|---|---|
| Runs on Linux | no | yes | yes | **yes** |
| CLI | no | ~sysfs~ | ~sysfs~ | **yes** |
| Terminal UI | no | no | no | **yes** |
| Desktop GUI | yes | no | no | **yes** |
| Unprivileged frontends | n/a | no (root sysfs) | no (root sysfs) | **yes**, group-scoped socket |
| Serialised firmware access | unknown | no | no | **yes**, and it matters — see below |
| Capability probing per model | no (ships dead controls) | no | no | **yes** |
| Refuses the persistent-CMOS hazard | n/a | no | no | **yes**, allowlisted |
| Profiles | yes | no | no | **yes**, built-in + user TOML |
| Machine-readable output | no | ~sysfs~ | ~sysfs~ | **yes** (`alien json`) |
| Diagnostics for bug reports | no | no | no | **yes** (`alien doctor`) |

### Serialised access is a correctness feature, not a nicety

`/proc/acpi/call` is a single global kernel buffer and write-then-read is not
atomic. Two processes using it directly interleave — one writes, the other
writes, the first reads the *other's* answer — with no error anywhere. Every
tool that pokes it from userspace without a lock has this bug latent. Alien's
daemon owns the file behind a mutex; measured under six concurrent clients,
150 responses, zero cross-talk.

### Honest about what does nothing

PredatorSense shows an Overclocking tab on machines where `Feature.ini`
disables CPU overclock and the switch is inert. Alien reports
*accepted-but-unverified* and explains why. That is the difference this project
is actually built around.

## Deliberately not implemented, with reasons

**Fan curves.** Genuinely wanted, and the pieces exist — manual per-fan duty is
verified working, and a userspace curve daemon on top would be straightforward.
It is not shipped because a curve daemon that gets hysteresis wrong oscillates
the fans audibly, and it needs sustained thermal testing to tune, not a
one-session implementation. Next feature in.

**GPU overclock MHz offsets.** The path is real: function 22 with the offsets
PredatorSense reads from `PredatorSense.ini [OC_GPU]`. Not implemented because
pushing clock offsets into a GPU is the one operation here that can damage
hardware or corrupt a session, and it cannot be validated safely on a single
machine. If it lands it will be behind an explicit opt-in.

**Intel XTU power limits.** What the vendor markets as "CPU turbo" on Intel
machines is PL1/PL2 through `XtuService.exe` and `iocbios2.sys`, not this WMI
interface. On Linux that territory belongs to `intel-undervolt` and the RAPL
sysfs knobs; duplicating it inside a vendor-WMI tool would be the wrong home
for it.

**Backlight timeout, battery limiter, USB charging, LCD overdrive.** Real
features on models that expose them, and Linuwu-Sense implements them. On the
PH315-53 the corresponding misc-setting sub-indices are **rejected by the
firmware** — probed read-only, sub-indices 0x01, 0x02, 0x05 and 0x07 are the
only ones accepted. Implementing blind against a machine that refuses them
would mean shipping controls nobody could test. Reports from models where they
work are wanted; the capability probe already has the shape to expose them.

**OSD overlay, GameSync, AppCenter.** PredatorSense bundles an on-screen
display, a refresh-rate/monitor pairing feature, and a launcher for Acer's
other software. The first is a compositor's job on Linux, and the other two
have no Linux counterpart to talk to.

## Reports wanted

The questions the reference machine cannot answer:

1. Does the **CPU overclock flag** do anything where `Feature.ini` enables it?
2. Do the keypress-reactive effects (shifting, zoom, ripple) behave on other
   models? They are accepted here and the four-zone hardware gives them little
   to work with.

`alien capabilities` and `alien doctor` print everything a report needs.
