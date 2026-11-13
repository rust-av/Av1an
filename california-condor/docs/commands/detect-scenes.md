# Detect Scenes

Detect scenes (triggers TUI). See [guide](../guide.md) and [type reference](../types.md).

```bash
$ av1an detect-scenes
$ av1an detect-scenes -i input.mp4 --method standard --min-scene-seconds 1 --max-scene-seconds 10
```

| Name                                                  | Flag                  | Type      | Default      |
| ----------------------------------------------------- | --------------------- | --------- | ------------ |
| [Input](#input--i)                                    | `-i`, `--input`       | Path      | Config input |
| [Decoder](#decoder---decoder)                         | `--decoder`           | [`DECODER`](../types/decoder.md) | `bestsource` |
| [Filters](#filters---filters)                         | `--filters`           | [Filters](../types/filters.md)   | Config input's filters |
| [VapourSynth Arguments](#vapoursynth-arguments---vs-args) | `--vs-args`       | [String List](../types/vs-args.md) |            |
| [Method](#method---method)                            | `--method`            | [`METHOD`](../types/scene-detection.md)  | `standard`   |
| [Min Scene Seconds](#min-scene-seconds---min-scene-seconds) | `--min-scene-seconds` | Integer | `1`       |
| [Max Scene Seconds](#max-scene-seconds---max-scene-seconds) | `--max-scene-seconds` | Integer | `10`      |

## Input `-i`

Path to the input file to detect scenes. Can be a video or VapourSynth script (`.py` or `.vpy`).

### Examples

- `> av1an detect-scenes -i ./video.mp4`
- `> av1an detect-scenes -i ./script.vpy`

## Decoder `--decoder`

Method used for decoding the input video. Methods besides `ffms2` require external VapourSynth plugins. Defaults to `bestsource`. See [Decoder](../types/decoder.md).

### Possible Values

- `bestsource`
- `vs-ffms2`
- `lsmash`
- `dgdecnv`
- `ffms2`

## Filters `--filters`

VapourSynth filters to apply to the Scene Detector input. Defaults to the input's own filters. See [Filters](../types/filters.md).

### Examples

- `> av1an detect-scenes --filters "resize:scaler=bilinear;width=540;height=960;"` - Downscale for faster detection

## VapourSynth Arguments `--vs-args`

VapourSynth/Python arguments to pass to the Scene Detector input script environment. See [VS Args](../types/vs-args.md).

### Examples

- `> av1an detect-scenes --vs-args "message=fluffy kittens" --vs-args "head=empty"`

## Method `--method`

Method used for detecting scenes. See [Scene Detection](../types/scene-detection.md).

### Possible Values

- `none` - No scene detection, chunks scenes by maximum length
- `fast` - Fast scene detection, uses av-scenechange with the fast algorithm
- `standard` - Standard scene detection, uses av-scenechange with the standard algorithm
- `transnetv2` - Neural network scene detection, uses TransNetV2 via ONNX Runtime

### Default

If not specified, `standard` is used.

## Min Scene Seconds `--min-scene-seconds`

Minimum scene duration in seconds.

### Default

If not specified, `1` is used.

## Max Scene Seconds `--max-scene-seconds`

Maximum scene duration in seconds.

### Default

If not specified, `10` is used.

## Edit saved settings

`info`, `set` and `reset` read, change and reset the saved scene-detection settings without running the command:

- `set method ALGORITHM [--minimum-frames N] [--maximum-frames N] [--method fast|standard] [--model-path P] [--save-scores [on|off]]` — pick the detection algorithm; flags you omit keep their current value when the chosen algorithm still supports them. `--save-scores` on its own means `on`, `--save-scores off` turns it off; there is no negating flag.
- `set input [--path P] [--decoder D] [--filters …] [--vs-args …] [--stream-concurrently [on|off]]` — the input scene detection reads instead of the main input.
- `info [method|input]` prints JSON; `reset method` resets the algorithm, `reset input` drops the override, and a bare `reset` restores the whole section to its defaults.

```bash
$ av1an detect-scenes set method avscenechange --minimum-frames 20 --maximum-frames 280
$ av1an detect-scenes info method
$ av1an detect-scenes reset method
```

Shared semantics (validation, staleness warnings): see [Editing saved settings](config-editing.md).

## Screenshots

The screenshots below are generated from the live interface. Click any image to open it full size.

<table>
  <tr>
    <td width="50%" valign="top">
      <a href="../media/tui/scene-detection-light.avif"><picture><source srcset="../media/tui/scene-detection-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/scene-detection-light.avif" alt="Detecting scenes" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">Detecting scenes</p>
    </td>
    <td width="50%" valign="top">
      <a href="../media/tui/help-detect-scenes-light.avif"><picture><source srcset="../media/tui/help-detect-scenes-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-detect-scenes-light.avif" alt="av1an detect-scenes --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an detect-scenes --help</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="../media/tui/help-detect-scenes-verbose-light.avif"><picture><source srcset="../media/tui/help-detect-scenes-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-detect-scenes-verbose-light.avif" alt="av1an detect-scenes --help --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an detect-scenes --help --verbose</p>
    </td>
    <td width="50%" valign="top">
      <a href="../media/tui/help-detect-scenes-set-light.avif"><picture><source srcset="../media/tui/help-detect-scenes-set-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-detect-scenes-set-light.avif" alt="av1an detect-scenes set --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an detect-scenes set --help</p>
    </td>
  </tr>
</table>
