# Brand assets

`logo.svg` is the mark: two bars and a check. The bars hold, the check proves.
It is three plain shapes with no font behind it, so it renders the same
everywhere, and it is the one drawing the other files reuse: the same bars and
the same amber check sit in `social-preview.svg`.

`social-preview.svg` is the source of `social-preview.png`. It says what the
project does in one sentence ("merges only on proof that a guard bites") and
draws the proof a merge waits for: a test that passes, the guard removed, the
same test failing, then the receipt.

`merge-path.svg` is the diagram in the README. It follows one pull request
through the three stages (the agent, `maintainer-merge verify`,
`maintainer-merge merge`) and shows what an unattended run cannot do. Edit the
SVG by hand; there is no generator.

The palette is the one the project's terminal output uses: background `#14171c`,
text `#e8e6e1`, muted `#9aa3b2`, amber `#e0a44c`, green `#7fb069`, red `#d9776b`.

## The social preview

`social-preview.png` is 1280x640, which is what GitHub recommends (minimum
640x320, maximum 1MB), cropped to 2:1. Every pixel that matters sits at least
130px from an edge, clear of the 80px border in GitHub's repository card
template. `tests/run-tests.sh` decodes the PNG and fails if anything drifts
inside that margin, so the background stays one flat colour.

Setting it is a **web UI step and has no API**: Settings, General, Social
preview, Upload an image. Regenerate the PNG from the SVG rather than editing it:

```sh
google-chrome --headless=new --disable-gpu --hide-scrollbars \
  --screenshot=assets/social-preview.png --window-size=1280,640 \
  "file://$PWD/assets/social-preview.html"
```

The SVG names DejaVu Sans Mono, which is what the PNG in this directory was
rendered with. A machine without it will substitute another monospace font and
the line breaks will move. `merge-path.svg` asks for the system monospace font
and leaves slack in every line, so a substitute still fits.
