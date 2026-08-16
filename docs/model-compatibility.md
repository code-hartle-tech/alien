# Acer model evidence catalog

Alien's recovered PredatorSense evidence contains **19 Acer model plug-ins for
18 unique SMBIOS model strings, across 9 model-code series**. The complete set
is below; PH315-53 is the live-verified reference machine, not the only model
represented by Acer's packages.

This is a tiered evidence catalog, not a blanket hardware support claim:

1. **Live-verified:** PH315-53 with BIOS V1.07 is the current Alien reference.
2. **Package-mapped:** a recovered PredatorSense plug-in provides a concrete
   OEM family/profile mapping, but only PH315-53 has live Alien validation.
3. **Other PredatorSense model:** official Acer product evidence associates the
   model with PredatorSense, but its plug-in and protocol are not yet mapped
   for Alien. Alien compatibility is unverified.

Neither package nor candidate membership proves that an Alien getter, setter,
fan layout, sensor ID, lighting effect or physical outcome works on that
machine, and catalog membership never bypasses a runtime or DMI safety gate.
Run `alien doctor` on the actual laptop; its getters remain authoritative.

## Derived catalog

`M`, `L` and `F` are the package's raw `MachineType.Type`,
`LightingType.Type` and `FanDtail.Type`. `PerKey` and `GPU OC` are likewise the
raw advertised flags, rather than claims about an observed keyboard or safe
clock behavior.

| Code series | SMBIOS model string | PredatorSense package | OEM family | L | F | PerKey | GPU OC | Evidence level |
|---|---|---:|---|---:|---:|---:|---:|---|
| PH315 | Predator PH315-53 | 3.00.3152 | M1 Covini | 1 | 1 | 0 | 1 | live-verified Alien reference |
| PH315 | Predator PH315-54 | 3.00.3198 | M8 Clubman | 1 | 1 | 0 | 1 | Acer package plug-in |
| PH315 | Predator PH315-55 | 3.00.3198 | M10 Evoque | 1 | 1 | 1 | 1 | Acer package plug-in |
| PH315 | Predator PH315-55s | 3.00.3198 | M10 Evoque | 1 | 1 | 1 | 1 | Acer package plug-in |
| PH317 | Predator PH317-54 | 3.00.3152 | M1 Covini | 1 | 1 | 0 | 1 | Acer package plug-in |
| PH317 | Predator PH317-55 | 3.00.3198 | M8 Clubman | 1 | 1 | 0 | 1 | Acer package plug-in |
| PH317 | Predator PH317-56 | 3.00.3198 | M10 Evoque | 1 | 1 | 1 | 1 | Acer package plug-in |
| PH517 | Predator PH517-52 | 3.00.3152 | M1 Covini | 1 | 1 | 0 | 1 | Acer package plug-in |
| PH517 | Predator PH517-52 | 3.00.3198 | M9 XC90 | 1 | 1 | 1 | 1 | Acer package plug-in; revised profile |
| PH517 | Predator PH517-53 | 3.00.3198 | M10 Evoque | 1 | 1 | 1 | 1 | Acer package plug-in |
| PH717 | Predator PH717-72 | 3.00.3152 | M5 Defender | 1 | 1 | 1 | 1 | Acer package plug-in |
| PT314 | Predator PT314-51s | 3.00.3198 | M8 Clubman | 1 | 1 | 0 | 1 | Acer package plug-in |
| PT314 | Predator PT314-52s | 3.00.3198 | M8 Clubman | 1 | 1 | 0 | 1 | Acer package plug-in |
| PT315 | Predator PT315-52 | 3.00.3152 | M1 Covini | 1 | 1 | 0 | 1 | Acer package plug-in |
| PT315 | Predator PT315-53 | 3.00.3198 | M8 Clubman | 1 | 1 | 0 | 1 | Acer package plug-in |
| PT316 | Predator PT316-51s | 3.00.3198 | M8 Clubman | 1 | 1 | 0 | 1 | Acer package plug-in |
| PT515 | Predator PT515-52 | 3.00.3152 | M6 Spyder | 1 | 1 | 1 | 1 | Acer package plug-in |
| PT516 | Predator PT516-51s | 3.00.3198 | M8 Clubman | 1 | 2 | 0 | 1 | Acer package plug-in |
| PT516 | Predator PT516-52s | 3.00.3198 | M8 Clubman | 1 | 1 | 0 | 1 | Acer package plug-in |

PH517-52 deliberately has two rows. Acer changed it from machine type 1 with
`PerKey=0` in 3.00.3152 to machine type 9 with `PerKey=1` in 3.00.3198. Alien
does not flatten that package drift into one invented profile.

## Other PredatorSense models — Alien compatibility unverified

Official Acer sources establish 36 additional Predator model codes—33 laptops
and 3 desktop PCs—in the PredatorSense ecosystem. They supply no Alien protocol
mapping and are research candidates only. This 2026-08-12 snapshot is not
presented as an immutable list of all Acer hardware.

- **Predator Helios / Helios Neo:** PH16-71, PH16-72, PH16-73; PH18-I71,
  PH18-71, PH18-72, PH18-73; PHN14-51, PHN14-71; PHN16-I31, PHN16-I71,
  PHN16-71, PHN16-72, PHN16-73; PHN16S-I51, PHN16S-I71, PHN16S-71;
  PHN18-I71, PHN18-71, PHN18-72; PH3D15-71; PH18P-73.
- **Predator Triton:** PT14-51, PT14-52T, PT16-51, PTN16-51, PTX17-71.
- **Earlier models on Acer's PredatorSense mobile list:** PH315-52, PH317-53,
  PH717-71, PT315-51, PT515-51, PT917-71.
- **Predator Orion desktop PCs on that list:** PO3-620, PO5-615s, PO9-920.
  Acer prints the last code as `PO9 920`; the catalog normalises the separator
  to match Acer's usual model-code form and retains this note.

The 26 current-laptop entries come from Acer's [Predator and Nitro GPU
table](https://www.acer.com/us-en/predator/laptops/predator-and-nitro-gaming-laptop-gpu-specs),
whose Predator rows include `PredatorSense Boost OC`. Six earlier codes come
from Acer's [PredatorSense mobile compatibility
list](https://community.acer.com/en/kb/articles/12700-predatorsense-mobile-application-compatibility),
which also supplies the three Orion desktop codes,
and Acer's [Helios 18P AI specification](https://news.acer.com/acer-unleashes-predator-helios-18p-ai-hybrid-gaming-laptop-for-work-and-play)
explicitly names PH18P-73 with PredatorSense 5.0.

## Derivation and evidence boundary

The catalog was generated from the model-directory names and INI values in the
locally extracted `plugs/` trees for PredatorSense 3.00.3152 and 3.00.3198. A
local analysis tool normalized each model's `Feature.ini`, `HW_Support.ini`,
`PredatorSense.ini`, and `AppManager_List.ini` into a JSON matrix. Those local
analysis inputs are not part of the public snapshot; no proprietary Acer binary
or decompiler output is shipped or linked from it.

The public table above is the reproducible projection: it retains every input
field used for the tier assignment, package version, duplicate-profile note,
and official Acer source for the candidate tier. A contributor can compare a
lawfully obtained package against those fields without needing any non-public
artifact from this investigation.

The About screen's searchable **Model evidence catalog** is a compact,
checked-in projection of those two generated matrices. It retains package
versions and raw profile fields so later evidence can be compared without
silently broadening live-support claims.
