# Evidence and open questions

Alien's rule is that a control is not "working" until something outside the
software says so. This page records where each claim sits, and what is still
unproven.

It exists so the README does not have to carry it. If you just want to use the
tool, you do not need this page.

---

## The tiers

| Tier | Meaning |
|---|---|
| **Accepted** | The firmware returned success. Nothing more. |
| **Getter-confirmed** | A read-back reports the value we wrote. |
| **Hardware-measured** | An instrument outside the firmware agrees — RPM, temperature, clock, throughput. |
| **Optically verified** | A camera saw it. Reserved for light and panel behaviour. |

The distinction is not pedantry. This interface is full of calls that return
success and change nothing, and three separate controls on the reference
machine measured **worse than doing nothing**.

## Where each control sits

| Control | Tier | Note |
|---|---|---|
| Fan auto / max / per-fan duty | **Hardware-measured** | RPM and throughput; +61.8 % sustained |
| Per-fan independent modes | **Hardware-measured** | CPU→3529 RPM while GPU held 6122 |
| Temperature curve controller | **Hardware-measured** | Verified against live telemetry |
| Five sensors, both fan RPMs | **Hardware-measured** | Ground-truthed against `nvidia-smi` and kernel thermal zones |
| CPU/GPU clocks, RAPL state | **Hardware-measured** | Cross-checked against `cpufreq` sysfs |
| Limiter attribution | **Hardware-measured** | Throttle counters correlate exactly with 93 °C excursions |
| Crash-safe profile leases | **Hardware-measured** | Simulated crash; recovery took fans 3157 → 5882 RPM |
| Suspend/resume re-assert | **Hardware-measured** | pre → 3125 RPM, post → 5769 RPM |
| Four-zone static colour | **Getter-confirmed** | Per-zone colour and enable state read back |
| Keyboard effects | **Getter-confirmed** | Protocol implemented; light output not yet filmed |
| CoolBoost | **Getter-confirmed** | Toggles and restores; **no sustained cooling benefit measured** |
| LCD overdrive | **Getter-confirmed** | Field confirmed; panel response not yet filmed |
| OEM GPU modes | **Getter-confirmed** | All four fields read back; **no measurable speed-up** |
| Keyboard backlight timeout | **Dead in PredatorSense too** | Its checkbox sends the identical `0x00080001` (hotkey index defaults to 0) and gets the identical `0xe2` |
| Battery charge limiter | **Not a PredatorSense feature** | Absent from the WMI surface, the misc selectors and all 19 model support files in 3.00.3152/3198 |
| Per-key colour | **Unverifiable here** | ITE 8291r3 transport implemented; every reachable machine is four-zone, so no controller has ever answered it |
| Boot animation | **Implemented, readback pending** | Misc selector 6, recovered from `Set/Get_Post_AnimationSound`; declared as `FUN1=Animation` for this model |

## Things that measured worse than nothing

Documented rather than shipped, because a control that does nothing is worse
than an absent one.

- **GPU clock offsets.** `+100/+60 MHz` produced scores of `855/853/855/830`.
  The GPU was sitting at 77–79 W against an ~80 W board cap, so shifting the
  voltage/frequency curve could not raise a clock the power budget was already
  deciding.
- **CoolBoost.** A controlled A/B/A under load found the same transient RPM dip
  and recovery on both state edges, and no sustained thermal difference.
- **TCC offset.** `MSR 0x1A2` ships an offset of 8, so the chip throttles at
  92 °C rather than its 100 °C limit, and the field is writable. Taking those
  8 °C measured **−45.3 %**, with the two stock blocks agreeing within 0.3 %.
- **Undervolting past stock.** Once BIOS 2.04 removed the +49.8 mV overvolt,
  a further −50 mV bought **+0.8 %** clock — noise.

The pattern: every one of these tuned a limiter that was not the binding one.
That is why `alien limiters` exists.

## Still open

Two claims stay open because software cannot settle them:

- **The full keyboard effect matrix** — effects, masks, brightness steps and Off
  — needs a camera physically framing the keyboard. A screenshot proves the
  interface rendered, not that light was emitted.
- **LCD overdrive** needs a 240 fps-or-faster camera filming the panel to
  observe pixel response. The firmware field is confirmed; the optical effect
  is not.

Both are reported in the UI as getter-confirmed rather than verified, and will
stay that way until the receipts exist.

## Measurement protocol

Any throughput claim here follows the same shape, because a short benchmark on
this chassis measures the wrong regime:

- **≥ 10 minutes discarded, ≥ 10 minutes measured** per condition. Heat soak
  takes that long to develop and *is* the entire effect — a 60-second run of
  the fan comparison measured +6.9 % where the full protocol measured +61.8 %.
- **A/B/A ordering**, so run-order drift is separable from the effect. The
  same-condition delta is reported alongside the result; if drift approaches
  the effect, the result is reported as inconclusive rather than as a number.
- **Hold constant**: ambient, AC power, fan state, dGPU power state.
- Raw CSVs and the harnesses live in `research/` in the development repository.

## Compatibility

One machine is live-verified. See
[model-compatibility.md](model-compatibility.md) for the full catalog and what
each tier means there.

Run `alien doctor` on the actual machine before any setter. Catalog membership
is never a runtime safety bypass.
