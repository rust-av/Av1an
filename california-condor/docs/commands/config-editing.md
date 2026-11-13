# Editing saved settings

The step commands double as configuration editors: alongside their run flags they
expose `info`, `set` and `reset` subcommands that read, change and restore their
saved section in `condor.json` **without running** the step. Run flags and edit
subcommands do not mix — passing both is a usage error, so a setting can never
change by accident.

`--config-file`, `--logs` and `--verbose` are genuine globals and work before or
after an edit command. `--temp` is a run-only option on supported routes, not an
edit option: remove it for `info`, `set` and `reset`.

## Shape

```text
av1an <command> info [KEY]        # pretty JSON of the whole section, or one key
av1an <command> set <PROPERTY> …  # typed property plus its own values and flags
av1an <command> reset [KEY]       # restore one key, or the whole section
```

Every level of `--help` lists exactly what can be changed and how:

```bash
$ av1an detect-scenes set --help            # the properties
$ av1an detect-scenes set method --help     # that property's values and flags
```

## Semantics

| Operation | Behaviour |
| ---- | ---- |
| `info` | Prints the section (or one key) as pretty JSON with the same shape as `condor.json`. Read-only; the file is never touched. |
| `set` | Creates the section when it is optional and currently `null`, applies the typed edit, validates it, saves, then names the property it changed. Values you omit keep their current value. |
| `reset KEY` | Restores one key to its model default, or clears an optional field, and names it. |
| `reset` | With no key, restores the whole section: an optional section goes back to `null`, a required one back to its defaults. `av1an init reset` needs a property, because the input and output paths it captures have no default. |
| Validation | Invariants (`min ≤ max` quantizers, positive probes, percentile `0..=100`, …) are checked **before** saving; a violation leaves the file untouched and exits non-zero. |
| Staleness | After any write a `WARN` states that saved results/files may be stale. Nothing is cleaned or recomputed. `av1an clean` is unavailable; [start a separate job](clean.md#start-a-separate-job) with a different `--config-file`, a distinct empty `--temp` directory, and a distinct output. |
| Config path | All operations honour `--config-file`, exactly like the run commands. |
| `--verbose` | Every write prints a status line naming the property and what happened to it; the value it was left with is printed only under `--verbose`. |

### Values vs flags

A value that is always required is a positional argument, not a flag; flags are
reserved for things that are genuinely optional or that select a sub-kind.
Repeating values (filter chains, speed–quantizer pairs) are positionals too.

Switches are written as a value, so a setting can be turned in **both**
directions without a second, negating flag:

```bash
$ av1an scale-noise set scale-chroma off
$ av1an detect-scenes set method transnetv2 --save-scores off
```

A switch whose value may be omitted means `on` (`set scale-chroma`), and
`--save-scores` on its own means `on`.

### Reports from other commands

An option that the command it is given to cannot honour is reported rather than
dropped: for example `--vs-args` only reaches a `.vpy`/`.py` input, so every
`set input` (and `init`) that receives it together with a video input logs a
`WARN` saying it was ignored.

## What each command edits

| Command | Section | `set` properties | Optional |
| ---- | ---- | ---- | ---- |
| [init](init.md) | top-level `condor` | `input`, `output`, `encoder`, `workers`, `concatenator`, `target-metric`, `target` | no |
| [detect-scenes](detect-scenes.md) | `scene_detector` | `method`, `input` | no |
| [detect-noise](detect-noise.md) | `noise_detector` | `input`, `reference-filters`, `denoised-filters` | yes |
| [scale-noise](scale-noise.md) | `noise_scaler` | `threshold`, `minimum-scaler`, `maximum-scaler`, `scale-chroma` | yes |
| [benchmark](benchmark.md) | `benchmarker` | `threshold`, `max-memory` | no |
| [target-quality](target-quality.md) | `target_quality` | `metric`, `target`, `maximum-probes`, `quantizer-range`, `interpolators`, `input`, `metric-input`, `strategy`, `statistic`, `params` | yes |
| [optimize-bitrate](optimize-bitrate.md) | `bitrate_optimizer` | `sigma-threshold` | no |
| [scale-speed](scale-speed.md) | `speed_scaler` | `speed-quantizers` (alias `speed-scalers`) | no |
| [encode](encode.md) | `parallel_encoder`, `encoder` | `workers`, `input`, `scenes-directory`, `encoder`, `executable`, `passes`, `params`, `photon-noise`, `ffmpeg-filter` | no |
| [concatenate](concatenate.md) | `scene_concatenator` | `method`, `output`, `scenes-directory` | no |
| [quality-check](quality-check.md) | `quality_check` | `metric`, `strategy`, `statistic`, `input` | yes |

`zone` has no editor: `av1an zone` / `--zones` replaces the whole `zoner`
section in one go, which is the only shape a zone list takes.

[scenes](scenes.md) edits the saved scene list rather than a step's section, so
its index comes first: `av1an scenes 1 info` addresses one scene, `0-4` a
range, `1-2,8-10,16` a list, and `-1`/`all`/`every` the whole list. `set`,
`reset` and `delete` change the addressed scenes in place, `add` inserts a new
one, and a keyless `reset` clears the list. Reads and writes use the same
`info`/`set`/`reset` shape described here, with the scene list's own keys
(`frames`, `encoder`, `sub-scenes`) and its sequence-data keys
(`scene-detection`, `noise-detection`, `noise-scaling`, `parallel-encoder`,
`target-quality`, `quality-check`).

## Shared properties

The probing strategy and statistic are the same setting in `target-quality` and
`quality-check`, so both take the same values:

```bash
$ av1an quality-check set strategy whole
$ av1an quality-check set strategy skip 12
$ av1an target-quality set strategy subset --position end --length-percent 25
$ av1an quality-check set statistic percentile 10
$ av1an target-quality set statistic standard-deviation-distance 1.5
```

Each `set input` property builds its input from the same flags as the owning
command's run flags — `--path`, `--decoder`, `--filters`, `--vs-args` and
`--stream-concurrently [on|off]` — and refuses an edit that sets none of them.
When a supplied path does not exist yet on disk, an otherwise valid edit succeeds
with a `WARN` noting that the file will be needed when the step runs.

The root `init set input --path` is stricter: a different normalized source path
requires no saved scenes and empty known encoding/concatenation work directories.
Otherwise use a new `--config-file`; no chunks are silently deleted. Equivalent
paths keep work and caches. An empty-job replacement is offline and atomically
updates root metadata, resets root cache/index choices, and rebases derived scenes
directories (unset or equal to the old `TEMP/scenes`); explicit custom scene directories
stay. Auto-hashed temp names change under the same parent using the new lexical
absolute input path, matching fresh init; custom temp roots stay. A same-path VS
decoder-kind change clears its cache selection; same-kind edits retain it.
Stage-specific inputs (including `encode` and quality-reference inputs) remain
separate, editable overrides and are not rewritten by a root edit. See [Init](init.md#input).

### Changing the temporary directory

On run commands, `--temp PATH` changes an empty job's temp root and its derived
encoding/concatenation directories (unset or the old `TEMP/scenes`). Target Quality
probes follow the encoding scenes directory. No files are moved, copied or deleted;
editing paths alone need not create directories.

Saved scenes, including detection-only results, or a nonempty known encoding or
concatenation directory prevent a root change before it is saved. Start a new job
with a new `--config-file` instead. A nonempty new default `PATH/scenes` is also
refused when it would be adopted, so foreign chunks cannot silently become job work.
An equivalent temp path, including a symlink alias, keeps the saved paths and results
and is accepted even for a populated job. Omitting `--temp` changes nothing.

Explicit custom scenes directories are preserved with a warning naming each path:
`--temp` does not move those files. This includes older configurations whose scenes
directories no longer match their saved temp root; their origin cannot be inferred.
To change these paths intentionally, use `av1an encode set scenes-directory PATH`
and `av1an concatenate set scenes-directory PATH`. These settings do not migrate
content either.

## Examples

```bash
# read what is saved
$ av1an target-quality info
$ av1an target-quality info quantizer-range

# change one property; omitted values keep their current value
$ av1an detect-scenes set method avscenechange --minimum-frames 20 --maximum-frames 280
$ av1an scale-noise set threshold 0.002
$ av1an scale-speed set speed-quantizers 20=6 35=4 55=2
$ av1an target-quality set quantizer-range 18-30
$ av1an target-quality set interpolators fritsch-carlson pchip
$ av1an target-quality set target 85
$ av1an quality-check set strategy subset --position end --length-percent 25
$ av1an detect-noise set reference-filters "resize:width=960;" "wnnm:sigma=3.0,0.0,0.0;"
$ av1an detect-noise set reference-filters none
$ av1an init set output final.mkv

# restore defaults, or drop an optional section entirely
$ av1an detect-scenes reset method
$ av1an detect-noise reset reference-filters
$ av1an detect-noise reset
```

Every write says what it did. The status line names the property and the verb,
which is enough to say what changed; the value it was left with is printed only
under `--verbose`:

```console
$ av1an benchmark set threshold 8
WARN Benchmark settings changed; saved results/files may be stale. Nothing was cleaned or recomputed.
Benchmark settings threshold updated

$ av1an --verbose benchmark set threshold 8
WARN Benchmark settings changed; saved results/files may be stale. Nothing was cleaned or recomputed.
Benchmark settings threshold updated
8

$ av1an detect-noise reset reference-filters
WARN Noise detection settings changed; saved results/files may be stale. Nothing was cleaned or recomputed.
Noise detection settings reference-filters reset to its default
To clear every reference filter instead, run `av1an detect-noise set reference-filters none`.
```

Invalid input is rejected before anything is written:

```bash
$ av1an target-quality set quantizer-range 30-10     # rejected: min > max
$ av1an detect-scenes set method none --minimum-frames 0   # rejected: not positive
$ av1an quality-check set statistic percentile --percentile 150  # rejected: out of range
```

## Screenshots

The property list of `set` is the discovery surface — one screenshot per command:

<table>
  <tr>
    <td width="50%" valign="top">
      <a href="../media/tui/help-detect-scenes-set-light.avif"><picture><source srcset="../media/tui/help-detect-scenes-set-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-detect-scenes-set-light.avif" alt="av1an detect-scenes set --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an detect-scenes set --help</p>
    </td>
    <td width="50%" valign="top">
      <a href="../media/tui/help-target-quality-set-light.avif"><picture><source srcset="../media/tui/help-target-quality-set-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-target-quality-set-light.avif" alt="av1an target-quality set --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an target-quality set --help</p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <a href="../media/tui/help-quality-check-set-light.avif"><picture><source srcset="../media/tui/help-quality-check-set-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-quality-check-set-light.avif" alt="av1an quality-check set --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an quality-check set --help</p>
    </td>
    <td width="50%" valign="top">
      <a href="../media/tui/help-scale-speed-set-light.avif"><picture><source srcset="../media/tui/help-scale-speed-set-dark.avif" media="(prefers-color-scheme: dark)"><img src="../media/tui/help-scale-speed-set-light.avif" alt="av1an scale-speed set --help" style="width:100%;border-radius:8px;display:block;"></picture></a>
      <p style="text-align:center;font-size:0.85em;opacity:0.75;margin:0.35em 0 0;">av1an scale-speed set --help</p>
    </td>
  </tr>
</table>
