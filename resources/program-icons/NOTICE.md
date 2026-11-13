# Program icons

The tiles and glyphs in `tiles/` and `glyphs/` are Superlogical's Rex icon set
([icons.superlogical.com](https://icons.superlogical.com)), the icons the official Rex clients
show for the program running in a terminal. The artwork is Superlogical's, and the logos it
draws are trademarks of their owners; Rex Rune uses them only to say which program a pane is
running, as Rex does. **They are not covered by this repository's MIT license** or the
dependencies' licenses. If you redistribute or package Rex Rune, check that doing so is
acceptable to Superlogical, or replace these files; ask Superlogical for the terms before reusing
them outside Rex Rune.

`scripts/program-icons.py` made these files from a copy of the set. It changed:

- Colours, from Display P3 and OKLCH mixes to sRGB, and long gradients to the stops that
  draw the same within a 255th.
- The glyph, a CSS mask over the tile, to an SVG alpha mask, and each drawing to a square
  canvas.
- Blurred mesh gradients (Antigravity, Charm, Crush, Gemini, Homey, Julia, Minimal, OrbStack
  and Pack) to a coarse grid of their colours under the same blur.
- Cua's bitmap glyph to traced rectangles.
- The second layers of Modal, Podman and Porter, whose copies were cut short, to shapes traced
  from the set's own renders, and Moon's to the complete mask in the same cell.
