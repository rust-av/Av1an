# VapourSynth Arguments

`key=value` pairs injected into the VapourSynth script environment. Used by `--vs-args`, `--scd-vs-args`, `--tq-vs-args` (see [init]\(../commands/init.md\), [encode]\(../commands/encode.md\), [pipeline](../commands/condor.md), [detect-scenes]\(../commands/detect-scenes.md\), [detect-noise]\(../commands/detect-noise.md\), [target-quality]\(../commands/target-quality.md\), [quality-check]\(../commands/quality-check.md\)).

## Syntax

Repeat the flag for multiple values:

```bash
$ av1an encode --vs-args "message=fluffy kittens" --vs-args "head=empty"
$ av1an encode --vs-args "denoiser=primary" --vs-args "downscaler=bicubic"
```

## Stage Overrides

| Flag           | Environment      | Falls Back To |
| -------------- | ---------------- | ------------- |
| `--vs-args`    | Encode input     | —             |
| `--scd-vs-args` | Scene detection | `--vs-args` is separate; unset by default |
| `--tq-vs-args`  | Target quality  | `--vs-args` is separate; unset by default |

## Examples

- `> av1an init script.vpy output.mkv --vs-args "message=fluffy kittens" --vs-args "head=empty"`
- `> av1an detect-scenes --vs-args "downscaler=bicubic"`
