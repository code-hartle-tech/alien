# Ready-to-post caption

We followed a button in Acer PredatorSense through a privileged Windows
service, undocumented WMI methods, ACPI, System Management Mode, shared memory,
and finally an ENE embedded controller.

What came back was **Alien**: an independent, from-scratch, GPL-licensed Linux
interoperability stack with a CLI, TUI, desktop app, guarded daemon, exact
payload validation, and hardware receipts instead of “it returned success”
folklore.

The investigation found:

• the PH315-53's real eight-byte Covini lighting record
• the SMM/EC path that gates four keyboard zones
• PredatorSense's compound Normal/Faster/Turbo GPU transaction
• identical OEM CPU policies hiding behind three profile names
• several convincing success codes that did not prove physical behavior

Read the complete field report:

**github.com/code-hartle-tech/alien/blob/main/docs/reverse-engineering-predatorsense.md**

Source, documentation, full-resolution screenshots and videos:

**github.com/code-hartle-tech/alien**

Alien is an independent interoperability project and is not affiliated with or
endorsed by Acer.

#opensource #linux #rustlang #reverseengineering #firmware #acpi #wmi
#linuxgaming #predator #hardwarehacking #foss #hartletech

## Slide alt text

1. Alien project mark beside the headline “We cracked PredatorSense,”
   introducing an open-source reverse-engineering story.
2. Green binary texture behind a terminal-themed explanation that Acer gaming
   controls were trapped behind Windows-only software.
3. Five-layer diagram from the PredatorSense application through its native
   service, WMI/ACPI, SMM, and embedded controller.
4. Diagram of the exact eight-byte Covini keyboard-lighting record and a note
   that the suspected trailing commit marker was absent.
5. Four cards tracing dynamic and static lighting through shared memory, SMM,
   and the ENE controller.
6. Table of Normal, Faster, and Turbo GPU offsets, fan-table values, and GPOC
   values, with Alien's guarded rollback design.
7. Diagram of Alien's GUI, TUI, CLI, typed socket, and privileged daemon.
8. High-resolution Alien dashboard and terminal UI screenshots.
9. Test and evidence metrics emphasizing measured results and open optical
   verification.
10. Alien logo, public GitHub link, GPL license, and invitation to contribute
    more Acer hardware evidence.
