# VP8/VP9

GIT: [libvpx](https://chromium.googlesource.com/webm/libvpx)

Condor drives the `vpxenc` binary, selected with `--encoder vpx`. `--params`
is merged into the parameters Condor supplies, so anything below can be
overridden without repeating the defaults. This page covers the options worth
knowing; for the full list read the
[VP8 encode parameter guide](https://www.webmproject.org/docs/encoder-parameters/)
and the [VP9 specific options](https://developers.google.com/media/vp9).

## Default parameters

Condor encodes VP9 with these parameters unless `--params` says otherwise:

| Parameter | Value | Meaning |
| --------- | ----- | ------- |
| `--codec` | `vp9` | Codec (use `vp8` for VP8; most tuning below is VP9-only) |
| `--end-usage` | `q` | Constant quality |
| `--cq-level` | `30` | Quantizer for constant quality (0-63) |
| `-b` | `10` | Bit depth 10 |
| `--profile` | `2` | 10-bit 4:2:0 profile |
| `--cpu-used` | `2` | Speed (VP9: -8 to 8, higher is faster) |
| `--threads` | `4` | Encoder threads |
| `--row-mt` | `1` | Row-based multithreading |
| `--auto-alt-ref` | `6` | Automatic alternate reference frames |
| `--kf-max-dist` | `9999` | Key frame interval, effectively "never" |
| `--disable-kf` | | Do not force key frames |

VP9 also takes two passes in Condor (`--passes 2`, the default for `vpx`), which
is why the defaults leave the rate control wide open.

## Rate control `--end-usage`

- `--end-usage=q --cq-level=N` — constant quality, the default. Lower `N` is
  higher quality; `--cq-level` must be 0-63.
- `--end-usage=vbr --target-bitrate=B` — variable bit-rate, `B` in kbps.
- `--end-usage=cbr --target-bitrate=B` — constant bit-rate, for streaming.
- `--end-usage=cq --cq-level=N --target-bitrate=B` — constrained quality: hold
  the quantizer where it can and drop it when the bitrate cap is reached.

## Speed `--cpu-used`

`--cpu-used` runs from -8 (slowest, best) to 8 (fastest). Condor uses `2`.
Realtime encodes instead set `--deadline=realtime`, which forces `--cpu-used`
to 5 or higher.

## Bit depth and profile

`-b 10` (or `--bit-depth=10`) encodes 10-bit, paired with `--profile=2` in the
defaults. 10-bit is recommended even for 8-bit sources: it avoids banding and
compresses more efficiently. Use `-b 8 --profile=0` for 8-bit, and
`--profile=1`/`3` for 4:4:4.

## Tiles and threading

`--tile-columns=N` and `--tile-rows=N` set the tile grid (log2 of the tile
counts), `--threads=N` caps the worker threads, and `--row-mt=1` enables
row-based multithreading. `--tile-columns=2 --tile-rows=1` is a reasonable
1080p starting point; raise it for higher resolution or frame rate.

## Key frames and alternate references

`--kf-min-dist` and `--kf-max-dist` bound the key frame interval in frames, and
`--auto-alt-ref=N` enables alternate reference frames (with `--lag-in-frames`
controlling the lookahead they need). Condor encodes scenes independently, so
its defaults keep key frames out of the way within a scene.

## Photon noise

Photon Noise is not available for `vpx`: `--photon-noise` and `--chroma-noise`
are rejected for it. Use `svt-av1`, `rav1e`, or `aom` for film grain
synthesis. See [Photon Noise](../types/photon-noise.md).

## Examples

```bash
$ av1an encode --encoder vpx --params "--cq-level=24"
$ av1an encode --encoder vpx --params "--end-usage=vbr --target-bitrate=3000"
$ av1an encode --encoder vpx --params "--codec=vp8 -b 8 --profile=0 --cq-level=20"
$ av1an encode --encoder vpx --params "--cpu-used=0 --tile-columns=2 --tile-rows=1"
```
