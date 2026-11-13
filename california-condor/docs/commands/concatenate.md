# Concatenate

Concatenate encoded scenes into output video (triggers TUI). See [guide](../guide.md) and [type reference](../types.md).

```bash
$ av1an concatenate
$ av1an concatenate --method mkvmerge
$ av1an concatenate --method mkvmerge --discard audio,subtitle
```

| Name                              | Flag       | Type     | Default    |
| --------------------------------- | ---------- | -------- | ---------- |
| [Method](#method---method)        | `--method` | [`CONCAT`](../types/concatenation.md) | `mkvmerge` |
| [Discard](#discard---discard)     | `--discard` | [String List](#discard---discard) | nothing |
| Overwrite | `--overwrite` | Flag | Keep saved consent (new jobs: false) |

Existing final outputs require explicit saved `output.overwrite` consent; no prompt
is used. `av1an concatenate --overwrite` saves consent, and omission retains it.
Revoke it with `av1an init set overwrite false` or `av1an init reset overwrite`.
Source aliases remain forbidden, including disabled extra/attachment inputs and
encoded chunks. Output admission precedes probing and saving; a missing output
parent fails without creating it. See [output safety](../configuration/condor/output.md).

## Method `--method`

Method used for concatenating the encoded chunks into the output file. See [Concatenation](../types/concatenation.md).

### Possible Values

- `mkvmerge` - MKVToolNix mkvmerge, Matroska (`.mkv`) only, generally best
- `ffmpeg` - FFmpeg, supports more formats but may break audio seeking
- `ivf` - Indeo Video Format (`.ivf`), video only

### Default

If not specified, `mkvmerge` is used.

### Examples

- `> av1an concatenate --method mkvmerge`
- `> av1an concatenate --method ffmpeg`

## Discard `--discard`

Tracks of the input file to leave out of the output. By default the concatenator copies the input's tracks, so dropping some of them normally means editing the `tracks` map of the saved [Scene Concatenator settings](../configuration/condor/sequence-config/scene-concatenator.md) by hand.

`--discard` probes the input with `mkvmerge --identify`, resolves the tracks of each named category, and writes a matching `"copy": false` entry for each one. The result is saved to `condor.json`, so the settings stay readable and editable afterwards.

The categories are:

- `audio` - every audio track
- `subtitle` - every subtitle track
- `attachment` - every attachment (fonts, etc.)
- `metadata` - inherited global tags and title; explicitly requested replacement metadata still applies
- `chapters` - the input's chapters
- `video` - the video track itself, which for `mkvmerge` is track `0` and for FFmpeg is stream `0`

Data streams are not a category and are always copied.

### Default

If not specified, every track of the input is copied, which is the same as leaving the settings unset.

### Notes

- The input is probed once, when the command runs; the saved entries are not refreshed if the input later gains or loses tracks.
- The `ffprobe` fallback supplies FFmpeg stream indices for the `ffmpeg` method, because `mkvmerge` reports its own track IDs, which differ for attachments. If `mkvmerge` cannot read the input, `ffprobe` alone describes it.
- mkvmerge warnings (exit 1) are accepted only with recognized, supported identification and no reported errors. Failed or unrecognized identification stops `--discard` without saving changes, unless the `ffmpeg` method can use its required `ffprobe` result. A valid video-only source with no matching tracks leaves the track map unset.
- `--discard` has no effect with `--method ivf`, which never carries source tracks.
- When the input is a VapourSynth script there is no container to probe, so the flag is skipped with a warning.

### Examples

- `> av1an concatenate --discard audio`
- `> av1an concatenate --discard audio,subtitle,chapters`
- `> av1an concatenate --discard audio --discard metadata`

## Edit saved settings

`info`, `set` and `reset` read, change and reset the saved concatenation settings without running the command:

- `set method METHOD` — `mkvmerge`, `ffmpeg` or `ivf`. Keeping the current tool keeps its settings; switching to another tool starts from that tool's defaults.
- `set output PATH` — the file the concatenated video is written to; `reset output` clears it.
- `set scenes-directory PATH` — the directory the encoded scenes are read from.
- `info [method|output|scenes-directory]` prints JSON; `reset KEY` resets one of them, and a bare `reset` restores the method and output to their defaults (the temp-derived `scenes-directory` stays).

```bash
$ av1an concatenate set method ffmpeg
$ av1an concatenate set output final.mkv
$ av1an concatenate reset output
```

Shared semantics (validation, staleness warnings): see [Editing saved settings](config-editing.md).

## Screenshots

The screenshots below are generated from the live interface. Click any image to open it full size.

<table>
  <tr>
    <td width="50%" valign="top">
      <a href="../media/tui/scene-concatenator-light.avif"><picture><source srcset="../media/tui/scene-concatenator-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/scene-concatenator-light.avif" alt="Scene concatenator" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">Scene concatenator</p>
    </td>
    <td width="50%" valign="top">
      <a href="../media/tui/help-concatenate-light.avif"><picture><source srcset="../media/tui/help-concatenate-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-concatenate-light.avif" alt="av1an concatenate --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an concatenate --help</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="../media/tui/help-concatenate-verbose-light.avif"><picture><source srcset="../media/tui/help-concatenate-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-concatenate-verbose-light.avif" alt="av1an concatenate --help --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an concatenate --help --verbose</p>
    </td>
    <td width="50%" valign="top">
      <a href="../media/tui/help-concatenate-set-light.avif"><picture><source srcset="../media/tui/help-concatenate-set-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-concatenate-set-light.avif" alt="av1an concatenate set --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an concatenate set --help</p>
    </td>
  </tr>
</table>
