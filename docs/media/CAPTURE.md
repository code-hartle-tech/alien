# Alien public media capture matrix

This is the reproducible capture contract for the public Alien media set. The
published files are direct compositor captures of a frozen build running on
the reference NixOS host, not mock-ups. Capturing a rendered control is
evidence for presentation and layout only; it is not evidence that the
physical keyboard, panel, fan, or GPU changed state.

## Frozen-build boundary

Before capture, record all of the following in the final receipt section of
this file:

- Public Alien source commit.
- NixOS generation number and Alien package closure. Do not publish the private
  system-configuration commit or the host name embedded in its system closure.
- SHA-256 of the deployed GUI and TUI executables.
- monitor mode, application/window geometry, and capture-tool versions.
- start/end timestamps and the post-capture daemon health check.

Do not reuse a historical screenshot after a GUI or TUI source change.

## Screenshot matrix

| Public file | Required rendered state | Target size | Interaction boundary |
|---|---|---:|---|
| `screenshots/alien-gui-splash-contacting.png` | Animated Alien mark splash while the first daemon attempt is pending | at least 1800x1000 | User-owned stalled QA socket only; production daemon untouched |
| `screenshots/alien-gui-dashboard.png` | Connected Dashboard with all telemetry cards and histories | at least 1800x1000 | Read-only |
| `screenshots/alien-gui-fans.png` | Fan Control page | at least 1800x1000 | Select page only; do not change mode or duty |
| `screenshots/alien-gui-lighting.png` | Lighting page and zone preview | at least 1800x1000 | Select page only; do not change effect, zones, brightness, or backlight |
| `screenshots/alien-gui-performance-initial.png` | Performance before the manual OEM GPU getter | at least 1800x1000 | Read-only |
| `screenshots/alien-gui-performance.png` | Performance after the explicit compound GPU getter | at least 1800x1000 | Getter only; no profile, GPU, CoolBoost, LCD, or raw-flag setter |
| `screenshots/alien-gui-about.png` | About/help page with project and target-boundary copy | at least 1800x1000 | Read-only |
| `screenshots/alien-gui-model-catalog.png` | Searchable model-catalog dialog with both support tiers visible | at least 1800x1000 | Read-only; scroll is allowed |
| `screenshots/alien-gui-first-run.png` | First-run/no-daemon recovery card | at least 1800x1000 | Deliberately absent user-owned QA socket |
| `screenshots/alien-gui-link-lost.png` | Established session after only its user-owned relay is stopped | at least 1800x1000 | Production daemon untouched; controls remain disabled |
| `screenshots/alien-gui-performance-narrow.png` | Responsive Performance composition at a constrained size | at least 960x720 | Read-only or getter only |
| `screenshots/alien-tui-rich.png` | Full rich TUI | at least 1200x800 | Read-only or one explicit getter refresh |
| `screenshots/alien-tui-tight.png` | Tight TUI composition | at least 900x650 | Read-only |
| `screenshots/alien-tui-confirmation.png` | Complete unsupported OEM GPU confirmation text | at least 1200x800 | Open, capture, then cancel with Escape; never confirm |

The wide GUI is launched on `DP-1` with `WINIT_X11_SCALE_FACTOR=2` so type,
icons, and vector art are rasterized densely. A dedicated clean workspace is
used. `grim` captures the exact live-window rectangle at PNG compression level
9. Screenshots do not include the cursor.

## Motion matrix

| Public file | Contents | Delivery target |
|---|---|---|
| `videos/alien-gui-walkthrough.mp4` | Splash; Dashboard; Fans; Lighting; Performance before/after its explicit getter; About; model-catalog scroll; first-run; link-lost; and narrow responsive Performance | H.264 High, 4:2:0, fast-start, 2560px-wide maximum |
| `videos/alien-gui-walkthrough.webm` | Same edited GUI master | VP9, 4:2:0, 2560px-wide maximum |
| `videos/alien-tui-walkthrough.mp4` | Rich TUI, confirmation/cancel boundary, cancellation result, and tight layout | H.264 High, 4:2:0, fast-start, 2560px-wide maximum |
| `videos/alien-tui-walkthrough.webm` | Same edited TUI master | VP9, 4:2:0, 2560px-wide maximum |
| `demo/alien-demo.gif` | Short, silent splash-to-dashboard-to-catalog loop | 1280px wide, 15fps, optimized palette, under 15MiB |

Record with `wf-recorder` on the monitor that contains the application. Preserve
the high-bitrate capture outside Git; publish only the visually lossless
derivatives above. Record without audio. If the GPU encoder cannot consume the
compositor frames directly, use `--no-dmabuf` and software H.264 instead of
changing the graphics stack.

## Visual acceptance gates

Inspect every final PNG at original resolution and inspect at least the first,
middle, and last frame of every final video. Reject and recapture if any item
has clipping, stale pixels, unrelated windows, cursor residue, unreadable
copy, missing focus state, animation tearing, an accidental setter result, or
a model-support claim broader than the catalog evidence.

Mechanical acceptance also requires GUI PNGs to meet their stated minimums,
rich/confirmation TUI PNGs to be at least 1200x800, the tight TUI PNG to be at
least 900x650, the GIF to remain below 15MiB, and every tracked video to remain
below GitHub's 100MiB per-file ceiling.

The final receipt must include dimensions, duration/frame rate for motion,
byte size, SHA-256, and a short per-artifact vision verdict. `scripts/verify-media.sh`
performs the mechanical half of that gate; vision inspection closes the visual
half.

## Final Alien 0.5.0 receipt — 2026-08-13

Verdict: **PASS for the captured software surfaces**. The set below comes from
Alien commit `086e30a` as deployed in NixOS generation 117. Commit `ab756d8`
adds only the media verifier/caption follow-up and does not change application
pixels. The filtered Nix source matched `086e30a` byte-for-byte before capture.

- System closure: `/nix/store/flj2qgg0krbiiwqvsmhic80l510km78w-nixos-system-studio-26.05.20260807.ee48b14`
- Alien closure: `/nix/store/sqm3jfc5i87pghq9k8b3dk2zf8dm470z-alien-0.5.0`
- GUI launcher SHA-256: `b540247a2ae02734f0650e00192428e76df26aa3b092f45819a46ef4d84743f4`
- GUI ELF SHA-256: `ddd25bd284c143083cbc31e6ecdad2b21adaa5128d722fd7257705e11ce0caee`
- TUI SHA-256: `08a5648d1b7f4a95cc2c199b4c29965e4885a7017631c602db1b80df30e47045`
- Daemon SHA-256: `ac87480145c8057acaeca36f880a8f58dcdde5a5d922d9a82e6374c84f647573`
- Capture surface: `DP-1` 3440x1440 at 144.001 Hz, scale 1.25 for wide
  PNGs; `eDP-1` 1920x1080 at 143.998 Hz for cursor-free motion.
- Toolchain: grim 1.5.0, wf-recorder 0.6.0 (`--no-dmabuf`, libx264),
  FFmpeg 8.1.2. Public H.264/VP9 files are silent 30 fps 4:2:0 derivatives.
- Capture window: 2026-08-13 08:46–18:50 WEST. The late link-lost and narrow
  recaptures replace rejected pointer-bearing candidates.
- Daemon after capture: active, PID 1136, zero restarts. Capture used read-only
  telemetry, one explicit GUI getter and one explicit TUI getter. The guarded
  TUI mutation prompt was cancelled with Escape; no setter was sent.

The machine-readable public manifest is [`verification.tsv`](verification.tsv).
All 14 PNGs were inspected at original resolution. The GUI and TUI videos were
checked at one-second intervals and at full-resolution boundary frames. The
accepted set has no cursor residue, wallpaper-only interval, clipping,
profile/status overlap, metadata collision, or accidental write result. The
automotive gauges and lighting RGB swatches remain intact; the new dense GUI
and TUI telemetry graphs are visible in the Dashboard, Fans, and rich TUI
captures.

Splash QA is retained under `qa/`: `splash-normal-a.png` and
`splash-normal-b.png` are distinct frames with the orbit/progress animation
advanced; `splash-reduced-a.png` and `splash-reduced-b.png` are byte-identical;
and the connected/handoff pair shows the original Alien mark handing off to
the live Dashboard. The GUI/TUI/GIF contact sheets are one-frame-per-second
visual indexes of the published motion.

This receipt proves rendered software state only. Keyboard lighting effects,
mask/brightness/Off behavior and LCD overdrive remain physical-camera proof
boundaries; software readback and these screen captures do not establish those
optical results.
