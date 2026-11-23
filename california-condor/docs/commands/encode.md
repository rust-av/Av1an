# Encode

Encode scenes in parallel (triggers TUI). See [guide](../guide.md) and [type reference](../types.md).

```bash
$ av1an encode
$ av1an encode -i input.mp4 -e svt-av1 -w 4 --params "--preset 4" --photon-noise 800
```

Encoder flags and `encode set`/`reset` edit the global default for future scenes.
Existing scenes keep their saved encoders, including detection and zone overrides.
To deliberately replace every saved scene's encoder with the current global default:

```bash
av1an scenes all reset encoder
```

This does not delete chunks or clear saved results; review stale results/files before
reusing them. It does not reapply zones. `av1an scenes all info encoder` shows the
actual saved encoders. The parallel and Target Quality TUI headers show the actual
uniform settings, or **Mixed encoders: per-scene settings** when any settings differ
(even within one codec). Target Quality shows resolved probe settings, not final
encode settings; worker panels name their actual scene codecs.

| Name                                                  | Flag              | Type        | Default                |
| ----------------------------------------------------- | ----------------- | ----------- | ---------------------- |
| [Input](#input--i)                                    | `-i`, `--input`   | Path        | Config input           |
| [Decoder](#decoder---decoder)                         | `--decoder`       | [`DECODER`](../types/decoder.md)   | `bestsource`           |
| [Filters](#filters---filters)                         | `--filters`       | [Filters](../types/filters.md)     | Saved effective-input filters |
| [VapourSynth Arguments](#vapoursynth-arguments---vs-args) | `--vs-args`   | [String List](../types/vs-args.md) |                        |
| [Workers](#workers--w---workers)                      | `-w`, `--workers` | Integer     | Benchmarker decides    |
| [Total attempts](#total-attempts---max-tries)           | `--max-tries`      | Integer ≥1  | Saved value, else `3`   |
| [Encoder](#encoder--e---encoder)                       | `-e`, `--encoder` | [`ENCODER`](../types/encoder.md)   | `svt-av1`              |
| [Passes](#passes---passes)                            | `--passes`        | Integer     | `2` for aom/vpx, else `1` |
| [Encoder Parameters](#encoder-parameters---params)    | `--params`        | [String](../types/encoder-params.md)      | Based on Encoder       |
| [Photon Noise](#photon-noise---photon-noise)          | `--photon-noise`  | [Integer](../types/photon-noise.md)     |                        |
| [Chroma Noise](#chroma-noise---chroma-noise)          | `--chroma-noise`  | [Integer](../types/photon-noise.md)     |                        |
| [FFmpeg Filter](#ffmpeg-filter---ffmpeg-filter)      | `--ffmpeg-filter` | String      |                        |

## Input `-i`

Path to the input file to encode. Can be a video or VapourSynth script (`.py` or `.vpy`).

### Examples

- `> av1an encode -i ./video.mp4`

## Decoder `--decoder`

Method used for decoding the input video. Methods besides `ffms2` require external VapourSynth plugins. Defaults to `bestsource`. See [Decoder](../types/decoder.md).

### Possible Values

- `bestsource`
- `vs-ffms2`
- `lsmash`
- `dgdecnv`
- `ffms2`

## Filters `--filters`

Filters replace those on the effective encoder input: the saved parallel-encoder
override when present, otherwise the main input. A filter-only edit needs no
`--input`, `--decoder` or `--vs-args` and does not create an override.
See [Filters](../types/filters.md) for decoder support; native FFMS2 rejects
unsupported explicit formats and cannot run VapourSynth-only filters.

When `--input`, `--decoder` or `--vs-args` creates or rebuilds an override, omitted
filters inherit from the previous effective input. Explicit filters replace them on the rebuilt
override. Neither case changes the main input or separate Target Quality/Quality
Check reference overrides. Without a parallel-encoder override, filtering the
main input also intentionally changes the reference used by stages that read it.

For saved edits with no existing override,
`encode set input --filters X --stream-concurrently on` filters the main input
first, then creates an override from it for the streaming toggle. This also
changes the main/reference filters, even when the toggle is ignored for a
non-script input; it is not a filter-only edit.

Migration: older versions applied encode filters to the main input even with an
override present. Review saved main-input filters from those edits; this repair
does not restore them automatically. Use `init set input --filters …` when you
intend to edit the main input explicitly.

### Default

Omitting `--filters` preserves the effective input's saved filters. New jobs
start with `resize:scaler=bicubic;format=yuv420p10le` (YUV 4:2:0 10-bit).

## VapourSynth Arguments `--vs-args`

VapourSynth/Python arguments to pass to the input script environment. See [VS Args](../types/vs-args.md).

## Workers `-w`, `--workers`

The amount of encoder processes to use at once.

### Default

If not specified, Benchmarker is used to determine the optimal number of workers.

## Total attempts `--max-tries`

Maximum total attempts per scene, including the initial attempt, not additional
retries. Defaults to `3` for new jobs and missing saved fields. Omitting the flag
preserves a saved custom value. Available on root/start, `init`, and `encode`.
Nonstreaming retries use extra disk space; see [retry and buffering policy](../configuration/condor/sequence-config/parallel-encoder.md).

Rerun `av1an encode` with the same job file to resume. Av1an skips repeat decode
checks for unchanged verified chunks and re-encodes missing or damaged chunks from the
same job. Each affected scene starts with a fresh attempt budget (`3` by default).
Standalone `concatenate` does not repair chunks; run `encode` first.

```bash
av1an encode --max-tries 2
av1an encode info max-tries
av1an encode set max-tries 5
av1an encode reset max-tries
```

Resetting this field or the whole encode section restores `3`.

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

Only frontend-supported totals are accepted: `1` or `2`, with `avm` limited to
`1`. `ffmpeg` uses one implicit pass and rejects this flag. SVT-AV1 two-pass
settings require VBR (`--rc 1`) and preset `<= 9`; CRF/CQP and CBR are single-pass.
See [settings validation](../configuration/condor/encoder.md#settings-validation).

Automatic two-pass encoding of a streamed input first saves the complete
uncompressed Y4M stream in the job's output area, then replays that same temporary
file for each pass with bounded input buffers. Allow disk space for **one full
uncompressed Y4M stream per active two-pass attempt**, plus output and statistics.
The spool is reused across passes and freed on completion or error. Capture must
finish before the first encoder starts: this trades disk I/O and startup latency
for bounded replay memory. One-pass encoding remains fully streaming.

### Default

If not specified, `2` for `aom` and `vpx` encoders, otherwise `1`. `aom` and `vpx` benefit from two-pass mode even with constant quality mode.

## Encoder Parameters `--params`

Parameters for the video encoder (`--encoder`). Passed directly to the encoder binary and merged with Condor defaults. See [Encoder Parameters](../types/encoder-params.md).

### Examples

- `> av1an encode --params "--preset 2 --crf 24 --aq-mode 2"`
- `> av1an encode --params "--cpu-used=3 --cq-level=30 --tune=ssim"`

## Photon Noise `--photon-noise`

Generate and apply a photon noise table using Film Grain Synthesis with the specified ISO strength. Only compatible with `aom`, `svt-av1`, and `rav1e`. AVM requires incompatible `filmgrn2`, not generated AV1 `filmgrn1` tables. A minimum ISO of `200` is recommended for reducing gradient banding. See [Photon Noise](../types/photon-noise.md).

## Chroma Noise `--chroma-noise`

Apply chroma noise of the specified ISO strength to the photon noise table using Film Grain Synthesis. Only compatible with `aom`, `svt-av1`, and `rav1e`. AVM requires incompatible `filmgrn2`, not generated AV1 `filmgrn1` tables. See [Photon Noise](../types/photon-noise.md).

Requires `--photon-noise` or an existing luma table. With an existing table,
changing only chroma noise keeps its luma strength and other settings.

## FFmpeg Filter `--ffmpeg-filter`

FFmpeg filtergraph applied to every scene before it reaches the encoder. The decoded frames are piped into an FFmpeg subprocess as Y4M and the filtered stream is piped straight into the encoder, so streaming memory usage is unchanged. Requires FFmpeg in PATH. See [`condor.encoder`'s `ffmpeg_filter`](../configuration/condor/encoder.md#ffmpeg-filter).

The flag writes the encoder's filter, so it applies to scenes detected later. Scenes already in the config keep their own encoder copies, matching how `--params` and `--encoder` behave.

Each worker pays FFmpeg startup and probe latency per scene plus the filter's CPU cost. Benchmarker calibration encodes with the same filter, so measured speeds account for it. Target Quality probes do **not** filter: they are scored against the unfiltered reference, so filtering would corrupt the metric the same way it does for Quality Check.

A picture-altering filter (`crop`, `hflip`, `negate`, ...) changes what gets encoded, so Quality Check compares an unfiltered reference against the filtered output and reports misleading scores. A warning is emitted when Quality Check runs in that state.

### Examples

- `> av1an encode --ffmpeg-filter "crop=iw-16:ih-16"`
- `> av1an encode --ffmpeg-filter "eq=brightness=0.05:saturation=1.2"`

## Edit saved settings

`info`, `set` and `reset` read, change and reset the saved parallel-encoder and encoder settings without running the command:

- `set workers N` — how many encoder processes run at once (`≥ 1`); `reset workers` clears it so Benchmarker decides.
- `set input [--path P] [--decoder D] [--filters …] [--vs-args …] [--stream-concurrently [on|off]]` — edits the effective encoder input using the [filter routing and inheritance rules](#filters---filters) above; a filter-only edit changes the saved override, or the main input when no override exists.
- `set scenes-directory PATH` — the directory the encoded scenes are written to.
- `set encoder ENCODER` — switches the encoder and starts from its defaults.
- `set executable PATH` — stores a custom binary path or PATH name unchanged, even when unavailable while editing. Validation/encoding fails if it cannot be resolved; there is no stock-binary fallback. `reset executable` restores lookup of the base's stock name on PATH.
- `set passes N` — a supported pass count, subject to the same backend/mode checks as `--passes`; rejected for `ffmpeg`.
- `set params "--crf 16 --tune 0"` — merges encoder parameters into the current encoder's options; `reset params` restores the encoder's defaults.
- `set photon-noise ISO [--chroma-noise ISO]` — sets the Photon Noise table; rejected for encoders that do not support Photon Noise; `reset photon-noise` clears it.
- `set append-color-data [on|off]` — whether the input's colour and HDR metadata is appended to the encoder parameters; defaults to `on`, and `reset append-color-data` restores that.
- `set ffmpeg-filter FILTER` — sets the FFmpeg filtergraph; `reset ffmpeg-filter` clears it.
- `info [KEY]` prints JSON for one key or both `parallel_encoder` and `encoder`; `reset KEY` resets one key, and a bare `reset` restores both to their defaults (preserving the temp-derived `scenes-directory`).

```bash
$ av1an encode set workers 4
$ av1an encode set photon-noise 800 --chroma-noise 200
$ av1an encode set append-color-data off
$ av1an encode set ffmpeg-filter "crop=iw-16:ih-16"
$ av1an encode reset photon-noise
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
      <a href="../media/tui/help-encode-light.avif"><picture><source srcset="../media/tui/help-encode-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-encode-light.avif" alt="av1an encode --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an encode --help</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="../media/tui/help-encode-verbose-light.avif"><picture><source srcset="../media/tui/help-encode-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-encode-verbose-light.avif" alt="av1an encode --help --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an encode --help --verbose</p>
    </td>
    <td width="50%" valign="top">
      <a href="../media/tui/help-encode-set-light.avif"><picture><source srcset="../media/tui/help-encode-set-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-encode-set-light.avif" alt="av1an encode set --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an encode set --help</p>
    </td>
  </tr>
</table>
