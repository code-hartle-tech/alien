# Credits and asset provenance

Short version: **there are no third-party assets on this site.** Nothing is
fetched at runtime, and nothing was downloaded to build it.

## Fonts

None bundled. The page uses the visitor's own system UI and monospace stacks
(`system-ui`, `ui-monospace` and their fallbacks). No web font is loaded, so
there is no font licence to honour and no third-party request on page load.

## Images

| Asset | Source |
|---|---|
| `alien.svg` | The project's own mark, from `assets/tech.hartle.Alien.svg` in this repository |

Everything else that looks like artwork — the motion-tracker sweep, the
scanlines, the wireframe lattice, the compartment grid, the passing shadow —
is CSS gradients, SVG primitives and blend modes. There are no raster assets
and **no video**.

No film, television or game footage appears on this site, in any form,
processed or otherwise. That includes silhouettes and blurred overlays: the
underlying frames would still be someone else's work. The shape that crosses
the display is a soft gradient mass with no outline, generated in CSS.

## Audio

### Soundtrack

| | |
|---|---|
| **Title** | Xenomorph |
| **Artist** | Dante Kuhn (`xenomorphillia`) |
| **Licence** | [Pixabay Content License](https://pixabay.com/service/license-summary/) |
| **Source** | https://pixabay.com/music/-236640/ |
| **Files** | `web/audio/alien-theme.opus`, `web/audio/alien-theme.mp3` |
| **Changes** | Loudness-normalised to −22 LUFS and re-encoded (Opus 72 kbps, MP3 112 kbps). Not otherwise edited. |

The Pixabay Content License permits commercial use and does not require
attribution. It is credited anyway — in the page footer beside the copyright
line, and here.

Supplied directly by the operator, which is also the provenance: the file came
from Pixabay's own download, not from a search result of unknown origin.

An earlier build used "They Come at Night" by morgantj under CC BY 3.0 from
ccMixter. That track and its attribution have been removed along with the
synthesised fallback bed.

### Announcer and comms

The sixteen spoken lines under `web/audio/voice/` are **rendered with
[espeak-ng](https://github.com/espeak-ng/espeak-ng)** (GPL-3.0) and processed
offline through a radio passband, broadcast compression, two short echo taps
and a slow phaser. Every line is written for this site; no dialogue is taken
from any film, game or recording.

espeak-ng rather than a system text-to-speech voice for two reasons: its
formant synthesis is already robotic, which is the timbre wanted, and its
output carries no redistribution question the way a proprietary system voice
would.

They are pre-rendered rather than spoken live by the browser because
`speechSynthesis` output cannot be routed through Web Audio — there is no
capture path for it — so it can never be filtered or reverberated and always
sounds like a screen reader. 120 KB for all sixteen.

### Interaction sounds

**Synthesised in the browser at runtime** from Web Audio oscillators and
generated noise. No sample is shipped or downloaded. `web/alien.js` holds:

- four interaction sounds — a motion-tracker ping, a wet squelch, a servo
  click and a confirmation blip;
- a convolution reverb whose impulse response is generated from decaying
  stereo noise, so the track and every interaction share one wet space;
- a limiter on the output, because layers plus a reverb tail sum past 1.0 on
  peaks and Web Audio clips as audible crackle rather than gracefully;
There is no generated background bed. The soundtrack above replaced it, so if
the track cannot play the page is quiet apart from interaction sounds.

Synthesis is used for these rather than sampling because the obvious sources
for this specific palette — sound-button sites, "movie SFX" archives — are
overwhelmingly ripped film audio, and "it was free to download" is not a
licence. The soundtrack above is different: it comes from Pixabay with a real
licence behind it.

## Background video layer

`web/video/backdrop.mp4` — 854x480, 73 s, looping.

| | |
|---|---|
| **Work** | Original animation |
| **Author** | HARTLE.TECH |
| **Tool** | DaVinci Resolve |
| **Licence** | Owned outright — first-party work, no third-party rights |

Made for this site rather than sourced, so there is nothing to licence and
nobody to attribute beyond the author.

### If it is ever replaced

Whatever goes in that slot is published on a public commercial site and needs a
licence that permits that. Pixabay, Pexels and Coverr carry usable stock
footage; the soundtrack on this page came from Pixabay. Record the title,
author, licence and source URL here, exactly as the soundtrack is recorded
above.

Footage from films, television or games may not be used — including material
labelled "free" or "scene pack", which is almost always ripped from a release,
and including silhouetted or keyed treatments of it, which are derivative works
rather than new ones.

### Performance

The CSS applies **no** `filter` and **no** `mix-blend-mode` to this layer, and
must not. An earlier build did both and took Chrome to ~12 GB: a fixed
full-viewport element that is filtered and blended forces the compositor to
allocate and re-blur a screen-sized surface every frame. Any softness belongs
in the file:

```sh
ffmpeg -i input.mp4 \
  -vf "gblur=sigma=14,scale=480:270,eq=saturation=0.25:brightness=-0.16:contrast=1.1" \
  -c:v libx264 -pix_fmt yuv420p -crf 32 -movflags +faststart -an \
  web/video/backdrop.mp4
```

## Aesthetic

The visual language is an original homage to biomechanical science-fiction
horror: acid green on near-black, scanlines, radar sweeps, thermal-vision
hovers. It uses no studio marks, no character likenesses, no film stills, no
title typography and no dialogue or score.

The project is not affiliated with, endorsed by or connected to Acer, nor to any
film studio or entertainment franchise. The name "Alien" here refers to this
software.
