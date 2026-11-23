# condor.scenes

`condor.scenes` in `condor.json`. See [condor](./index.md). Written by [`detect-scenes`](../../commands/detect-scenes.md), updated per scene by [`target-quality`](../../commands/target-quality.md), [`encode`](../../commands/encode.md), [`detect-noise`](../../commands/detect-noise.md), [`scale-noise`](../../commands/scale-noise.md), [`quality-check`](../../commands/quality-check.md). Read, edited, added to and reset without re-detecting with [`scenes`](../../commands/scenes.md).

Array of scene objects. `[]` on `init`.

| Field | Type | Required | Description |
| ----- | ---- | -------- | ----------- |
| `start_frame` | Integer | Yes | Inclusive start frame |
| `end_frame` | Integer | Yes | Exclusive end frame |
| `sub_scenes` | Array or null | Yes | Optional splits from [`optimize-bitrate`](../../commands/optimize-bitrate.md). Each item: `{ "start_frame": <int>, "end_frame": <int> }` |
| `encoder` | Object | Yes | Per-scene encoder override. Same shape as [condor.encoder](./encoder.md). Cloned from global encoder on detection; per-scene quantizer/speed/noise edits apply here |
| `sequence_data` | Object | Yes | Per-scene step results |

`sequence_data` fields:

| Field | Type | Required | Description |
| ----- | ---- | -------- | ----------- |
| `scene_detection` | Object | Yes | `{ "scenecut_scores": { "<frame>": {...} } or null, "transnetv2_scores": { "<frame>": <f32> } or null, "created_on": <SystemTime> }` |
| `noise_detection` | Object or null | Yes | `{ "noise": <f64>, "luminance": <f64>, "created_on": <SystemTime> }` |
| `noise_scaling` | Object or null | Yes | `{ "scaler": <f64> }` |
| `parallel_encoder` | Object | Yes | Encoding timestamps (`started_on`, `completed_on`, milliseconds since Unix epoch), chunk `bytes`, provenance, and an optional `verification` level. `frame_count` confirms decoded frame count; `nonempty` confirms a nonempty regular file only. |
| `target_quality` | Object | Yes | `{ "passes": [ { "quantizer": <f64>, "scores": [<f64>], "bitrate": <f64>, "started_on": <u128 ms epoch>, "completed_on": <u128 ms epoch> } ] }` |
| `quality_check` | Object | Yes | `{ "quality": { "quantizer": <f64>, "scores": [<f64>], "bitrate": <f64>, "started_on": <u128 ms epoch>, "completed_on": <u128 ms epoch> } }` |
| `zoner` | Object | No | `{ "zoned": <bool> }`. Absent in configurations written before zoning became idempotent; defaults to `false`. Set to `true` when the Zoner has applied the zones to the list; `av1an scenes reset zoner` clears it |

Example (two scenes, fresh detection):

```json
[
    {
        "start_frame": 0,
        "end_frame": 121,
        "sub_scenes": null,
        "encoder": {
            "SVTAV1": {
                "executable": null,
                "ffmpeg_filter": null,
                "pass": { "All": 1 },
                "options": {
                    "preset": { "Number": { "prefix": "--", "delimiter": " ", "value": 4.0 } },
                    "crf": { "Number": { "prefix": "--", "delimiter": " ", "value": 25.0 } }
                },
                "photon_noise": null
            }
        },
        "sequence_data": {
            "scene_detection": { "scenecut_scores": null, "transnetv2_scores": null, "created_on": { "secs_since_epoch": 0, "nanos_since_epoch": 0 } },
            "noise_detection": null,
            "noise_scaling": null,
            "parallel_encoder": { "started_on": null, "completed_on": null, "bytes": null },
            "target_quality": { "passes": [] },
            "quality_check": { "quality": { "quantizer": 0.0, "scores": [], "bitrate": 0.0, "started_on": 0, "completed_on": 0 } },
            "zoner": { "zoned": false }
        }
    },
    {
        "start_frame": 121,
        "end_frame": 240,
        "sub_scenes": null,
        "encoder": {
            "SVTAV1": {
                "executable": null,
                "ffmpeg_filter": null,
                "pass": { "All": 1 },
                "options": {
                    "preset": { "Number": { "prefix": "--", "delimiter": " ", "value": 4.0 } },
                    "crf": { "Number": { "prefix": "--", "delimiter": " ", "value": 25.0 } }
                },
                "photon_noise": null
            }
        },
        "sequence_data": {
            "scene_detection": { "scenecut_scores": null, "transnetv2_scores": null, "created_on": { "secs_since_epoch": 0, "nanos_since_epoch": 0 } },
            "noise_detection": null,
            "noise_scaling": null,
            "parallel_encoder": { "started_on": null, "completed_on": null, "bytes": null },
            "target_quality": { "passes": [] },
            "quality_check": { "quality": { "quantizer": 0.0, "scores": [], "bitrate": 0.0, "started_on": 0, "completed_on": 0 } },
            "zoner": { "zoned": false }
        }
    }
]
```

Do not hand-edit `sequence_data` timestamps or scores; rerun the owning command instead, or clear a scene's saved results with `av1an scenes <INDEX> reset <KEY>` (see [scenes](../../commands/scenes.md)).
