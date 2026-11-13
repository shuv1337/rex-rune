# Symbols Nerd Font

`SymbolsNerdFont-Regular.ttf` is the symbols-only font of [Nerd Fonts](https://github.com/ryanoasis/nerd-fonts)
release [v3.5.1](https://github.com/ryanoasis/nerd-fonts/releases/tag/v3.5.1), unchanged, from
its `NerdFontsSymbolsOnly.tar.xz` asset. The client embeds it as a glyph fallback for the
terminal grid, after the named fallbacks and IBM Plex Mono and before the system's automatic
fallbacks (`src/app/fonts.rs`), so a prompt drawn for a Nerd Font has its branch, Powerline
and file icons whichever family the terminal uses.

| File | SHA-256 |
| --- | --- |
| `NerdFontsSymbolsOnly.tar.xz` (release asset) | `01172f37db8543edb102e5cb5c64101c9f4686630804d49b419aa07b23a69996` |
| `SymbolsNerdFont-Regular.ttf` | `2839f0a572d4559f3f17a6fb74b8772e183f0c0a47150998ab194932cad55829` |

The same archive's `SymbolsNerdFontMono-Regular.ttf` was not chosen: it gives every glyph a
1 em advance and centres it there, and Rex's grid places a glyph from its cell's left edge, so
one-cell glyphs such as U+E0A0 drew half a cell to the right, into the next cell. The record is
[docs/parity-2026-11/M7-fonts.md](../../docs/parity-2026-11/M7-fonts.md).

## License

Nerd Fonts distributes the font under the MIT license in
[`SymbolsNerdFont-LICENSE`](SymbolsNerdFont-LICENSE), copied from the archive. Its glyphs come
from the icon sets below; the archive's own README lists them with these licenses.

| Icon set | Upstream | Version | License |
| --- | --- | --- | --- |
| Codicons | https://github.com/microsoft/vscode-codicons | 0.0.45 | CC BY 4.0 |
| Devicons | https://github.com/devicons/devicon | 2.17.0 | MIT |
| extraglyphs | https://github.com/source-foundry/Hack | - | MIT |
| Font Awesome | https://github.com/FortAwesome/Font-Awesome | 6.5.1 | CC BY 4.0 |
| Font Awesome Extension | https://github.com/AndreLZGava/font-awesome-extension | 0.0.3 | MIT |
| Font Logos | https://github.com/lukas-w/font-logos | 1.3.0 | unlicensed |
| MaterialDesign | https://github.com/Templarian/MaterialDesign-Font | Oct 6, 2022 | Apache 2.0 |
| Octicons | https://github.com/primer/octicons | 18.3.0 | MIT |
| Seti and original | https://github.com/jesseweed/seti-ui | 0.8.1 | MIT |
| Pomicons | https://github.com/gabrielelana/pomicons | 1.001 | OFL 1.1 RFN |
| Powerline Extra | https://github.com/ryanoasis/powerline-extra-symbols | 1.200 | MIT |
| Powerline Symbols | https://github.com/powerline/powerline | 1.000 (ca 2013) | MIT |
| Power Symbols IEC | https://github.com/jloughry/Unicode | Feb 2015 | MIT |
| Weather Icons | https://github.com/erikflowers/weather-icons | 2.0.10 (1.100) | OFL 1.1 |

To update it, download the release's `NerdFontsSymbolsOnly.tar.xz`, check it against the
release's published SHA-256 digest, replace the font and license, and update the version,
hashes and table here.
