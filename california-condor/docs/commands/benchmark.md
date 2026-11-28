# Benchmark

Benchmark the optimum amount of workers (triggers TUI). See [guide](../guide.md).

```bash
$ av1an benchmark
$ av1an benchmark --threshold 5
```

| Name                                           | Flag           | Type    | Default                |
| ---------------------------------------------- | -------------- | ------- | ---------------------- |
| [Threshold](#threshold---threshold)            | `--threshold`  | Integer | Saved value, else `5`  |
| [Maximum Memory](#maximum-memory---max-memory) | `--max-memory` | Integer | Saved value, else none |

Each trial's row shows its worker count, FPS, whether it met the threshold, its
peak CPU and encoder memory under `Peak Usage`, and its frame count. `Peak Usage`
reads `-` when the platform cannot sample the encoder's resources.

## Threshold `--threshold`

The minimum speed increase (in percent) required to add an additional worker.

### Possible Values

Integer from `0` to `100`.

### Default

If not specified, the saved value is used; a new job starts at `5`.

### Examples

- `> av1an benchmark --threshold 5`

## Maximum Memory `--max-memory`

The most RAM, in megabytes, the encoder processes may use together during one
trial.

### Possible Values

Integer megabytes. `0` allows no memory, so any measured peak exceeds it.

### Default

If not specified, the saved value is used; a new job has no limit.

### Behaviour

Each completed trial reports the peak memory of its encoder processes. That peak
is checked before the trial's speed increase is compared with the threshold, so a
trial over the limit is never adopted: it is reported as failed, naming the peak
and the limit, and the benchmark stops.

The last worker count that fit is kept: the trial before the failing one, which
for the one-worker baseline is that single worker. Because the baseline already
uses the smallest configuration, a limit no single encoder can meet ends the
benchmark at one worker instead of trying more.

Peaks are measured when a trial finishes, so a trial that is still running can
exceed the limit before the benchmark notices. Only the encoder process counts;
Av1an's own memory and the decoder do not. When encoder usage cannot be sampled,
no peak is recorded and the limit does not trigger.

### Examples

- `> av1an benchmark --max-memory 8192`

## Edit saved settings

`info`, `set` and `reset` read, change and reset the saved benchmark settings without running the command:

- `set threshold PERCENT` — speed increase required to add another worker (`0..=100`).
- `set max-memory MB` — RAM allowed across all workers; `reset max-memory` clears it.
- `info [threshold|max-memory]` prints JSON; `reset KEY` resets one of them, and a bare `reset` restores the whole section to its defaults.

```bash
$ av1an benchmark set threshold 8
$ av1an benchmark info threshold
$ av1an benchmark reset max-memory
```

Shared semantics (validation, staleness warnings): see [Editing saved settings](config-editing.md).

## Screenshots

The screenshots below are generated from the live interface. Click any image to open it full size.

<table>
  <tr>
    <td width="50%" valign="top">
      <a href="../media/tui/benchmarker-light.avif"><picture><source srcset="../media/tui/benchmarker-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/benchmarker-light.avif" alt="Benchmarking workers" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">Benchmarking workers</p>
    </td>
    <td width="50%" valign="top">
      <a href="../media/tui/help-benchmark-light.avif"><picture><source srcset="../media/tui/help-benchmark-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-benchmark-light.avif" alt="av1an benchmark --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an benchmark --help</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="../media/tui/help-benchmark-verbose-light.avif"><picture><source srcset="../media/tui/help-benchmark-verbose-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-benchmark-verbose-light.avif" alt="av1an benchmark --help --verbose" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an benchmark --help --verbose</p>
    </td>
    <td width="50%" valign="top">
      <a href="../media/tui/help-benchmark-set-light.avif"><picture><source srcset="../media/tui/help-benchmark-set-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-benchmark-set-light.avif" alt="av1an benchmark set --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an benchmark set --help</p>
    </td>
  </tr>
</table>
