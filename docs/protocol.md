# The Acer gaming WMI protocol

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

WMI GUID `7A4DDFE7-5B5D-40B4-8595-4408E0CC7F56` maps to an ACPI method, on this
machine `\_SB.PCI0.WMID.WMBH(instance, function, buffer)`. It lives in
**SSDT12**, not the DSDT — decompile *every* table or you will conclude the
interface does not exist.

The kernel exposes the GUID at `/sys/bus/wmi/devices/` but offers no userspace
invoke path, so Alien goes through the out-of-tree `acpi_call` module:

```
echo '\_SB.PCI0.WMID.WMBH 0x1 0x05 {0x01,0x02}' > /proc/acpi/call
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
| `0x05` | 5 | sensors | `{0x01, id}` |
| `0x06` / `0x07` | 6 / 7 | per-zone static colour | `{0, zone, r, g, b}` |
| `0x0E` | 14 | **fan behaviour** | **u64** — see below |
| `0x10` / `0x11` | 16 / 17 | manual fan duty | `{fan_id, percent}` |
| `0x12` / `0x13` | 18 / 19 | fan table | `{mode}` |
| `0x14` / `0x15` | 20 / 21 | backlight effect | `{effect, speed, brightness, _, dir, r, g, b}` |
| `0x16` / `0x17` | 22 / 23 | misc settings, incl. turbo | `{sub, value}` |

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

### Keyboard backlight — `0x14` / `0x15`

**The input is SIXTEEN bytes, not eight:**

```
[ mode, speed, brightness, 0, direction, R, G, B, 0x03, 0x01, 0,0,0,0,0,0 ]
```

Effects: 0 static, 1 breath, 2 neon, 3 wave, 4 shifting, 5 zoom, 6 ripple.
Direction: 1 right-to-left, 2 left-to-right. Getter input is `u64 = 1`.

**The `0x03, 0x01` at offsets 8–9 are what make the write take effect.** An
eight-byte buffer that stops before them is accepted, returns status 0, stores
every field it did receive, and reads back perfectly through `0x15` — and
lights nothing at all. This cost a long detour and produced a completely wrong
conclusion in an earlier version of this document, which described the readback
as proof the control worked.

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

Order matters when setting: put the keyboard in static mode and set brightness
with function `0x14` **first**, then write each zone with `0x06`. Changing mode
afterwards reinitialises the zones and discards the colours.

Note the consequence for `0x15`: in static mode the visible colour comes from
the per-zone registers, so the RGB field the backlight getter reports says
nothing about what the keyboard looks like.

### Turbo / overclock — `0x16` / `0x17`

`{sub, value}`; sub **5** = GPU, sub **7** = CPU; value **0** = normal,
**2** = turbo. Two, not one — sampling the firmware while the chassis Turbo
button was physically held showed both sub-indices at 2.

The CPU flag is **inert on this model**, and the reason is not mysterious:
PredatorSense gates CPU overclock on `Feature.ini → OverclockSupport CPU`,
which is `0` for the PH315-53, and its service carries a "Not support CPU
overclock" path. What the vendor markets as CPU turbo on Intel machines is
**Intel XTU** — `.xtu` profiles through `XtuService.exe` and `iocbios2.sys`,
adjusting PL1/PL2 power limits and turbo ratios, not this interface and not
base clock. On a locked 10th-gen H CPU, power limits are the only lever.

GPU overclock does go through this function, with MHz offsets PredatorSense
reads from `PredatorSense.ini [OC_GPU]`.

A/B tested with fans pinned at maximum so thermals could not mask the result:
no measurable clock or benchmark change on this SKU. The Turbo button's real
effect here is the fan curve.

### Fan table — `0x12` / `0x13`

Present. The getter returns a constant `1` regardless of the mode argument, and
the setter cannot be verified without changing behaviour we do not understand,
so **Alien does not expose it** and the daemon's policy does not allow it.

### Per-key colour — a hardware question, not a software one

Acer ships **two different keyboard backlights**, and no amount of software
turns one into the other:

* **Four-zone**, driven by the gaming WMI functions above. The keys are wired
  into four banks sharing LEDs. There is no per-key addressing to reach, so a
  per-key mode is physically impossible. The PH315-53 is one of these.
* **Per-key**, driven by a separate **ITE 8291 rev 3** USB controller
  (`048d:6004`, also `048d:ce00`). Triton 500 SE, Helios 16/18, later Helios
  300 revisions. Nothing to do with this WMI interface.

Alien implements both and detects which is present (`alien capabilities`).
Frame format for the ITE controller: a 6 × 21 matrix, sent one row at a time,
and the row payload is **planar, not interleaved** — all reds for the row, then
all greens, then all blues. Interleaving produces a keyboard that lights up in
convincing but entirely wrong colours.

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

## Not achievable on this model

Documented so nobody re-runs it: the GPU power limit is VBIOS-locked
(`nvidia-smi -pl` returns "not supported"), CPU undervolt needs MSR `0x150`
which firmware can lock, and RAPL limits already sit far above what the chassis
sustains — the EC owns the thermal envelope.

## Reports from other models welcome

The function numbers are believed common across the interface, but the fan
bitmask, the sensor ids and the turbo values are worth confirming per model.
`alien doctor` prints what a report needs. The two most interesting questions
are whether per-zone RGB and CPU overclock work anywhere, since both go quiet
on the reference machine.
