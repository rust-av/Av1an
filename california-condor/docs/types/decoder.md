# Decoder

`DECODER` selects how `av1an` decodes input video. Used by `--decoder`, `--scd-decoder`, `--tq-decoder` (see [pipeline](../commands/condor.md), [init]\(../commands/init.md\), [encode]\(../commands/encode.md\), [detect-scenes]\(../commands/detect-scenes.md\), [detect-noise]\(../commands/detect-noise.md\), [target-quality]\(../commands/target-quality.md\), [quality-check]\(../commands/quality-check.md\)).

| Value        | Source                              | Notes                                              |
| ------------ | ----------------------------------- | -------------------------------------------------- |
| `bestsource` | BestSource VapourSynth plugin       | Default. Requires external VapourSynth plugin.     |
| `vs-ffms2`   | FFmpegSource VapourSynth plugin     | Requires external VapourSynth plugin.              |
| `lsmash`     | L-SMASH-Works VapourSynth plugin    | Requires external VapourSynth plugin.              |
| `dgdecnv`    | DGDecodeNV VapourSynth plugin       | Requires external VapourSynth plugin (NVIDIA GPU). |
| `ffms2`      | FFmpegSource (integrated)           | No external plugin required.                       |

## Default

If not specified, `bestsource` is used. Stage overrides (`--scd-decoder`, `--tq-decoder`) fall back to `--decoder`.

## Examples

- `> av1an -i input.mp4 -o output.mkv --decoder vs-ffms2`
- `> av1an detect-scenes --decoder ffms2`
