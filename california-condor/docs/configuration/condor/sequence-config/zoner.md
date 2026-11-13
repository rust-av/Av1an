# sequence_config.zoner

`sequence_config.zoner` in `condor.json`. See [sequence_config](../sequence_config.md). Created by [`zone`](../../../commands/zone.md) or [`av1an --zones`](../../../commands/condor.md#zones---zones) (`null` on `init`), and applied by [`av1an`](../../../commands/condor.md) right after Scene Detection.

| Field | Type | Required | Default | Description |
| ----- | ---- | -------- | ------- | ----------- |
| `zones` | Array | No | `[]` | The zone overrides, applied in start-frame order. Each item is a [Zone](../../../types/zone.md) |

CLI mapping: `zone ZONE...` and `av1an --zones ZONE...`, where each value is a zone definition or a path to a JSON, JSON lines, or text zone file. Values replace the zones already configured.

Ranges are half-open: all zone starts/ends cut existing scenes before settings or `maximum_frames` apply. Existing cuts, order and gaps remain; outside pieces keep their original encoder and are not capped by adjacent zones. Inside pieces use balanced length-cap splits, with crossing sub-scenes clipped at every cut.

## Implementation structure

Zoner belongs entirely to `california-condor`; it does not add a sequence or zone configuration to `andean-condor`.

- `src/zoner/zone.rs` owns the zone model, encoder overrides, and scene splitting.
- `src/zoner/parse.rs` reads inline definitions and JSON, JSON lines, or text files, reusing the encoder parameter parser.
- `src/zoner/mod.rs` owns `ZonerConfig`, `ZonerData`, range validation, ordered application, and the `Sequence` adapter. `sequence_data.zoner.lifecycle` on the first scene stores `algorithm_revision`, `baseline`, structured `zones`, and `applied` definitions. Definitions contain only ranges, sub-scenes and encoders, never nested scene results.
- The handler stages definitions in memory. The sequence publishes scenes, lifecycle and invalidation together only after validation/application. Known unchanged definitions do not reapply or save. Changed/removed definitions rebuild from the original baseline. `zone --dry-run` uses the same sequence with a no-op save callback and disposable state. The pipeline applies zones immediately after Scene Detector and before noise processing. All CLI sequence runners share one configuration-save callback; direct callers must retain their job lock through the callback.
- `andean-condor` only supplies generic encoder accessors and equality derives, not zoning policy.

Affected derived results are cleared before publication; old claims/files set the core-readable `sequence_data.parallel_encoder.invalidated` barrier. Files are never automatically removed. Explicit result reset clears the marker, not files or inherited encoder settings: recompute only in an empty/new work directory.

Legacy zoning flags without lifecycle proof refuse. Rebasing also refuses if manual or downstream encoder edits no longer match the last applied scene definitions. Preserve edits and reset/redetect the whole scene list in an empty work directory, or start a distinct job; clearing only the flag cannot recover original settings.

Missing `lifecycle.algorithm_revision` defaults to `0` (whole-scene zoning). Current `1` means exact boundary partitioning. For unchanged revision `0` zones, the current algorithm first runs on the proven original baseline. If its output equals recorded `applied` definitions, only the revision tag is saved once, preserving actual downstream encoders and all results. Otherwise the baseline must replay through normal invalidation, or refuse if doing so would overwrite manual/downstream edits. Changed definitions do not use the revision-only shortcut; unsupported future revisions refuse without publication. Old physical files are preserved, not certified for new indices. Preview with `zone --dry-run`; affected saved results may require a separate job. Unchanged revision `1` definitions need no extra save. This tag versions zoning only, not metric/result provenance.

Scene CRUD carries an existing first-scene lifecycle onto the new first scene
after deletion/prepending, but does not rewrite its `applied` definitions. A full
list reset clears it. Unchanged definitions continue to skip; changed definitions
still refuse unsafe rebasing over manual edits.

Example:

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
                    "options": { "Merge": "--preset 3 --crf 23" },
                    "photon_noise": { "Merge": { "iso": 400 } }
                }
            }
        }
    ]
}
```
