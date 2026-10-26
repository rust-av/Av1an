# Encoder

`ENCODER` selects the encoder binary. Used by `--encoder` (see [init]\(../commands/init.md\), [encode]\(../commands/encode.md\), [pipeline](../commands/condor.md)).

| Value     | Binary          | Default Passes | Default TQ Quantizer Range | Photon Noise |
| --------- | --------------- | -------------- | -------------------------- | ------------ |
| `aom`     | aomenc          | `2`            | `5`-`55`                   | Yes          |
| `rav1e`   | rav1e           | `1`            | `50`-`140`                 | Yes          |
| `vpx`     | vpxenc          | `2`            | `5`-`55`                   | No           |
| `svt-av1` | SvtAv1EncApp    | `1`            | `5`-`55`                   | Yes          |
| `avm`     | avmenc          | `1`            | `5`-`250`                  | Yes          |
| `x264`    | x264            | `1`            | `5`-`35`                   | No           |
| `x265`    | x265            | `1`            | `5`-`35`                   | No           |
| `vvenc`   | vvenc           | `1`            | `5`-`35`                   | No           |
| `ffmpeg`  | ffmpeg          | `1`            | `15`-`50`                  | No           |

## Default

If not specified, `svt-av1` is used.

## Default Parameters

Each encoder ships with Condor defaults (merged with `--params`):

| Encoder | Defaults                                                                 |
| ------- | ------------------------------------------------------------------------ |
| `aom`   | `--threads=8 --cpu-used=6 --cq-level=30 --kf-max-dist=9999 --end-usage=q` |
| `rav1e` | `--speed 8 --quantizer 100 --keyint 0 --no-scene-detection -y`            |
| `vpx`   | `--profile=2 --threads=4 --cpu-used=2 --cq-level=30 --codec=vp9 --end-usage=q` and more |
| `svt-av1` | `--preset 4 --keyint 0 --scd 0 --rc 0 --crf 25 --progress 2`            |
| `avm`   | `--threads=8 --cpu-used=6 --qp=30 --kf-max-dist=9999 --end-usage=q`       |
| `x264`  | `--crf 25 --scenecut 0 --preset slow --keyint infinite --stitchable`      |
| `x265`  | `--crf 25 --level-idc 5 --keyint -1 --scenecut 0 -D 10 --y4m --preset slow` |
| `vvenc` | none                                                                     |
| `ffmpeg` | none                                                                    |

Target Quality (`--tq-params`) additionally strips psychovisual parameters (e.g. film-grain tables) for metric accuracy.

## Colour Metadata

Every encoder carries an `append_color_data` boolean that defaults to `true`.
When set, the input's colour and HDR metadata — primaries, transfer
characteristics, matrix coefficients, chroma sample location, range, mastering
display and content light level — is written into the encoder's parameters
after the pass parameters and before `options`, so a matching user parameter
overrides it. Set it to `false` to leave the command line untouched. See
[`condor.encoder`'s Colour Metadata](../configuration/condor/encoder.md#colour-metadata)
for the per-encoder flags.

## Photon Noise

`--photon-noise` / `--chroma-noise` (Film Grain Synthesis tables) are only compatible with `aom`, `svt-av1`, `rav1e`, and `avm`. Do not combine with internal encoder grain (e.g. `--film-grain` in `svt-av1`). Minimum ISO `200` recommended against banding.

## Examples

- `> condor encode -e rav1e --params "--speed 4"`
- `> condor -i input.mp4 -o output.mkv -e svt-av1 --passes 1`
