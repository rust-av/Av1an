# rav1e

GIT: [rav1e](https://github.com/xiph/rav1e)

## Table of Contents

- [Important command line options](#important-command-line-options)
- [Example settings and explanation](#example-settings-and-explanation)

### Important command line options

| Command Line | Description  |
| -------------| -------------|
| --quantizer  | Quantizer (0-255), smaller values are higher quality (default: 100)|
| -s, --speed | Speed level 0-10 (0 is best quality, 10 is fastest) (default: 6)|

### Example settings and explanation

rav1e is selected with `--encoder rav1e` and tuned through `--params`, which is
merged into the parameters Condor supplies. Condor's defaults for rav1e are
`--speed 8 --quantizer 100 --keyint 0 --no-scene-detection -y`: scenes are
encoded independently, so scene change detection inside the encoder is off and
key frames are not forced.

##### Constant quality

` --quantizer 60 --speed 6 `

rav1e's default rate control is constant quantizer. The `--quantizer` range is
0-255, and lower values are higher quality. Anything below roughly 50 is
considered high quality; 100 is a fast, low-bitrate starting point, and it is
what Condor encodes with by default.

##### Speed

` --speed 4 `

One knob trades encode time for bitrate: 0 is the slowest and most efficient,
10 the fastest. Condor encodes with `--speed 8` by default. Speed scaling
(`av1an scale-speed`) writes this same parameter per scene.

##### Key frames

` --keyint 0 --no-scene-detection `

Condor encodes each scene in isolation, so intra-scene key frames are
unnecessary: `--keyint 0` means "no forced key frames" and
`--no-scene-detection` stops rav1e from cutting again inside a scene. Leave both
alone unless you have a reason to break a scene up further.

##### Photon noise

` --photon-noise-table <file> `

When `--photon-noise` (or `--chroma-noise`) is set, Condor generates a film
grain table and passes it to rav1e as `--photon-noise-table`.
