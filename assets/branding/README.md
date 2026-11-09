# RIM branding

The mark is an R in three pieces that never touch: the stem, the bowl and the leg, held together
by one node in the gap between them. It stands for independent servers forming one network. The
wordmark sets that R beside an I drawn as a lightning bolt and an M, all at one cap height on a
214 × 80 grid. Corners are square except the R's stem (radius 4 on the 100-unit grid).

| File | Use |
|---|---|
| `rim-mark.svg` / `rim-mark-inverse.svg` | The R alone on a 100-unit grid: ink `#111318` with node `#E04A12` for light grounds / paper `#EDEEF0` with node `#F2541B` for dark grounds, transparent |
| `rim-lockup.svg` / `rim-lockup-inverse.svg` | The RIM wordmark, the same two colourings, with the bolt in the node's colour |
| `rim-app-icon.svg` | The paper mark on a dark rounded square (`#181B22` to `#0B0C10`) with a faint orange edge |
| `rim-app-icon-1024.png` | Source for the desktop and tray icon set |
| `rim-favicon.svg` / `favicon.ico` | The SVG is the mark redrawn on a 16-unit grid, ink or paper by the browser's colour scheme; the ICO is the app icon at 16, 32 and 48 px |
| `rim-lockup.png` | 1200 px raster of the ink wordmark, transparent |
| `rim-splash.svg` / `rim-splash-inverse.svg` | The wordmark with a glow behind the bolt that pulses on a 1.8 s loop, for splash and loading screens only. Under `prefers-reduced-motion` the glow holds still |

Accent colour: signal orange `#F2541B` on dark grounds and `#E04A12` on light ones, where
`#F2541B` falls just under the 3:1 contrast that graphics need. It is used for the node and the
bolt and nowhere else in the logo.

The geometry, in the masters' own units:

- Stem `rect x=12 y=8 width=20 height=84 rx=4`; bowl `M40 8 H60 A24 24 0 0 1 60 56 H40 Z M40 25 V39 H58.5 A7 7 0 0 0 58.5 25 Z` (even-odd); leg `M54 64 H75 L92 92 H71 Z`; node `circle cx=43 cy=68 r=6`.
- In the wordmark the R takes `translate(-3.144 1.904) scale(.762)`; the bolt is `M115 8 L84 44 H100 L91 72 L122 36 H106 Z` and the M `M140 72 V8 H158 L175 40 L192 8 H210 V72 H194 V36 L180 62 H170 L156 36 V72 Z`.

To regenerate the Tauri icons in `rim-ui`:

```sh
cd ../rim-ui && npx tauri icon ../rim-server/assets/branding/rim-app-icon-1024.png
```
