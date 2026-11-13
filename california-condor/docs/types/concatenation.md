# Concatenation

`CONCAT` selects how encoded scenes are joined. Used by `--concat` and `concatenate --method` (see [init](../commands/init.md), [pipeline](../commands/condor.md), [concatenate](../commands/concatenate.md)).

| Value      | Tool              | Output Formats | Notes                                                        |
| ---------- | ----------------- | -------------- | ------------------------------------------------------------ |
| `mkvmerge` | MKVToolNix        | `.mkv` only    | Default. Generally best; requires mkvmerge installed. Honors the `mkvmerge` settings under `scene_concatenator.method`. |
| `ffmpeg`   | FFmpeg            | many           | Supports non-Matroska/IVF; may break audio seeking. Honors the `ffmpeg` settings (filters/codecs) under `scene_concatenator.method`. |
| `ivf`      | IVF concatenation | `.ivf` only    | Video only; drops audio, subtitles, chapters, metadata.      |

## Default

If not specified, `mkvmerge` is used.

## Discarding source tracks

Both `mkvmerge` and `ffmpeg` copy the source's tracks by default and take per-track overrides under `scene_concatenator.method`. [`--discard`](../commands/concatenate.md#discard---discard) writes those overrides for you from the input's probed track layout, so a specific category can be dropped without editing the track map by hand. `ivf` ignores it, as it never carries source tracks.

For `mkvmerge`, audio/subtitle overrides go in `tracks`, attachment overrides in
the independent `attachments` map, and video discard targets encoded output
track `0`. Audio `1` and attachment `1` remain independent. See the
[map format and legacy migration](../configuration/condor/sequence-config/scene-concatenator.md#mkvmerge).

## Examples

- `> av1an concatenate --method mkvmerge`
- `> av1an -i input.mp4 -o output.mp4 --concat ffmpeg`
