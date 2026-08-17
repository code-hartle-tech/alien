# Ghost in the firmware

## How we cracked PredatorSense's hardware protocol—and rebuilt it for Linux

> The UI was never the lock. The lock was a chain of assumptions: a Windows
> package, a privileged service, an undocumented WMI method, an SMI handler,
> and an embedded controller quietly watching memory for bytes nobody had
> named.

Alien is an independent, from-scratch Linux interoperability stack for Acer
Predator hardware. This is the story of how we followed PredatorSense from a
button on screen to the last controller that actually changes the machine—and
how several convincing answers turned out to be wrong.

No Acer code, artwork, firmware, or binaries are distributed with Alien. We
studied publicly obtainable vendor packages and the firmware present on our own
hardware for interoperability, recorded facts about the interface, and wrote a
new implementation from scratch.

| Challenge card | |
|---|---|
| **Target** | Acer PredatorSense 3.00.3152 on a Predator PH315-53 / BIOS V1.07 |
| **Objective** | Recover the hardware contract and build safe native Linux controls |
| **Attack surface** | Installer → managed UI → native services → WMI/ACPI → SMM → ENE EC |
| **False flag** | A firmware success status that did not prove a physical effect |
| **Win condition** | Independent code, exact-target guards, readback, rollback, and honest open gates |

---

## 00 · The target

The reference machine was a Predator Helios 300, model PH315-53, running Linux
as its only operating system. The immediate goals sounded simple:

- read temperatures and fan speed;
- return the fans to firmware control or force them to maximum;
- control its four-zone keyboard;
- recover PredatorSense's Normal, Faster, and Turbo behavior;
- expose the characterized controls through an unprivileged CLI, TUI, and
  desktop app.

The machine already had a community-known Acer gaming WMI interface. That gave
us a door. It did not give us a map.

Alien builds on the public groundwork of `facer`, Linuwu-Sense, and Linux's
in-tree `acer-wmi` driver. Where those projects' published function numbers
matched our measurements, we retained the credit; where the exact hardware
disagreed, we recorded the counter-evidence instead of silently rewriting
history.

### Rules of engagement

This was an interoperability investigation, not a binary redistribution
project. We kept vendor inputs read-only, hashed the artifacts we examined,
separated exact-target evidence from later-package comparisons, and kept
proprietary installers, extracted code, firmware images, and Acer artwork out
of the public repository.

The tool belt was deliberately ordinary: archive/MSI extraction, PE and CLR
metadata, managed-source and CIL recovery, x86-64 disassembly, BMOF decoding,
ACPI table decompilation, UEFI extraction, 8051 control-flow tracing, and live
Linux readback. The useful part was not any single tool. It was preserving the
same field as it crossed every boundary.

## 01 · First contact

The firmware advertises two one-instance WMI devices. Acer's gaming GUID lands
in `WMBH`; the generic APGe GUID lands in `WMAA`. Both live in an SSDT rather
than the DSDT, and both use instance index zero.

That detail matters. Search only the DSDT and the methods appear not to exist.
Use instance one because the table says “one instance” and the call may appear
to work only because this firmware ignores the argument. Reverse engineering
is full of results that are correct for the wrong reason.

```text
Alien frontend
      │ Unix socket
      ▼
alien-daemon
      │ serialized /proc/acpi/call
      ▼
WMBH / WMAA → SMI / shared memory → embedded controller
```

Alien makes the daemon the sole owner of `/proc/acpi/call`. The kernel module
exposes a single global write-then-read buffer; two independent callers can
silently consume each other's replies. The daemon's mutex is not an
optimization. It is part of the protocol's correctness.

## 02 · Enter the package

Acer's certified PredatorSense 3.00.3152 package for the PH315-53 became our
challenge binary. We inventoried the installer, extracted the model plug-ins,
decompiled the managed assemblies, and disassembled the native services.

The inventory covered 617 installed files and 75 unique PE contents. We did
not promote every DLL to “reversed” merely because a tool produced assembly.
We classified the package, isolated the components on the hardware path, and
followed their call sites: the WPF application for intent, `PSSvc.exe` for
privileged dispatch, mixed C++/CLI at the bridge, and the model plug-ins for
feature gates.

The managed application revealed intent: command numbers, feature gates,
effect names, sliders, profile fields, and the order in which the UI asked for
operations. The native service revealed the boundary that mattered: how those
managed arguments were repacked into WMI values.

We then compared a later PredatorSense 3.00.3198 package. That comparison
produced the first major trap.

## 03 · The sixteen-byte decoy

The newer “Clubman” lighting path uses a longer record. It was tempting to copy
its trailing bytes—`3,1`—and call them a commit marker. The call returned
success. The keyboard still did the wrong thing.

The PH315-53 does not use Clubman. It uses the older **Covini** family.

On the exact 3.00.3152 service, the dynamic command accepts one 64-bit value,
copies its eight bytes into a 16-byte WMI array, and explicitly zeroes the
tail. The semantic record is:

```text
byte  0      1       2          3       4          5  6  7
     effect  speed   brightness flags   direction   R  G  B
```

The alleged commit bytes never existed on this model. A successful return had
only proved that the firmware accepted *some bytes*, not that those bytes
described the intended effect.

That discovery reset the project around a rule we kept for every later
feature: **status is transport evidence; hardware behavior needs its own
evidence.**

## 04 · Down the rabbit hole

The ACPI method for dynamic lighting wrote the record into a shared `PECM`
memory page and returned. No follow-up call. No delay. No notification. No
visible host-side trigger.

So we went lower.

We obtained the exact V1.07 firmware update, extracted the Intel BIOS and ENE
embedded-controller images, located the SMM WMI handler, and followed the data
through banked 8051 code. The final consumer was not Windows at all. The ENE
controller polls the shared lighting fields, compares mode, direction, and an
enable bit against cached values, and enters its reconfiguration path when one
changes.

Static-zone enablement took a different route. PredatorSense sends:

```text
8 | (zone_mask << 40)
```

WMI function 2 carries that scalar through software SMI `0xd0`; an ODM SMM
service writes the mask to the controller-facing byte and reads it back until
it matches. Bits zero through three gate the four zones.

The ghost in the machine had a polling loop.

## 05 · Command 45

PredatorSense's GPU buttons looked like one setting. They were a three-act
transaction.

For the exact RTX 2060 Mobile configuration, command 45 performs:

1. NVIDIA P0 graphics/memory offsets through a private Pstates20 path;
2. an Acer fan-table selection;
3. a WMI/EC `GPOC` value.

| Mode | Graphics | Memory | Fan table | GPOC |
|---|---:|---:|---:|---:|
| Normal | +0 MHz | +0 MHz | 1 | 0 |
| Faster | +50 MHz | +30 MHz | 2 | 1 |
| Turbo | +100 MHz | +60 MHz | 3 | 2 |

The vendor path reports only the NVIDIA setter's result and offers no
transactional rollback. Alien deliberately does more: exact PCI/DMI guards,
live range checks, readback of every leg, reverse-order rollback, explicit
risk acceptance, and an audited daemon route.

On the real machine, Normal → Faster → Turbo → Normal matched all four fields
and restored cleanly. A bounded load test remained stable but showed no
measurable performance gain. We ship the semantics, not a miracle claim.

## 06 · The profiles that did not change

PredatorSense stores encrypted Intel tuning profiles. After recovering the
profile format, the PH315-53's Normal, Fast, and Turbo files decoded to the same
28 tuning controls:

- PL1: 70 W;
- PL2: 107 W;
- short-power enable: on;
- PL1 window: 28 seconds;
- voltage offsets: zero;
- ratio fields: descriptors marked non-modifiable.

The names changed. The CPU policy did not. Alien therefore reports Linux RAPL
state and the exact OEM target but does not invent three different CPU modes
or expose an unsafe raw MSR mailbox.

## 07 · Building the safe side of the glass

Reverse engineering found operations the user should *not* receive as generic
primitives. Alien's daemon is intentionally narrow:

- exact payload and reply shapes;
- per-model and per-device guards;
- no arbitrary WMI proxy;
- no raw EC access;
- no persistent CMOS sub-index;
- one process, one socket, one serialized firmware owner;
- typed GPU operations with cooldown, audit, readback, and rollback.

The frontends never need root. They ask the daemon for characterized actions,
not for permission to improvise firmware calls.

## 08 · Proof, not vibes

We kept separate evidence lanes:

- **source/static** — package, IL, disassembly, AML, SMM, and EC control flow;
- **readback** — exact reply shape and value on the target;
- **hardware** — temperatures, RPM, clocks, and bounded-load behavior;
- **visual** — rendered GUI/TUI state inspected from fresh captures;
- **optical** — what a camera sees the keyboard or physical LCD emit.

That separation caught false positives, stale streaming frames, wrong payload
families, inferred fan modes, and “success” replies with no physical effect.
It also leaves two claims intentionally open: the full keyboard animation
matrix and LCD-overdrive panel response still need a properly framed camera.

| Claim | Strongest current evidence | Verdict |
|---|---|---|
| Fan max/auto/manual and RPM | live firmware readback plus measured ramp | verified on the reference machine |
| Four static zone colours | readback plus direct keyboard observation | optically verified on the reference machine |
| Covini effects, masks, brightness and Off | managed/native/AML/SMM/EC trace plus register state | implemented; complete optical matrix still open |
| Normal/Faster/Turbo GPU transaction | four-leg readback and bounded live load | exact semantics verified; no speed-up claim |
| CoolBoost | getter/setter/restore and A/B/A telemetry | state path verified; no sustained cooling benefit in that run |
| LCD overdrive | getter/setter/restore | protocol state verified; 240-fps panel evidence open |

## 09 · One reference, not one-model software

PH315-53 is where Alien has the strongest evidence. It is not presented as the
only machine in the project.

The two studied PredatorSense releases contain 19 model plug-ins covering 18
unique SMBIOS strings, nine code series, and six OEM protocol families. Alien
preserves those as package-mapped targets rather than flattening Covini,
Clubman, Evoque, XC90, Defender, and Spyder into one guessed protocol.

That count is **18 package-mapped models total**, including the one
live-verified reference; it is not 18 plus PH315-53.

A second tier records 36 additional PredatorSense model codes—33 laptops and
three desktops—from official Acer product evidence. Those are leads, not
support claims. Only the live reference may wear the live-verified label today;
the rest need per-machine capabilities, readback, and physical receipts.

That distinction is how the project scales without turning a successful call
on one chassis into folklore for fifty more.

## 10 · Liberation, with receipts

The result is not a wrapper around PredatorSense. It is an independent Rust
stack: core protocol library, privileged daemon, CLI, TUI, and desktop app,
packaged for native Linux systems with sandboxed frontend options.

The source is GPL-2.0-or-later. The wire notes are public. The failure modes are
documented. New models can be added by evidence tier instead of folklore.

The closed box did not open because of one cinematic exploit. It opened one
byte, one misleading success code, one disassembler window, and one physical
measurement at a time.

And now the machine answers to Linux.

---

## Continue the investigation

- [Install and use Alien](../README.md)
- [Read the protocol notes](protocol.md)
- [Compare feature coverage](parity.md)
- [Explore the model evidence catalog](model-compatibility.md)
- [View the release media kit](media/README.md)
- [Contribute a new model or hardware receipt](../CONTRIBUTING.md)

Alien is not affiliated with or endorsed by Acer. Acer, Predator, and
PredatorSense are trademarks of their respective owners and are used only to
identify interoperable hardware and software.
