# Built-in terminal themes: sources and licences

Rex Linux ships 27 terminal themes (`src/terminal_theme/builtin.rs`): 17 dark and 10 light.
This file records where each one's colours come from. A theme is added to the table in the
source only together with a row here, and only when its authors publish it under a permissive
licence. Release packages carry this file as `licenses/TERMINAL-THEMES.md`.

A built-in theme's id (`rex/<id>` in `workspace.json`) never changes once released.

## Rex Linux's own

| Theme | Id | Notes |
| --- | --- | --- |
| Merino Dark | `merino-dark` | Designed for Rex Linux; the default dark theme of a light and dark pair. |
| Merino Light | `merino-light` | Designed for Rex Linux; the default light theme of a light and dark pair. |
| Midnight | `midnight` | The Midnight chrome's navy behind Rex's default text and palette. |

Merino is not derived from another theme, and none of its colours were taken from the macOS
Rex app's themes of the same names. In both, text is at least 7:1 on the background, palette
colours 1 to 6 and 9 to 14 are at least 4.5:1 (WCAG AA), and white and bright black are at
least 3.5:1; `cargo test` checks this (`merino_is_the_default_pair_and_meets_aa_in_every_colour`).
They are covered by this repository's MIT licence.

## Themes by other authors

The colours are the published palettes of the projects below, as the Ghostty theme file of the
same theme draws them (background, foreground, cursor, cursor text, selection and palette
colours 0 to 15). Ghostty's theme files come from the
[iTerm2-Color-Schemes](https://github.com/mbadolato/iTerm2-Color-Schemes) collection (MIT).
Gruvbox Dark, paired with the Gruvbox Dark chrome, sets its background, text, cursor and palette
only and leaves the selection to the chrome.

| Themes | Ids | Project | Copyright | Licence |
| --- | --- | --- | --- | --- |
| Ayu Dark, Ayu Mirage, Ayu Light | `ayu-dark`, `ayu-mirage`, `ayu-light` | [ayu-theme/ayu-colors](https://github.com/ayu-theme/ayu-colors) | Ike Ku and contributors | MIT † |
| Catppuccin Frappé, Macchiato, Mocha and Latte | `catppuccin-frappe`, `catppuccin-macchiato`, `catppuccin-mocha`, `catppuccin-latte` | [catppuccin/catppuccin](https://github.com/catppuccin/catppuccin) | Copyright (c) 2021 Catppuccin | MIT |
| Dracula | `dracula` | [dracula/dracula-theme](https://github.com/dracula/dracula-theme) | Dracula Theme | MIT † |
| Everforest Dark, Everforest Light (medium contrast) | `everforest-dark`, `everforest-light` | [sainnhe/everforest](https://github.com/sainnhe/everforest) | Copyright (c) 2019 sainnhe | MIT |
| Flexoki Dark, Flexoki Light | `flexoki-dark`, `flexoki-light` | [kepano/flexoki](https://github.com/kepano/flexoki) | Copyright (c) 2023 Steph Ango | MIT |
| Gruvbox Dark, Gruvbox Light | `gruvbox-dark`, `gruvbox-light` | [morhetz/gruvbox](https://github.com/morhetz/gruvbox) | Pavel Pertsev | MIT/X11 † |
| Kanagawa Wave, Kanagawa Lotus | `kanagawa-wave`, `kanagawa-lotus` | [rebelot/kanagawa.nvim](https://github.com/rebelot/kanagawa.nvim) | Copyright (c) 2021 Tommaso Laurenzi | MIT |
| Nord | `nord` | [nordtheme/nord](https://github.com/nordtheme/nord) | Sven Greb | MIT † |
| One Dark, One Light | `one-dark`, `one-light` | Atom's One Dark and One Light syntax themes ([atom/atom](https://github.com/atom/atom)) | GitHub Inc. | MIT † |
| Rosé Pine, Rosé Pine Moon, Rosé Pine Dawn | `rose-pine`, `rose-pine-moon`, `rose-pine-dawn` | [rose-pine/rose-pine-theme](https://github.com/rose-pine/rose-pine-theme) | Copyright (c) 2023 Rosé Pine | MIT |
| Solarized Dark, Solarized Light | `solarized-dark`, `solarized-light` | [altercation/solarized](https://github.com/altercation/solarized) | Ethan Schoonover | MIT † |

† The licence and copyright holder in this row were written down from the project's published
terms without a fresh look at its licence file when the table was made (November 2026). The rows
without the mark were checked against the licence file of the same author's project. Check the
marked rows against the linked repositories before a release that relies on them.

The MIT permission notice that applies to the rows above:

> Permission is hereby granted, free of charge, to any person obtaining a copy of this software
> and associated documentation files (the "Software"), to deal in the Software without
> restriction, including without limitation the rights to use, copy, modify, merge, publish,
> distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the
> Software is furnished to do so, subject to the following conditions:
>
> The above copyright notice and this permission notice shall be included in all copies or
> substantial portions of the Software.
>
> THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING
> BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
> NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM,
> DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
> OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

## Left out

- **Tokyo Night.** The Ghostty theme files follow
  [folke/tokyonight.nvim](https://github.com/folke/tokyonight.nvim), which is Apache-2.0.
  Shipping it means carrying that licence's text in every package; it is left out until that
  is set up. With Ghostty installed, its TokyoNight themes are in the library under Ghostty.
- **GitHub Light and Dark** (Primer, MIT). Left out because the macOS Rex app's default light
  theme uses very similar colours, and this set is meant to be independent of that app's.

## Legibility

`cargo test` checks every built-in theme: its scheme (`light` or `dark`) is the one its
background is, its text is at least 4:1 on its background, and selected text is at least 2:1
on the selection. The published palettes are otherwise kept as they are: several light themes
have a yellow or a bright colour under 3:1 on their background.
