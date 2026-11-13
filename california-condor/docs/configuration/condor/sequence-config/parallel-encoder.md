# sequence_config.parallel_encoder

`sequence_config.parallel_encoder` in `condor.json`. See [sequence_config](../sequence_config.md). Updated by [`encode`](../../../commands/encode.md) and [`av1an`](../../../commands/condor.md).

| Field | Type | Required | Default | Description |
| ----- | ---- | -------- | ------- | ----------- |
| `workers` | Integer (u8) or null | Yes | `null` | Worker count. `null` means Benchmarker decides. CLI `-w`/`--workers` |
| `max_tries` | Integer (u32, ≥1) | No | `3` | Total attempts per scene including the initial attempt. CLI `--max-tries`. |
| `scenes_directory` | String (path) | Yes | `<temp>/scenes` | Directory for per-scene encodes |
| `input` | Input object or null | Yes | `null` | Override input. Same shape as [condor.input](../input.md) |
| `validate_frame_count` | Boolean | No | `true` | Decode-count completed chunks with `ffprobe` before publishing or resuming. Set `false` only for filters intentionally changing the frame count. |

Encode-time `--filters` edits this `input` when set, otherwise `condor.input`.
Rebuilding an override inherits filters from the previous effective input unless
explicitly replaced. With an override, these edits preserve the main reference;
separate metric-reference inputs are not edited. See [routing and migration](../../../commands/encode.md#filters---filters).

Count validation requires `ffprobe` on `PATH` with a decoder for the output codec.
The saved policy also applies to automatic worker discovery and Target Quality
individual/zone outputs. Strict stages check for `ffprobe` before encoding;
opting out removes only that counter prerequisite, not metric decoder requirements.
It counts decoded frames in the actual container, not progress or header counts;
decode errors, unavailable decoders, and missing/malformed counts fail closed.
Setting `validate_frame_count` to `false` disables decode-count verification only:
successful encoder exit and a nonempty regular output file are still required.
With this opt-out, corrupt but nonempty output cannot be decode-verified.

Resume requires a saved completed encoding record and a currently valid chunk.
Bare files are not adopted. Ineligible files are preserved. Missing, empty or
nonregular chunks lose their unusable completion metadata; count/probe/read
failures retain historical records but withhold current eligibility. Repair the
probe or intentional count policy and retry the same job; the historical record
alone never authorizes resume. Otherwise recompute in a new job, or move the
chunk aside and clear its saved completion record. Count validation does not
establish that a chunk matches changed source media or encoder settings.

Strict resume fully decodes each completed chunk again, sequentially, before new
encoding; large jobs can spend substantial time validating. The UI reports
validation progress separately from encoded frames. Graceful cancellation lets
the active check finish, then stops before another check or new encode; Force
terminates the active probe.

Frame buffering is chosen from the input:

- Video inputs and BestSource, FFMS2, or L-SMASH VapourSynth imports stream frames to each worker as it encodes, keeping a few frames in memory per worker.
- DGDecNV imports and nonstreaming VapourSynth scripts wait for a free worker before decoding each scene. Queued scenes are not decoded; an admitted scene can still buffer its frames.

With `max_tries > 1`, nonstreaming scenes first capture one anonymous uncompressed
Y4M disk spool under the scenes directory, then replay it in bounded chunks.
Allow disk space for one full scene per active worker, plus any multipass spool.
`max_tries = 1` keeps direct streaming. Concurrent inputs reread the scene instead.
Each attempt starts all passes with fresh owned output/statistics and filter state;
only validated successful output is published and saved. Explicit native statistics
path overrides remain caller-owned and cannot be cleaned automatically.

Only native encoder/filter failures and failed new-output validation retry. Invalid
settings, source/spool errors, publication/save failures and worker panics stop the
scene. Cancellation prevents replacement attempts; Graceful lets active passes
finish, while Force interrupts them. The last error retains scene/attempt context.
The saved budget also covers individual Target Quality probes and worker discovery,
not whole-zone batches or serial stages.

Example:

```json
{
    "workers": null,
    "max_tries": 3,
    "scenes_directory": "./a1b2c3d4/scenes",
    "input": null,
    "validate_frame_count": true
}
```
