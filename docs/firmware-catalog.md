# Acer firmware catalog

**What each BIOS version actually does**, read out of Acer's own firmware images
rather than from forum memory.

Firmware questions — *is undervolting locked on my BIOS? does upgrading break
anything? which version should I be on?* — currently get answered with folklore,
and the folklore is often wrong. This page answers them from the vendor's own
UEFI setup forms.

Every row below was produced by extracting the IFR (Internal Forms
Representation) from the firmware image and reading the question's shipped
default. The method is at the bottom so anyone can check the work.

---

## Predator Helios 300 · PH315-53 (board FH53M)

### Overclocking Lock — controls CPU undervolting

| BIOS | released | qid | `CpuSetup` offset | default | ships |
|---|---|---|---|---|---|
| 1.07 | 2020-08-28 | 0x017A | `0xDA` | 1 | 🔒 **LOCKED** |
| 1.08 | 2020-12-16 | 0x017A | `0xDA` | 1 | 🔒 **LOCKED** |
| 2.02 | 2021-03-24 | 0x017B | `0xDA` | 0 | 🔓 **UNLOCKED** |
| 2.04 | 2021-07-26 | 0x017B | `0xDA` | 0 | 🔓 **UNLOCKED** |

The firmware's own help text for the field:

> *"Enable/Disable Overclocking Lock (BIT 20) in FLEX_RATIO(194) MSR"*

**The widely repeated claim that 2.04 re-locks undervolting is false.** Both
2.0x releases ship the lock disabled. Confirmed on hardware after flashing
1.07 → 2.04: `rdmsr -f 20:20 0x194` reads `0`, and MSR `0x150` accepts writes
with readback on both the CORE and CACHE planes.

**Why the myth persists.** A BIOS update *preserves* existing NVRAM setup values
rather than resetting them. Anyone locked on 1.07 or 1.08 who upgraded stayed
locked, and attributed it to the new version. Applying the `0xDA` edit once on
2.0x makes it stick.

### The 1.07 overvolt is real

| BIOS | CORE offset | CACHE offset |
|---|---|---|
| 1.07 | **+49.8 mV** | **+49.8 mV** |
| 2.04 | 0.0 mV | 0.0 mV |

Measured by reading MSR `0x150` **without writing to it first**. This matters:
the usual writability probe writes a value and then reads back, so it reports
its own write rather than what the firmware applied — which is why this stayed
folklore for years.

Those are the two planes sharing the core rail, which is why they match.

**Effect of removing it:** idle went from 93–95 °C spikes with a constantly
advancing throttle counter to **58 °C with zero throttle events**.

**Undervolting further is not worth it on this machine.** A/B/A at 0 mV /
−50 mV / 0 mV with fans pinned measured **+0.8 %** clock against the fair
comparison — noise. Throughput drift across the run (7.2 %) exceeded the
apparent effect (4.7 %). The first 50 mV was already banked by leaving 1.07.

### TCC offset — do not touch

`MSR 0x1A2` ships Tjmax 100 °C with a **TCC offset of 8**, so the CPU throttles
at 92 °C rather than its actual limit. `MSR_PLATFORM_INFO` bit 30 is set, so the
field *is* writable. It looks like 8 °C of free headroom.

Setting it to 0 measured **−45.3 %**, with the two stock blocks agreeing within
0.3 %. With the ceiling raised the chip ran *cooler* and at *half* the clock,
close to its minimum ratio — removing the CPU's graduated throttle appears to
hand control to the EC's bang-bang protection.

Acer's offset is load-bearing. Leave it alone.

### ⚠ Upgrade hazards

**The flash resets BIOS setup to factory defaults**, and one default stops Linux
booting.

- **Storage mode reverts to Optane/RST**, which hides the NVMe behind Intel RST
  remapping. Linux cannot see the root device and panics on every kernel — it
  looks like a kernel fault and is not.
- The relevant field is `PCIe Storage Dev On Port 1` ("Enable/Disable RST Pcie
  Storage Remapping") at `PchSetup` offset `0xB8`, default enabled on 2.04.
- Also reset: Fast Boot, Wake-on-LAN, lid behaviour, USB configuration, network
  boot.

**Those options are hidden from the default menu.** Press **`Ctrl+S`** on the
Main page to reveal the Advanced items. Photograph every setting *before*
flashing.

### Upgrading from Linux — no USB stick needed

Acer ships these as UEFI capsules inside Windows driver packages. The INF
targets `UEFI\RES_{4B0D4F8B-ACD0-409D-B16A-2D6851075B17}`, which is the ESRT
entry the machine advertises, so `fwupd` can apply them.

```sh
fwupdtool install <cab> 4b0d4f8b-acd0-409d-b16a-2d6851075b17
```

LVFS carries nothing for this model, so the cab has to be built by hand from the
catalog package. `fwupd` refuses unsigned locally-built cabs, which is why
`fwupdtool` rather than `fwupdmgr`. Its closing *"does not currently allow
updates"* is a second-pass artifact, not a failure — check for
`Update State: Needs reboot` instead.

Capsule versions: 1.07 `0x59303107`, 1.08 `0x59303108`, **2.02 `0x59319202`,
2.04 `0x59319204`**. The `0x5930…` scheme holds only through the 1.0x line.

---

## Where the images come from

**Microsoft Update Catalog**, published as `Insyde Software - Firmware -
5.34.<major>.<minor>`. Acer's own site blocks scripted access.

⚠ **The 2.0x packages are dual-board.** Each ships `BIOS_2.0x_FH53M.FD` *and*
`BIOS_2.0x_GH53M.FD` (GUID `51F90FEF-41F4-47DC-8C46-B0CC2BD554EC`, a different
machine). Flashing the wrong one writes another board's firmware.

Verify you have the right image: each carries a `$BVDT$` table with the board
and version (`V2.04 / FH53M`), and the capsule GUID appears in the FH53M images
and not the GH53M ones.

## How to check this yourself

```sh
# 1. The .FD is a PE executable, not a flash image — uefi_firmware cannot parse
#    it and will silently find nothing. The 16 MiB SPI image is embedded
#    contiguously: find the Intel flash descriptor and carve from 0x10 before it.
python3 -c "
d=open('BIOS_2.04_FH53M.FD','rb').read()
i=d.find(bytes.fromhex('5aa5f00f'))-0x10
open('2.04-spi.bin','wb').write(d[i:i+16*1024*1024])"

# 2. Read a named setup question: its offset and its shipped default.
python3 research/tools/ifr-question.py 2.04-spi.bin "Overclocking Lock"

# 3. Or sweep every setup varstore for a term.
python3 research/tools/ifr-search.py 2.04-spi.bin "sata mode" "vmd" "rst"
```

Two traps in the parser worth knowing if you write your own: opcode scope must
be tracked by *depth* rather than "the next `END`", and `EFI_IFR_ONE_OF_OPTION`
opcodes are **7 bytes** — requiring 8 silently skips every boolean option and
reports "no default stated".

## Contributing a model

This catalog covers one board because that is the hardware available to verify
against. Adding another needs its firmware image and the extraction above — no
physical machine required for the setup-form data, though hardware confirmation
is what turns a table row into a fact.

Reports welcome: see [CONTRIBUTING.md](../CONTRIBUTING.md).
