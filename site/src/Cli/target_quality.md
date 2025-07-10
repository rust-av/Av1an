# Target Quality

Name | Flag | Type | Default
--- | --- | --- | ---
[Target Metric](#target-metric---target-metric) | `--target-metric` | `TARGET_METRIC` | `vmaf`
[Target Quality](#target-quality---target-quality) | `--target-quality` | Float | 
[Probes](#probes---probes) | `--probes` | Integer | `4`
[Probe Resolution](#probe-resolution---probe-res) | `--probe-res` | String |
[Probing Rate](#probing-rate---probing-rate) | `--probing-rate` | Integer | `1`
[Probing Speed](#probing-speed---probing-speed) | `--probing-speed` | `PROBING_SPEED` |
[Probing Statistic](#probing-statistic---probing-stat) | `--probing-stat` | String | `percentile=1`
[Probe Slow](#probe-slow---probe-slow) | `--probe-slow` || 
[Minimum Quantizer](#minimum-quantizer---min-q) | `--min-q` | Integer | Based on Encoder
[Maximum Quantizer](#maximum-quantizer---max-q) | `--max-q` | Integer | Based on Encoder


## Target Metric `--target-metric`

Metric used for Target Quality.

### Possible Values

Can be any of the following:

* `vmaf` - [Video Multi-Method Assessment Fusion](https://github.com/Netflix/vmaf)
    * Requires FFmpeg with [libvmaf](https://ffmpeg.org/ffmpeg-filters.html#libvmaf-1) enabled
* `ssimulacra2` - [Structural SIMilarity Unveiling Local And Compression Related Artifacts](https://github.com/cloudinary/ssimulacra2)
    * Requires VapourSynth plugin [Vapoursynth-HIP](https://github.com/Line-fr/Vship) for Hardware-accelerated processing (recommended) or [Vapoursynth-Zig Image Process](https://github.com/dnjulek/vapoursynth-zip) for CPU processing
    * Requires [Chunk Method](./encoding.md#chunk-method--m---chunk-method) to be `lsmash`, `ffms2`, `bestsource`, or `dgdecnv`
* `butteraugli-inf` - [butteraugli](https://github.com/google/butteraugli) Infinite-Norm with a target intensity of 203 nits
    * Requires VapourSynth plugin [Vapoursynth-HIP](https://github.com/Line-fr/Vship) for Hardware-accelerated processing (recommended) or [vapoursynth-julek-plugin](https://github.com/dnjulek/vapoursynth-julek-plugin) for CPU processing
    * Requires [Chunk Method](./encoding.md#chunk-method--m---chunk-method) to be `lsmash`, `ffms2`, `bestsource`, or `dgdecnv`
* `butteraugli-3` - [butteraugli](https://github.com/google/butteraugli) 3-Norm with a target intensity of 203 nits
    * Requires VapourSynth plugin [Vapoursynth-HIP](https://github.com/Line-fr/Vship) for Hardware-accelerated processing (recommended) or [vapoursynth-julek-plugin](https://github.com/dnjulek/vapoursynth-julek-plugin) for CPU processing
    * Requires [Chunk Method](./encoding.md#chunk-method--m---chunk-method) to be `lsmash`, `ffms2`, `bestsource`, or `dgdecnv`
* `xpsnr` - [Extended Perceptually Weighted Peak Signal-to-Noise Ratio](https://github.com/fraunhoferhhi/xpsnr) using the minimum of the `Y`, `U`, and `V` scores
    * Requires FFmpeg with [libxpsnr](https://ffmpeg.org/ffmpeg-filters.html#xpsnr-1) enabled when [Probing Rate](#probing-rate---probing-rate) is unspecified or `1`
    * Requires VapourSynth plugin [Vapoursynth-Zig Image Process](https://github.com/dnjulek/vapoursynth-zip) for CPU processing when [Probing Rate](#probing-rate---probing-rate) is greater than `1`
        * Requires [Chunk Method](./encoding.md#chunk-method--m---chunk-method) to be `lsmash`, `ffms2`, `bestsource`, or `dgdecnv`
* `xpsnr-weighted` - Weighted [Extended Perceptually Weighted Peak Signal-to-Noise Ratio](https://github.com/fraunhoferhhi/xpsnr) using the formula: `((4 * Y) + U + V) / 6`
    * Requires FFmpeg with [libxpsnr](https://ffmpeg.org/ffmpeg-filters.html#xpsnr-1) enabled when [Probing Rate](#probing-rate---probing-rate) is unspecified or `1`
    * Requires VapourSynth plugin [Vapoursynth-Zig Image Process](https://github.com/dnjulek/vapoursynth-zip) for CPU processing when [Probing Rate](#probing-rate---probing-rate) is greater than `1`
        * Requires [Chunk Method](./encoding.md#chunk-method--m---chunk-method) to be `lsmash`, `ffms2`, `bestsource`, or `dgdecnv`

### Default

If not specified, `vmaf` is used.

### Examples

* `> av1an -i input.mkv -o output.mkv --target-quality 95` - Target a VMAF score of 95
* `> av1an -i input.mkv -o output.mkv --target-metric ssimulacra2 --target-quality 80` - Target a SSIMULACRA2 score of 80
* `> av1an -i input.mkv -o output.mkv --target-metric butteraugli-3 --target-quality 2` - Target a Butteraugli 3-Norm score of 2
* `> av1an -i input.mkv -o output.mkv --target-metric xpsnr-weighted --target-quality 40` - Target a Weighted XPSNR score of 40

## Target Quality `--target-quality`

Target a metric quality score using the specified [`--target-metric`](#target-metric---target-metric) or [VMAF](https://github.com/Netflix/vmaf) by default. This is the score for encoding.

For each chunk, Target Quality searches for the quantizer/crf needed to achieve a certain metric score. Target Quality mode is much slower than normal encoding, but can improve the consistency of quality in some cases.

Metric score ranges:

* [VMAF](https://github.com/Netflix/vmaf) and [SSIMULACRA2](https://github.com/cloudinary/ssimulacra2) - 0 as the worst quality, and 100 as the best quality
* [butteraugli](https://github.com/google/butteraugli)("butteraugli-inf" and "butteraugli-3") - 0 as the best quality and increases as quality decreases towards infinity.
* [XPSNR](https://github.com/fraunhoferhhi/xpsnr)("xpsnr" and "xpsnr-weighted") - 0 as the worst quality and increases as quality increases towards infinity.

### Possible Values

Any float value for the specified [`--target-metric`](#target-metric---target-metric):

* "vmaf" - `0`-`100`, where `0` is the worst quality and `100` is the best
* "ssimulacra2" - `0`-`100`, where `0` is the worst quality and `100` is the best
* "butteraugli-inf" - `0` to any positive value, where `0` is the best quality and increases as quality decreases
* "butteraugli-3" - `0` to any positive value, where `0` is the best quality and increases as quality decreases
* "xpsnr" - `0` to any positive value, where `0` is the worst quality, and increases as quality increases
* "xpsnr-weighted" - `0` to any positive value, where `0` is the worst quality, and increases as quality increases

### Examples

* `> av1an -i input.mkv -o output.mkv --target-quality 80` - Target a VMAF score of 80
* `> av1an -i input.mkv -o output.mkv --target-quality 90.5` - Target a VMAF score of 90.5
* `> av1an -i input.mkv -o output.mkv --target-metric ssimulacra2 --target-quality 75` - Target a SSIMULACRA2 score of 75
* `> av1an -i input.mkv -o output.mkv --target-metric butteraugli-inf --target-quality 5.4` - Target a Butteraugli Infinite-Norm score of 5.4
* `> av1an -i input.mkv -o output.mkv --target-metric butteraugli-3 --target-quality 1.5` - Target a Butteraugli 3-Norm score of 1.5
* `> av1an -i input.mkv -o output.mkv --target-metric xpsnr --target-quality 50` - Target a XPSNR score of 40
* `> av1an -i input.mkv -o output.mkv --target-metric xpsnr-weighted --target-quality 40` - Target a Weighted XPSNR score of 40

## Probes `--probes`

Maximum number of probes allowed for Target Quality.

### Possible Values

Can be any positive integer.

### Default

If not specified, `4` is used.

## Probing Rate `--probing-rate`

Framerate for probes.

### Possible Values

Can be any integer from `1` to `4`.

### Default

If not specified, `1` is used.

## Probe Resolution `--probe-res`

Resolution used for Target Quality probe calculation.

### Possible Values

Can be a string in the format of `widthxheight` where `width` and `height` are positive integers.

### Default

If not specified, the input resolution is used.

## Probing Speed `--probing-speed`

Speed for probes.

If used with `--probe-slow`, it overrides the respective speed parameter (eg. `--cpu-used`, `--preset`, etc.)

### Possible Values

Can be any of the following:

* `veryslow`
* `slow`
* `medium`
* `fast`
* `veryfast`

### Default

If not specified, `veryfast` is used unless `--probe-slow` is specified.

## Probing Statistic `--probing-stat`

Statistical method for calculating target quality from sorted probe results.

### Possible Values

Can be any of the following:

* `auto` - Automatically choose the best method based on the target metric, the probing speed, and the quantizer
* `mean` - Arithmetic mean (average)
* `median` - Middle value
* `harmonic` - Harmonic mean (emphasizes lower scores)
* `root-mean-square` - Root mean square (quadratic mean)
* `percentile=<FLOAT>` - Percentile of a specified `<FLOAT>` value, where `<FLOAT>` is a value between 0.0 and 100.0
* `standard-deviation=<FLOAT>` - Standard deviation distance from mean (σ) clamped by the minimum and maximum probe scores of a specified `<FLOAT>` value, where `<FLOAT>` can be a positive or negative value
* `mode` - Most common integer-rounded value
* `minimum` - Lowest value
* `maximum` - Highest value

### Default

If not specified, `auto` is used.

### Examples

* `> av1an -i input.mkv -o output.mkv --target-quality 80 --probing-statistic mean` - Target a VMAF score of 80 using the mean statistic
* `> av1an -i input.mkv -o output.mkv --target-quality 95 --probing-statistic percentile-25` - Target a VMAF score of 95 using the 25th percentile statistic
* `> av1an -i input.mkv -o output.mkv --target-quality 90 --probing-statistic standard-deviation--0.8` - Target a VMAF score of 90 using the value that is 0.8 standard deviations below the mean
* `> av1an -i input.mkv -o output.mkv --target-quality 75 --probing-statistic standard-deviation-2` - Target a VMAF score of 75 using the value that is 2 standard deviations above the mean.

## Probe Slow `--probe-slow`

Use encoding settings for probes specified by `--video-params` rather than faster, less accurate settings.

Note that this always performs encoding in one-pass mode, regardless of `--passes`.

## Minimum Quantizer `--min-q`

Lower bound for Target Quality Quantizer-search early exit.

If the minimum quantizer is tested and the probe's quality score is lower than the Target Quality ([`--target-quality`](#target-quality---target-quality)), the Quantizer-search exits early and the minimum quantizer is used for the chunk.

### Possible Values

Depends on the encoder.

### Default

If not specified, the default value is used (chosen per encoder).

## Maximum Quantizer `--max-q`

Upper bound for Target Quality Quantizer-search early exit.

If the maximum quantizer is tested and the probe's quality score is higher than the Target Quality ([`--target-quality`](#target-quality---target-quality)), the Quantizer-search exits early and the maximum quantizer is used for the chunk.

### Possible Values

Depends on the encoder.

### Default

If not specified, the default value is used (chosen per encoder).
