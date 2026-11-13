# sequence_config.noise_scaler

`sequence_config.noise_scaler` in `condor.json`. See [sequence_config](../sequence_config.md). Created by [`scale-noise`](../../../commands/scale-noise.md) (`null` on `init`).

| Field | Type | Required | Default | Description |
| ----- | ---- | -------- | ------- | ----------- |
| `threshold` | Number (f64) | Yes | — | Minimum noise value for a scene to scale Photon Noise ISO. Recommended `0.002` |
| `minimum_scaler` | Number (f64) | Yes | — | Minimum scale factor for Photon Noise ISO scaling |
| `maximum_scaler` | Number (f64) | Yes | — | Maximum scale factor for Photon Noise ISO scaling |
| `scale_chroma` | Boolean | Yes | — | Whether to also scale chroma noise |

CLI mapping: `scale-noise --threshold <float> --minimum-scaler <float> --maximum-scaler <float> --scale-chroma`.

All three numeric fields must be finite and non-negative, with `minimum_scaler <= maximum_scaler`; calibration values have no arbitrary upper cap. A newly enabled section defaults to `0` for all three and `false` for `scale_chroma`.

When the maximum recorded noise across all scenes is at or below `threshold`, scaling does nothing: ISO, chroma and scaling metadata are preserved without saving. Missing measurements remain missing. With a greater maximum, scenes below the threshold are skipped; equality receives `minimum_scaler`, and the maximum receives `maximum_scaler`, interpolating linearly between them. ISO is rounded as an integer. Explicit zero factors are valid.

`scale_chroma` affects only an existing chroma ISO; `null` stays `null`. Only Photon Noise on AOM, rav1e and SVT-AV1 is scaled; SVT-AV1 with native `film-grain` is excluded, as are already-scaled scenes. Recorded noise must be finite and non-negative even on excluded scenes. Invalid data, non-finite calculations or a rounded ISO outside `u32` return an error before scene changes or a save. Lower the factors if the resulting ISO is too large. See [Scale Noise](../../../commands/scale-noise.md) for safe saved-setting repairs.

New successful scaling records include optional `baseline` in
`scenes[].sequence_data.noise_scaling`, with exact `original` and `scaled`
PhotonNoise plus a scoped `context` of declared source, detector/scaler settings,
range/sub-scenes, measurement and global maximum. This is not a new source-file
or binary attestation framework. Old factor-only JSON still parses; absence is
not proof of an invertible original ISO. Unchanged legacy peers are not certified
or rewritten by an unrelated scene edit.

Scene edits retain valid scaling and range-mapped measurements. If scaling must
be withdrawn, its original PhotonNoise is restored before publication without
resetting zone-specific encoder options. Missing proof or conflicting PhotonNoise
edits refuse and preserve the old job. `av1an scenes INDEX reset noise-scaling`
uses the same restoration before clearing metadata. Reset it before editing ISO,
chroma or calibration fields; never recover an original by dividing rounded ISO.
New producer records with changed dependencies refuse direct rescaling until
explicitly reset. See [scene recovery](../../../commands/scenes.md#invalidation-and-recovery)
for the distinction between an already-empty same-job workflow and a new CLI job.

Example:

```json
{
    "threshold": 0.002,
    "minimum_scaler": 0.5,
    "maximum_scaler": 2.0,
    "scale_chroma": true
}
```
