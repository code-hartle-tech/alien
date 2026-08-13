# Alien Instagram carousel

Ten editable 4:5 slides introducing Alien's PredatorSense reverse-engineering
story. The source is plain HTML/CSS and uses the same bundled fonts and logo as
the application.

```sh
./render.sh
```

This produces exact 1080×1350 PNG files under `png/`. Render only after the
release screenshots exist in `docs/media/screenshots/`; slide 8 uses the actual
GUI and TUI captures. `caption.md` contains the final post copy, direct field-
report link, hashtags, and accessible alt text for every slide.

Publish the numbered PNG files in order. The source intentionally keeps all
text inside a 76-pixel-or-larger safe margin for Instagram's 4:5 feed crop.
Before posting, compare the rendered set against
[`../../media/CAPTURE.md`](../../media/CAPTURE.md): all ten images must be
1080×1350, and the embedded application captures must come from the same frozen
build as the public media kit.
