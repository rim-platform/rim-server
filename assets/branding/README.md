# RIM branding

The wordmark is "Connectors": every letter is two pieces joined a different way. The R's stem
plugs into its bowl with a `>` tab, the I is cut by a `v` so its top stacks on its bottom, and
the M splits into two mirrored halves. All joints share one gap (5 units) and one tab angle, on a
100-unit grid with an 80-unit cap height and 19-unit stems. The mark is the R alone.

| File | Use |
|---|---|
| `rim-mark.svg` / `rim-mark-inverse.svg` | The R alone, ink `#111318` / paper `#EDEEF0`, transparent |
| `rim-lockup.svg` / `rim-lockup-inverse.svg` | The RIM wordmark |
| `rim-app-icon.svg` | Paper R on an ink rounded square |
| `rim-app-icon-1024.png` | Source for the desktop icon set |
| `rim-favicon.svg` / `favicon.ico` | App icon redrawn with wider cuts so the joints survive at 16–48 px |
| `rim-lockup.png` | 1200 px raster of the wordmark |
| `rim-splash.svg` / `rim-splash-inverse.svg` | R, I and M each in a circle, bouncing like a typing indicator; for splash and loading only, never as the logo. The R is centred on its ink mass, not its bounding box |

Accent colour: signal orange `#F2541B`, used for the splash's M circle and nowhere in the logo itself.

To regenerate the Tauri icons in `rim-ui`:

```sh
cd ../rim-ui && npx tauri icon ../rim-server/assets/branding/rim-app-icon-1024.png
```
