# Photon Noise

Film Grain Synthesis tables generated from an ISO strength. Used by `--photon-noise` and `--chroma-noise` (see [init]\(../commands/init.md\), [encode]\(../commands/encode.md\), [pipeline](../commands/condor.md)). Scaled per scene by [scale-noise]\(../commands/scale-noise.md\) after [detect-noise]\(../commands/detect-noise.md\).

## Struct

| Field        | Type    | Required | Description                                              |
| ------------ | ------- | -------- | -------------------------------------------------------- |
| `iso`        | Integer | Yes      | Luma grain ISO strength (`--photon-noise`)               |
| `chroma_iso` | Integer | No       | Chroma grain ISO strength (`--chroma-noise`, defaults to `iso`) |
| `width`      | Integer | No       | Grain table width                                        |
| `height`     | Integer | No       | Grain table height                                       |
| `c_y`        | List    | No       | Custom luma AR coefficients; determines lag              |
| `ccb`        | List    | No       | Custom chroma-blue AR coefficients                       |
| `ccr`        | List    | No       | Custom chroma-red AR coefficients                        |

The `fgs` [filter](./filters.md) exposes the same struct plus `dynamic_seed` (randomize grain seed per frame).

## Coefficient shapes

Effective coefficient lengths must match exactly:

| Lag | `c_y` (`N`) | `ccb` and `ccr` (each `N+1`) |
| --- | ----------- | -------------------------- |
| 0   | 0           | 1                          |
| 1   | 4           | 5                          |
| 2   | 12          | 13                         |
| 3   | 24          | 25                         |

Omitted or `null` coefficients inherit generator defaults: `c_y=[]`,
`ccb=[0]`, `ccr=[0]` (lag 0). Explicit `[]` supplies zero coefficients:
valid for luma lag 0, invalid for either chroma plane. Partial overrides are
allowed only when all effective lengths match. Changing luma to a nonzero lag
requires supplying both chroma vectors at the matching length. Coefficients
are never padded or truncated.

These rules cover generated parameters with luma scaling points and the
supported text-table format, which requires `N+1` chroma slots even when
chroma is inactive or output is monochrome. Arbitrary segments without luma
scaling points are unsupported. Native AV1 chroma AR signaling is conditional:
only active planes (including chroma-from-luma) signal coefficients, and
monochrome suppresses chroma fields.

## Compatibility

Only `aom`, `svt-av1`, and `rav1e` encoders (see [encoder](./encoder.md)). Do not combine with internal encoder grain such as `--film-grain` in `svt-av1`.

The supported `avm` CLI has a native `--film-grain-table` option, but requires
`filmgrn2` tables with 13 `p` values. Condor generates AV1 `filmgrn1` tables
with 12 `p` values, so application Photon Noise settings are rejected for AVM.
Legacy AVM `photon_noise` values remain stored and readable; clear them with
`av1an encode reset photon-noise` (or set the field to `null`), or switch
encoders. Caller-supplied native tables remain available through
`--params "--film-grain-table=path/to/native-filmgrn2.tbl"`; Condor does not
convert or validate their format.

## Defaults and Guidance

- Minimum ISO `200` recommended for reducing gradient banding.
- [scale-noise]\(../commands/scale-noise.md\) recommended threshold: `0.002`. Scenes below threshold keep unscaled ISO.

## Examples

- `> av1an encode --photon-noise 800`
- `> av1an encode --photon-noise 800 --chroma-noise 400`
- `> av1an scale-noise --threshold 0.002 --minimum-scaler 0.5 --maximum-scaler 2.0 --scale-chroma`
