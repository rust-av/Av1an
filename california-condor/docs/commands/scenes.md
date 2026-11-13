# Scenes

Inspect and edit the scenes saved in the configuration. Scenes hold everything the pipeline saved per scene — frame ranges, sub-scenes, the per-scene encoder and the step results — and this command reads and changes them in place, without running anything. See [guide](../guide.md), [condor.scenes](../configuration/condor/scene.md), and [Editing saved settings](./config-editing.md).

```bash
$ av1an scenes                          # every scene, as JSON
$ av1an scenes 1 info                   # one scene
$ av1an scenes 1,3 info frames          # a list of scenes, one key each
$ av1an scenes 43-46 info quality-check
$ av1an scenes 0 set start 23           # change a scene's first frame
$ av1an scenes 48 set end 8943          # change a scene's last frame
$ av1an scenes -1 reset target-quality  # clear the saved results
$ av1an scenes 3 delete                 # drop one scene
$ av1an scenes add 1200 1450            # insert a new one
```

The index comes first and every action works on the scenes it selects. After its own output, each run reports how many scenes it addressed and the frames they cover, so an index can be checked at a glance:

```console
$ av1an scenes 1
{
  "start_frame": 121,
  "end_frame": 240,
  …the rest of the scene…
}
INFO Scenes: 1 Frames: 121-240
```

The summary covers the scenes that run addressed, never the whole list when an index narrowed it: `av1an scenes 0 info frames` reports `Scenes: 1`, `delete` reports the scenes it removed and `add` the scene it inserted.

Every write names the scenes it changed and what it did. The full JSON value a `set` or `reset` left behind is printed only under `--verbose`; the status line is enough to say what happened. `add` and `delete` report the frames the scenes held, the compact form of the same information:

```console
$ av1an scenes add 1200 1450
Added scene 5: 1200-1450
INFO Scenes: 1 Frames: 1200-1450

$ av1an scenes 1,3 delete
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

`info` accepts no index at all, which means the whole list. `set` and `delete` need one — write `all` to address every scene — and `add` takes none, because it places the scene itself: `av1an scenes 1 add 10 20` is a usage error, and the only way to add a scene is `av1an scenes add START END`.

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
| `parallel-encoder` | Encode progress, provenance and the `invalidated` eligibility barrier |
| `target-quality` | The target-quality passes |
| `quality-check` | The quality-check results |
| `zoner` | The pass flag and first scene's compact original/applied lifecycle record |

```bash
$ av1an scenes info                     # the whole list
$ av1an scenes 1 info                   # scene 1, on its own
$ av1an scenes 2 info frames
$ av1an scenes 1,3-4 info encoder
$ av1an scenes all info quality-check
```

`info` never writes. Without an action at all the addressed scenes are printed, so `av1an scenes` and `av1an scenes 1` are shorthand for the two `info` reads above.

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
$ av1an scenes 0 set start 23
$ av1an scenes 48 set end 8943
$ av1an scenes 3-6 set end 2400
```

Nothing checks the new frame against the input's length: the command never opens the input, so a frame past the last one is saved as written. Changing a frame clips the scene's sub-scenes to the new range, the same way [`av1an zone`](./zone.md) clips them when it splits a scene.

Moving a frame that reaches into another scene is **allowed**, with a warning naming every scene the new range now shares frames with:

```console
$ av1an scenes 1 set start 50
WARN scene 1 (50..240) overlaps scene 0 (23..121); the pipeline would encode the shared frames twice
Scenes scene 1 start_frame updated
INFO Scenes: 1 Frames: 50-240
```

Overlapping scenes are not repaired for you: the neighbouring scene keeps its range, and the pipeline encodes the shared frames twice. Move the other scene's frame as well, or delete it.

### Encoder and its settings

Each saved scene owns its encoder. Later global encoder edits affect future
detection/new scenes only; they do not overwrite scene or zone overrides.
`info encoder` reports the actual saved encoder, not the current global default.
Use `av1an scenes all reset encoder` to copy that default into every saved scene
deliberately. This preserves frame ranges, sub-scenes and files; actual encoder
changes withdraw affected results as described below. It does not reapply zones.

`set encoder` switches the scene to another encoder, starting from that encoder's defaults, and applies any flags given on the same line after the switch:

```bash
$ av1an scenes 43-46 set encoder svt-av1 --passes 2 --params "--crf 18" --iso 800 --chroma-iso 200
$ av1an scenes 2 set encoder x265 --executable /tools/x265 --params "--crf 19 --tune 0"
$ av1an scenes 2 set encoder x264 --append-color-data off
```

The individual properties remain available when only one setting changes:

```bash
$ av1an scenes 2 set encoder x265
$ av1an scenes 2 set executable /tools/x265
$ av1an scenes 3 set passes 2
$ av1an scenes 2 set params "--crf 18"
$ av1an scenes 4 set photon-noise 800 --chroma-noise 200
$ av1an scenes 2 set append-color-data off
$ av1an scenes 0 set ffmpeg-filter hflip
$ av1an scenes 1 set sub-scenes 121-180 180-240
```

An encoder that cannot honour a setting is reported rather than given a command line it would reject, and the scene is left untouched: `--passes` on a single-pass encoder such as FFmpeg, `--iso`/`--chroma-iso` on an encoder without a Photon Noise table (only AOM, RAV1E and SVT-AV1 have one), `--ffmpeg-filter` on FFmpeg itself, and `--chroma-iso` without `--iso`. A path that does not exist yet only warns: it will be needed when the encoder runs.

## Reset `reset [KEY]`

Restores what a detection run would have written, without re-detecting:

| Form | Restores |
| ---- | -------- |
| `reset KEY` | That key of every addressed scene, e.g. `encoder`, `sub-scenes`, `target-quality` |
| `reset` (with an index) | The scenes themselves: the configuration's encoder and no sub-scenes |
| `reset` (no index) | The whole list, which is emptied with a hint to re-detect |

Saved result keys can be reset before recomputing them:

```bash
$ av1an scenes -1 reset target-quality
$ av1an scenes 43-46 reset quality-check
$ av1an scenes all reset parallel-encoder
$ av1an scenes 7 reset noise-detection
```

`zoner` is not a result: erasing its proof on an already-zoned list is refused.
Known changed definitions rebase automatically from the original scene baseline.
For unknown legacy state or unsafe manual edits, preserve the job and reset/redetect
the whole scene list in an empty work directory, or start a separate job.

`frames` is the one key with no default: a frame range is the output of detection, so it can only be changed with `set start`/`set end` or rebuilt with [`av1an detect-scenes`](./detect-scenes.md). Most result resets leave the encoder alone. `reset noise-scaling` instead restores the producer-recorded original PhotonNoise before clearing the scaling record. It refuses legacy records without that proof, or conflicting PhotonNoise edits; it never divides rounded ISO values or resets other encoder options. Reset scaling **before** editing PhotonNoise ISO/chroma/calibration fields.

## Delete `delete`

Removes the addressed scenes from the list and reports the frames they held:

```bash
$ av1an scenes 3 delete
Deleted scene 3: 480-528

$ av1an scenes 1,4-6 delete
Deleted scenes 1,4-6: 121-240, 480-528, 528-720
```

Under `--verbose` the removed scenes are printed as JSON as well.

Later scenes keep their order but shift down. Their old index-named files no longer match; affected results are withdrawn and eligibility is blocked, while files are left alone.

## Add `add START END`

Inserts a new scene spanning `START`..`END`. The scene takes the configuration's encoder, no sub-scenes and no saved results, exactly like a freshly detected one. `START` must be below `END`.

`add` is the one scene action that takes no index, so it is not listed among the index-addressed actions in `av1an scenes --help`; the command's own help (`av1an scenes add --help`) and an indexed attempt's error both name the form.

By default the scene is inserted so the list keeps ascending start frames, which puts a scene starting at frame 0 first:

```bash
$ av1an scenes add 1200 1450
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

## Invalidation and recovery

Writes report overlaps and stale results:

- **Overlaps** name each scene the new range shares frames with.
- **Staleness** covers everything the change may have invalidated: saved results (noise, target-quality, encode, quality-check) of the touched scenes, and existing index-named encode/probe files on disk. Scenes whose index shifted — because a scene was added or deleted — are reported too.

Range, sub-scene, encoder and index changes clear affected target-quality passes,
selection/speed transitions, encode timestamps/bytes/provenance and claimed QC
before saving. Detection and noise measurements survive valid range/sub-scene
mappings, including moved ranges. Known source/detector changes invalidate their
noise measurements. Noise scaling survives only with matching producer proof for
PhotonNoise, declared source/detector/scaler settings, measurement and global noise
maximum. Withdrawing scaling restores its exact original PhotonNoise, preserving
zone-specific encoder options; unknown or conflicting originals refuse before save.
An encoder-only edit does not withdraw otherwise-valid noise results. Unaffected
results are preserved. Result-only resets do not count as scene edits, except that
restoring an unscaled ISO is an actual encoder change. Clean jobs remain runnable.

`sequence_data.parallel_encoder.invalidated: true` blocks CLI and direct core
encoding even when old files are absent, because the encoder may retain a derived
quantizer. Resetting `parallel-encoder` explicitly clears that barrier, **not** the
physical files or inherited encoder settings. The CLI cannot relocate a populated
scene job using `--temp`. For a job with old probe/encode/quality artifacts or an
unknown inherited quantizer, [start a separate job](clean.md#start-a-separate-job)
with a distinct config, empty temp directory and output. Existing chunks without
matching current provenance remain refused; a new config filename alone does not
make the old work directory safe. There is no automatic deletion or cleanup.

For an already-empty work directory with only noise results (no inherited TQ
quantizer or old probe/encode/quality files), known scaling can be reset/restored
without resetting the whole encoder or losing zone overrides. Make encoder/range
edits first, reset applicable TQ/QC records, and reset `parallel-encoder` **last**
before rerunning detection/scaling. A later encoder edit can set the barrier again.
The direct Rust API can also reset records and select an empty work directory;
that API control is not a CLI work-directory migration feature.

The safer alternative is to [start a separate job](clean.md#start-a-separate-job)
with a distinct `--config-file`, empty `--temp` directory and output. Full
`av1an scenes reset` drops the scene list and its zoning lifecycle before
redetection; it does not remove files. Deleting/prepending the first scene carries
existing zoning proof onto the new first scene, without changing its old applied
definitions: unchanged zones still skip, while unsafe changed-zone rebases refuse.
`av1an clean` is unavailable. Failed
validation/save leaves original JSON and files unchanged. The generic `WARN`
described in [Editing saved settings](./config-editing.md) is also reported.

## Editing saved scenes

The list is edited in place, unlike [`av1an zone`](./zone.md), whose declarative zones split scenes when they are applied. Scenes must exist to be edited: run [`av1an detect-scenes`](./detect-scenes.md) first, or let [`av1an`](./condor.md) run detection before its Zoner step.
