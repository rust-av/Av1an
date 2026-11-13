# Quality Metric

`METRIC` selects the perceptual quality metric for Target Quality and Quality Check. Used by `--target-metric`, `--metric` (see [init]\(../commands/init.md\), [target-quality]\(../commands/target-quality.md\), [quality-check]\(../commands/quality-check.md\), [pipeline](../commands/condor.md)).

| Value           | Metric                                                        | Plugin Requirement                                  |
| --------------- | ------------------------------------------------------------- | --------------------------------------------------- |
| `ssimulacra2`   | Structural SIMilarity Unveiling Local And Compression Artifacts | Vship (GPU, recommended) or vszip (CPU)            |
| `butteraugli`   | butteraugli Infinite-Norm                                     | Vship (GPU, recommended) or julek plugin (CPU)      |
| `butteraugli-3` | butteraugli 3-Norm                                            | Vship (GPU, recommended) or julek plugin (CPU)      |
| `xpsnr`         | Extended Perceptually Weighted PSNR (min of Y, U, V)          | vszip (CPU)                                         |
| `cvvdp`         | ColorVideoVDP                                                 | Vship (GPU required)                                |
| `vmaf`          | Video Multi-Method Assessment Fusion                          | libvmaf (built with CUDA for GPU, optional)      |

## Default

If not specified, `ssimulacra2` is used.

## Temporal CVVDP scenes

With temporal CVVDP enabled (the default), Quality Check and Target Quality start
a fresh metric session for each scene. History and accumulated scores do not
carry between scenes; ordered cumulative per-frame values are retained within
each scene. Direct clip-wide scoring APIs still use one session per sequence.

Scene decisions, quantizer history, live scene scores and saved-data displays use
the **final ordered cumulative value**, regardless of the configured statistic.
Prefixes remain available for progress and charts; their mean/minimum/maximum
are not independent-frame quality statistics. The Quality Check report omits the
Per-Frame row in this mode. Its Per-Scene statistics describe separate final scene
results, not a clip-wide CVVDP score. Independent-frame CVVDP retains the configured
per-frame statistics and report row.

No saved-vector schema or provenance migration is performed. Applying this
reduction does not establish that legacy jobs used fresh scene sessions; their
resume compatibility remains a separate policy.

Source gaps *within* a scene retain their backend restrictions: native Vship can
clear history while retaining accumulation; fmetrics and the VapourSynth CVVDP
plugin cannot. Use independent frames (`disable_temporal=true`) or native Vship
for those selections. Independent-frame CVVDP and other metrics are unchanged.

## Examples

- `> av1an target-quality --metric ssimulacra2 --target 85`
- `> av1an quality-check --metric xpsnr --profile fast`
