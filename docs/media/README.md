# Media kit

Every file here is a capture of the real application running on the reference
Predator Helios 300. Nothing is a mock-up, a composite or a redraw.

## Demo loops

| File | |
|---|---|
| [`demo/alien-demo.gif`](demo/alien-demo.gif) | 1200 px, the desktop app across all five screens — the front-page loop |
| [`demo/alien-demo.mp4`](demo/alien-demo.mp4) · [`.webm`](demo/alien-demo.webm) | Same cut, 1440×900, for anywhere a video is preferable |
| [`demo/alien-tui-demo.gif`](demo/alien-tui-demo.gif) | 1200 px, the terminal UI updating live |
| [`demo/alien-tui-demo.mp4`](demo/alien-tui-demo.mp4) | Same cut as video |

The GUI loop is Dashboard → Fan control → Lighting → Performance → About, in
that order. The TUI has one live screen, so its loop is a single take.

## Screens

Window-only captures at 1440×900 logical, rendered at 2× — no desktop
background, no window borders, no cursor.

| File | Screen |
|---|---|
| [`screenshots/alien-gui-dashboard.png`](screenshots/alien-gui-dashboard.png) | Telemetry, history graphs, profiles |
| [`screenshots/alien-gui-fans.png`](screenshots/alien-gui-fans.png) | Fan modes and per-fan duty |
| [`screenshots/alien-gui-lighting.png`](screenshots/alien-gui-lighting.png) | Four-zone colour and effects |
| [`screenshots/alien-gui-performance.png`](screenshots/alien-gui-performance.png) | Power state and guarded GPU modes |
| [`screenshots/alien-gui-about.png`](screenshots/alien-gui-about.png) | Build and capability summary |
| [`screenshots/alien-tui-rich.png`](screenshots/alien-tui-rich.png) | Terminal UI, full width |

Older captures of states the current harness cannot reach without input — the
model catalog dialog, first run, a lost daemon link, the startup splash and the
narrow responsive layout — are kept alongside these at their original ultrawide
size. They are labelled by filename and are not used on the front page.

---

## How this is captured

Not by filming a desktop. The GUI runs on a **private headless Sway
compositor** at an exact canvas size, so the output has no wallpaper, no
borders and no cursor by construction, and one command reproduces the whole set
after a UI change. `research/tools/record-demo.sh` in the development
repository does it in a single pass — stills, GUI loop and TUI loop together —
so every asset shares one canvas, one theme and one telemetry state.

Six things it has to get right, each of which silently produces useless output:

- **Never drive the live session.** An earlier attempt captured five frames of
  the *lock screen* — the session was locked, and the synthetic keystrokes went
  into the password field.
- **Sway, not Hyprland.** Hyprland's aquamarine backend fails headless on this
  hardware with `CBackend::create() failed`.
- **Headless wlroots has no input devices**, so the seat advertises no keyboard,
  clients never bind `wl_keyboard`, and `wtype` exits 0 with nothing happening.
  Each screen therefore gets its own instance on its own workspace, switched
  over the sway IPC socket, which needs no seat at all.
- **The launch delay must outlast the window map.** A window that maps after
  the next workspace switch lands on the wrong one, and two tiled half-width
  windows sharing a screen is a failure no file-size check can see. The harness
  asserts exactly one window per workspace before capturing anything.
- **`winit` dlopens `libwayland-client.so.0` by name.** Nothing on the default
  loader path provides it here, and the GUI exits with `NoWaylandLib` before it
  draws; `LD_LIBRARY_PATH` has to name the store paths explicitly.
- **Warm up first.** The history graphs fill at 1 Hz. Capture immediately and
  the dashboard reads `7 / 120` samples over three empty plots. Every instance
  is launched up front and warmed in parallel for 80 seconds.

The TUI is a pty application, so it is recorded inside a real terminal on the
same compositor rather than through a separate asciicast pipeline — same
canvas, same 2× scale, same crispness as everything else.

Encoding uses **gifski at quality 100**, not ffmpeg's palette filters. Below
100, gifski's lossy inter-frame reuse ghosts the previous screen's text into the
dark areas of the next one at every cut. `wf-recorder` writes damage-driven
VFR, so the concatenation is re-encoded to CFR once and every derivative comes
from that master.

## A note on video in Markdown

GitHub **strips the `<video>` tag**. Verified against GitHub's own renderer:
`POST /markdown` returns zero `<video>` elements for every form of the tag,
whether `src` points at `raw.githubusercontent.com`, a repo-relative path, or
`github.com/.../raw/...`.

There is exactly one route to a real player — upload through the web UI and
paste the resulting `https://github.com/user-attachments/assets/…` URL as a bare
link. That works, and was verified end to end. It is **not** what the front page
uses, because the asset then lives outside the repository with no documented
retention, and a purge would blank the hero.

The committed GIFs are the durable answer: they are in the tree, they play
everywhere including the mobile app, and nothing outside this repository has to
stay alive for the front page to work.
