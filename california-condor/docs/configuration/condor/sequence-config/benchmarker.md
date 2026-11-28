# sequence_config.benchmarker

`sequence_config.benchmarker` in `condor.json`. See [sequence_config](../sequence_config.md). Updated by [`benchmark`](../../../commands/benchmark.md).

| Field | Type | Required | Default | Description |
| ----- | ---- | -------- | ------- | ----------- |
| `threshold` | Integer (u8) | Yes | `5` | Percent of speed increase required to add an additional worker. CLI `--threshold` (0–100) |
| `max_memory` | Integer (u32) or null | Yes | `null` | Peak encoder memory in megabytes allowed across all workers; `null` disables the limit. CLI `--max-memory` |

`max_memory` is checked against a trial's peak encoder memory before its
speed increase is compared with `threshold`. A trial over the limit is reported
as failed, the benchmark stops, and the previous worker count - the last one that
fit - is kept. A one-worker baseline over the limit therefore ends at one worker.
Peaks are measured when a trial finishes, and only the encoder process is
counted.

Example:

```json
{
    "threshold": 5,
    "max_memory": null
}
```
