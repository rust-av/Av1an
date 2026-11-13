# Detect Noise

Detect noise per scene (triggers TUI). See [guide](../guide.md) and [type reference](../types.md).

```bash
$ av1an detect-noise
$ av1an detect-noise --reference-filters "wnnm:sigma=3.0,0.0,0.0;"
```

| Name                                                              | Flag                  | Type        | Default                  |
| ----------------------------------------------------------------- | --------------------- | ----------- | ------------------------ |
| [Input](#input---input)                                           | `--input`             | Path        | Config input             |
| [Decoder](#decoder---decoder)                                     | `--decoder`           | [`DECODER`](../types/decoder.md)   | `bestsource`             |
| [VapourSynth Arguments](#vapoursynth-arguments---vs-args)         | `--vs-args`           | [String List](../types/vs-args.md) |                          |
| [Reference Filters](#reference-filters---reference-filters)       | `--reference-filters` | [Filters](../types/filters.md)     | `wnnm:sigma=3.0,0.0,0.0;` |
| [Denoised Filters](#denoised-filters---denoised-filters)          | `--denoised-filters`  | [Filters](../types/filters.md)     | `wnnm:sigma=6.0,0.0,0.0;` |

## Input `--input`

Path to the input file to detect noise. Can be a video or VapourSynth script (`.py` or `.vpy`).

### Examples

- `> av1an detect-noise --input ./video.mp4`

## Decoder `--decoder`

Method used for decoding the input video. Methods besides `ffms2` require external VapourSynth plugins. Defaults to `bestsource`. See [Decoder](../types/decoder.md).

### Possible Values

- `bestsource`
- `vs-ffms2`
- `lsmash`
- `dgdecnv`
- `ffms2`

## VapourSynth Arguments `--vs-args`

VapourSynth/Python arguments to pass to the input script environment. See [VS Args](../types/vs-args.md).

### Examples

- `> av1an detect-noise --vs-args "denoiser=primary"`

## Reference Filters `--reference-filters`

VapourSynth filters to apply to the Reference VideoNode. See [Filters](../types/filters.md).

### Default

If not specified, `wnnm:sigma=3.0,0.0,0.0;` is used.

## Denoised Filters `--denoised-filters`

VapourSynth filters to apply to the Denoised VideoNode. See [Filters](../types/filters.md).

### Default

If not specified, `wnnm:sigma=6.0,0.0,0.0;` is used.

## Edit saved settings

`info`, `set` and `reset` read, change and reset the saved noise-detection settings without running the command:

- `set input [--path P] [--decoder D] [--filters …] [--vs-args …] [--stream-concurrently [on|off]]` — the input the Noise Detector reads instead of the main input.
- `set reference-filters FILTER …` — replaces the chain that builds the noisy reference (default: one `wnnm` with `sigma 3`). With no filters, or with `none`, `empty`, `clear` or `remove`, the chain is emptied.
- `set denoised-filters FILTER …` — replaces the denoised chain (default: `wnnm` with `sigma 6`). Same syntax.
- `info [input|reference-filters|denoised-filters]` prints JSON; `reset reference-filters` restores the default chain (and says how to empty it instead); `reset` with no key drops the whole optional section back to `null`.

```bash
$ av1an detect-noise set reference-filters "resize:width=960;" "wnnm:sigma=3.0,0.0,0.0;"
$ av1an detect-noise set reference-filters none
$ av1an detect-noise reset reference-filters
$ av1an detect-noise reset
```

Shared semantics (validation, staleness warnings): see [Editing saved settings](config-editing.md).

## Screenshots

The screenshots below are generated from the live interface. Click any image to open it full size.

<table>
  <tr>
    <td width="50%" valign="top">
      <a href="../media/tui/noise-detection-light.avif"><picture><source srcset="../media/tui/noise-detection-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/noise-detection-light.avif" alt="Detecting noise" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">Detecting noise</p>
    </td>
    <td width="50%" valign="top">
      <a href="../media/tui/help-detect-noise-light.avif"><picture><source srcset="../media/tui/help-detect-noise-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-detect-noise-light.avif" alt="av1an detect-noise --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an detect-noise --help</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="../media/tui/help-detect-noise-verbose-light.avif"><picture><source srcset="../media/tui/help-detect-noise-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-detect-noise-verbose-light.avif" alt="av1an detect-noise --help --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an detect-noise --help --verbose</p>
    </td>
    <td width="50%" valign="top">
      <a href="../media/tui/help-detect-noise-set-light.avif"><picture><source srcset="../media/tui/help-detect-noise-set-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-detect-noise-set-light.avif" alt="av1an detect-noise set --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an detect-noise set --help</p>
    </td>
  </tr>
</table>
