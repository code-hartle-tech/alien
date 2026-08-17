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
| **Title** | They Come at Night |
| **Artist** | morgantj |
| **Licence** | [CC BY 3.0](https://creativecommons.org/licenses/by/3.0/) |
| **Source** | https://ccmixter.org/files/morgantj/25838 |
| **Files** | `web/audio/alien-theme.opus`, `web/audio/alien-theme.mp3` |
| **Changes** | Loudness-normalised to −22 LUFS and re-encoded (Opus 72 kbps, MP3 112 kbps). Not otherwise edited. |

CC BY requires attribution, so it appears in three places: the page footer, this
file, and the copyright frame inside the MP3.

The licence was verified rather than assumed. ccMixter's own site content is
CC BY-NC, and that string appears in the footer of every track page — reading
the page casually gives you the wrong answer. The track's actual licence is the
`rel="license"` link on its page, and it is corroborated by the ID3 copyright
frame in the original download:

```
2010 morgantj Licensed to the public under
http://creativecommons.org/licenses/by/3.0/
Verify at http://ccmixter.org/files/morgantj/25838
```

CC BY 3.0 permits commercial use, which matters here — this is a company's
site, so an NC-licensed track would not have been usable no matter how well it
fitted.

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
- a five-layer generative bed — sub, wobble bass, air, drips, metallic hits —
  used **only as a fallback** if the soundtrack cannot be played.

Synthesis was chosen for these rather than sampling because the obvious sources
for this specific palette — sound-button sites, "movie SFX" archives — are
overwhelmingly ripped film audio, and "it was free to download" is not a
licence. The soundtrack above is different: it has a real, verifiable licence.

## Aesthetic

The visual language is an original homage to biomechanical science-fiction
horror: acid green on near-black, scanlines, radar sweeps, thermal-vision
hovers. It uses no studio marks, no character likenesses, no film stills, no
title typography and no dialogue or score.

The project is not affiliated with, endorsed by or connected to Acer, nor to any
film studio or entertainment franchise. The name "Alien" here refers to this
software.
