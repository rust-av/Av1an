# Init

Create a new `condor.json` configuration, or edit the top-level settings of an existing one. See [guide](../guide.md), [configuration](../configuration.md), and [type reference](../types.md).

```bash
$ condor init "input.mp4" "output.mkv"
$ condor init input.mp4 output.mkv --encoder svt-av1 --target-metric ssimulacra2 --target 85
$ condor init set output final.mkv
```

Creating a configuration needs `INPUT` and `OUTPUT`; the `info`, `set` and `reset` subcommands edit an existing one instead. The input and output of a running pipeline are changed here — no other command owns them.

| Name                                                        | Flag              | Type      | Default                |
| ----------------------------------------------------------- | ----------------- | --------- | ---------------------- |
| [Input](#input)                                             | `INPUT`           | Path      |                        |
| [Output](#output)                                           | `OUTPUT`          | Path      |                        |
| [Decoder](#decoder---decoder)                               | `--decoder`       | [`DECODER`](../types/decoder.md) | `bestsource`           |
| [Filters](#filters---filters)                               | `--filters`       | [Filters](../types/filters.md)   | `resize:scaler=bicubic;format=yuv420p10le` |
| [VapourSynth Arguments](#vapoursynth-arguments---vs-args)   | `--vs-args`       | [String List](../types/vs-args.md) |                      |
| [Concatenation](#concatenation---concat)                     | `--concat`        | [`CONCAT`](../types/concatenation.md)  | `mkvmerge`             |
| [Discard](#discard---discard)                               | `--discard`       | [String List](#discard---discard)      | nothing                |
| [Workers](#workers--w---workers)                            | `-w`, `--workers` | Integer   | Benchmarker decides    |
| [Encoder](#encoder--e---encoder)                             | `-e`, `--encoder` | [`ENCODER`](../types/encoder.md) | `svt-av1`              |
| [Encoder Parameters](#encoder-parameters---params)          | `--params`        | [String](../types/encoder-params.md)    | Based on Encoder       |
| [Photon Noise](#photon-noise---photon-noise)                | `--photon-noise`  | [Integer](../types/photon-noise.md)   |                        |
| [Disable Colour Data](#disable-colour-data---disable-color-data) | `--disable-color-data` |  | off             |
| [Target Metric](#target-metric---target-metric)              | `--target-metric` | [`METRIC`](../types/quality-metric.md)  | `ssimulacra2`          |
| [Target](#target---target)                                   | `--target`        | Float     |                        |

## Input

Path to the input file to encode. Can be a video or VapourSynth script (`.py` or `.vpy`).

### Examples

- `> condor init ./video.mp4 ./output.mkv`
- `> condor init ./script.vpy ./output.mkv`

## Output

Path to the output video file. Extension must be supported by `--concat`.

### Examples

- `> condor init input.mp4 ./output.mkv`
- `> condor init input.mp4 /mnt/encoded/flock.ivf`

## Decoder `--decoder`

Method used for decoding the input video. Methods besides `ffms2` require external VapourSynth plugins. See [Decoder](../types/decoder.md).

### Possible Values

- `bestsource` - BestSource VapourSynth plugin
- `vs-ffms2` - FFmpegSource VapourSynth plugin
- `lsmash` - L-SMASH-Works VapourSynth plugin
- `dgdecnv` - DGDecodeNV VapourSynth plugin
- `ffms2` - FFmpegSource (integrated)

### Default

If not specified, `bestsource` is used.

## Filters `--filters`

VapourSynth filters to apply to the input. See [Filters](../types/filters.md).

### Default

If not specified, `resize:scaler=bicubic;format=yuv420p10le` (YUV 4:2:0 10-bit) is used.

### Examples

- `> condor init input.mp4 output.mkv --filters "resize:scaler=bilinear;width=1920;height=1080;format=yuv420p10le"`
- `> condor init input.mp4 output.mkv --filters "crop:top=140;bottom=140;"`

## VapourSynth Arguments `--vs-args`

VapourSynth/Python arguments to pass to the input script environment. See [VS Args](../types/vs-args.md).

### Examples

- `> condor init script.vpy output.mkv --vs-args "message=fluffy kittens" --vs-args "head=empty"`

## Concatenation `--concat`

Method used for concatenating encoded chunks into the output file. See [Concatenation](../types/concatenation.md).

### Possible Values

- `mkvmerge` - MKVToolNix mkvmerge, Matroska (`.mkv`) only, generally best
- `ffmpeg` - FFmpeg, supports more formats but may break audio seeking
- `ivf` - Indeo Video Format (`.ivf`), video only

### Default

If not specified, `mkvmerge` is used.

## Discard `--discard`

Tracks of the input file to leave out of the output. By default the concatenator copies the input's tracks, so dropping some of them normally means editing the `tracks` map of the saved [Scene Concatenator settings](../configuration/condor/sequence-config/scene-concatenator.md) by hand.

`--discard` probes the input with `mkvmerge --identify`, resolves the tracks of each named category, and writes a matching `"copy": false` entry for each one into `condor.json`.

The categories are `audio`, `subtitle`, `attachment`, `metadata`, `chapters` and `video`; see [the concatenate reference](concatenate.md#discard---discard) for what each one does.

### Default

If not specified, every track of the input is copied.

### Examples

- `> condor init input.mkv output.mkv --discard audio`
- `> condor init input.mkv output.mkv --concat ffmpeg --discard audio,subtitle`

## Workers `-w`, `--workers`

The amount of encoder processes to use at once.

### Default

If not specified, Benchmarker is used to determine the optimal number of workers.

## Encoder `-e`, `--encoder`

Video encoder to use. See [Encoder](../types/encoder.md).

### Possible Values

- `aom` - Alliance for Open Media AV1 encoder
- `rav1e` - Rust AV1 encoder
- `vpx` - WebM VP8/VP9 encoder
- `svt-av1` - Scalable Video Technology for AV1
- `avm` - Alliance for Open Media Video Model AV2 encoder
- `x264` - x264 H.264 encoder
- `x265` - x265 H.265 encoder
- `vvenc` - Fraunhofer Versatile Video Encoder H.266 encoder
- `ffmpeg` - FFmpeg

### Default

If not specified, `svt-av1` is used.

## Encoder Parameters `--params`

Parameters for the video encoder (`--encoder`). Passed directly to the encoder binary and merged with Condor defaults. See [Encoder Parameters](../types/encoder-params.md).

### Examples

- `> condor init input.mp4 output.mkv --params "--preset 2 --crf 24 --aq-mode 0"`
- `> condor init input.mp4 output.mkv --params "--cpu-used=3 --cq-level=30 --tune=ssim"`

## Photon Noise `--photon-noise`

Generate and apply a photon noise table using Film Grain Synthesis with the specified ISO strength. Only compatible with `aom`, `svt-av1`, `rav1e`, and `avm`. Do not use with internal encoder grain (e.g. `--film-grain` in `svt-av1`). See [Photon Noise](../types/photon-noise.md).

### Examples

- `> condor init input.mp4 output.mkv --photon-noise 800`

## Disable Colour Data `--disable-color-data`

Turns off the colour metadata appending that is on by default: the input's colour primaries, transfer characteristics, matrix coefficients, chroma sample location, range, mastering display and content light levels are normally written into the encoder's own parameters, after the pass parameters but before anything set with `--params`, so a value you set overrides it. See [`condor.encoder`'s `append_color_data`](../configuration/condor/encoder.md#colour-metadata).

### Examples

- `> condor init input.mp4 output.mkv --disable-color-data`

## Target Metric `--target-metric`

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

- `> condor init input.mp4 output.mkv --target-metric ssimulacra2 --target 85`

## Edit saved settings

`info`, `set` and `reset` read, change and reset the top-level settings of an existing configuration without running anything:

- `set input [--path P] [--decoder D] [--filters …] [--vs-args …] [--stream-concurrently [on|off]]` — the input the pipeline reads, built from the same flags as `init` itself; the path the pipeline records is updated too.
- `set output PATH` — the file the pipeline writes.
- `set encoder [--encoder E] [--executable PATH] [--params …] [--photon-noise ISO]` — the encoder and its parameters; provide at least one flag, values you omit keep their current value.
- `set workers N` — encoder processes to run at once (`≥ 1`).
- `set concatenator METHOD` — the tool that joins the encoded scenes; keeping the current tool keeps its settings.
- `set target-metric METRIC` / `set target SCORE` — the Target Quality metric and the score it aims for.
- `info [KEY]` prints one key as JSON, or the whole top level; `reset KEY` restores one key. A bare `reset` is refused, because the input and output of a pipeline have no default to restore.

```bash
$ condor init set output final.mkv
$ condor init set input --path ./new-input.mp4 --decoder ffms2
$ condor init set encoder --params "--crf 20 --tune 0"
$ condor init info encoder
$ condor init reset encoder
```

Shared semantics (validation, staleness warnings): see [Editing saved settings](config-editing.md).

## Screenshots

The screenshots below are generated from the live interface. Click any image to open it full size.

<table>
  <tr>
    <td width="50%" valign="top">
      <a href="../media/tui/help-init-light.avif"><picture><source srcset="../media/tui/help-init-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-init-light.avif" alt="condor init --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">condor init --help</p>
    </td>
    <td width="50%" valign="top"></td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="../media/tui/help-init-verbose-light.avif"><picture><source srcset="../media/tui/help-init-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-init-verbose-light.avif" alt="condor init --help --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">condor init --help --verbose</p>
    </td>
    <td width="50%" valign="top">
      <a href="../media/tui/help-init-set-light.avif"><picture><source srcset="../media/tui/help-init-set-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-init-set-light.avif" alt="condor init set --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">condor init set --help</p>
    </td>
  </tr>
</table>
