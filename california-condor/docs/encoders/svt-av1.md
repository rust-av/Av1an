# SVT-AV1

This is a quick guide to the SVT-AV1 options worth setting in `--params`; everything here can also be set per scene through `condor.encoder`. For the complete reference, read the official documentation:

- [SVT-AV1 Docs - GitLab](https://gitlab.com/AOMediaCodec/SVT-AV1/-/tree/master/Docs)

Make sure your svt-av1 encoder is up-to-date.

# Rate control

## --rc

The `--rc` option selects what rate control strategy you want to use.

- `--rc 0` - Constant rate factor
- `--rc 1` - Variable bit rate
- `--rc 2` - Constant bit rate

## Constant rate factor 

```
--rc 0 --crf
```

Constant rate factor, a.k.a. constant quality. This is the most common way of determining video quality. You set --crf to be a number between 1-63, and the encoder will work out a bit-rate to keep constant quality. The lower the number, the less compression, the higher it is, the more compression will be used. Anything lower than 20 is considered "hi-fi", 30+ is for mini encodes.

**Example:**

```
condor encode --params "--rc 0 --crf 24 --preset 4 --input-depth 10 --tune 0"
```

## Variable bit-rate

```
--rc 1 --tbr
```

Variable bit-rate. Requires you to set target bit-rate `--tbr`.

**Example:**

```
condor encode --passes 2 --params "--rc 1 --tbr 2000 --preset 4 --input-depth 10 --tune 0"
```

# Preset

```
--preset
```

If RC controls the compression strategy, then the preset determines what optimisation features get enabled (at the cost of encode time). Realistically the range for preset is 0-13, with 0 being the most optimized (and slow). 0-3 is only for the latest and fastest system, or the most patient of people. 4-6 is more common among enthusiasts. You should go as low as you can bare, 4 is a good starting place for most.

**Example:**

```
... --preset 4 ...
```

# Tune

```
--tune
```

- `--tune 0` - VQ
- `--tune 1` - PSNR

VQ is subjective quality, while PSNR is an objective measurement. Most will recommend using VQ, it seems to make the image sharper as well. Default is PSNR.

# Film grain

```
--film-grain --film-grain-denoise
```

Synthesize film grain! `--film-grain` can be set to 1-50 (default is 0 - off), the higher the number the stronger the effect. The default behaviour is to denoise, then add the synthesized noise. But this can remove fine detail, so it is recommended to disable the denoise stage by setting `--film-grain-denoise` to 0.

You can also disable the encoders denoise, it is possible to use denoising filters provided by ffmpeg and vaporsynth. These will give better results than any of the encoders internal denoise filter. ffmpeg's hqdn3d, and nlmeans filters should be a good starting point. Common vaporsynth filters include: BM3D, DFTTest, SMDegrain, and KNLMeansCL.

**Example:**

```
... --film-grain 10 --film-grain-denoise 0 ...
```

# Colour metadata

Condor writes the input's colour and HDR metadata into the encoder parameters by
default. SVT-AV1 accepts all of it: `--color-primaries`,
`--transfer-characteristics`, `--matrix-coefficients`, `--color-range`,
`--chroma-sample-position`, `--mastering-display` and `--content-light`. Set
`append_color_data` to `false` (or `condor encode set append-color-data off`) to
leave it out, and set the flags yourself in `--params` if the source is tagged
wrongly — your options replace the appended ones. See
[`condor.encoder`'s Colour Metadata](../configuration/condor/encoder.md#colour-metadata).

# Input depth

```
--input-depth 10
```

You can choose a bit-depth of 8bit or 10bit. It is almost always recommended to use 10bit, even if the source is only 8bit. It is by far the most optimized, and can even fix problems inherent with 8bit.

You might not want to use 10bit if fast encode/decode is more important than video quality.

# Lookahead and key frames

```
--lookahead --keyint
```

These optimisations come at the cost of increased RAM usage during encode, and worse performance when seeking during playback. But they are worth the compromise.

Lookahead will affect how many future frames the encoder will look forward to. Increases effectiveness of several optimizations. Max is 120.

Modern video files include key frames which are Intra coded pictures, and Inter frames, which store only information changed since the previously encoded reference frames. Setting `keyint=24` will give you 1 second in a 24fps video. It is recommended to have 10 seconds of GOP, for a 24fps video this will now be `keyint=240`.

**24fps:**

```
--lookahead 120 --keyint 240
```

**30fps:**

```
--lookahead 120 --keyint 300
```
