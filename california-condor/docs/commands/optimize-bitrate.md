# Optimize Bitrate

Optimize bitrate for scenes that exceed normal bitrate after Target Quality. Scenes above the bitrate threshold have their quantizer raised to the rounded mean of eligible scenes only when that mean is higher than their selected value. The mean may be unmeasured, so this does not guarantee a bitrate reduction or meeting a bitrate budget. See [guide](../guide.md).

```bash
$ av1an optimize-bitrate
$ av1an optimize-bitrate --sigma-threshold 3
```

| Name                                                    | Flag                | Type    | Default |
| ------------------------------------------------------- | ------------------- | ------- | ------- |
| [Sigma Threshold](#sigma-threshold---sigma-threshold)   | `--sigma-threshold` | Integer |         |

## Sigma Threshold `--sigma-threshold`

Minimum bitrate sigma (σ) threshold for a scene to be optimized.

### Possible Values

Integer from `1` to `10`.

### Examples

- `> av1an optimize-bitrate --sigma-threshold 3`

## Edit saved settings

`info`, `set` and `reset` read, change and reset the saved bitrate-optimization settings without running the command:

- `set sigma-threshold SIGMA` — how many standard deviations above the average bitrate a scene is clamped (`1..=10`).
- `info [sigma-threshold]` prints JSON; `reset sigma-threshold` clears it, and a bare `reset` restores the whole section to its defaults.

```bash
$ av1an optimize-bitrate set sigma-threshold 4
$ av1an optimize-bitrate info sigma-threshold
$ av1an optimize-bitrate reset sigma-threshold
```

Shared semantics (validation, staleness warnings): see [Editing saved settings](config-editing.md).

## Screenshots

The screenshots below are generated from the live interface. Click any image to open it full size.

<table>
  <tr>
    <td width="50%" valign="top">
      <a href="../media/tui/help-optimize-bitrate-light.avif"><picture><source srcset="../media/tui/help-optimize-bitrate-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-optimize-bitrate-light.avif" alt="av1an optimize-bitrate --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an optimize-bitrate --help</p>
    </td>
    <td width="50%" valign="top">
      <a href="../media/tui/help-optimize-bitrate-verbose-light.avif"><picture><source srcset="../media/tui/help-optimize-bitrate-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-optimize-bitrate-verbose-light.avif" alt="av1an optimize-bitrate --help --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an optimize-bitrate --help --verbose</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="../media/tui/help-optimize-bitrate-set-light.avif"><picture><source srcset="../media/tui/help-optimize-bitrate-set-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-optimize-bitrate-set-light.avif" alt="av1an optimize-bitrate set --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an optimize-bitrate set --help</p>
    </td>
    <td width="50%" valign="top"></td>
  </tr>
</table>
