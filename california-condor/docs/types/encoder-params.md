# Encoder Parameters

Raw encoder CLI strings merged with Condor defaults. Used by `--params` and `--tq-params` (see [init]\(../commands/init.md\), [encode]\(../commands/encode.md\), [target-quality]\(../commands/target-quality.md\), [pipeline](../commands/condor.md)).

## Syntax

Pass one string using POSIX-style shell quoting **inside** the outer `--params`
quotes. Condor removes those inner quotes once and passes each value as one
encoder argument, including spaces or an empty string. No shell is invoked:
variables, globs, and command substitutions are not expanded by Condor.

```bash
av1an encode --params "--preset 2 --crf 24 --aq-mode 2"
av1an encode --params "--cpu-used=3 --cq-level=30 --tune=ssim"
av1an encode -e x265 --params "--no-sao --crf 20 --analysis-save 'a b.dat'"
```

- Options accept `--name value`, `--name=value`, single-dash options, and
  dashless `name=value` or `name value` pairs. Prefixes and `=` versus separate
  arguments are preserved. Only the first `=` separates a name from its value.
- Spaces, tabs, and newlines separate tokens. A following token starting with
  `-` starts another option unless it is a finite signed number, such as `-2`,
  `-0.5`, or `-1e-3`. Use `--name=-literal` for a nonnumeric dash-leading value,
  even if you would otherwise quote it. Numeric values are stored as numbers,
  so their spelling can change (for example, `-1e-3` becomes `-0.001`).
- Single quotes preserve backslashes literally, for example the inner string
  `--key 'C:\my clips\source.y4m'`. Double quotes support POSIX escapes such as
  `\"`; an unquoted backslash escapes the next character. This inner syntax is
  not Windows cmd.exe or PowerShell quoting; your outer shell must deliver it
  intact. An unquoted `#` at the start of a token begins a comment to end of line.
- Use `--name ""` or `--name=` for an empty value. Unmatched quotes, dangling
  escapes, empty/whitespace/control-character option names, and NUL bytes are
  errors; malformed tails never produce a partial successful parse.
- Each option name must be unique within the string, regardless of prefix.
  Duplicates are errors. Option order is not guaranteed: arbitrary positional
  arguments, ordered repeated options, and the standalone `--` end-of-options
  marker are not supported. A bare `-` is not an option name either.

## Merge Rules

- `--params` merges over per-[encoder](./encoder.md) defaults listed in [encoder](./encoder.md#default-parameters).
- `--tq-params` is used for Target Quality probes only; if omitted, `--params` is used. Psychovisual parameters are stripped for metric accuracy.
- A later configuration source can still override an earlier value by name.
- Use the selected encoder's `--help` for valid options (e.g. the x264 binary
  takes `--crf`, not FFmpeg's `-crf`; FFmpeg options belong to the FFmpeg encoder).

## Examples

- `> av1an encode -e rav1e --params "--speed 4"`
- `> av1an target-quality --params "--preset 4" --metric ssimulacra2 --target 85`
