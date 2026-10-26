# Zone

A Zone is a frame range whose scenes take its encoder settings. Zones are the successor to Av1an's zones, and reach Condor through [`condor.sequence_config.zoner`](../configuration/condor/sequence-config/zoner.md) via the [`--zones`](../commands/condor.md#zones---zones) flag or the [`zone`](../commands/zone.md) command.

Every scene sharing at least one frame with the zone is modified whole: a scene is never cut at a zone boundary, so a scene that reaches into two zones takes both, the later zone last. Zones must not overlap. Individual overrides are idempotent, but splitting can change which zones a scene intersects on a later run; use freshly detected scenes when reapplying interacting zones.

| Field | Type | Required | Description |
| ----- | ---- | -------- | ----------- |
| `start_frame` | Integer or null | No | First frame of the zone (inclusive). `null` is the input's first frame |
| `end_frame` | Integer or null | No | End of the zone (exclusive). `null` is the input's last frame |
| `maximum_frames` | Integer or null | No | Splits scenes longer than this, so no scene a zone matches exceeds it |
| `encoder` | Object or null | No | The encoder override the zone's scenes take, `null` to only cap scene length |

Frames are half-open, matching [`condor.scene`](../configuration/condor/scene.md): `"start_frame": 0, "end_frame": 200` covers frames 0 through 199, and the next zone can start at 200. An explicit end may equal the input frame count but cannot exceed it; empty/reversed ranges and zero length caps are rejected. Sub-scenes crossing a length-cap split are clipped to each new parent range.

## Zone Strings

The [`--zones`](../commands/condor.md#zones---zones) flag and [`zone`](../commands/zone.md) command take the same definition:

```text
<start> <end> [replace|merge] [encoder[(executable)]] ["options"] [flags]
```

| Part | Description |
| ---- | ----------- |
| `<start>` `<end>` | Frame numbers, or `-1` for the input's first or last frame |
| `replace` | Replaces the scene's encoder outright. The default, `merge`, patches the scene's own |
| `encoder` | The encoder base the zone's settings belong to. A merge that names another base leaves the scene unchanged |
| `(executable)` | The encoder binary, in parentheses after the base (e.g. `svt-av1("/tools/fish")`). A path that does not exist falls back to the base's default binary |
| `"options"` | Encoder parameters, as in [`--params`](./encoder-params.md) |
| `--maximum-frames` | Splits the zone's scenes past this many frames |
| `--ffmpeg-filter` | FFmpeg filtergraph applied to the zone's scenes; unsupported for an FFmpeg encoder, which uses native FFmpeg options |
| `--iso` | Photon Noise ISO strength |
| `--chroma-iso` | Chroma ISO strength for the Photon Noise table |
| `--width` `--height` | Photon Noise table dimensions, defaulting to the input's |
| `--sy` `--ccb` `--ccr` | Comma-separated Photon Noise tables for the Y, Cb and Cr planes |
| `--append-color-data` | Whether the input's colour and HDR metadata is appended to the encoder parameters, as `on`/`off` (defaults to `on`) |

An encoder name is required whenever the zone sets options, Photon Noise or a filtergraph; a zone that only caps scene length needs none.

Merging keeps whatever the zone does not set: options are merged key by key, and a Photon Noise override patches the scene's table field by field. Replacing options seeds the encoder base's defaults first, so `"--preset 3 --crf 23"` replaces the scene's set the way `--encoder svt-av1 --params "--preset 3 --crf 23"` builds it.

### Examples

- `condor zone '-1 200 svt-av1 "--preset 3 --crf 23" --iso 400'`
- `condor zone '-1 200 replace aom("/tools/aomenc") "--cpu-used=4" --maximum-frames 240'`
- `condor zone '200 -1 x265 "--crf 18" --ffmpeg-filter "crop=iw-16:ih-16"'`
- `condor zone '-1 200 x264 --append-color-data off'`

## Zone Files

A value that names an existing file is read as a zone file, by extension:

- `.json`: a `{ "zones": [ ... ] }` object, as in `condor.json`, or a bare array of zones. JSON objects can name the finer `options` and `photon_noise` replacement modes, including in JSON lines files.
- `.jsonl`, `.ndjson`: one zone object per line.
- anything else: a text file of zone strings, one per line, with `#` comments and blank lines.

```json
{
    "zones": [
        {
            "start_frame": null,
            "end_frame": 200,
            "maximum_frames": 240,
            "encoder": {
                "Merge": {
                    "base": "SVTAV1",
                    "executable": "/tools/fish",
                    "options": { "Merge": "--preset 3 --crf 23" },
                    "photon_noise": { "Merge": { "iso": 400, "chroma_iso": 800 } },
                    "append_color_data": false
                }
            }
        }
    ]
}
```

The JSON format is lossless: `"Replace"` swaps the whole encoder instead of merging, `options` and `photon_noise` take their own `"Replace"` or `"Merge"`, and a `photon_noise` `"Replace"` needs an `iso`. `append_color_data` is a nullable boolean on the merge object; `null` (or leaving it out) keeps the scene's own value.
