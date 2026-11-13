# Filters

VapourSynth filter chains applied to inputs, stored on each input in `condor.json`. Used by `--filters`, `--scd-filters`, `--tq-filters`, `--reference-filters`, `--denoised-filters` (see [init]\(../commands/init.md\), [encode]\(../commands/encode.md\), [pipeline](../commands/condor.md), [detect-scenes]\(../commands/detect-scenes.md\), [detect-noise]\(../commands/detect-noise.md\), [target-quality]\(../commands/target-quality.md\), [quality-check]\(../commands/quality-check.md\)).

Every filter below is a VapourSynth filter, so every one of them needs a VapourSynth input. A
native `--decoder ffms2` input cannot run them; see [FFMS2 Filters](#ffms2-filters).

## Syntax

One or more `name:key=value;key=value;` chains, passed as repeated flags or a single string:

```bash
$ av1an encode --filters "resize:scaler=bilinear;width=1920;height=1080;format=yuv420p10le"
$ av1an detect-scenes --filters "resize:scaler=bilinear;width=540;height=960;"
$ av1an encode --filters "crop:top=140;bottom=140;" --filters "trim:start=24;end=240;"
```

Filter names are case-sensitive; keys are case-insensitive. Omitted optional keys keep
their defaults. The colon is required, including for defaults-only filters such as
`crop:`. Trailing semicolons are allowed. List values use commas (e.g.
`sigma=3.0,0.0,0.0`); spaces around keys, typed values and list items are ignored.
String values such as `degrain`'s `prefix` retain their case, Unicode, spaces, colons
and additional `=` characters.

Unknown or repeated keys, missing `=`, empty typed values/lists, malformed numbers
and invalid enum values are errors. Booleans accept only `true`, `false`, `1` or `0`,
including in lists. Floating-point values must be finite. Dimensions must be positive;
crop amounts and frame indices must be nonnegative and fit the plugin's integer type.
When both trim bounds are supplied, `end` must be at least `start`. Zero remains valid
for fields that support it, such as crop amounts and WNNM's passthrough sigma.

For example, `crop:top=abc` reports the filter, field and invalid value instead of
panicking. Invalid CLI filter edits leave the saved configuration unchanged; parsing
does not open video or load filter plugins. Clip-dependent constraints are checked
when the filter runs.

## Filter Reference

| Filter      | Arguments                                                                 | Notes                                    |
| ----------- | ------------------------------------------------------------------------- | ---------------------------------------- |
| `resize`    | `scaler?`, `width?`, `height?`, `format?`                                 | See [Scaler](#scaler) and [Format](#format) |
| `output-format` / `output_format` | `bit_depth?`, `chroma?`, `width?`, `height?` | Preserves omitted depth/chroma until decoding. Same fields and validation as [FFMS2 Filters](#ffms2-filters). |
| `crop`      | `top?`, `bottom?`, `left?`, `right?` (pixels)                             |                                          |
| `trim`      | `start?`, `end?` (frames)                                                 |                                          |
| `rescale`   | `kernel` (required), `width` (required), `height` (required), `doubler` (required) | Requires vs-jetpack + vodesfunc. See [Kernel](#kernel) and [Doubler](#doubler) |
| `wnnm`      | `sigma?`, `block_size?`, `block_step?`, `group_size?`, `bm_range?`, `radius?`, `ps_num?`, `ps_range?`, `residual?`, `adaptive_aggregation?` | WNNM denoise. Requires vszip. |
| `bilateral` | `sigma_s?`, `sigma_r?`, `planes?`, `algorithm?`, `pbficnum?`              | Bilateral filter. Requires vszip.        |
| `degrain`   | MVUtensils motion-compensated denoise (many optional keys: `blksize`, `overlap`, `pel`, `radius`, `thsad`, `thsad2`, `planes`, `limit`, `thscd1`, `thscd2`, `recalculate`, ...) | Composite Super → Analyse → Degrain chain |
| `zoom_degrain` | ZooMVTools motion-compensated denoise (many optional keys: `hpad`, `vpad`, `pel`, `blksize`, `thsad`, `thsadc`, `plane`, `limit`, `recalculate`, ...) | Composite ZMVSuper → Analyse → Degrain chain |
| `fgs`       | `iso` (required), `chroma_iso?`, `cy?`, `ccb?`, `ccr?`, `dynamic_seed?`   | Film Grain Synthesis via dav1d grain engine. See [Photon Noise](./photon-noise.md) |

## Scaler

`resize` scaler values:

| Value      |
| ---------- |
| `bicubic`  |
| `bilinear` |
| `bob`      |
| `lanczos`  |
| `point`    |
| `spline16` |
| `spline36` |
| `spline64` |

## Format

`resize` format values are FFmpeg pixel formats (`-pix_fmt` strings):

| Value          | Description              |
| -------------- | ------------------------ |
| `yuv420p`      | YUV 4:2:0 8-bit          |
| `yuv420p10le`  | YUV 4:2:0 10-bit         |
| `yuv420p12le`  | YUV 4:2:0 12-bit         |
| `yuv422p`      | YUV 4:2:2 8-bit          |
| `yuv422p10le`  | YUV 4:2:2 10-bit         |
| `yuv422p12le`  | YUV 4:2:2 12-bit         |
| `yuv444p`      | YUV 4:4:4 8-bit          |
| `yuv444p10le`  | YUV 4:4:4 10-bit         |
| `yuv444p12le`  | YUV 4:4:4 12-bit         |
| `yuv440p` etc. | YUV 4:4:0 family         |
| `gbrp`, `gbrp10le`, `gbrp12le` | Planar RGB family |
| `gray`, `gray10le`, `gray12le` | Grayscale family  |
| `nv12`, `nv16`, `nv20le`, `nv21` | Semi-planar family |
| `yuva420p`, `yuvj420p`, `yuvj422p`, `yuvj444p` | Alpha / full-range JPEG family |

Default encode filter is `resize:scaler=bicubic;format=yuv420p10le`.

## Kernel

`rescale` kernel values (vs-jetpack):

`Bicubic`, `BSpline`, `Hermite`, `Mitchell`, `Catrom`, `FFmpegBicubic`, `AdobeBicubic`, `AdobeBicubicSharper`, `AdobeBicubicSmoother`, `BicubicSharp`, `RobidouxSoft`, `Robidoux`, `RobidouxSharp`, `BicubicAuto`, `Spline16`, `Spline36`, `Spline64`, `Bilinear`, `Lanczos`, `Point`.

## Doubler

`rescale` doubler values:

| Doubler   | Models |
| --------- | ------ |
| `ArtCNN`  | `C4F32`, `C4F32_DS`, `C16F64`, `C16F64_DS`, `R16F96`, `R8F64` (default), `R8F64_DS`, `R8F64_Chroma`, `C4F16`, `C4F16_DS`, `R16F96_Chroma` |
| `Waifu2x` | `AnimeStyleArt`, `AnimeStyleArtRGB`, `Photo`, `UpConv7AnimeStyleArt`, `UpConv7Photo`, `UpResNet10`, `Cunet` (default), `SwinUnetArt` |

## FFMS2 Filters

A native `--decoder ffms2` input cannot run VapourSynth filters. FFMS2 can only convert the
frames it decodes, so that input takes a single filter, `output-format`, applied by the
decoder itself. Because it happens during decode, it is cheaper than resizing afterwards.

| Field      | Type                | Required | Default | Description |
| ---------- | ------------------- | -------- | ------- | ----------- |
| `bit_depth` | Integer (8, 10, 12) | No | unchanged | Target bits per component |
| `chroma`    | String              | No | unchanged | `Yuv420`, `Yuv422`, `Yuv444`, `Monochrome` |
| `width`     | Integer             | No | unchanged | Target width in pixels |
| `height`    | Integer             | No | unchanged | Target height in pixels |

Scaling always uses bicubic, which is all FFMS2 offers. 8/10/12-bit are supported across
YUV420/422/444 and monochrome; any other combination is an error rather than a silent
substitution.

```json
[
    { "OutputFormat": { "bit_depth": 10, "chroma": null, "width": null, "height": null } }
]
```

A `--filters resize:...` string given to a native input is translated to this where it maps
(a resize to a different bit depth, chroma or resolution), and dropped with a warning where
it does not (a crop, a trim, or any denoiser). Use `--decoder vs-ffms2` to run those.

An explicit format includes its bit depth: `resize:format=yuv420p;` requests 8-bit output
even from a 10-bit source. Native FFMS2 rejects unsupported explicit formats such as
`nv12`, `gbrp`, or `yuv440p`; a failed edit leaves the saved configuration unchanged.

Switching a native input to VapourSynth saves the same `OutputFormat` fields on the
VapourSynth input. Missing depth/chroma are resolved when decoding, from each filter's
actual input node, not guessed as 8-bit or 4:2:0. Configuration edits do not open or index
the source. For example, this keeps a 4:4:4 source at 4:4:4 while converting it to 10-bit:

```json
{ "OutputFormat": { "bit_depth": 10, "chroma": null, "width": null, "height": null } }
```

Both direct decoding and generated scripts resolve filters in order, so a chroma-only
conversion followed by a depth-only conversion retains the new chroma. Existing concrete
`Resize` configurations remain supported.

## Defaults

| Flag                  | Default                               |
| --------------------- | ------------------------------------- |
| `--filters`           | `resize:scaler=bicubic;format=yuv420p10le` |
| `--scd-filters`       | inherits `condor.input`               |
| `--tq-filters`        | inherits `condor.input`               |
| `--reference-filters` | `wnnm:sigma=3.0,0.0,0.0;`             |
| `--denoised-filters`  | `wnnm:sigma=6.0,0.0,0.0;`             |

`--scd-filters` and `--tq-filters` set filters on the Scene Detector and Target Quality
inputs, creating those inputs if needed. Each inherits the path and decoder of
`condor.input` when it has none of its own, so they never replace the filters on
`condor.input` itself. `--filters` always applies to `condor.input`.

## Examples

- `> av1an encode --filters "resize:scaler=bilinear;width=1280;height=720;format=yuv420p;"` — Bilinear resize to 1280x720 8-bit
- `> av1an encode --filters "crop:top=140;bottom=140;"` — Crop 140px top and bottom
- `> av1an encode --filters "trim:start=24;end=240;"` — Trim to frames 24-240
- `> av1an encode --filters "rescale:kernel=Mitchell;width=720;height=1280;doubler=ArtCNN;"` — VSJET rescale with ArtCNN
- `> av1an encode --decoder ffms2 --filters "resize:format=yuv420p10le;"` — 10-bit output on a native FFMS2 input

## JSON Shape

Filters are stored on the input they modify, in `condor.input.filters` and each
`condor.sequence_config.*.input.filters`. `reference_filters` and `denoised_filters` stay
on the noise detector config. Each CLI string is stored as one externally-tagged object.
Exactly one key per object. `scaler` uses PascalCase (`Bicubic`), `format` uses UPPER
(`YUV420P10LE`).

```json
[
    { "Crop": { "top": 140, "bottom": 140, "left": null, "right": null } },
    { "Resize": { "scaler": "Bicubic", "width": null, "height": null, "format": "YUV420P10LE" } },
    { "Trim": { "start": 24, "end": 240 } },
    { "WNNM": { "sigma": [3.0, 0.0, 0.0], "block_size": null, "block_step": null, "group_size": null, "bm_range": null, "radius": null, "ps_num": null, "ps_range": null, "residual": null, "adaptive_aggregation": null } },
    { "Bilateral": { "sigma_s": null, "sigma_r": null, "planes": null, "algorithm": null, "pbficnum": null } },
    { "FGS": { "iso": 800, "chroma_iso": null, "cy": null, "ccb": null, "ccr": null, "dynamic_seed": null } }
]
```

`Rescale` requires `kernel`, `width`, `height`, `doubler`:

```json
[
    {
        "Rescale": {
            "kernel": { "Mitchell": { "linear": false, "border_handling": "MIRROR" } },
            "width": 720,
            "height": 1280,
            "doubler": { "ArtCNN": "R8F64" }
        }
    }
]
```

`Degrain` and `ZoomDegrain` are composite denoisers with all-optional fields; omitted fields are `null`. See variant field lists in the Filter Reference table above. `BorderHandling` is `MIRROR`, `ZERO`, or `REPEAT`. `Doubler` is `{ "ArtCNN": "<model>" }` or `{ "Waifu2x": "<model>" }` with models listed in [Doubler](#doubler).
