# Feature parity

What PredatorSense, facer and Linuwu-Sense do, and where Alien stands against
each. Written to be checkable rather than asserted: where Alien does not have
something, it says so, and where something cannot work on a given machine it
says why.

Legend: **yes** · **no** · *n/a* (hardware cannot) · ~partial~

**Parity and benefit are two different questions, and this table answers only
the first.** A row is **yes** when Alien performs the same operation the vendor
tool performs — the same command, against the same firmware endpoint, with the
same values, confirmed by readback. Whether that operation then produces a
*measurable* improvement is a separate finding, reported separately and never
folded into the verdict.

They were conflated here previously, and it made the table wrong in a specific
way: CoolBoost and the GPU modes were marked *partial* because controlled A/B/A
runs found no sustained cooling lift and no measurable speedup. But Acer's own
tool issues exactly the same commands and cannot be shown to do better with
them. A feature that is fully implemented is not partially implemented because
the hardware it drives turned out not to help — that finding is a fact about
the hardware, not a gap in this software, and burying it in a verdict column
loses both facts at once.

So the measurements stay, verbatim and unsoftened, in the notes beside each row
and in [`evidence.md`](evidence.md). Only the verdict changed.

## Core hardware control

| | PredatorSense (Win) | facer | Linuwu-Sense | **Alien** |
|---|---|---|---|---|
| Fan: automatic / maximum | yes | ~model-gated~ | yes | **yes** |
| Fan: Custom per-fan Auto/Manual duty | yes | no | yes | **yes**, confirmed by firmware readback |
| User-authored temperature curve | no, not in PH315-53 Covini Custom | no | ~fan table~ | **yes** — `alien-cooling`, an Alien-only superset with hysteresis, dwell timers and a critical latch |
| CoolBoost | yes | no | no | **yes**, typed CLI/GUI/TUI + live getter/toggle/restore. Separately measured: PH315-53 setter reinit transient confirmed, controlled A/B/A found no sustained cooling lift |
| Fan RPM telemetry | yes | no | yes | **yes** |
| CPU / GPU / board temperature | yes | no | ~partial~ | **yes**, all three, ground-truthed |
| CPU / GPU clock dashboard | yes | no | no | **yes**, live Linux MHz measurements |
| Raw WMBH GPU flag, sub-index 5 | ~command-45 side effect~ | yes | yes | **manual read**, values 0/2; independent setters blocked because this is not an OEM OC mode |
| CPU WMI flag | yes | yes | yes | **read-only**, inert on PH315-53 |
| GPU overclock (MHz offsets) | yes | no | no | **yes on the exact PH315-53 target**, guarded Normal/Faster/Turbo transaction + readback/rollback; no measured speedup |
| CPU "turbo" (Intel XTU) | yes | no | no | **read-only status**, exact target + live named RAPL limits |
| Keyboard: 4-zone static | yes | yes | yes | **yes**, colour and enable state per zone |
| Keyboard: effects | yes | yes | yes | **yes**, exact PH315-53 Covini protocol implemented; optical revalidation pending |
| Keyboard: per-key colour | yes, on per-key models | no | no | **implemented, unverifiable here** — ITE 8291r3 transport written from the published protocol, but every machine this project can reach is four-zone, where per-key addressing is physically absent. No per-key controller has ever answered it |
| Backlight timeout | **presented but dead here** | no | yes | **no** — PredatorSense sends the identical call and gets the identical `0xe2`; see below |
| Battery charge limiter | **not in 3.00.3152/3198** | no | yes | **no** — absent from the recovered protocol surface entirely |
| USB charging while off | **not in 3.00.3152/3198** | no | yes | **no** — absent from the recovered protocol surface entirely |
| LCD overdrive | runtime-conditional | no | yes | **yes**, typed CLI/GUI/TUI + live getter/toggle/restore; panel effect unverified |
| Boot animation / sound | yes | no | yes | **yes**, misc selector 6, getter-gated; live readback on the reference machine pending |

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
| Per-game apply/restore lifecycle | yes | no | no | **yes**, `alien gamesync` — pidfd-tracked, survives `SIGKILL` and suspend |
| Crash-safe restore | no | no | no | **yes**, baseline written to disk *before* the hardware is touched |
| Which limiter is binding | no | no | no | **yes**, `alien limiters` — thermal, power, or none |
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
user-authored temperature curve. The userspace curve daemon that *is* one now
ships as `alien-cooling`: a stepped threshold table with asymmetric up/down
hysteresis, dwell timers, a critical latch and boot-into-failsafe. It is
deliberately not a PID — the plant has a 10:1 gain variation across the duty
range, integer °C sensors destroy the derivative term, and a 92 °C thermal pin
guarantees integral windup. This is an Alien-only addition with no
PredatorSense equivalent.

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
### Keyboard backlight timeout

The timeout getter returned firmware status `0xe2`, and Alien exposes no
control and sends no write. What changed is the reason: this is not Alien
declining a feature PredatorSense has. **PredatorSense's own timeout checkbox
is dead on this model**, and the decompiled package shows why.

The call takes a hotkey index `H`:

```text
get: WMAA(instance=0, method=2, input = 0x00080001 | (H << 8))
set: WMAA(instance=0, method=1, input = 0x00080002 | (H << 8) | (B << 32) | (T << 40))
```

`H` comes from `HKLM\SOFTWARE\OEM\PredatorSense\BK_Hotkey_Number`, read through
`TsDotNetLib/Registry.cs:30` — whose signature is
`CheckLM(path, value, uint defaultValue = 0)` and which, when the value is
absent, **writes the default back and returns it**. So a stock install uses
`H = 0`.

The branch is not in doubt either. `Advanced_Setting_CheckBox.cs:191-215` picks
the USB path only when `Feature.ini` says `KeyboardType/PerKey == 1`; the
reference model's plug says `PerKey=0`, so it falls through to
`CommonFunction.get_backlight_off()` — the APGe getter above.

PredatorSense therefore emits exactly `0x00080001`, which is byte-for-byte what
Alien emits, and this firmware answers `0xe2` to it. No non-zero
`BK_Hotkey_Number` is provisioned anywhere in either acquired package, so
nothing suggests a different index was ever intended for this model.

Alien will still not sweep the unproven `0..255` index space to manufacture
support. The difference between the two tools is only that Alien declines to
draw a control it cannot operate, while PredatorSense draws one that silently
does nothing — the same behaviour the capability-probing row above measures.

### Battery charge limiter and USB charging

Both rows previously credited PredatorSense with features it does not have in
the recovered versions. Neither appears in the Acer WMI method surface
(service IDs 9–35), among the decoded misc selectors, or in any of the nineteen
per-model `HW_Support.ini` files across 3.00.3152 and 3.00.3198. The complete
set of functions those files declare is Animation, AutoOverclocking, Backlight,
Discrete_GPU, LCD, Lightingeffect, Sound, Sticky_Key, Temperature and
Windowskey1.

Scope the claim to what was examined: newer Acer firmware and tooling do carry
an 80% charge limiter, and Linuwu-Sense targets models where it exists. The
correction is that **these** PredatorSense versions do not implement it, so the
table was reporting a parity gap that never existed.

**LCD overdrive.** Alien implements the gaming-profile getter and fixed on/off
setter, but exposes the setter only when the live getter reports byte 6 as 0 or
1; `0xff` remains unsupported. The PH315-53 toggled, read back, and restored
the field successfully. Getter echo is not proof of a panel timing or ghosting
change, so physical verification remains open. Battery limiting and USB
charging remain separate unsupported/model-dependent controls.

**GameSync lifecycle.** PredatorSense maps executables to whole-system settings,
applies them when a process starts, and restores the previous state when it
stops. `alien gamesync` now does the same and goes further: the baseline is
written to `/run/alien` **before** the first hardware write, the watched process
is held by a `pidfd` rather than a PID (so PID reuse cannot misfire), and the
lease is re-asserted across suspend/resume. A simulated crash recovered the
fans from 3157 to 5882 RPM; a suspend/resume cycle went 3125 → 5769 RPM.

The state lives in `/run/alien`, not `/var/lib`, on purpose: firmware fan state
does not survive a power cycle, so a lease that outlived a reboot would restore
a baseline the hardware no longer holds.

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
