# Av1an

Av1an is a video encoding framework.
It can increase your encoding speed and improve cpu utilization by running multiple encoder processes in parallel.
Target quality, VMAF plotting, and more, available to take advantage for video encoding.

For help with av1an, please reach out to us on Discord or file a GitHub issue

## Parameters

For more details, see documentation for each parameter or run `av1an --help`.

### [General](./Cli/general.md)

Name | Flag | Type | Default
--- | --- | --- | ---
[Input](./Cli/general.md#input--i) | `-i` | Path
[Proxy](./Cli/general.md#proxy---proxy) | `--proxy` | Path
[Output](./Cli/general.md#output--o) | `-o` | Path
[Temporary](./Cli/general.md#temporary---temp) | `--temp` | Path | Input file name hash
[Quiet](./Cli/general.md#quiet--q---quiet) | `-q` | 
[Verbose](./Cli/general.md#verbose---verbose) | `--verbose` | 
[Log File](./Cli/general.md#log-file--l---log-file) | `-l`, `--log-file` | Path | `./logs/av1an.log`
[Log Level](./Cli/general.md#log-level---log-level) | `--log-level` | `LOG_LEVEL` | `debug`
[Resume](./Cli/general.md#resume---resume) | `--resume` | 
[Keep](./Cli/general.md#keep--k---keep) | `-k`, `--keep` | 
[Force](./Cli/general.md#force---force) | `--force` | 
[No Defaults](./Cli/general.md#no-defaults---no-defaults) | `--no-defaults` | 
[Overwrite](./Cli/general.md#overwrite--y) | `-y` | 
[Never Overwrite](./Cli/general.md#never-overwrite--n) | `-n` | 
[Max Tries](./Cli/general.md#max-tries---max-tries) | `--max-tries` | Integer | 3
[Workers](./Cli/general.md#workers---workers) | `--workers` | Integer | `0` (Automatic)
[Thread Affinity](./Cli/general.md#thread-affinity---set-thread-affinity) | `--set-thread-affinity` | Integer | 
[Scaler](./Cli/general.md#scaler---scaler) | `--scaler` | `SCALER` | `bicubic`
[VSPipe Arguments](./Cli/general.md#vspipe-arguments---vspipe-args) | `--vspipe-args` | String List | 
[Help](./Cli/general.md#help--h---help) | `-h`, `--help` | 
[Version](./Cli/general.md#version--v---version) | `-V`, `--version` | 

### [Scene Detection](./Cli/scene_detection.md)

Name | Flag | Type | Default
--- | --- | --- | ---
[Scenes](./Cli/scene_detection.md#scenes--s---scenes) | `-s`, `--scenes` | Path | 
[Scene Detection Only](./Cli/scene_detection.md#scene-detection-only---sc-only) | `--sc-only` | 
[Split Method](./Cli/scene_detection.md#split-method---split-method) | `--split-method` | `SPLIT_METHOD` | `av-scenechange`
[Scene Detection Method](./Cli/scene_detection.md#scene-detection-method---sc-method) | `--sc-method` | `SC_METHOD` | `standard`
[Scene Downscale Height](./Cli/scene_detection.md#scene-downscale-height---sc-downscale-height) | `--sc-downscale-height` | Integer | 
[Scene Pixel Format](./Cli/scene_detection.md#scene-pixel-format---sc-pix-format) | `--sc-pix-format` | `PIXEL_FORMAT` | 
[Extra Split Frames](./Cli/scene_detection.md#extra-split-frames--x---extra-split) | `-x`, `--extra-split` | Integer | 
[Extra Split Seconds](./Cli/scene_detection.md#extra-split-seconds---extra-split-sec) | `--extra-split-sec` | Integer | 10
[Minimum Scene Length](./Cli/scene_detection.md#minimum-scene-length---min-scene-len) | `--min-scene-len` | Integer | 24
[Force Keyframes](./Cli/scene_detection.md#force-keyframes---force-keyframes) | `--force-keyframes` | Integer List

### [Encoding](./Cli/encoding.md)

Name | Flag | Type | Default
--- | --- | --- | ---
[Encoder](./Cli/encoding.md#encoder--e---encoder) | `-e`, `--encoder` | `ENCODER` | `svt-av1`
[Video Parameters](./Cli/encoding.md#video-parameters--v---video-params) | `-v`, `--video-params` | String List | Based on Encoder
[Passes](./Cli/encoding.md#passes--p---passes) | `-p`, `--passes` | Integer | 1
[Tile Auto](./Cli/encoding.md#tile-auto---tile-auto) | `--tile-auto` || 
[FFmpeg Parameters](./Cli/encoding.md#ffmpeg-filter-arguments--f---ffmpeg) | `-f`, `--ffmpeg` | String |
[Audio Parameters](./Cli/encoding.md#audio-parameters--a---audio-params) | `-a`, `--audio-params` | String |
[Ignore Frame Mismatch](./Cli/encoding.md#ignore-frame-mismatch---ignore-frame-mismatch) | `--ignore-frame-mismatch` | 
[Chunk Method](./Cli/encoding.md#chunk-method--m---chunk-method) | `-m`, `--chunk-method` | `CHUNK_METHOD` | `lsmash`
[Chunk Order](./Cli/encoding.md#chunk-order---chunk-order) | `--chunk-order` | `CHUNK_ORDER` | `long-to-short`
[Photon Noise](./Cli/encoding.md#photon-noise---photon-noise) | `--photon-noise` | Integer |
[Chroma Noise](./Cli/encoding.md#chroma-noise---chroma-noise) | `--chroma-noise` || 
[Photon Noise Width](./Cli/encoding.md#photon-noise-width---photon-noise-width) |`--photon-noise-width` | Integer |
[Photon Noise Height](./Cli/encoding.md#photon-noise-height---photon-noise-height) | `--photon-noise-height` | Integer |
[Concatenation Method](./Cli/encoding.md#concatenation-method--c---concat) | `-c`, `--concat` | `CONCAT` | `ffmpeg`
[Pixel Format](./Cli/encoding.md#pixel-format---pix-format) | `--pix-format` | `PIX_FORMAT` | `yuv420p10le`
[Zones](./Cli/encoding.md#zones---zones) | `-z`, `--zones` | Path | 

### [VMAF](./Cli/vmaf.md)

Name | Flag | Type | Default
--- | --- | --- | ---
[VMAF](./Cli/vmaf.md#vmaf---vmaf) | `--vmaf` || 
[VMAF Path](./Cli/vmaf.md#vmaf-path---vmaf-path) | `--vmaf-path` | String | 
[VMAF Resolution](./Cli/vmaf.md#vmaf-resolution---vmaf-res) | `--vmaf-res` | String | `1920x1080`
[VMAF Threads](./Cli/vmaf.md#vmaf-threads---vmaf-threads) | `--vmaf-threads` | Integer | 
[VMAF Filter](./Cli/vmaf.md#vmaf-filter---vmaf-filter) | `--vmaf-filter` | String | 

### [Target Quality](./Cli/target_quality.md)

Name | Flag | Type | Default
--- | --- | --- | ---
[Target Metric](./Cli/target_quality.md#target-metric---target-metric) | `--target-metric` | `TARGET_METRIC` | `VMAF`
[Target Quality](./Cli/target_quality.md#target-quality---target-quality) | `--target-quality` | Float | 
[Probes](./Cli/target_quality.md#probes---probes) | `--probes` | Integer | `4`
[Probe Resolution](./Cli/target_quality.md#probe-resolution---probe-res) | `--probe-res` | String |
[Probing Rate](./Cli/target_quality.md#probing-rate---probing-rate) | `--probing-rate` | Integer | `1`
[Probing Speed](./Cli/target_quality.md#probing-speed---probing-speed) | `--probing-speed` | `PROBING_SPEED` |
[Probing Statistic](./Cli/target_quality.md#probing-statistic---probing-stat) | `--probing-stat` | String | `percentile=1`
[Probe Slow](./Cli/target_quality.md#probe-slow---probe-slow) | `--probe-slow` || 
[Minimum Quantizer](./Cli/target_quality.md#minimum-quantizer---min-q) | `--min-q` | Integer | Based on Encoder
[Maximum Quantizer](./Cli/target_quality.md#maximum-quantizer---max-q) | `--max-q` | Integer | Based on Encoder
