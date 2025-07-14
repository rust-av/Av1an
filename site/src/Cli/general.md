# General

Name | Flag | Type | Default
--- | --- | --- | ---
[Input](#input--i) | `-i` | Path
[Proxy](#proxy---temp) | `--proxy` | Path
[Output](#output--o) | `-o` | Path
[Temporary](#temporary---temp) | `--temp` | Path | Input file name hash
[Quiet](#quiet--q---quiet) | `-q` | 
[Verbose](#verbose---verbose) | `--verbose` | 
[Log File](#log-file--l---log-file) | `-l`, `--log-file` | Path | `./logs/av1an.log`
[Log Level](#log-level---log-level) | `--log-level` | `LOG_LEVEL` | `debug`
[Resume](#resume---resume) | `--resume` | 
[Keep](#keep--k---keep) | `-k`, `--keep` | 
[Force](#force---force) | `--force` | 
[No Defaults](#no-defaults---no-defaults) | `--no-defaults` | 
[Overwrite](#overwrite--y) | `-y` | 
[Never Overwrite](#never-overwrite--n) | `-n` | 
[Max Tries](#max-tries---max-tries) | `--max-tries` | Integer | 3
[Workers](#workers---workers) | `--workers` | Integer | `0` (Automatic)
[Thread Affinity](#thread-affinity---set-thread-affinity) | `--set-thread-affinity` | Integer | 
[Scaler](#scaler---scaler) | `--scaler` | `SCALER` | `bicubic`
[VSPipe Arguments](#vspipe-arguments---vspipe-args) | `--vspipe-args` | String List | 
[Help](#help--h---help) | `-h`, `--help` | 
[Version](#version--v---version) | `-V`, `--version` | 

## Input `-i`

Input file to encode.

Can be a video or a VapourSynth (`.py`, `.vpy`) script.

### Examples

* `> av1an -i ./input.mkv -o output.mkv`
* `> av1an -i C:\Videos\input.mp4 -o output.mkv`
* `> av1an -i /home/videos/vapoursynth/script.vpy -o output.mkv`
* `> av1an -i ./script.py -o output.mkv`

## Proxy `--proxy`

Proxy file for the Input. When specified, Scene Detection and Target Quality probing will use this file instead of the input file.

Can be a video or a VapourSynth (`.py`, `.vpy`) script. Must have the same number of frames as the input file.

Useful for when the Input is a very intermediary file or a VapourSynth script that is computationally expensive.

### Examples

* `> av1an -i complex_input.vpy --proxy input.mkv -o output.mkv --target-quality 98` - Encodes with `complex_input.vpy` and uses `input.mkv` for Scene Detection and Target Quality
* `> av1an -i complex_input.vpy --proxy simple_input.vpy -o output.mkv` - Encodes with `complex_input.vpy` and uses `simple_input.vpy` for Scene Detection

## Output `-o`

Video output file.

### Examples

* `> av1an -i input.mkv -o C:\Encodes\output.mkv`
* `> av1an -i input.mkv -o output.mkv`
* `> av1an -i input.mkv -o /home/videos/av1an/done.mkv`

## Temporary `--temp`

Temporary directory to use.

### Default

If not specified, the temporary directory name is a hash of the input file name.

### Examples

* `> av1an -i input.mkv -o output.mkv` - Creates temporary directory `./.bf937a7/`
* `> av1an -i input.mkv -o output.mkv --temp temporary` - Creates temporary directory `./temporary/`
* `> av1an -i input.mkv -o output.mkv --temp C:\tmp\av1an` - Creates temporary directory `C:\tmp\av1an\`

## Quiet `-q`, `--quiet`

Disable printing progress to the terminal.

## Verbose `--verbose`

Print extra progress info and stats to the terminal.

## Log File `-l`, `--log-file`

Log file location under `./logs`.

Must be a relative path. Prepending with `./logs` is optional.

### Default

If not specified, logs to `./logs/av1an.log.{DATE}` where `{DATE}` is the current date in [ISO-8601](https://www.iso.org/iso-8601-date-and-time-format.html) format.

### Examples

* `> av1an -i input.mkv -o output.mkv` - Logs to `./logs/av1an.log.2020-1-10`
* `> av1an -i input.mkv -o output.mkv -l log.txt` - Logs to `./logs/log.txt`
* `> av1an -i input.mkv -o output.mkv --log-file ./today/1.log` - Logs to `./logs/today/1.log`

## Log Level `--log-level`

Set log level for log file (does not affect command-line log level)

### Possible Values

* `error`: Designates very serious errors.
* `warn`: Designates hazardous situations.
* `info`: Designates useful information.
* `debug`: Designates lower priority information.
* `trace`: Designates very low priority, often extremely verbose, information. Includes rav1e scenechange decision info.

### Default

If not specified, log level is set to `debug`.

## Resume `--resume`

Resume previous session from temporary directory.

## Keep `-k`, `--keep`

Do not delete the temporary folder after encoding has finished

Necessary for resuming a session.

## Force `--force`

Do not check if the encoder arguments specified by `-v`/`--video-params` are valid.

## No Defaults `--no-defaults`

Do not include Av1an's default set of encoder parameters.

## Overwrite `-y`

Overwrite output file, without confirmation

## Never Overwrite `-n`

Never overwrite output file, without confirmation

## Max Tries `--max-tries`

Maximum number of chunk restarts for an encode.

### Possible Values

Can be an integer greater than or equal to `1`.

### Default

If not specified, max tries is set to `3`.

## Workers `-w`, `--workers`

Number of workers to spawn.

### Default

If not specified or set to `0`, the number of workers is automatically determined.

### Examples

* `> av1an -i input.mkv -o output.mkv` - Spawns workers automatically
* `> av1an -i input.mkv -o output.mkv -w 4` - Spawns 4 workers
* `> av1an -i input.mkv -o output.mkv --workers 2` - Spawns 2 workers

## Thread Affinity `--set-thread-affinity`

Pin each worker to a specific set number of threads.

This is currently only supported on Linux and Windows, and does nothing on unsupported platforms. Leaving this option unspecified allows the OS to schedule all processes spawned.

### Possible Values

Can be an integer greater than or equal to `1`.

### Default

If not specified, thread affinity is disabled and the OS will schedule all processes spawned.

## Scaler `--scaler`

Scaler used for scene detection when downscaling (`--sc-downscale-height`) or for VMAF calculation

### Possible Values

Valid scalers are based on the scalers available in [FFmpeg](https://ffmpeg.org/ffmpeg-scaler.html#toc-Scaler-Options), including `lanczos[1-9]` with `[1-9]` defining the width of the lanczos scaler.

### Default

If not specified, the scaler is set to `bicubic`.

### Examples

* `> av1an -i input.mkv -o output.mkv --sc-downscale-height 720 --scaler bilinear` - Downscale to 720p using bilinear scaling
* `> av1an -i input.mkv -o output.mkv --sc-downscale-height 540 --scaler lanczos3` - Downscale to 540p using lanczos3

## VSPipe Arguments `--vspipe-args`

Additional arguments to pass to vspipe.

Only applicable when using VapourSynth chunking methods (`--chunk-method`) such as lsmash or ffms2 or when the input is a VapourSynth script.

### Possible Values

Can be a string or a list of strings separated by spaces in the format of `"key1=value1" "key2=value2"`. See the VSPipe [documentation](https://www.vapoursynth.com/doc/output.html#options) for more information.

### Examples

* `> av1an -i input.mkv -o output.mkv --vspipe-args "message=fluffy kittens" "head=empty"` - Passes `message=fluffy kittens` and `head=empty` to vspipe with generated loadscript.vpy
* `> av1an -i input.vpy -o output.mkv --vspipe-args "blur=10"` - Passes `blur=10` to vspipe with input.vpy

## Help `-h`, `--help`

Print help information.

## Version `-V`, `--version`

Print version information.
