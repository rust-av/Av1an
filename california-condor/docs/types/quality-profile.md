# Quality Profile

`PROFILE` selects the probe strategy and statistic for Target Quality and Quality Check. Used by `--profile`, `--target-profile` (see [target-quality]\(../commands/target-quality.md\), [quality-check]\(../commands/quality-check.md\), [pipeline](../commands/condor.md)).

| Value      | Strategy                                              |
| ---------- | ----------------------------------------------------- |
| `fast`     | Measures the average of the middle 11 frames          |
| `standard` | Measures the root-mean-square of the middle 25%       |
| `slow`     | Measures the 10th percentile of all frames            |

Standard rounds 25% of the scene length to a frame count, with at least one frame.
For a 100-frame scene starting at frame 0, it selects frames 38 through 62 inclusive.

## Default

If not specified, `standard` is used.

## Examples

- `> av1an target-quality --profile fast --metric ssimulacra2 --target 85`
- `> av1an quality-check --profile slow`
