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
| Fan: Custom per-fan Auto/Manual duty | yes | no | yes | **yes**, confirmed by firmware readback |
| User-authored temperature curve | no, not in PH315-53 Covini Custom | no | ~fan table~ | **no** — optional Alien superset, not an OEM parity gap |
| CoolBoost | yes | no | no | **partial**, typed CLI/GUI/TUI + live getter/toggle/restore; PH315-53 setter reinit transient confirmed, controlled A/B/A found no sustained cooling lift |
| Fan RPM telemetry | yes | no | yes | **yes** |
| CPU / GPU / board temperature | yes | no | ~partial~ | **yes**, all three, ground-truthed |
| CPU / GPU clock dashboard | yes | no | no | **yes**, live Linux MHz measurements |
| Raw WMBH GPU flag, sub-index 5 | ~command-45 side effect~ | yes | yes | **manual read**, values 0/2; independent setters blocked because this is not an OEM OC mode |
| CPU WMI flag | yes | yes | yes | **read-only**, inert on PH315-53 |
| GPU overclock (MHz offsets) | yes | no | no | **yes on the exact PH315-53 target**, guarded Normal/Faster/Turbo transaction + readback/rollback; no measured speedup |
| CPU "turbo" (Intel XTU) | yes | no | no | **read-only status**, exact target + live named RAPL limits |
| Keyboard: 4-zone static | yes | yes | yes | **yes**, colour and enable state per zone |
| Keyboard: effects | yes | yes | yes | **partial**, exact PH315-53 Covini protocol implemented; optical revalidation pending |
| Keyboard: per-key colour | yes, on per-key models | no | no | **experimental/source-mapped** — ITE 8291 transport exists, but has no live hardware validation or packaged `hidraw` permission rule |
| Backlight timeout | yes | no | yes | **no on PH315-53**, proven fallback getter returns status `0xe2`; typed path remains model-gated |
| Battery charge limiter | yes | no | yes | **no** — sub-index rejected here |
| USB charging while off | yes | no | yes | **no** — not exposed on this model |
| LCD overdrive | runtime-conditional | no | yes | **partial**, typed CLI/GUI/TUI + live getter/toggle/restore; panel effect unverified |
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
| Capability probing per model | no (ships dead controls) | no | no | **yes**, side-effecting GPU getter deliberately excluded |
| Refuses the persistent-CMOS hazard | n/a | no | no | **yes**, allowlisted |
| Profiles | yes | no | no | **yes**, built-in + GUI-created user profiles |
| Machine-readable output | no | ~sysfs~ | ~sysfs~ | **yes** (`alien json`) |
| Diagnostics for bug reports | no | no | no | **yes** (`alien doctor`) |

### Serialised access is a correctness feature, not a nicety

`/proc/acpi/call` is a single global kernel buffer and write-then-read is not
atomic. Two processes using it directly interleave — one writes, the other
writes, the first reads the *other's* answer — with no error anywhere. Every
tool that pokes it from userspace without a lock has this bug latent. Alien's
daemon owns the file behind a mutex; measured under six concurrent clients,
150 responses, zero cross-talk.

### Model-specific verification

PredatorSense shows an Overclocking tab on machines where `Feature.ini`
disables CPU overclock and the switch is inert. Alien reports
*accepted-but-unverified* and explains why. That is the difference this project
is actually built around.

## Remaining gaps and model limits

**Fan curves.** PredatorSense's PH315-53 Covini **Custom** mode is the manual
per-fan Auto/Manual percentage control Alien already implements; it is not a
user-authored temperature curve. A future userspace curve daemon would be an
Alien-only addition. It is not shipped because hysteresis and fail-safe behavior
need sustained thermal testing.

**GPU Normal / Faster / Turbo.** PredatorSense command 45 applies Nvidia
Pstates20 P0 core/memory offsets `0/0`, `+50/+30`, or `+100/+60` MHz, selects
the shared Acer fan table as `max(CPU level, GPU level)+1`, then writes WMI
selector 5 / EC `GPOC` as 0/1/2. Alien now implements the closest public-Linux
equivalent with the current NVML per-Pstate offset API in that order. The path
is gated to the exact PH315-53 PCI/subsystem/DMI/BIOS target, checks live driver
ranges, requires an explicit unsupported-clock acknowledgement, reads every
leg back and attempts reverse-order rollback on any partial failure. This
target disables CPU OC and Alien exposes no CPU-mode setter, so its fan-table
contribution is fixed at CPU level 0; the resulting 1/2/3 mapping is not
generic to platforms with a higher CPU contribution. The deployed 0.5.0
daemon completed socket-only Normal/Faster/Turbo/Normal transitions with exact
readback of both offsets, fan table and `GPOC`. A guarded 15-second-per-mode
load remained stable below its thermal abort threshold, but scores of
`855/853/855/830` showed no measurable speedup. This confirms the compound
command path, not an isolated physical fan-curve or downstream `GPOC` effect.
The public [reverse-engineering write-up](reverse-engineering-predatorsense.md#05--command-45)
summarizes the recovered command and live-result boundary.

The nominal GPOC getter sends an OEM GPU notification, so GUI/TUI telemetry and
automatic capability probing never call it. Status is an explicitly labelled
manual snapshot. Profiles ignore old `gpu_turbo`/`turbo` fields with a warning,
new profiles omit them, and raw GPU setters are blocked in CLI, `Device` and the
daemon raw-call policy so they cannot silently split offsets/fan/GPOC state.

**Intel XTU power limits.** Static reverse engineering recovered one common
PH315-53 CPU policy from all three Acer profiles: PL1 70 W, PL2 107 W, short
power enabled, and a 28-second PL1 window. All voltage offsets are zero and
the listed ratios are non-modifiable. Alien now reports the OEM target and
live Linux powercap values, discovering PL1/PL2 by constraint name rather than
numeric index. It remains read-only: the target was unavailable for the live
write/readback/rollback and short-enable mapping needed to justify a
privileged writer.

**CoolBoost and keyboard timeout.** These are typed operations on the separate
APGe `WMAA` endpoint, not gaming-WMI misc sub-indices. Alien implements exact
getters and fixed setters, preserves the timeout getter's brightness byte,
confirms readback, and getter-confirms rollback on mismatch. On the reference
machine CoolBoost toggled, read back, and restored successfully. A controlled
PH315-53 A/B/A run confirmed a setter-linked reinitialization transient but no
sustained cooling lift under that tested workload; this model-scoped result
does not establish behavior on other Acer models. The public
[case-file verdict](reverse-engineering-predatorsense.md#08--proof-not-vibes)
keeps that no-benefit result separate from protocol-state confirmation.
The timeout getter at the exact native fallback hotkey index returned firmware
status `0xe2`; Alien therefore exposes no timeout control and sends no write.
It will not scan unproven indices to manufacture support.

**LCD overdrive.** Alien implements the gaming-profile getter and fixed on/off
setter, but exposes the setter only when the live getter reports byte 6 as 0 or
1; `0xff` remains unsupported. The PH315-53 toggled, read back, and restored
the field successfully. Getter echo is not proof of a panel timing or ghosting
change, so physical verification remains open. Battery limiting and USB
charging remain separate unsupported/model-dependent controls.

**GameSync lifecycle.** PredatorSense maps executables to whole-system settings,
applies them when a process starts, and restores the previous state when it
stops. Alien has manually applied profiles but no process watcher, executable
mapping, or crash-safe restore lifecycle. That is a real cross-platform product
gap, not a Windows-only plumbing dismissal.

**OSD overlay and AppCenter.** These remain platform integrations: a Linux OSD
belongs at the compositor/session layer, while AppCenter manages Windows apps.

## Reports wanted

The recovered Acer packages cover 18 unique model strings across 9 code series
and 6 OEM protocol families; official Acer sources add 36 separately labelled
[PredatorSense candidates](model-compatibility.md). PH315-53 remains the only
live-verified reference. Neither evidence tier is cross-model hardware
verification. The questions the reference machine cannot answer:

1. Does the **CPU overclock flag** do anything where `Feature.ini` enables it?
2. Do the corrected PH315-53 Covini modes visibly match PredatorSense on the
   physical keyboard? Firmware readback is not optical proof.
3. Which models genuinely use Clubman modes Meteor and Twinkling? They are not
   exposed by Acer's model-certified PH315-53 package.

`alien capabilities` and `alien doctor` print everything a report needs.
