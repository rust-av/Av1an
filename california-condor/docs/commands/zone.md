# Zone

Apply zone overrides to exact half-open frame ranges. Zones are the successor to Av1an's zones: scenes are cut at every zone boundary, then each zone merges or replaces encoder settings and caps length only inside its range. Existing scene cuts and settings outside zones survive. See [guide](../guide.md), [Zone](../types/zone.md), and [zoner](../configuration/condor/sequence-config/zoner.md).

```bash
$ av1an zone '-1 200 svt-av1 "--preset 3 --crf 23"'
$ av1an zone '-1 200 replace aom("/tools/aomenc") "--cpu-used=4" --maximum-frames 240'
$ av1an zone ./zones.json
$ av1an zone --dry-run '-1 200 svt-av1 "--preset 3" --maximum-frames 96'
```

| Name                | Flag      | Type                    | Default |
| ------------------- | --------- | ----------------------- | ------- |
| [Zones](#zones-zone) | `ZONE...` | [Zone](../types/zone.md) |         |
| [Dry run](#dry-run---dry-run) | `--dry-run` | Boolean | `false` |

## Zones `ZONE...`

One or more zone definitions, or paths to zone files. Each definition reads

`<start> <end> [replace|merge] [encoder[(executable)]] ["options"] [flags]`

where the frames are half-open with `-1` for the input's first or last frame, `replace` replaces the scene's encoder instead of merging into it, the encoder accepts its binary in parentheses, the quoted string holds encoder parameters (as in [`--params`](../types/encoder-params.md)), and the flags are `--maximum-frames`, `--ffmpeg-filter`, `--append-color-data`, `--iso`, `--chroma-iso`, `--width`, `--height`, `--sy`, `--ccb` and `--ccr`. See [Zone](../types/zone.md) for every part.

A value that names an existing file is read as a zone file: `.json` for `{ "zones": [ ... ] }` or a bare array, `.jsonl`/`.ndjson` for one zone per line, and anything else as a text file of definitions with `#` comments.

### Examples

- `> av1an zone '-1 200 svt-av1 "--preset 3 --crf 23"'` - Merges into the scenes of the first 200 frames
- `> av1an zone '-1 200 replace svt-av1("/tools/fish") "--preset 3 --crf 23" --iso 400'` - Replaces the encoder of those scenes, with Photon Noise
- `> av1an zone '-1 200 x264 "--crf 18" --append-color-data off'` - Leaves the colour and HDR metadata out of that zone's encoder parameters
- `> av1an zone ./zones.txt` - Applies every definition in a text file
- `> av1an zone` - Applies the zones already in the configuration

## Dry run `--dry-run`

Preview changes with the same validation, INFO scene/split reports, and warnings as a normal run. Supply definitions or files, or use `av1an zone --dry-run` to preview the configured zones. The output is explicitly marked as a dry-run.

No zone definitions or scene changes are saved, and no encode/probe outputs are modified. Normal log files and input-opening/index-cache activity still occur; dry-run does not mean zero filesystem activity.

## Zoned Scenes

After validation and application succeed, zones, scenes, and invalidation metadata are saved together. The first pass stores the original per-scene ranges, sub-scenes and encoders, plus the structured applied definitions. Known unchanged definitions skip without another save, including after reloading the job. Changed or removed overrides rebuild from that original baseline, not from global defaults or previous overrides. To remove all definitions, set `condor.sequence_config.zoner` to `null` (or its `zones` to `[]`) and run `av1an zone`.

A failed command or dry-run leaves configuration bytes unchanged. For example, scene `[0,100)` with zone `[25,75)` becomes `[0,25)`, `[25,75)`, `[75,100)`; only the middle piece takes the zone's settings. `maximum_frames` then splits each inside piece evenly, without capping outside pieces. Sub-scenes crossing either kind of cut are clipped to each new parent.

**Existing jobs:** a saved lifecycle without `algorithm_revision` records the old whole-scene algorithm. When its original baseline and last-applied definitions still match, the next zoning pass rebuilds exact boundaries and invalidates affected results, even if zone definitions are unchanged. Old chunks and probes remain on disk but are not adopted for the new ranges/indices. Unsafe manual/downstream edits and unsupported future revisions refuse without saving. Preview with `av1an zone --dry-run` before migrating; use a separate job if affected results cannot be safely recomputed. Current revision `1` with unchanged definitions still skips without saving.

For unchanged old zone definitions, Zoner first applies the exact algorithm to the proven original baseline. If the result equals the recorded applied definitions (for example, already-aligned or whole-input zones), no scene rewrite is needed: only the algorithm revision is saved once. Actual downstream NoiseScaler encoders, manual edits and all results remain untouched. This does not certify old artifacts; result admission still applies. A failed revision-only save preserves the original job, and the next unchanged current run saves nothing.

Scene-list edits carry the existing lifecycle onto the new first scene when the
old first scene is deleted or displaced. They preserve the old applied definitions,
so unchanged zones still skip without saving, but changed-zone rebasing cannot
overwrite those manual edits. Resetting the whole list clears its lifecycle.

A merge whose encoder base differs from the scene's leaves that scene unchanged with a warning, rather than put one encoder's parameters on another's command line.

`av1an zone` needs scenes to zone: run [detect-scenes](./detect-scenes.md) first, or let [`av1an`](./condor.md) run detection before its Zoner step.

Actual changes clear affected probe/selection, encode and quality-check records before saving. Valid range-mapped noise measurements and proven scaling are retained; withdrawn scaling must restore its exact original PhotonNoise or refuse. Shifted indices count as changes even when a moved range is identical. Invalidated claims/artifacts set a persisted barrier even if old files are later absent. Files, source and final output are preserved; there is no automatic cleanup. See [scene invalidation and recovery](scenes.md#invalidation-and-recovery).

Legacy `zoned: true` without original/applied proof cannot safely skip or restore settings. Manual scene edits (or downstream encoder changes) can also make a later rebase unsafe. These cases preserve edits and refuse: reset/redetect the **whole scene list** in a new/empty work directory, or [start a separate job](clean.md#start-a-separate-job) with a distinct config, empty temp directory and output. Clearing only the old zoning flag is not a safe migration.

## Console reporting

Zoner reports its work at `INFO` level, both for `av1an zone` and in the full pipeline:

- Each encoder change identifies the zone and scene's half-open frame range.
- Each split identifies the original scene, the resulting frame ranges, and how many scenes were added.
- Boundary partitioning reports added pieces separately, before overrides and caps. Each zone then reports matched, modified, cap-split, added, and matched-but-unchanged partition counts. A partition changed and cap-split by the same zone counts as one modified scene. No-op overrides and skipped encoder mismatches do not count as encoder changes.
- The final summary reports the number of zones, scene changes across zones after partitioning, total scenes added (including boundary cuts), before/after scene counts, and application warnings. Additional stale-result warnings are reported separately. These are not counts of unique original scenes.
- A run with proven unchanged definitions reports that at `INFO` and applies nothing.

The command modifies scene configuration; no encoding or artifact deletion happens.

## Screenshots

The screenshots below are generated from the live interface. Click any image to open it full size.

<table>
  <tr>
    <td width="50%" valign="top">
      <a href="../media/tui/help-zone-light.avif"><picture><source srcset="../media/tui/help-zone-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-zone-light.avif" alt="av1an zone --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an zone --help</p>
    </td>
    <td width="50%" valign="top">
      <a href="../media/tui/help-zone-verbose-light.avif"><picture><source srcset="../media/tui/help-zone-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-zone-verbose-light.avif" alt="av1an zone --help --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an zone --help --verbose</p>
    </td>
  </tr>
</table>
