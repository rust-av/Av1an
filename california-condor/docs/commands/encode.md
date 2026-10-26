# Encode

Encode scenes in parallel (triggers TUI). See [guide](../guide.md) and [type reference](../types.md).

```bash
$ condor encode
$ condor encode -i input.mp4 -e svt-av1 -w 4 --params "--preset 4" --photon-noise 800
```

| Name                                                  | Flag              | Type        | Default                |
| ----------------------------------------------------- | ----------------- | ----------- | ---------------------- |
| [Input](#input--i)                                    | `-i`, `--input`   | Path        | Config input           |
| [Decoder](#decoder---decoder)                         | `--decoder`       | [`DECODER`](../types/decoder.md)   | `bestsource`           |
| [Filters](#filters---filters)                         | `--filters`       | [Filters](../types/filters.md)     | `resize:scaler=bicubic;format=yuv420p10le` |
| [VapourSynth Arguments](#vapoursynth-arguments---vs-args) | `--vs-args`   | [String List](../types/vs-args.md) |                        |
| [Workers](#workers--w---workers)                      | `-w`, `--workers` | Integer     | Benchmarker decides    |
| [Encoder](#encoder--e---encoder)                       | `-e`, `--encoder` | [`ENCODER`](../types/encoder.md)   | `svt-av1`              |
| [Passes](#passes---passes)                            | `--passes`        | Integer     | `2` for aom/vpx, else `1` |
| [Encoder Parameters](#encoder-parameters---params)    | `--params`        | [String](../types/encoder-params.md)      | Based on Encoder       |
| [Photon Noise](#photon-noise---photon-noise)          | `--photon-noise`  | [Integer](../types/photon-noise.md)     |                        |
| [Chroma Noise](#chroma-noise---chroma-noise)          | `--chroma-noise`  | [Integer](../types/photon-noise.md)     |                        |
| [FFmpeg Filter](#ffmpeg-filter---ffmpeg-filter)      | `--ffmpeg-filter` | String      |                        |

## Input `-i`

Path to the input file to encode. Can be a video or VapourSynth script (`.py` or `.vpy`).

### Examples

- `> condor encode -i ./video.mp4`

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

## Workers `-w`, `--workers`

The amount of encoder processes to use at once.

### Default

If not specified, Benchmarker is used to determine the optimal number of workers.

## Encoder `-e`, `--encoder`

Video encoder to use. See [Encoder](../types/encoder.md).

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

### Default

If not specified, `svt-av1` is used.

## Passes `--passes`

Number of encoder passes.

### Default

If not specified, `2` for `aom` and `vpx` encoders, otherwise `1`. `aom` and `vpx` benefit from two-pass mode even with constant quality mode.

## Encoder Parameters `--params`

Parameters for the video encoder (`--encoder`). Passed directly to the encoder binary and merged with Condor defaults. See [Encoder Parameters](../types/encoder-params.md).

### Examples

- `> condor encode --params "--preset 2 --crf 24 --aq-mode 0"`
- `> condor encode --params "--cpu-used=3 --cq-level=30 --tune=ssim"`

## Photon Noise `--photon-noise`

Generate and apply a photon noise table using Film Grain Synthesis with the specified ISO strength. Only compatible with `aom`, `svt-av1`, `rav1e`, and `avm`. A minimum ISO of `200` is recommended for reducing gradient banding. See [Photon Noise](../types/photon-noise.md).

## Chroma Noise `--chroma-noise`

Apply chroma noise of the specified ISO strength to the photon noise table using Film Grain Synthesis. Only compatible with `aom`, `svt-av1`, `rav1e`, and `avm`. See [Photon Noise](../types/photon-noise.md).

## FFmpeg Filter `--ffmpeg-filter`

FFmpeg filtergraph applied to every scene before it reaches the encoder. The decoded frames are piped into an FFmpeg subprocess as Y4M and the filtered stream is piped straight into the encoder, so streaming memory usage is unchanged. Requires FFmpeg in PATH. See [`condor.encoder`'s `ffmpeg_filter`](../configuration/condor/encoder.md#ffmpeg-filter).

The flag writes the encoder's filter, so it applies to scenes detected later. Scenes already in the config keep their own encoder copies, matching how `--params` and `--encoder` behave.

Each worker pays FFmpeg startup and probe latency per scene plus the filter's CPU cost. Benchmarker calibration encodes with the same filter, so measured speeds account for it. Target Quality probes do **not** filter: they are scored against the unfiltered reference, so filtering would corrupt the metric the same way it does for Quality Check.

A picture-altering filter (`crop`, `hflip`, `negate`, ...) changes what gets encoded, so Quality Check compares an unfiltered reference against the filtered output and reports misleading scores. A warning is emitted when Quality Check runs in that state.

### Examples

- `> condor encode --ffmpeg-filter "crop=iw-16:ih-16"`
- `> condor encode --ffmpeg-filter "eq=brightness=0.05:saturation=1.2"`

## Edit saved settings

`info`, `set` and `reset` read, change and reset the saved parallel-encoder and encoder settings without running the command:

- `set workers N` — how many encoder processes run at once (`≥ 1`); `reset workers` clears it so Benchmarker decides.
- `set input [--path P] [--decoder D] [--filters …] [--vs-args …] [--stream-concurrently [on|off]]` — the input Parallel Encoder reads instead of the main input; `--filters` applies to the main input.
- `set scenes-directory PATH` — the directory the encoded scenes are written to.
- `set encoder ENCODER` — switches the encoder and starts from its defaults.
- `set executable PATH` — pins a custom encoder binary path; `reset executable` restores PATH lookup.
- `set passes N` — number of encoder passes (`≥ 1`); rejected for single-pass-only `ffmpeg`.
- `set params "--crf 16 --tune 0"` — merges encoder parameters into the current encoder's options; `reset params` restores the encoder's defaults.
- `set photon-noise ISO [--chroma-noise ISO]` — sets the Photon Noise table; rejected for encoders that do not support Photon Noise; `reset photon-noise` clears it.
- `set append-color-data [on|off]` — whether the input's colour and HDR metadata is appended to the encoder parameters; defaults to `on`, and `reset append-color-data` restores that.
- `set ffmpeg-filter FILTER` — sets the FFmpeg filtergraph; `reset ffmpeg-filter` clears it.
- `info [KEY]` prints JSON for one key or both `parallel_encoder` and `encoder`; `reset KEY` resets one key, and a bare `reset` restores both to their defaults (preserving the temp-derived `scenes-directory`).

```bash
$ condor encode set workers 4
$ condor encode set photon-noise 800 --chroma-noise 200
$ condor encode set append-color-data off
$ condor encode set ffmpeg-filter "crop=iw-16:ih-16"
$ condor encode reset photon-noise
```

Shared semantics (validation, staleness warnings): see [Editing saved settings](config-editing.md).

## Screenshots

The screenshots below are generated from the live interface. Click any image to open it full size.

<table>
  <tr>
    <td width="50%" valign="top">
      <a href="../media/tui/parallel-encoder-light.avif"><picture><source srcset="../media/tui/parallel-encoder-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/parallel-encoder-light.avif" alt="Parallel encoder" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">Parallel encoder</p>
    </td>
    <td width="50%" valign="top">
      <a href="../media/tui/help-encode-light.avif"><picture><source srcset="../media/tui/help-encode-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-encode-light.avif" alt="condor encode --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">condor encode --help</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="../media/tui/help-encode-verbose-light.avif"><picture><source srcset="../media/tui/help-encode-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-encode-verbose-light.avif" alt="condor encode --help --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">condor encode --help --verbose</p>
    </td>
    <td width="50%" valign="top">
      <a href="../media/tui/help-encode-set-light.avif"><picture><source srcset="../media/tui/help-encode-set-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-encode-set-light.avif" alt="condor encode set --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">condor encode set --help</p>
    </td>
  </tr>
</table>
