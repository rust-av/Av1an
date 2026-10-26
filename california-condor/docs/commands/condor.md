# Condor

Run the full pipeline with no subcommand (triggers TUIs per step). Runs scene detection, zoning, noise detection, benchmark, target quality, bitrate optimization, speed scaling, encode, concatenation, and quality check in order. See [guide](../guide.md) and [type reference](../types.md).

```bash
$ condor -i "input.mp4" -o "output.mkv"
$ condor --config-file ./config.json --skip-scd
```

| Name                                                              | Flag                  | Type        | Default                |
| ----------------------------------------------------------------- | --------------------- | ----------- | ---------------------- |
| [Input](#input--i)                                                | `-i`, `--input`       | Path        |                        |
| [Output](#output--o)                                              | `-o`, `--output`      | Path        |                        |
| [Scene Detector Input](#scene-detector-input---scd-input)         | `--scd-input`         | Path        | Input                  |
| [Target Quality Input](#target-quality-input---tq-input)          | `--tq-input`          | Path        | Input                  |
| [Decoder](#decoder---decoder)                                     | `--decoder`           | [`DECODER`](../types/decoder.md)   | `bestsource`           |
| [Scene Detector Decoder](#scene-detector-decoder---scd-decoder)   | `--scd-decoder`       | [`DECODER`](../types/decoder.md)   | Decoder                |
| [Target Quality Decoder](#target-quality-decoder---tq-decoder)    | `--tq-decoder`        | [`DECODER`](../types/decoder.md)   | Decoder                |
| [Filters](#filters---filters)                                     | `--filters`           | [Filters](../types/filters.md)     | `resize:scaler=bicubic;format=yuv420p10le` |
| [Scene Detector Filters](#scene-detector-filters---scd-filters)   | `--scd-filters`       | [Filters](../types/filters.md)     | Input's filters        |
| [Target Quality Filters](#target-quality-filters---tq-filters)    | `--tq-filters`        | [Filters](../types/filters.md)     | Input's filters        |
| [VapourSynth Arguments](#vapoursynth-arguments---vs-args)         | `--vs-args`           | [String List](../types/vs-args.md) |                        |
| [Scene Detector VS Args](#scene-detector-vs-args---scd-vs-args)   | `--scd-vs-args`       | [String List](../types/vs-args.md) |                        |
| [Target Quality VS Args](#target-quality-vs-args---tq-vs-args)    | `--tq-vs-args`        | [String List](../types/vs-args.md) |                        |
| [Concatenation](#concatenation---concat)                           | `--concat`            | [`CONCAT`](../types/concatenation.md)    | `mkvmerge`             |
| [Discard](#discard---discard)                                     | `--discard`           | [String List](#discard---discard)        | nothing                |
| [Workers](#workers--w---workers)                                  | `-w`, `--workers`     | Integer     | Benchmarker decides    |
| [Encoder](#encoder--e---encoder)                                   | `-e`, `--encoder`     | [`ENCODER`](../types/encoder.md)     | `svt-av1`              |
| [Passes](#passes---passes)                                        | `--passes`            | Integer     | `2` for aom/vpx, else `1` |
| [Encoder Parameters](#encoder-parameters---params)                | `--params`            | [String](../types/encoder-params.md)      | Based on Encoder       |
| [Target Quality Parameters](#target-quality-parameters---tq-params) | `--tq-params`       | [String](../types/encoder-params.md)      | Params                 |
| [Photon Noise](#photon-noise---photon-noise)                      | `--photon-noise`      | [Integer](../types/photon-noise.md)      |                        |
| [Chroma Noise](#chroma-noise---chroma-noise)                      | `--chroma-noise`      | [Integer](../types/photon-noise.md)      |                        |
| [FFmpeg Filter](#ffmpeg-filter---ffmpeg-filter)                  | `--ffmpeg-filter`    | String      |                        |
| [Disable Colour Data](#disable-colour-data---disable-color-data) | `--disable-color-data` |            | off                    |
| [Zones](#zones---zones)                                           | `--zones`             | [Zone](../types/zone.md)   |                        |
| [Target Metric](#target-metric---target-metric)                    | `--target-metric`     | [`METRIC`](../types/quality-metric.md)    | `ssimulacra2`          |
| [Target](#target---target)                                         | `--target`            | Float       |                        |
| [Minimum Quantizer](#minimum-quantizer---minimum-quantizer)       | `--minimum-quantizer` | Integer     | Based on Encoder       |
| [Maximum Quantizer](#maximum-quantizer---maximum-quantizer)       | `--maximum-quantizer` | Integer     | Based on Encoder       |
| [Target Profile](#target-profile---target-profile)                 | `--target-profile`    | [`PROFILE`](../types/quality-profile.md)   | `standard`             |
| [Skip SCD](#skip-scd---skip-scd)                                  | `--skip-scd`          |             |                        |

## Input `-i`

Path to the input file to encode. Can be a video or VapourSynth script (`.py` or `.vpy`).

## Output `-o`

Path to the output video file. Extension must be supported by `--concat`.

## Scene Detector Input `--scd-input`

Path to the input file to detect scenes. Falls back to `--input`.

## Target Quality Input `--tq-input`

Path to the input file to encode and measure quality in Target Quality. Falls back to `--input`.

## Decoder `--decoder`

Method used for decoding the input video. Defaults to `bestsource`. See [Decoder](../types/decoder.md).

### Possible Values

- `bestsource`
- `vs-ffms2`
- `lsmash`
- `dgdecnv`
- `ffms2`

## Scene Detector Decoder `--scd-decoder`

Method used for decoding the Scene Detector input. Falls back to `--decoder`. See [Decoder](../types/decoder.md).

## Target Quality Decoder `--tq-decoder`

Method used for decoding the Target Quality input. Falls back to `--decoder`. See [Decoder](../types/decoder.md).

## Filters `--filters`

VapourSynth filters to apply to the input. Defaults to `resize:scaler=bicubic;format=yuv420p10le`. See [Filters](../types/filters.md).

## Scene Detector Filters `--scd-filters`

VapourSynth filters to apply to the Scene Detector input. See [Filters](../types/filters.md).

### Examples

- `> condor --scd-filters "resize:scaler=bilinear;width=540;height=960;" -i input.mp4 -o output.mkv`

## Target Quality Filters `--tq-filters`

VapourSynth filters to apply to the Target Quality input. See [Filters](../types/filters.md).

## VapourSynth Arguments `--vs-args`

VapourSynth/Python arguments to pass to the input script environment. See [VS Args](../types/vs-args.md).

## Scene Detector VS Args `--scd-vs-args`

VapourSynth/Python arguments for the Scene Detector input script environment. See [VS Args](../types/vs-args.md).

## Target Quality VS Args `--tq-vs-args`

VapourSynth/Python arguments for the Target Quality input script environment. See [VS Args](../types/vs-args.md).

## Concatenation `--concat`

Method used for concatenating encoded chunks. Defaults to `mkvmerge`. See [Concatenation](../types/concatenation.md).

### Possible Values

- `mkvmerge`
- `ffmpeg`
- `ivf`

## Discard `--discard`

Tracks of the input file to leave out of the output. The concatenator copies the input's tracks by default; `--discard` probes the input with `mkvmerge --identify` and writes `"copy": false` entries for the named categories into `condor.json`.

The categories are `audio`, `subtitle`, `attachment`, `metadata`, `chapters` and `video`; see [the concatenate reference](concatenate.md#discard---discard) for what each one does.

### Examples

- `> condor -i input.mkv -o output.mkv --discard audio`

## Workers `-w`, `--workers`

The amount of encoder processes to use at once. If omitted, Benchmarker decides.

## Encoder `-e`, `--encoder`

Video encoder to use. Defaults to `svt-av1`. See [Encoder](../types/encoder.md).

### Possible Values

- `aom`
- `rav1e`
- `vpx`
- `svt-av1`
- `avm`
- `x264`
- `x265`
- `vvenc`
- `ffmpeg`

## Passes `--passes`

Number of encoder passes. Defaults to `2` for `aom`/`vpx`, else `1`.

## Encoder Parameters `--params`

Parameters for the video encoder, merged with Condor defaults. See [Encoder Parameters](../types/encoder-params.md).

## Target Quality Parameters `--tq-params`

Parameters for the video encoder used in Target Quality. If omitted, `--params` is used. Psychovisual parameters are omitted. See [Encoder Parameters](../types/encoder-params.md).

## Photon Noise `--photon-noise`

Photon Noise ISO strength for Film Grain Synthesis. Compatible with `aom`, `svt-av1`, `rav1e`, `avm`. See [Photon Noise](../types/photon-noise.md).

## Chroma Noise `--chroma-noise`

Chroma noise ISO strength for the photon noise table. Compatible with `aom`, `svt-av1`, `rav1e`, `avm`. See [Photon Noise](../types/photon-noise.md).

## FFmpeg Filter `--ffmpeg-filter`

FFmpeg filtergraph applied to every scene before it reaches the encoder. The decoded frames are piped into an FFmpeg subprocess as Y4M and the filtered stream is piped straight into the encoder, so streaming memory usage is unchanged. Requires FFmpeg in PATH. See [`condor.encoder`'s `ffmpeg_filter`](../configuration/condor/encoder.md#ffmpeg-filter).

The flag writes the encoder's filter, so it applies to scenes detected later. Scenes already in the config keep their own encoder copies, matching how `--params` and `--encoder` behave.

Each worker pays FFmpeg startup and probe latency per scene plus the filter's CPU cost. Benchmarker calibration encodes with the same filter, so measured speeds account for it. Target Quality probes do **not** filter: they are scored against the unfiltered reference, so filtering would corrupt the metric the same way it does for Quality Check.

A picture-altering filter (`crop`, `hflip`, `negate`, ...) changes what gets encoded, so Quality Check compares an unfiltered reference against the filtered output and reports misleading scores. A warning is emitted when Quality Check runs in that state.

### Examples

- `> condor --ffmpeg-filter "crop=iw-16:ih-16"`
- `> condor --ffmpeg-filter "eq=brightness=0.05:saturation=1.2"`

## Disable Colour Data `--disable-color-data`

Turns off the colour metadata appending that is on by default: the input's colour primaries, transfer characteristics, matrix coefficients, chroma sample location, range, mastering display and content light levels are normally written into the encoder's own parameters, after the pass parameters but before anything set with `--params`, so a value you set overrides it. See [`condor.encoder`'s `append_color_data`](../configuration/condor/encoder.md#colour-metadata).

The flag writes the encoder's setting, so it applies to scenes detected later. Scenes already in the config keep their own encoder copies, matching how `--params` and `--encoder` behave.

### Examples

- `> condor -i input.mkv -o output.mkv --disable-color-data`

## Zones `--zones`

Zone overrides to apply after Scene Detection. Each value is a zone definition or a path to a JSON, JSON lines, or text zone file. See [Zone](../types/zone.md) and [zone](./zone.md).

A zone marks the scenes that share a frame with it, merges its encoder settings into the scenes' own (or replaces their encoder outright with `replace`), and splits scenes longer than its `--maximum-frames`. Zones are written to `condor.sequence_config.zoner`, replacing the zones already configured. With `--skip-scd`, zones apply to the existing scenes; if earlier zoning split scenes, their intersections with zones may have changed.

### Examples

- `> condor --zones '-1 200 svt-av1 "--preset 3 --crf 23" --iso 400'`
- `> condor --zones '-1 200 replace aom("/tools/aomenc") "--cpu-used=4"' --zones ./zones.json`

## Target Metric `--target-metric`

The quality metric used for Target Quality. Defaults to `ssimulacra2`. See [Quality Metric](../types/quality-metric.md).

### Possible Values

- `ssimulacra2`
- `butteraugli`
- `butteraugli-3`
- `xpsnr`
- `cvvdp`
- `vmaf` (requires `libvmaf`)

## Target `--target`

The quality metric score that Target Quality will aim for.

## Minimum Quantizer `--minimum-quantizer`

The lowest quantizer for Target Quality search. Default depends on encoder.

## Maximum Quantizer `--maximum-quantizer`

The highest quantizer for Target Quality search. Default depends on encoder.

## Target Profile `--target-profile`

The preset profile for Target Quality Probe Strategy and Statistic. Defaults to `standard`.

### Possible Values

- `fast`
- `standard`
- `slow`

## Skip SCD `--skip-scd`

Skip Scene Detection. Useful when encoding a subset of scenes.

## Screenshots

The screenshots below are generated from the live interface. Click any image to open it full size.

<table>
  <tr>
    <td width="50%" valign="top">
      <a href="../media/tui/help-light.avif"><picture><source srcset="../media/tui/help-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-light.avif" alt="condor --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">condor --help</p>
    </td>
    <td width="50%" valign="top">
      <a href="../media/tui/help-verbose-light.avif"><picture><source srcset="../media/tui/help-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-verbose-light.avif" alt="condor --help --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">condor --help --verbose</p>
    </td>
  </tr>
</table>
