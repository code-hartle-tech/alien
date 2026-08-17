# The Acer gaming and APGe WMI protocols

Everything here was confirmed against a Predator Helios 300 PH315-53 unless
marked otherwise. Acer publishes no documentation for this interface, so each
entry says how it was established and — where it matters — how it can be
wrong.

Re-verified 2026-08-10 with **no other Acer driver present** — `facer` and the
in-tree `acer_wmi` both unloaded, and the machine's own fan unit stopped — so
none of the results below depend on another driver having set something up.
The full feature sweep passed identically either way.

Read the [traps](#traps) section before you experiment. Several things on this
interface return success and do nothing, and one of them writes a byte that
survives a power cycle.

---

## Reaching it

The PH315-53 exposes two WMI method objects. Gaming GUID
`7A4DDFE7-5B5D-40B4-8595-4408E0CC7F56` maps to
`\_SB.PCI0.WMID.WMBH(instance, method, buffer)`; APGe GUID
`61EF69EA-865C-4BC3-A502-A0DEBA0CB531` maps to the same-shaped
`\_SB.PCI0.WMID.WMAA`. They live in **SSDT12**, not the DSDT — decompile
*every* table or you will conclude the interfaces do not exist.

Both `_WDG` records declare an instance count of one. WMI instances are
zero-based, so the only valid instance/index is **0**. The PH315-53 AML ignores
`Arg0`, which made the old value 1 appear to work, but it was not the declared
WMI instance.

The kernel exposes the GUID at `/sys/bus/wmi/devices/` but offers no userspace
invoke path, so Alien goes through the out-of-tree `acpi_call` module:

```
echo '\_SB.PCI0.WMID.WMBH 0x0 0x05 {0x01,0x02}' > /proc/acpi/call
cat /proc/acpi/call
{0x00, 0xfa, 0x16, 0x00, 0x00, 0x00, 0x00, 0x00}
```

Byte 0 of every reply is a status code; zero means the firmware accepted the
call. **It does not mean the hardware did anything.**

### Two ways to get nothing back

**Read `/proc/acpi/call` with a single `read()`.** Language helpers that read
to EOF return an *empty string*: procfs reports `st_size` 0, so the size hint
is 0, the first read is zero-length, and `Ok(0)` is correctly treated as end of
file. The call really executed and userspace silently sees nothing. In Rust,
`fs::read` and `read_to_string` both fail this way.

**It is a single global kernel buffer, and write-then-read is not atomic.** Two
processes using it concurrently interleave — A writes, B writes, A reads *B's*
answer — with no error anywhere. Alien solves this with a daemon that owns the
file behind a mutex; anything else driving it directly must guarantee it is the
only user.

---

## Functions

**The numbers are decimal in the vendor and community sources.** `14` is
`0x0E`; reading it as hex sends you to `0x14`, the effect setter, which will
happily return success.

| Fn | dec | Purpose | Payload |
|---|---|---|---|
| `0x02` / `0x04` | 2 / 4 | gaming LED behavior | u64; Covini static-zone mask is `8 + (mask<<40)` |
| `0x05` | 5 | sensors | `{0x01, id}` |
| `0x06` / `0x07` | 6 / 7 | per-zone static colour | u64 `[zone, r, g, b, 0,0,0,0]` |
| `0x0E` | 14 | **fan behaviour** | **u64** — see below |
| `0x10` / `0x11` | 16 / 17 | manual fan duty | `{fan_id, percent}` |
| `0x12` / `0x13` | 18 / 19 | fan table | `{mode}` |
| `0x14` / `0x15` | 20 / 21 | backlight effect | `{effect, speed, brightness, mode_flag, dir, r, g, b}` |
| `0x16` / `0x17` | 22 / 23 | misc settings, incl. raw firmware flags | `{sub, value}` |

### Sensors — `0x05`

Identified by sweeping ids 1–32 read-only and matching each against ground
truth sampled at the same instant.

| id | reading | matched against | verdict |
|---|---|---|---|
| 1 | 86 | `coretemp` package 88, ACPI `B0D4` 86 | CPU temp |
| 2 | 6000 | — | CPU fan RPM |
| 3 | 72 | ACPI `SEN2` 71, `pch_cometlake` 71 | board / chassis temp |
| 6 | 6122 | — | GPU fan RPM |
| `0x0A` | 74 | `nvidia-smi` 74, in 11 of 11 samples | **GPU temp** |

`0x0A` is the GPU, not a generic "system" sensor — no other kernel thermal zone
reads its value, and the two track exactly. Getting this wrong files the GPU's
temperature under a CPU heading.

Value is little-endian in bytes 1–2. A sensor that is not populated reads zero.

### Fan behaviour — `0x0E`

The one every other project either misses or buries, and the only control that
changes performance. It does **not** take the `{subunit, value}` byte pair its
neighbours use — it takes a 64-bit word:

```
0x820009   both fans MAXIMUM
0x410009   both fans AUTO (hand back to the EC curve)
0xC30009   both fans MANUAL (then set duty with 0x10)
0x030001   CPU manual      0x010001  CPU auto
0xC00008   GPU manual      0x400008  GPU auto
```

Low 16 bits are a fan bitmask, `1 << (id-1)` — CPU is id **1** (bit 0), GPU is
id **4** (bit 3), so both is `0b1001` = `0x9`. Above that, a two-bit mode per
fan at bit `16 + 2*(id-1)`: **1 = auto, 2 = max, 3 = manual**.

That shift is `16 + 2*(id-1)`, not 32. Getting it wrong still produces
plausible words for the both-fan cases and breaks the single-fan ones.

### Manual duty — `0x10` / `0x11`

`{fan_id, percent}`, and the fan must already be in manual mode (`0x0E`) or the
percentage is ignored. Setting only the target fan's bit is enough; the other
fan keeps its mode.

Readback (`0x11`) returns the requested percentage in **byte 1** — verified as
45 → `0x2d` and 75 → `0x4b`. Byte 2 reads `0x17` regardless of fan or value, so
decoding bytes 1–2 as a u16 turns 45% into 5933.

This works, and a previous version of these notes said it did not. That was a
measurement artifact: the RPM was sampled 600 ms after the call, and these fans
take **eight to ten seconds** to settle, so every check landed mid-ramp. Duty
is not linear in RPM — 30% ≈ 2100–3300, 45% ≈ 4700, 90% ≈ 5450, 100% = 5882 on
the reference CPU fan.

PredatorSense calls this per-fan Auto/Manual percentage UI **Custom**. It is not
a user-authored temperature curve; a future Alien curve service would be an
additional feature rather than missing reproduction of this protocol.

### Advanced typed controls — APGe and gaming profile

Alien exposes only fixed, typed operations for these paths. Direct writes are
DMI-gated to the Acer Predator PH315-53 with BIOS V1.07; each setter first reads
the supported state, writes one exact payload, reads it back under the shared
WMI mutex, and attempts rollback on mismatch. The daemon/socket does not expose
an arbitrary endpoint, method, selector, or u64 proxy.

The group-accessible daemon audits typed mutations and admits them no faster
than one per 100 ms. A root process can deliberately use Alien's direct
transport when the daemon is absent; that path still takes the cross-process
interface lock and enforces readback/rollback, but root is the trust boundary
and is not constrained by the daemon's group-client rate limiter.
Audited runners can set `ALIEN_REQUIRE_SOCKET=1`: `Device::open` then returns
the daemon connection error immediately and never detects or opens direct
`AcpiCall`. Unset or `0` keeps the normal daemon-first fallback; every other
value is rejected as invalid so a misspelled guard cannot weaken the audit.
Socket clients use bounded five-second I/O. Any write, read, EOF or malformed
response failure permanently drops that connection; callers must reopen the
device. A daemon may finish after a client timeout, and reusing the stream could
otherwise let that late reply satisfy the next request. A complete `ERR` line
preserves framing and does not by itself invalidate the connection.

The typed OEM GPU-mode endpoint expands the `alien` group's authority: an
exact-target member who supplies the acknowledgement can request privileged,
unsupported Nvidia clock offsets. The acknowledgement is an accident guard,
not authentication; group membership remains the security boundary. The
service retains only `CAP_CHOWN` plus `CAP_SYS_ADMIN`: the former owns the
socket, while Nvidia's matching 595.71.05 open-driver source defines its
privileged-caller check as `capable(CAP_SYS_ADMIN)`
([NVIDIA `nv-linux.h`, tag 595.71.05](https://github.com/NVIDIA/open-gpu-kernel-modules/blob/595.71.05/kernel-open/common/inc/nv-linux.h#L499)).
The rest of the systemd sandbox remains enabled. After deployment, the first
GPU validation must be the socket-forced, getter-only `gpu-mode status` path;
it proves the service can bind the exact NVML device and read both ranges before
any setter is considered, while explicitly disclosing its one GPOC-notification
side effect.

| Control | Getter and support predicate | Setter |
|---|---|---|
| CoolBoost | `WMAA(0,2,[07,02,00,00])`; require 8+ bytes, status 0, state byte 0/1 | `WMAA(0,1,u64(7 + (state<<16)))` |
| Keyboard timeout | `WMAA(0,2,[01,00,08,00])`; require status 0 and timeout byte 5 in 0/30; byte 4 is current brightness | `WMAA(0,1,[02,00,08,00,brightness,timeout,00,00])`, preserving getter brightness |
| LCD overdrive | `WMBH(0,3,u32(0))`; byte 6 is 0 off, 1 on, `0xff` unsupported | only after a 0/1 getter: `WMBH(0,1,u64(0x10 + (state<<48)))` |

CoolBoost and timeout belong to the separate APGe class; they are not gaming
misc sub-indices and CoolBoost is not fan Maximum. LCD overdrive is
runtime-conditional, so `0xff` is a normal unsupported result and no setter is
sent. Alien exposes all three typed paths through the daemon and supported
frontends, but getter/readback establishes only stored protocol state. On the
live reference machine CoolBoost and LCD overdrive each toggled, read back, and
restored to off. A controlled PH315-53 CoolBoost A/B/A run confirmed a
setter-linked reinitialization transient but no sustained cooling lift under
that tested workload; this does not establish behavior on other Acer models.
The public [case-file verdict](reverse-engineering-predatorsense.md#08--proof-not-vibes)
keeps that physical result separate from getter/setter evidence.
LCD panel timing was not measured. The timeout getter at the sole proven
fallback index returned status `0xe2`; Alien marks it unsupported and does not
write or scan speculative indices.

### Keyboard backlight — `0x14` / `0x15`

The PH315-53 is PredatorSense machine type 1, **Covini**. Its setter input is
exactly one u64, little-endian:

```
[ mode, speed, brightness, mode_flag, direction, R, G, B ]
```

| PredatorSense selection | mode | colour | direction |
|---|---:|---|---|
| Breathing | 1 | yes | no |
| Wave | 3 | no, firmware palette | yes |
| Zoom | 5 | yes | no |
| Shifting | 4 | yes | yes |
| Neon | 2 | no, firmware palette | no |

Static is mode 0. Direction is 1 right-to-left or 2 left-to-right. Wave also
sets `mode_flag = 8`; every other mode leaves it zero. The exact WPF controls
set speed 1–9 and five brightness levels; managed code converts brightness
levels 1–5 to wire values `0,25,50,75,100`. Getter input is `u64 = 1`.

This contract is independently recovered at four layers:

1. Acer's PH315-53 `Feature.ini` selects Covini, not Clubman.
2. `LightingDynamicUI_Covini` packs the fields into a `ulong`.
3. PredatorSense 3.00.3152 `PSSvc` rejects inputs longer than eight bytes,
   then zero-pads bytes 8–15 only when building the WMI SAFEARRAY.
4. PH315-53 BIOS `WMBH` function `0x14` dereferences only bytes 0–7.

The old Alien implementation derived a sixteen-byte Clubman buffer from
PredatorSense 3.00.3198, a package that does not contain a PH315-53 plug-in.
Its `3,1` tail is not a commit marker: native service disassembly shows no
branch or secondary action for those bytes, and the PH315-53 BIOS never reads
them. Meteor (6) and Twinkling (7) are likewise Clubman modes, not PH315-53
features.

**A readback proves the firmware stored a value. It does not prove the
hardware acted on it.** On this interface the two come apart, and the only
ground truth for lighting is looking at the keyboard.

Speed 0 means "do not advance the animation". An animated effect at speed 0 is
accepted, reads back correctly, and does not move — indistinguishable from an
unsupported effect. It happens naturally when a UI carries the speed over from
static, whose speed legitimately is 0.

### Per-zone static colour — `0x06` / `0x07`

Works. An earlier version of this document said it was unverified and probably
inert on this model; that was wrong, and the cause was the payload encoding.

**Function 6 takes a u64, not a byte record:**

```
zone | (R << 8) | (G << 16) | (B << 24)
```

so on the wire, little-endian: `[zone, R, G, B, 0, 0, 0, 0]`. Zone ids are
`1, 2, 4, 8`, left to right.

⚠️ **The vendor-derived notes give this as `(R<<24) | (G<<16) | (B<<8) | zone`,
which is transposed.** Following them lights the keyboard **blue** when you ask
for red. Measured on hardware: four zones set to red, green, blue and white
each read back correctly through function 7 and displayed correctly.

Getter `0x07` takes the zone id and returns `[status, R, G, B]`.

PredatorSense also lets each static zone be disabled. Function `0x02` takes:

```
8 | (zone1_on << 40) | (zone2_on << 41) |
    (zone3_on << 42) | (zone4_on << 43)
```

Unlike the direct static-colour and dynamic-backlight branches, function 2
falls through `WMBH` to the firmware's `WSMI` SMI mailbox. Alien therefore
allowlists only this exact low-byte-8, bits-40-through-43 shape.

Order matters when setting static state: send the function-2 enable mask,
switch to mode 0 and set brightness with function `0x14`, then write each
enabled zone with `0x06`. Changing mode afterwards reinitialises the zones and
discards the colours.

Note the consequence for `0x15`: in static mode the visible colour comes from
the per-zone registers, so the RGB field the backlight getter reports says
nothing about what the keyboard looks like.

### Raw firmware flags and OEM GPU modes — `0x16` / `0x17`

`{sub, value}`; sub **5** reaches the EC `GPOC` field and sub **7** is the CPU
flag. Alien's boolean API uses only values **0** and **2**, matching the states
sampled while the chassis Turbo button was released/held. These are raw stored
firmware values, not names for PredatorSense Normal/Faster/Turbo modes.

The CPU flag is **inert on this model**, and the reason is not mysterious:
PredatorSense gates CPU overclock on `Feature.ini → OverclockSupport CPU`,
which is `0` for the PH315-53, and its service carries a "Not support CPU
overclock" path. What the vendor markets as CPU turbo on Intel machines is
**Intel XTU** — `.xtu` profiles through `XtuService.exe` and `iocbios2.sys`,
adjusting package power controls, not this interface and not base clock. The
exact PH315-53 Normal, Fast and Turbo files are CPU-identical: PL1 70 W, PL2
107 W, short power enabled, and a 28-second PL1 window. Their voltage offsets
are zero; 45/44/43/42 ratios are marked non-modifiable and belong to an
embedded i5-10300H hardware record despite the i7-10750H directory name.
Alien therefore does not create three CPU modes or write the inert WMI CPU
flag. `alien power status` reads named Linux powercap constraints and reports
the unverified write gap. That status probe reads sysfs directly in the
unprivileged client; the daemon remains WMI-only and exposes no powercap write
request.

The exact PredatorSense GPU control instead sends PSSvc command 45, which runs
this compound sequence for the PH315-53:

1. Apply a sparse Nvidia Pstates20 v3 P0 record to every enumerated physical
   Nvidia GPU: Normal `0/0`, Faster `+50/+30`, or Turbo `+100/+60` MHz
   graphics/memory offsets.
2. Select Acer's fan table as `max(requested CPU level, requested GPU level)+1`.
3. Because both target WMI rules are enabled, call WMBH function 22, sub-index
   5 with value 0, 1, or 2, which stores `GPOC` and notifies the discrete GPU.

The order is Nvidia offsets, fan table, then WMI/`GPOC`; these are not alternate
branches. The service reports only the Nvidia setter result, performs the later
two actions even after an Nvidia failure, has no rollback, and does not read the
offsets back. The public
[command-45 case study](reverse-engineering-predatorsense.md#05--command-45)
summarizes the recovered order and the evidence boundary without distributing
vendor binaries or decompiler output.

Alien implements the closest public-Linux command-45 equivalent with the
current privileged NVML per-Pstate offset API followed by the Acer fan-table
and `GPOC` legs in OEM order. Normal is `0/0`, Faster is `+50/+30`, and Turbo
is `+100/+60` MHz, with fan tables 1/2/3 and `GPOC` 0/1/2. Those fan-table
values are exact only because this PH315-53 package sets CPU OC support to
zero and Alien exposes no CPU-mode setter, so the CPU contribution is fixed at
Normal level 0. They are not a generic `GPU level + 1` rule: a target with a
higher requested CPU level must retain the native `max(cpu,gpu)+1` selection.
The path is gated to
the exact PH315-53 PCI/subsystem/DMI/BIOS target, validates live driver ranges,
requires an explicit unsupported-overclock acknowledgement, reads every leg
back, and attempts reverse-order rollback on any partial failure. NVML offsets
last only for the Nvidia driver lifetime.
Rollback treats a setter error as possibly applied too: it always follows a
restore-setter attempt with the corresponding getter and reports both results,
including the case where the setter returned failure but saved state landed.

After a manual snapshot, GUI/TUI enable only modes whose two requested offsets
fit the reported live NVML ranges; an out-of-range mode shows a reason instead
of opening confirmation. This is presentation gating, not the safety boundary:
the setter re-queries and validates both ranges immediately before every write.

The nominal WMBH function-23 `GPOC` getter is not side-effect-free: selector 5
also sends the OEM discrete-GPU notification. GUI/TUI telemetry therefore does
not poll it; mode state is refreshed only through an explicitly labelled user
action or around an accepted mode mutation. CLI `gpu-mode status` warns about
the notification. A successful mutation necessarily has three notifications
(pre-state GPOC getter, OEM GPOC setter, GPOC readback); final compound
verification reuses that readback instead of issuing a redundant fourth. The
raw 0/2 sub-index-5 control remains labelled as a legacy
flag, never as Normal/Faster/Turbo. A prior A/B test of that raw flag alone,
with fans pinned at maximum, showed no measurable clock or benchmark change;
that says nothing about the full OEM compound action.

Legacy profile TOML keys `gpu_turbo` and `turbo` remain parseable for migration,
but profile application deliberately ignores them and frontends warn when a
saved value is present. Replaying raw GPOC independently could overwrite one
leg of a Faster/Normal/Turbo snapshot without updating NVML offsets or the fan
table. Profiles therefore mutate only fans and lighting; GPU mode changes
require the explicit guarded `gpu-mode` transaction, while the separately
labelled raw CLI status command remains an expert diagnostic surface. Raw
`gpu-flag on|off` and its deprecated `turbo` setter alias are disabled because
they would mutate only GPOC and silently split the compound state.

Automatic capability discovery also skips misc sub-index 5. Its nominal getter
sends `Notify(PEGP, 0xC0)`, so support remains `Unknown` until the user chooses
an explicitly labelled manual GPU status refresh; GUI/TUI connect and reconnect
paths do not cross this notification boundary. Raw daemon `CALL` access to
fn22/sub-index-5 writes and fn23/sub-index-5 reads is denied. Explicit GPU
status routes
through the typed compound getter, whose notifying read has a daemon-wide
one-second cooldown and request/success/failure/rate-limit audit messages. A
trusted-root direct transport bypasses this daemon cadence, as it does the
mutation audit, and remains a deliberate root-only boundary.

### Fan table — `0x12` / `0x13`

Present. Alien exposes functions 18/19 only inside the exact-target typed OEM
GPU-mode transaction; raw socket `CALL` policy remains closed to them. The
getter response is status byte 0 plus table byte 1, and each conditional setter
is read back. Live table switching and physical fan-curve effects remain a
separate hardware-validation gate.

### Per-key colour — a hardware question, not a software one

Acer ships **two different keyboard backlights**, and no amount of software
turns one into the other:

* **Four-zone**, driven by the gaming WMI functions above. The keys are wired
  into four banks sharing LEDs. There is no per-key addressing to reach, so a
  per-key mode is physically impossible. The PH315-53 is one of these.
* **Per-key**, driven by a separate **ITE 8291 rev 3** USB controller
  (`048d:6004`, also `048d:ce00`). Triton 500 SE, Helios 16/18, later Helios
  300 revisions. Nothing to do with this WMI interface.

Alien implements the four-zone WMI path. It also contains capability detection
and an experimental ITE transport for per-key hardware, but no project-owned
per-key machine has validated that path end to end and the current packages do
not install a `hidraw` permission rule. Treat it as source-mapped,
hardware-unverified work—not supported per-key control.

The source-mapped ITE frame is a 6 × 21 matrix, sent one row at a time. Its row
payload is **planar, not interleaved**: all reds for the row, then all greens,
then all blues. An interleaved implementation would address the wrong byte
layout.

---

## Traps

**`CLCK` / `DUTY` / `THEN` under `EC0` are not fan control.** They are the
Intel CPU clock-throttle register at I/O `0x1810` (ACPI `ABASE+0x10`,
PROC_CNT). Confirmed three ways: `/proc/ioports`, `_PTC` pointing there, and
those methods having **zero callers** in the firmware. Writes never latch
because `_PTC` selects FFixedHW (MSR `IA32_CLOCK_MODULATION`) on modern CPUs,
leaving the legacy register inert. It looks exactly like a fan override.

**⚠️ Function `0x16` sub-index 6 writes a persistent CMOS byte.** It survives
reboots and power cycles. Do not fuzz function `0x16`. Alien refuses this
sub-index on both the setter and the getter.

**Forcing `acer_wmi predator_v4=1` on a model that is not v4** creates a
`platform_profile` that returns `EIO` on every access and provides no fan
control. It does enable fan-RPM hwmon, which is the only reason to do it.

**⚠️ A malformed call returns the PREVIOUS call's data.** The firmware does not
populate a response buffer for a request it cannot parse, so what comes back is
whatever the last call left there — status byte included. Measured on function
7 while it was being sent the wrong payload shape:

```
cold                      {0x00, 0x00, 0x17, ...}   status 0
after ~20 other calls     {0x01, 0x00, 0x00, ...}   status 1, rejected
after a capability sweep  {0x00, 0xEA, 0x17, ...}   status 0 — and 0x17EA is
                                                    6122, the GPU fan RPM from
                                                    an earlier call
```

This was originally written up as "an *unsupported* function returns residue",
and used as evidence that per-zone colour was unsupported. It was not: the call
was malformed. Send function 7 correctly and it answers properly every time.

**Unsupported and malformed look identical from the outside.** Before
concluding a machine lacks a feature, make sure the request was well-formed.

Note this does not affect well-formed calls: 250 back-to-back sensor reads
returned 0 rejections and 0 implausible values, and 60 consecutive full
`sensors()` sweeps showed no cross-contamination.

**A loose interface probe picks the wrong method.** On this machine `WMBE`
exists, accepts `GetSysInfo`, and answers a bare `0x0` — well-formed and
meaningless. A "did it reply?" test selects it while the working method sits
next in the list, after which every call succeeds against something inert.
Demand a buffer-shaped reply with a plausible reading.

---

## Measured impact

Helios 300 PH315-53 (i7-10750H, RTX 2060 Mobile 80 W), same benchmark, same
session, fans the only variable:

| | 7-zip (MIPS) | GPU idle |
|---|---|---|
| EC automatic curve | 14,492 / 11,603 (~26.1k) | 86–89 °C |
| fans forced to maximum | **20,961 / 17,708 (~38.7k)** | **81 °C** |

**≈ +48%.** Published figures for an unthrottled i7-10750H are 35–45k, so the
stock curve was holding the CPU in thermal throttle.

## Deliberately read-only or unavailable on this model

The GPU power limit is VBIOS-locked (`nvidia-smi -pl` reports not supported).
The exact CPU profiles request zero voltage offset, while any offset path would
need the firmware-lock-sensitive MSR 0x150 mailbox, so Alien does not implement
it. RAPL power limits are now reported read-only. A writer remains gated on
live PH315-53 proof of named writable constraints, bounds, readback, rollback,
and the XTU short-power-enable mapping.

## Reports from other models welcome

PredatorSense 3.00.3152 and 3.00.3198 provide 19 model plug-ins covering 18
unique model strings across 9 code series and 6 distinct OEM protocol families.
Acer's official sources supply a separate 36-entry **Alien compatibility
unverified** PredatorSense tier. See the complete [model evidence
catalog](model-compatibility.md). Neither tier is live hardware verification of
Alien's paths on a model.

The function numbers are believed common across the gaming interface, but fan
bitmasks, sensor IDs, raw-flag values, and APGe/profile capabilities still need
per-model confirmation. `alien doctor` prints what a report needs. The most
interesting open questions are whether the raw CPU flag has a physical effect
where Acer enables it, and which firmware families expose the typed APGe and
LCD-profile fields.
