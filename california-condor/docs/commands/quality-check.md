# Quality Check

Measure the quality of the concatenated output per scene (triggers TUI). See [guide](../guide.md) and [type reference](../types.md).

```bash
$ av1an quality-check
$ av1an quality-check --metric ssimulacra2 --profile standard
```

| Name                                                  | Flag          | Type        | Default       |
| ----------------------------------------------------- | ------------- | ----------- | ------------- |
| [Input](#input--i)                                    | `-i`, `--input` | Path      | Config input  |
| [Decoder](#decoder---decoder)                         | `--decoder`   | [`DECODER`](../types/decoder.md)   | `bestsource`  |
| [Filters](#filters---filters)                         | `--filters`   | [Filters](../types/filters.md)     | `resize:scaler=bicubic;format=yuv420p10le` |
| [VapourSynth Arguments](#vapoursynth-arguments---vs-args) | `--vs-args` | [String List](../types/vs-args.md) |             |
| [Metric](#metric---metric)                            | `--metric`    | [`METRIC`](../types/quality-metric.md)    | `ssimulacra2` |
| [Profile](#profile---profile)                         | `--profile`   | [`PROFILE`](../types/quality-profile.md)   | `standard`    |

## Input `-i`

Path to the input file. Can be a video or VapourSynth script (`.py` or `.vpy`).

## Decoder `--decoder`

Method used for decoding the input video. Methods besides `ffms2` require external VapourSynth plugins. Defaults to `bestsource`. See [Decoder](../types/decoder.md).

### Possible Values

- `bestsource`
- `vs-ffms2`
- `lsmash`
- `dgdecnv`
- `ffms2`

## Filters `--filters`

VapourSynth filters to apply to the input. See [Filters](../types/filters.md).

### Default

If not specified, `resize:scaler=bicubic;format=yuv420p10le` (YUV 4:2:0 10-bit) is used.

## VapourSynth Arguments `--vs-args`

VapourSynth/Python arguments to pass to the input script environment. See [VS Args](../types/vs-args.md).

## Metric `--metric`

The quality metric used for Quality Check. See [Quality Metric](../types/quality-metric.md).

### Possible Values

- `ssimulacra2` - Structural SIMilarity Unveiling Local And Compression Related Artifacts
- `butteraugli` - butteraugli Infinite-Norm
- `butteraugli-3` - butteraugli 3-Norm
- `xpsnr` - Extended Perceptually Weighted PSNR (min of Y, U, V)
- `cvvdp` - ColorVideoVDP
- `vmaf` - Video Multi-Method Assessment Fusion (requires `libvmaf`)

### Default

If not specified, `ssimulacra2` is used.

## Profile `--profile`

The preset profile to choose the Quality Check Strategy and Statistic. See [Quality Profile](../types/quality-profile.md).

### Possible Values

- `fast` - Measures the average of the middle 11 frames
- `standard` - Measures the root-mean-square of the middle 25% of frames
- `slow` - Measures the 10th percentile of all frames

### Default

If not specified, `standard` is used.

## Edit saved settings

`info`, `set` and `reset` read, change and reset the saved quality-check settings without running the command:

- `set metric METRIC` — the metric measured per scene (same values as `--metric`).
- `set strategy KIND [FRAMES]` — which frames are probed: `whole`, `skip FRAMES`, or `subset [--position start|middle|end] [--length-frames N | --length-percent P]`. Values and flags of another kind are rejected.
- `set statistic KIND [VALUE] …` — how the probe scores become one result: `mean`, `median`, `harmonic`, `percentile PERCENT` (or `--percentile P`), `standard-deviation-distance SIGMA` (or `--sigma S`), `mode`, `minimum`, `maximum`, `root-mean-square`.
- `set input [--path P] [--decoder D] [--filters …] [--vs-args …] [--stream-concurrently [on|off]]` — the input Quality Check measures.
- `info [metric|strategy|statistic|input]` prints JSON; `reset KEY` resets one of them, and `reset` with no key drops the whole optional section back to `null`.

```bash
$ av1an quality-check set strategy subset --position end --length-percent 25
$ av1an quality-check set statistic percentile 10
$ av1an quality-check reset statistic
$ av1an quality-check reset
```

Shared semantics (validation, staleness warnings): see [Editing saved settings](config-editing.md).

## Screenshots

The screenshots below are generated from the live interface. Click any image to open it full size.

<table>
  <tr>
    <td width="50%" valign="top">
      <a href="../media/tui/help-quality-check-light.avif"><picture><source srcset="../media/tui/help-quality-check-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-quality-check-light.avif" alt="av1an quality-check --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an quality-check --help</p>
    </td>
    <td width="50%" valign="top">
      <a href="../media/tui/help-quality-check-verbose-light.avif"><picture><source srcset="../media/tui/help-quality-check-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-quality-check-verbose-light.avif" alt="av1an quality-check --help --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an quality-check --help --verbose</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="../media/tui/help-quality-check-set-light.avif"><picture><source srcset="../media/tui/help-quality-check-set-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-quality-check-set-light.avif" alt="av1an quality-check set --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an quality-check set --help</p>
    </td>
    <td width="50%" valign="top"></td>
  </tr>
</table>
