# Encoding

Name | Flag | Type | Default
--- | --- | --- | ---
[Encoder](#encoder--e---encoder) | `-e`, `--encoder` | `ENCODER` | `svt-av1`
[Video Parameters](#video-parameters--v---video-params) | `-v`, `--video-params` | String List | Based on Encoder
[Passes](#passes--p---passes) | `-p`, `--passes` | Integer | 1
[Tile Auto](#tile-auto---tile-auto) | `--tile-auto` || 
[FFmpeg Parameters](#ffmpeg-filter-arguments--f---ffmpeg) | `-f`, `--ffmpeg` | String |
[Audio Parameters](#audio-parameters--a---audio-params) | `-a`, `--audio-params` | String |
[Ignore Frame Mismatch](#ignore-frame-mismatch---ignore-frame-mismatch) | `--ignore-frame-mismatch` | 
[Chunk Method](#chunk-method--m---chunk-method) | `-m`, `--chunk-method` | `CHUNK_METHOD` | `lsmash`
[Chunk Order](#chunk-order---chunk-order) | `--chunk-order` | `CHUNK_ORDER` | `long-to-short`
[Photon Noise](#photon-noise---photon-noise) | `--photon-noise` | Integer |
[Chroma Noise](#chroma-noise---chroma-noise) | `--chroma-noise` || 
[Photon Noise Width](#photon-noise-width---photon-noise-width) |`--photon-noise-width` | Integer |
[Photon Noise Height](#photon-noise-height---photon-noise-height) | `--photon-noise-height` | Integer |
[Concatenation Method](#concatenation-method--c---concat) | `-c`, `--concat` | `CONCAT` | `ffmpeg`
[Pixel Format](#pixel-format---pix-format) | `--pix-format` | `PIX_FORMAT` | `yuv420p10le`
[Zones](#zones---zones) | `-z`, `--zones` | Path | 

## Encoder `-e`, `--encoder`

Video encoder to use.

### Possible Values

* `aom` - [aomenc](https://aomedia.googlesource.com/aom/)
* `rav1e` - [rav1e](https://github.com/xiph/rav1e)
* `vpx` - [vpxenc](https://chromium.googlesource.com/webm/libvpx/)
* `svt-av1` - [SvtAv1EncApp](https://gitlab.com/AOMediaCodec/SVT-AV1)
* `x264` - [x264](https://www.videolan.org/developers/x264.html)
* `x265` - [x265](https://www.videolan.org/developers/x265.html)

### Default

If not specified, `svt-av1` will be used.

## Video Parameters `-v`, `--video-params`

Parameters for video encoder.

These parameters are for the encoder binary directly, so the FFmpeg syntax cannot be used. For example, CRF is specified in ffmpeg via `-crf <CRF>`, but the x264 binary takes this value with double dashes, as in `--crf <CRF>`. See the `--help` output of each encoder for a list of valid options. This list of parameters will be merged into Av1an's default set of encoder parameters unless `--no-defaults` is specified.

## Passes `-p`, `--passes`

Number of encoder passes.

Two-pass mode is used by default for `aom` and `vpx`. Unlike other encoders which two-pass mode is used for more accurate VBR rate control, `aom` and `vpx` benefit from two-pass mode even with constant quality mode.

When using `aom` or `vpx` with RT mode (`--rt`), one-pass mode is always used regardless of the value specified by this flag (as RT mode in `aom` and `vpx` only supports one-pass encoding).

### Possible Values

Can be one of the following integers: `1` or `2`.

### Default

If not specified, `1` is used unless encoding with `aom` or `vpx` without RT mode (`--rt`), in which case `2` is used.

## Tile Auto `--tile-auto`

Estimate tile count based on resolution, and set encoder parameters, if applicable.

## FFmpeg Filter Arguments `-f`, `--ffmpeg`

Video filter arguments (FFmpeg syntax).

### Possible Values

Any of the valid FFmpeg [Video Filter Options](https://ffmpeg.org/ffmpeg.html#Video-Options).

### Examples

* `> av1an -i input.mkv -o output.mkv -f "-vf crop=100:100:100:100"` - Crops the video by 100 pixels from the top, left, bottom, and right
* `> av1an -i input.mkv -o output.mkv -f "-vf scale=1920:1080"` - Scales the video to 1920x1080


## Audio Parameters `-a`, `--audio-params`

Audio encoding parameters (FFmpeg syntax).

Do not use FFmpeg's `-map` syntax with this option. Instead, use the colon syntax ([Stream specifiers](https://ffmpeg.org/ffmpeg.html#Stream-specifiers-1)) with each parameter you specify.

Subtitles are always copied by default.

### Possible Values

Any of the valid FFmpeg [Audio Options](https://ffmpeg.org/ffmpeg.html#Audio-Options).

### Default

If not specified, `-c:a copy` is used.

### Examples

* `> av1an -i input.mkv -o output.mkv -a "-c:a libopus -b:a 128k"` - Encodes all audio tracks with [libopus][ffmpeg-libopus] at 128k
* `> av1an -i input.mkv -o output.mkv --audio-params "-c:a:0 libopus -b:a:0 128k -c:a:1 aac -ac:a:1 1 -b:a:1 24k"` - Encodes the first audio track with [libopus][ffmpeg-libopus] at 128k and the second audio track with [aac][ffmpeg-aac] at 24k and downmixed to a single channel

## Ignore Frame Mismatch `--ignore-frame-mismatch`

Ignore any detected mismatch between scene frame count and encoder frame count

## Chunk Method `-m`, `--chunk-method`

Method used for piping exact ranges of frames to the encoder.

Some methods require external VapourSynth plugins to be installed. The rest only require FFmpeg.

### Possible Values

* `lsmash` - [L-SMASH-Works](https://github.com/HomeOfAviSynthPlusEvolution/L-SMASH-Works)
  * Requires VapourSynth plugin
  * Generally the best and most accurate method
  * Does not require intermediate files
  * Errors generally only occur if the input file itself is broken (for example, if the video bitstream is invalid in some way, video players usually try to recover from the errors as much as possible even if it results in visible artifacts, while lsmash will instead throw an error)
* `ffms2` - [FFmpegSource](https://github.com/FFMS/ffms2)
  * Requires VapourSynth plugin
  * Accurate
  * Slightly faster than lsmash for y4m input
  * Does not require intermediate files
  * Can sometimes have bizarre bugs that are not present in lsmash (that can cause artifacts in the piped output)
* `dgdecnv` - [DGDecNV](https://www.rationalqm.us/dgdecnv/dgdecnv.html)
  * Requires VapourSynth plugin
  * Requires `dgindexnv` to be present in system path
  * Requires an NVIDIA GPU that supports CUDA video decoding
  * Very fast but only decodes AVC, HEVC, MPEG-2, and VC1
* `bestsource` - [BestSource](https://github.com/vapoursynth/bestsource)
  * Requires VapourSynth plugin
  * Slow but most accurate
  * Linearly decodes input files
  * Does not require intermediate files
* `hybrid` - Hybrid (Segment + Select)
  * Requires FFmpeg
  * Usually accurate but requires intermediate files (which can be large)
  * Avoids decoding irrelevant frames by seeking to the first keyframe before the requested frame and decoding only a (usually very small) number of irrelevant frames until relevant frames are decoded and piped to the encoder
* `select` - Select
  * Requires FFmpeg
  * Extremely slow, but accurate
  * Does not require intermediate files
  * Decodes from the first frame to the requested frame, without skipping irrelevant frames (causing quadratic decoding complexity)
* `segment` - Segment
  * Requires FFmpeg
  * Create chunks based on keyframes in the source
  * Not frame exact, as it can only split on keyframes in the source
  * Requires intermediate files (which can be large)

### Default

If not specified, the first available method is used in this order:

* `lsmash`
* `ffms2`
* `dgdecnv`
* `bestsource`
* `hybrid`

### Examples

* `> av1an -i input.mkv -o output.mkv -m lsmash` - Use L-SMASH-Works for chunking
* `> av1an -i input.mkv -o output.mkv -m ffms2` - Use FFmpegSource for chunking
* `> av1an -i input.mkv -o output.mkv -m hybrid` - Use hybrid for chunking

## Chunk Order `--chunk-order`

The order in which Av1an will encode chunks.

### Possible Values

* `long-to-short` - The longest chunks will be encoded first. This method results in the smallest amount of time with idle cores, as the encode will not be waiting on a very long chunk to finish at the end of the encode after all other chunks have finished.
* `short-to-long` - The shortest chunks will be encoded first.
* `sequential` - The chunks will be encoded in the order they appear in the video.
* `random` - The chunks will be encoded in a random order. This will provide a more accurate estimated filesize sooner in the encode.

### Default

If not specified, `long-to-short` is used.

### Examples

* `> av1an -i input.mkv -o output.mkv --chunk-order short-to-long` - Encodes the shortest chunks first
* `> av1an -i input.mkv -o output.mkv --chunk-order random` - Encodes the chunks in a random order

## Photon Noise `--photon-noise`

Generates a photon noise table and applies it using grain synthesis.

Photon noise tables are more visually pleasing than the film grain generated by aomenc, and provide a consistent level of grain regardless of the level of grain in the source. Strength values correlate to ISO values, e.g. `1` = ISO 100, and `64` = ISO 6400. This option currently only supports aomenc, rav1e, and SvtAv1EncApp.

An encoder's grain synthesis will still work without using this option, by specifying the correct parameter to the encoder. However, the two should not be used together, and specifying this option will disable or overwrite the encoder's internal grain synthesis.

### Possible Values

Can be any integer from `0` to `64`.

### Default

If not specified, `0` is used.

### Examples

* `> av1an -i input.mkv -o output.mkv --photon-noise 1` - Applies a ISO 100 photon noise table
* `> av1an -i input.mkv -o output.mkv --photon-noise 12` - Applies a ISO 1200 photon noise table

## Chroma Noise `--chroma-noise`

Adds chroma grain synthesis to the grain table generated by `--photon-noise`.

## Photon Noise Width `--photon-noise-width`

Manually set the width for the photon noise table.

### Possible Values

Can be any positive integer.

## Photon Noise Height `--photon-noise-height`

Manually set the height for the photon noise table.

### Possible Values

Can be any positive integer.

## Concatenation Method `-c`, `--concat`

Determines method used for concatenating encoded chunks and audio into output file.

### Possible Values

* `ffmpeg` - FFmpeg
  * Unfortunately, ffmpeg sometimes produces file with partially broken audio seeking, so `mkvmerge` should generally be preferred if available. FFmpeg concatenation also produces broken files with the `--enable-keyframe filtering=2` option in aomenc, so it is disabled if that option is used. However, FFmpeg can mux into formats other than Matroska (`.mkv`), such as WebM. To output WebM, use a `.webm` extension in the output file.
* `mkvmerge` - Matroska
  * Generally the best concatenation method (as it does not have either of the aforementioned issues that ffmpeg has), but can only produce matroska (.mkv) files. Requires mkvmerge to be installed.
* `ivf` - IVF
  * Experimental concatenation method implemented in Av1an itself to concatenate to an IVF file (which only supports VP8, VP9, and AV1, and does not support audio).

### Default

If not specified, `ffmpeg` is used.

## Pixel Format `--pix-format`

FFmpeg pixel format to use when encoding.

### Possible Values

Any valid pixel format name. See [FFmpeg](https://www.ffmpeg.org/doxygen/0.11/pixfmt_8h.html#60883d4958a60b91661e97027a85072a) for a full list.

### Examples

* `> av1an -i input.mkv -o output.mkv` - Use YUV420P10LE by default
* `> av1an -i input.mkv -o output.mkv --pix-format yuv420p` - Use YUV420P
* `> av1an -i input.mkv -o output.mkv --pix-format yuv444p` - Use YUV444P

### Default

If not specified, `yuv420p10le` is used.

## Zones `--zones`

Path to a file specifying zones within the video with differing encoder settings.

### Possible Values

The zones file should include one zone per line, with each arg within a zone separated by spaces. No quotes or escaping are needed around the encoder args, as these are assumed to be the last argument.

The zone args on each line should be in this order:

```
start_frame end_frame encoder reset(opt) video_params
```

`start_frame` is inclusive and `end_frame` is exclusive and both will be used as scene cuts. Additional scene detection will still be applied within each zone. `-1` can be used to indicate the end of the video.

The `reset` keyword instructs Av1an to ignore any settings which affect the encoder, and use only the parameters from this zone.

The video parameters which may be specified include any parameters that are allowed by the encoder, as well as the following Av1an options:

* [Extra Split Frames](./scene_detection.md#extra-split-frames--x---extra-split) `-x`, `--extra-split`
* [Minimum Scene Length](./scene_detection.md#minimum-scene-length---min-scene-len) `--min-scene-len`
* [Passes](#passes--p---passes) `-p`, `--passes`
* [Photon Noise](#photon-noise---photon-noise) `--photon-noise` (aomenc/rav1e/SvtAv1EncApp only)
* [Photon Noise Width](#photon-noise-width---photon-noise-width) `--photon-noise-width` (aomenc/rav1e/SvtAv1EncApp only)
* [Photon Noise Height](#photon-noise-height---photon-noise-height) `--photon-noise-height` (aomenc/rav1e/SvtAv1EncApp only)
* [Chroma Noise](#chroma-noise---chroma-noise) `--chroma-noise` (aomenc/rav1e/SvtAv1EncApp only)

For segments where no zone is specified, the settings passed to av1an itself will be used.


### Examples

* `> av1an -i input.mkv -o output.mkv --zones zones.txt` - Use the zones file `./zones.txt`
* `> av1an -i input.mkv -o output.mkv --zones C:\custom\configuration\zones.txt` - Use the zones file `C:\custom\configuration\zones.txt`

#### `./zones.txt`:
```
136 169 aom --photon-noise 4 --cq-level=32
169 1330 rav1e reset -s 3 -q 42
```

Line 1 will encode frames 136-168 using aomenc with the argument `--cq-level=32` and enable Av1an's `--photon-noise` option.

The default behavior as shown on line 1 is to preserve any options passed to `--video-params` or `--photon-noise`
in Av1an, and append or overwrite the additional zone settings.

Line 2 will encode frames 169-1329 using rav1e with only the arguments `-s 3 -q 42`.

[ffmpeg-libopus]: https://ffmpeg.org/ffmpeg-codecs.html#libopus-1
[ffmpeg-aac]: https://ffmpeg.org/ffmpeg-codecs.html#aac