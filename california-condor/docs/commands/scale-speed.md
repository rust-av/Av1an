# Scale Speed

Apply encoder speed based on each scene's quantizer. Two points use linear interpolation; three or more retain the natural cubic spline. Quantizers must be finite and unique, regardless of point order. An empty saved list disables scaling; otherwise at least two points are required. See [guide](../guide.md).

Outside the quantizer range, the corresponding endpoint's speed is used. Inside it, the estimate is clamped to the configured minimum and maximum speed before rounding to an integer. For example, `20=4 40=8` gives speed 6 at quantizer 30 and speed 8 at quantizer 39. Scenes without a quantizer are unchanged; nonfinite scene quantizers or estimates fail without applying any scene edits.

Edits check the frontend's known encoder settings without probing binaries: RAV1E speeds are `0..=10`, SVT-AV1 presets `-1..=13`, and the existing X264/X265/VVenC named-preset mapping still clamps its endpoints. This does not validate every native restriction: AOM, AVM, VPX and arbitrary FFmpeg encoders have no universal numeric speed range here. The selected encoder must accept the resulting settings when encoding.

```bash
$ av1an scale-speed --encoder svt-av1 --quantizers 20 --speeds 5 --quantizers 30 --speeds 4 --quantizers 55 --speeds 2
```

| Name                                      | Flag           | Type         | Default |
| ----------------------------------------- | -------------- | ------------ | ------- |
| [Quantizers](#quantizers---quantizers)    | `--quantizers` | Integer List |         |
| [Speeds](#speeds---speeds)                | `--speeds`     | Integer List |         |

## Quantizers `--quantizers`

Unique integer quantizer values for speed-quantizer pairs. Must match number of speeds. Requires `--speeds`. Saved JSON may contain fractional quantizer knots; edits that only replace speeds preserve those values.

### Examples

- `> av1an scale-speed --encoder svt-av1 --quantizers 20 --speeds 5 --quantizers 30 --speeds 4 --quantizers 55 --speeds 2`
- `> av1an scale-speed --encoder aom --quantizers 10 --speeds 6 --quantizers 25 --speeds 4 --quantizers 40 --speeds 3`

## Speeds `--speeds`

Speed values for speed-quantizer pairs. Must match number of quantizers. Requires `--quantizers`.

### Examples

- `> av1an scale-speed --encoder svt-av1 --quantizers 20 --speeds 5 --quantizers 30 --speeds 4 --quantizers 55 --speeds 2`
- `> av1an scale-speed --encoder aom --quantizers 10 --speeds 6 --quantizers 25 --speeds 4 --quantizers 40 --speeds 3`

## Edit saved settings

`info`, `set` and `reset` read, change and reset the saved speed-scaling settings without running the command:

- `set speed-quantizers QUANTIZER=SPEED …` — replaces the pairs Convex Hull interpolates between; provide at least two, with unique quantizers (they are stored sorted, speed first).
- `info [speed-quantizers]` prints the pairs as JSON; `reset speed-quantizers` (or a bare `reset`) empties them, which turns speed scaling off.

```bash
$ av1an scale-speed set speed-quantizers 20=6 35=4 55=2
$ av1an scale-speed info speed-quantizers
$ av1an scale-speed reset speed-quantizers
```

Shared semantics (validation, staleness warnings): see [Editing saved settings](config-editing.md).

## Screenshots

The screenshots below are generated from the live interface. Click any image to open it full size.

<table>
  <tr>
    <td width="50%" valign="top">
      <a href="../media/tui/help-scale-speed-light.avif"><picture><source srcset="../media/tui/help-scale-speed-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-scale-speed-light.avif" alt="av1an scale-speed --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an scale-speed --help</p>
    </td>
    <td width="50%" valign="top">
      <a href="../media/tui/help-scale-speed-verbose-light.avif"><picture><source srcset="../media/tui/help-scale-speed-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-scale-speed-verbose-light.avif" alt="av1an scale-speed --help --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an scale-speed --help --verbose</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="../media/tui/help-scale-speed-set-light.avif"><picture><source srcset="../media/tui/help-scale-speed-set-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-scale-speed-set-light.avif" alt="av1an scale-speed set --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an scale-speed set --help</p>
    </td>
    <td width="50%" valign="top"></td>
  </tr>
</table>
