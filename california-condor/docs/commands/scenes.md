# Scenes

Inspect and edit the scenes saved in the configuration. Scenes hold everything the pipeline saved per scene — frame ranges, sub-scenes, the per-scene encoder and the step results — and this command reads and changes them in place, without running anything. See [guide](../guide.md), [condor.scenes](../configuration/condor/scene.md), and [Editing saved settings](./config-editing.md).

```bash
$ condor scenes                          # every scene, as JSON
$ condor scenes 1 info                   # one scene
$ condor scenes 1,3 info frames          # a list of scenes, one key each
$ condor scenes 43-46 info quality-check
$ condor scenes 0 set start 23           # change a scene's first frame
$ condor scenes 48 set end 8943          # change a scene's last frame
$ condor scenes -1 reset target-quality  # clear the saved results
$ condor scenes 3 delete                 # drop one scene
$ condor scenes add 1200 1450            # insert a new one
```

The index comes first and every action works on the scenes it selects. After its own output, each run reports how many scenes it addressed and the frames they cover, so an index can be checked at a glance:

```console
$ condor scenes 1
{
  "start_frame": 121,
  "end_frame": 240,
  …the rest of the scene…
}
INFO Scenes: 1 Frames: 121-240
```

The summary covers the scenes that run addressed, never the whole list when an index narrowed it: `condor scenes 0 info frames` reports `Scenes: 1`, `delete` reports the scenes it removed and `add` the scene it inserted.

Every write names the scenes it changed and what it did. The full JSON value a `set` or `reset` left behind is printed only under `--verbose`; the status line is enough to say what happened. `add` and `delete` report the frames the scenes held, the compact form of the same information:

```console
$ condor scenes add 1200 1450
Added scene 5: 1200-1450
INFO Scenes: 1 Frames: 1200-1450

$ condor scenes 1,3 delete
Deleted scenes 1,3: 121-240, 480-528
INFO Scenes: 2 Frames: 121-528
```

## Index

| Form | Meaning |
| ---- | ------- |
| `1` | One scene |
| `0-4` | An inclusive range: scenes 0, 1, 2, 3 and 4 |
| `1,3` | A list; ranges may appear in it |
| `1-2,8-10,16` | Any mix of ranges and single indices |
| `-1`, `all`, `every` | Every scene |

The index is resolved against the saved list, so a selection that reaches past the end is rejected and names the lowest index that does not exist. Repeated and unordered values are accepted: `3,1-2,1` resolves to the ascending set `1,2,3`.

`info` accepts no index at all, which means the whole list. `set` and `delete` need one — write `all` to address every scene — and `add` takes none, because it places the scene itself: `condor scenes 1 add 10 20` is a usage error, and the only way to add a scene is `condor scenes add START END`.

## Info `info [KEY]`

Prints the addressed scenes as pretty JSON, in the same shape they have in `condor.json`. One addressed scene is printed on its own; several, or the whole list, print an array.

| Key | Prints |
| --- | ------ |
| *(none)* | The scenes themselves, with everything saved per scene |
| `frames` | Each scene's `start_frame` and `end_frame` |
| `encoder` | Each scene's encoder and its parameters |
| `sub-scenes` | Each scene's sub-scenes |
| `scene-detection` | The detection scores and when they were computed |
| `noise-detection` | The noise detection results |
| `noise-scaling` | The noise scaling results |
| `parallel-encoder` | The encode progress: `started_on`, `completed_on` and `bytes` |
| `target-quality` | The target-quality passes |
| `quality-check` | The quality-check results |
| `zoner` | Whether the Zoner has processed the scene list |

```bash
$ condor scenes info                     # the whole list
$ condor scenes 1 info                   # scene 1, on its own
$ condor scenes 2 info frames
$ condor scenes 1,3-4 info encoder
$ condor scenes all info quality-check
```

`info` never writes. Without an action at all the addressed scenes are printed, so `condor scenes` and `condor scenes 1` are shorthand for the two `info` reads above.

## Set `set <PROPERTY>`

Changes the named setting of every addressed scene. Nothing is saved unless every scene accepts the change: one rejected scene leaves the configuration file exactly as it was.

| Property | Changes | Flags |
| -------- | ------- | ----- |
| `start FRAME` | The frame the scene starts at (inclusive) | |
| `end FRAME` | The frame the scene ends at (exclusive) | |
| `encoder ENCODER` | The scene's encoder, from that encoder's defaults | `--executable PATH`, `--passes PASSES`, `--params PARAMS`, `--iso LUMA`, `--chroma-iso CHROMA`, `--append-color-data [on\|off]`, `--ffmpeg-filter FILTER` |
| `executable PATH` | The encoder binary to use | |
| `passes PASSES` | How many passes the encoder makes | |
| `params PARAMS` | Encoder parameters (`--crf 16 --tune 0`), merged into the encoder's own | |
| `photon-noise ISO` | The Photon Noise table's luma ISO | `--chroma-noise ISO` |
| `append-color-data [on\|off]` | Whether the input's colour and HDR metadata is appended to the encoder parameters (defaults to `on`) | |
| `ffmpeg-filter FILTER` | A filtergraph applied to the frames before they reach the encoder | |
| `sub-scenes RANGE...` | The scene's sub-scenes, as `START-END` ranges; `none` clears them | |

### Frame ranges

`start` and `end` are the scene's half-open range, the same values `info frames` prints. The only rule is that the scene stays non-empty (`start` below its `end`, `end` above its `start`), so scenes may be moved to leave a gap between them.

```bash
$ condor scenes 0 set start 23
$ condor scenes 48 set end 8943
$ condor scenes 3-6 set end 2400
```

Nothing checks the new frame against the input's length: the command never opens the input, so a frame past the last one is saved as written. Changing a frame clips the scene's sub-scenes to the new range, the same way [`condor zone`](./zone.md) clips them when it splits a scene.

Moving a frame that reaches into another scene is **allowed**, with a warning naming every scene the new range now shares frames with:

```console
$ condor scenes 1 set start 50
WARN scene 1 (50..240) overlaps scene 0 (23..121); the pipeline would encode the shared frames twice
Scenes scene 1 start_frame updated
INFO Scenes: 1 Frames: 50-240
```

Overlapping scenes are not repaired for you: the neighbouring scene keeps its range, and the pipeline encodes the shared frames twice. Move the other scene's frame as well, or delete it.

### Encoder and its settings

`set encoder` switches the scene to another encoder, starting from that encoder's defaults, and applies any flags given on the same line after the switch:

```bash
$ condor scenes 43-46 set encoder svt-av1 --passes 2 --params "--crf 18" --iso 800 --chroma-iso 200
$ condor scenes 2 set encoder x265 --executable /tools/x265 --params "--crf 19 --tune 0"
$ condor scenes 2 set encoder x264 --append-color-data off
```

The individual properties remain available when only one setting changes:

```bash
$ condor scenes 2 set encoder x265
$ condor scenes 2 set executable /tools/x265
$ condor scenes 3 set passes 2
$ condor scenes 2 set params "--crf 18"
$ condor scenes 4 set photon-noise 800 --chroma-noise 200
$ condor scenes 2 set append-color-data off
$ condor scenes 0 set ffmpeg-filter hflip
$ condor scenes 1 set sub-scenes 121-180 180-240
```

An encoder that cannot honour a setting is reported rather than given a command line it would reject, and the scene is left untouched: `--passes` on a single-pass encoder such as FFmpeg, `--iso`/`--chroma-iso` on an encoder without a Photon Noise table (only AOM, RAV1E and SVT-AV1 have one), `--ffmpeg-filter` on FFmpeg itself, and `--chroma-iso` without `--iso`. A path that does not exist yet only warns: it will be needed when the encoder runs.

## Reset `reset [KEY]`

Restores what a detection run would have written, without re-detecting:

| Form | Restores |
| ---- | -------- |
| `reset KEY` | That key of every addressed scene, e.g. `encoder`, `sub-scenes`, `target-quality` |
| `reset` (with an index) | The scenes themselves: the configuration's encoder and no sub-scenes |
| `reset` (no index) | The whole list, which is emptied with a hint to re-detect |

Every sequence-data key can be reset, which is how saved results are cleared before recomputing them:

```bash
$ condor scenes -1 reset target-quality
$ condor scenes 43-46 reset quality-check
$ condor scenes all reset parallel-encoder
$ condor scenes 7 reset noise-detection
```

The one key that is not a saved result is `zoner`, the record [`condor zone`](./zone.md) writes when it has applied the configured zones to the list. Clearing it does not restore anything; it lets the next Zoner run apply the zones again:

```bash
$ condor scenes all reset zoner
```

`frames` is the one key with no default: a frame range is the output of detection, so it can only be changed with `set start`/`set end` or rebuilt with [`condor detect-scenes`](./detect-scenes.md). Resetting scene data never touches the frame range or the encoder.

## Delete `delete`

Removes the addressed scenes from the list and reports the frames they held:

```bash
$ condor scenes 3 delete
Deleted scene 3: 480-528

$ condor scenes 1,4-6 delete
Deleted scenes 1,4-6: 121-240, 480-528, 528-720
```

Under `--verbose` the removed scenes are printed as JSON as well.

Later scenes keep their order but shift down, so their indices — and the encode/probe files named after them — no longer match. That staleness is reported as a warning, and the files are left alone.

## Add `add START END`

Inserts a new scene spanning `START`..`END`. The scene takes the configuration's encoder, no sub-scenes and no saved results, exactly like a freshly detected one. `START` must be below `END`.

`add` is the one scene action that takes no index, so it is not listed among the index-addressed actions in `condor scenes --help`; the command's own help (`condor scenes add --help`) and an indexed attempt's error both name the form.

By default the scene is inserted so the list keeps ascending start frames, which puts a scene starting at frame 0 first:

```bash
$ condor scenes add 1200 1450
Added scene 8: 1200-1450
```

Under `--verbose` the new scene is printed as JSON as well.

| Flag | Inserts |
| ---- | ------- |
| *(none)* | Where ascending start frames put it |
| `--append` | At the end of the list |
| `--prepend` | At the start of the list |
| `--after N` | Directly after scene N |

The new scene is not checked against its neighbours either: a range that reaches into another scene is added with the same overlap warning `set start`/`set end` gives.

## Warnings

Every write warns in two cases, and never changes anything on its own:

- **Overlaps** name each scene the new range shares frames with.
- **Staleness** covers everything the change may have invalidated: saved results (noise, target-quality, encode, quality-check) of the touched scenes, and existing index-named encode/probe files on disk. Scenes whose index shifted — because a scene was added or deleted — are reported too.

Results and files are never deleted or recomputed: clean or recompute the reported artifacts before resuming, for example with [`condor clean`](./clean.md), then rerun the step that produced them. The generic `WARN` every configuration write prints (see [Editing saved settings](./config-editing.md)) is reported alongside these.

## Editing saved scenes

The list is edited in place, unlike [`condor zone`](./zone.md), whose declarative zones split scenes when they are applied. Scenes must exist to be edited: run [`condor detect-scenes`](./detect-scenes.md) first, or let [`condor`](./condor.md) run detection before its Zoner step.
