# Contributing to Alien

Alien exists because Acer's gaming controls should not disappear when Windows
does. Contributions are welcome from users, reverse engineers, designers,
packagers, and maintainers—especially people with Predator hardware we have not
yet been able to test in person.

Good first contributions include:

- a complete `alien doctor` report from a catalogued model;
- packaging fixes for a distribution you can test;
- reproducible read-only protocol evidence;
- GUI/TUI accessibility and responsive-layout improvements;
- documentation, translations, screenshots, and release-media corrections.

## Before you start

- Read the [protocol notes](docs/protocol.md) and the
  [feature-parity matrix](docs/parity.md).
- Run `alien doctor` and include its output when reporting a new machine.
- Never guess a firmware payload. A successful WMI status does not prove that a
  command is correct or that the hardware changed.
- Keep observed, inferred, and physically verified behavior clearly separated.
- Do not attach proprietary Acer binaries, installers, decompiler output, or
  firmware images to issues or pull requests.

## Development setup

The workspace lives in `code/` and its locked dependency graph requires Rust
1.88 or newer.

```sh
cd code
cargo fmt --all -- --check
cargo test --workspace --all-targets --locked
cargo test --workspace --doc --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

On NixOS or any machine with Nix installed, `nix develop` supplies the Rust and
desktop build dependencies.

The source layout is intentionally small:

| Path | Responsibility |
|---|---|
| `code/alien-core` | Protocol types, transports, policy and profiles |
| `code/alien-daemon` | Privileged socket owner and mutation allowlist |
| `code/alien-cli` | Scriptable command-line interface |
| `code/alien-tui` | Keyboard-first terminal interface |
| `code/alien-gui` | egui desktop application |
| `packaging/` | Native and sandboxed distribution recipes |
| `docs/` | Public protocol, compatibility, media and investigation notes |

## Reporting a new model

Please open an issue with:

1. Exact model, board, BIOS version, CPU, GPU PCI ID, and keyboard type.
2. `alien doctor` and `alien capabilities` output.
3. Which PredatorSense package Acer publishes for the model.
4. A read-only result first. Do not start by probing unknown setters.
5. A camera observation when the claim concerns lighting, fans, or a physical
   display effect. Screenshots prove interface state, not emitted light.

Compatibility is tracked by evidence level:

- **Live verified** — exercised on real hardware with readback and, where
  applicable, physical observation.
- **Package mapped** — Acer ships a model plug-in whose protocol family has
  been decoded, but the model still needs a community hardware receipt.
- **Candidate** — PredatorSense is available for the model, but its protocol
  family has not yet been mapped to Alien.

## Adding a firmware operation

Every new operation must include:

- an exact model or protocol-family guard;
- exact payload and reply-shape validation;
- a daemon policy entry that exposes no broader raw primitive;
- unit tests for accepted and rejected shapes;
- getter/readback handling when the firmware offers it;
- rollback or an explicit explanation of why rollback is impossible;
- user-facing wording that does not claim a physical effect before it has been
  observed.

Persistent CMOS writes, arbitrary EC access, and generic WMI passthroughs are
out of scope.

## Pull requests

Keep changes focused and explain:

- what changed;
- why the evidence supports it;
- which hardware and software paths were tested;
- what remains unverified.

Run the full check sequence above and include screenshots for GUI/TUI changes.
Inspect those screenshots at original resolution; rendering a control proves
presentation only, not that a keyboard, fan, GPU or panel physically changed.
Keep generated build output, vendor packages, and private machine data out of
commits.

By contributing, you agree that your contribution is licensed under
[GPL-2.0-or-later](LICENSE).
