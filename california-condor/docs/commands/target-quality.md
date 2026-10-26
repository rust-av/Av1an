# Target Quality

Calculate the optimum quantizer per scene for a given metric target (triggers TUI). See [guide](../guide.md) and [type reference](../types.md).

```bash
$ condor target-quality --metric ssimulacra2 --target 85
$ condor target-quality -i input.mp4 --metric butteraugli --target 90 --profile standard --min-q 10 --max-q 50
```

| Name                                                  | Flag          | Type        | Default       |
| ----------------------------------------------------- | ------------- | ----------- | ------------- |
| [Input](#input--i)                                    | `-i`, `--input` | Path      | Config input  |
| [Decoder](#decoder---decoder)                         | `--decoder`   | [`DECODER`](../types/decoder.md)   | `bestsource`  |
| [Filters](#filters---filters)                         | `--filters`   | [Filters](../types/filters.md)     |               |
| [VapourSynth Arguments](#vapoursynth-arguments---vs-args) | `--vs-args` | [String List](../types/vs-args.md) |             |
| [Encoder Parameters](#encoder-parameters---params)    | `--params`    | [String](../types/encoder-params.md)      | Based on Encoder |
| [Metric](#metric---metric)                            | `--metric`    | [`METRIC`](../types/quality-metric.md)    | `ssimulacra2` |
| [Target](#target---target)                            | `--target`    | Float       |               |
| [Minimum Quantizer](#minimum-quantizer---min-q)       | `--min-q`     | Integer     | Based on Encoder |
| [Maximum Quantizer](#maximum-quantizer---max-q)       | `--max-q`     | Integer     | Based on Encoder |
| [Profile](#profile---profile)                         | `--profile`   | [`PROFILE`](../types/quality-profile.md)   | `standard`    |

## Input `-i`

Path to the input file to encode and measure quality in Target Quality. Can be a video or VapourSynth script (`.py` or `.vpy`).

### Examples

- `> condor target-quality -i ./video.mp4 --metric ssimulacra2 --target 85`

## Decoder `--decoder`

Method used for decoding the input video. Methods besides `ffms2` require external VapourSynth plugins. Defaults to `bestsource`. See [Decoder](../types/decoder.md).

### Possible Values

- `bestsource`
- `vs-ffms2`
- `lsmash`
- `dgdecnv`
- `ffms2`

## Filters `--filters`

VapourSynth filters to apply to the Target Quality input. See [Filters](../types/filters.md).

## VapourSynth Arguments `--vs-args`

VapourSynth/Python arguments to pass to the input script environment. See [VS Args](../types/vs-args.md).

## Encoder Parameters `--params`

Parameters for the video encoder used in Target Quality. Passed directly to the encoder binary, merged with Condor defaults; psychovisual parameters are omitted. See [Encoder Parameters](../types/encoder-params.md).

### Examples

- `> condor target-quality --params "--preset 2 --crf 24 --aq-mode 0"`

## Metric `--metric`

The quality metric used for Target Quality. See [Quality Metric](../types/quality-metric.md).

### Possible Values

- `ssimulacra2` - Structural SIMilarity Unveiling Local And Compression Related Artifacts
- `butteraugli` - butteraugli Infinite-Norm
- `butteraugli-3` - butteraugli 3-Norm
- `xpsnr` - Extended Perceptually Weighted PSNR (min of Y, U, V)
- `cvvdp` - ColorVideoVDP
- `vmaf` - Video Multi-Method Assessment Fusion (requires `libvmaf`)

### Default

If not specified, `ssimulacra2` is used.

## Target `--target`

The quality metric score that Target Quality will aim for.

### Examples

- `> condor target-quality --metric ssimulacra2 --target 85`

## Minimum Quantizer `--min-q`

The lowest quantizer for Target Quality to try when searching for the optimal quantizer.

### Default

Default depends on the specified encoder (`--encoder`).

## Maximum Quantizer `--max-q`

The highest quantizer for Target Quality to try when searching for the optimal quantizer.

### Default

Default depends on the specified encoder (`--encoder`).

## Profile `--profile`

The preset profile to choose the Target Quality Probe Strategy and Statistic. See [Quality Profile](../types/quality-profile.md).

### Possible Values

- `fast` - Measures the average of the middle 11 frames
- `standard` - Measures the root-mean-square of the middle 25% of frames
- `slow` - Measures the 10th percentile of all frames

### Default

If not specified, `standard` is used.

## Edit saved settings

`info`, `set` and `reset` read, change and reset the saved Target Quality settings without running the command:

- `set metric METRIC` — the quality metric the search measures (same values as `--metric`). Switching the metric resets its target range.
- `set target SCORE` — folds the score into the metric's acceptable range; `reset target` restores the metric's default range.
- `set maximum-probes N` — probes a scene may take (`≥ 1`).
- `set quantizer-range MIN-MAX` — the quantizers the search may probe, as one value (`18-30`); rejected unless `MIN ≤ MAX`.
- `set interpolators BELOW ABOVE` — the spline methods used below and above a probe value (`linear`, `quadratic`, `natural`, `fritsch-carlson`, `pchip`, `catmull`, `akima`, `cubic-polynomial`).
- `set strategy KIND [FRAMES]` — the probing strategy: `whole`, `skip FRAMES`, or `subset [--position start|middle|end] [--length-frames N | --length-percent P]` (same values as `quality-check set strategy`).
- `set statistic KIND [VALUE] …` — how probe scores become one result: `mean`, `median`, `harmonic`, `percentile PERCENT` (or `--percentile P`), `standard-deviation-distance SIGMA` (or `--sigma S`), `mode`, `minimum`, `maximum`, `root-mean-square`.
- `set params "--crf 16 --tune 0"` — the encoder parameters the probes encode with (same syntax as `--params`); pairs you omit keep their value, `reset params` falls back to the encoder defaults.
- `set input …` / `set metric-input …` — the input the search encodes and measures, and the input only the metric measures.
- `info [KEY]` prints one key as JSON; `reset KEY` resets it, and `reset` with no key drops the whole optional section back to `null`.

```bash
$ condor target-quality set target 85
$ condor target-quality set quantizer-range 18-30
$ condor target-quality set interpolators fritsch-carlson pchip
$ condor target-quality set strategy subset --position end --length-percent 25
$ condor target-quality set params "--crf 16 --tune 0"
$ condor target-quality reset target
```

Shared semantics (validation, staleness warnings): see [Editing saved settings](config-editing.md).

## Screenshots

The screenshots below are generated from the live interface. Click any image to open it full size.

<table>
  <tr>
    <td width="50%" valign="top">
      <a href="../media/tui/target-quality-encoding-light.avif"><picture><source srcset="../media/tui/target-quality-encoding-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/target-quality-encoding-light.avif" alt="Target Quality — encoding" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">Target Quality — encoding</p>
    </td>
    <td width="50%" valign="top">
      <a href="../media/tui/target-quality-comparing-light.avif"><picture><source srcset="../media/tui/target-quality-comparing-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/target-quality-comparing-light.avif" alt="Target Quality — comparing" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">Target Quality — comparing</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="../media/tui/help-target-quality-light.avif"><picture><source srcset="../media/tui/help-target-quality-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-target-quality-light.avif" alt="condor target-quality --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">condor target-quality --help</p>
    </td>
    <td width="50%" valign="top">
      <a href="../media/tui/help-target-quality-verbose-light.avif"><picture><source srcset="../media/tui/help-target-quality-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-target-quality-verbose-light.avif" alt="condor target-quality --help --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">condor target-quality --help --verbose</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="../media/tui/help-target-quality-set-light.avif"><picture><source srcset="../media/tui/help-target-quality-set-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-target-quality-set-light.avif" alt="condor target-quality set --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">condor target-quality set --help</p>
    </td>
    <td width="50%" valign="top"></td>
  </tr>
</table>
