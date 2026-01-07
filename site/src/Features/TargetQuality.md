# Target Quality

## Table of Contents

1. [Description](#Description)
2. [Requirements](#Requirements)
3. [Commands](#Commands)
4. [Example of usage](#Example-of-usage)

## Description

Target Quality has a really simple goal, instead of guessing what the CQ/CRF value to choose for desired level of video quality we set quality level we want and let the algorithm find CRF/CQ value that will result in that score, for each segment. Which simultaneously achieve 3 things, if compared to usual, single value CRF/CQ encode.

- Ensuring better level of visual consistency than default rate controls
- Give enough bitrate to complex segments to match target quality.
- Save bitrate by not overspending on scenes, which saves bit rate.

## Metrics

Target Quality supports the following metrics:

- [VMAF](https://github.com/Netflix/vmaf)
- [SSIMULACRA2](https://github.com/cloudinary/ssimulacra2)
- [Butteraugli](https://github.com/google/butteraugli)
- [XPSNR](https://github.com/fraunhoferhhi/xpsnr)

## Requirements

Depends on the specified Target Metric ([`--target-metric`](../Cli/target_quality.md#target-metric---target-metric))

### VMAF

- Working VMAF setup
  - FFMPEG with libvmaf (It's de facto default configuration from 2020)
  - Installed or manually selected VMAF models
    - by default it grabs /usr/share/model/vmaf_v0.6.1.pkl

### SSIMULACRA2

- VapourSynth
  - VapourSynth plugin [Vapoursynth-HIP](https://github.com/Line-fr/Vship) for Hardware-accelerated processing (recommended) or [Vapoursynth-Zig Image Process](https://github.com/dnjulek/vapoursynth-zip) for CPU processing
  - [Chunk Method](../Cli/encoding.md#chunk-method--m---chunk-method) must be either `lsmash`, `ffms2`, `bestsource`, or `dgdecnv`

### Butteraugli

- VapourSynth
  - VapourSynth plugin [Vapoursynth-HIP](https://github.com/Line-fr/Vship) for Hardware-accelerated processing (recommended) or [vapoursynth-julek-plugin](https://github.com/dnjulek/vapoursynth-julek-plugin) for CPU processing
  - [Chunk Method](../Cli/encoding.md#chunk-method--m---chunk-method) must be either `lsmash`, `ffms2`, `bestsource`, or `dgdecnv`

### XPSNR

- Working FFMPEG XPSNR setup
  - FFMPEG with libxpsnr
  - [Probing Rate](../Cli/target_quality.md#probing-rate---probing-rate) must be 1 or unspecified

Alternatively:

- Working VapourSynth XPSNR setup
  - VapourSynth plugin [Vapoursynth-Zig Image Process](https://github.com/dnjulek/vapoursynth-zip)
  - [Chunk Method](../Cli/encoding.md#chunk-method--m---chunk-method) must be either `lsmash`, `ffms2`, `bestsource`, or `dgdecnv`

### Encoders

- Quality/Constant Rate control (Target quality change crf/cq value for each segment). Which means that encoders must be in mode that use CRF/CQ and have those options specified ( `--crf 30`, `--cq-level=30`) those values get replaced for each segment

## Commands

- [`--target-metric TargetMetric`](../Cli/target_quality.md#target-metric---target-metric) - Chooses the metric used to evaluate quality of each segment

- [`--target-quality FLOAT-FLOAT`](../Cli/target_quality.md#target-quality---target-quality) - enables target quality with default settings for that encoder, targets a range with FLOAT values

- [`--probes INT`](../Cli/target_quality.md#probes---probes) - Overrides maximum amount of probes to make for each segment (Default 4)

- [`--probe-res "INTxINT"`](../Cli//target_quality.md#probe-resolution---probe-res) - Overrides the resolution of the probes during calculation

- [`--probing-rate INT`](../Cli/target_quality.md#probing-rate---probing-rate) - Divides the framerate of the probes by this value (Default 1)

- [`--probing-speed ProbeSpeed`](../Cli/target_quality.md#probing-speed---probing-speed) - Overrides the default or specified preset/cpu-used/speed for that encoder

- [`--probe-slow`](../Cli/target_quality.md#probe-slow---probe-slow) - Overrides the default settings for that encoder with the specified settings from [`--video-params`](../Cli/encoding.md#video-parameters--v---video-params)

More details can be found in the [Target Quality](../Cli/target_quality.md) CLI documentation

## Example of usage

`av1an -i file --target-quality 90` - Will run svt-av1 with default settings of target-quality

`av1an -i file --target-quality 95 --vmaf_path "vmaf_v.0.6.3.pkl" --probes 6` - With specified path to vmaf model and 6 probes per segment

`av1an -i file --encoder rav1e --target-metric ssimulacra2 --target-quality 90 --probing-rate 2` - Will run rav1e with default settings and probe every other frame with SSIMULACRA2

`av1an -i file --encoder svt-av1 --video-params "--preset 2 --enable-variance-boost 1" --target-metric butteraugli-3 --target-quality 2 --probe-slow --probing-rate 3` - Will run svt-av1 with preset 2 and variance-boost enabled and probe 1 of every 3 frames with Butteraugli 3-Norm

`av1an -i file --encoder x264 --video-params "--preset placebo --tune film" --target-metric xpsnr --target-quality 45 --probe-slow --probing-speed medium` - Will run x264 with preset slower and tune film with XPSNR

## Scaling

By default VMAF calculation is done at 1920x1080 with default model.
VMAF calculation resolution can be changed

`--vmaf-res 3840x2160`

Calculations with other metrics can be changed as well

`--probe-res 1280x720`

## Cropping with target quality

Filter with crop should be supplied for both ffmpeg options and vmaf filter

`--ffmpeg "-vf crop=3840:1900:0:0" --vmaf-filter "crop=3840:1900:0:0" --vmaf-res "3840x1900"`

or cropping and resizing could be done with vapoursynth script 
` -i 4k_crop.vpy --vmaf-res "3840x1600" --target-quality 90 -o test.mkv `
` -i 4k_crop.vpy --probe-res "1920x800" --target-quality 3 --target-metric butteraugli-3 -o test.mkv `
