# California Condor

![Demonstration of the California Condor TUI](./media/demo.avif)

[![Discord server](https://discordapp.com/api/guilds/696849974230515794/embed.png)](https://discord.gg/Ar8MvJh)
[![CI tests](https://github.com/rust-av/Av1an/actions/workflows/tests.yml/badge.svg)](https://github.com/rust-av/Av1an/actions/workflows/tests.yml)

<a href="https://www.buymeacoffee.com/master_of_zen" target="_blank"><img src="https://cdn.buymeacoffee.com/buttons/v2/default-yellow.png" alt="Buy Me A Coffee" style="height: 60px !important;width: 217px !important;" ></a>

California Condor is the command-line interface and Terminal User Interface (TUI) for Av1an. The `av1an` binary manages all processes including encoding with a single JSON configuration file. It wraps Av1an's library, Andean Condor.

## Features

* Cross-platform CLI and TUI for Windows, Linux, and MacOS
* Chunked parallel encoding pipeline: scene detection, noise detection, benchmarking, target quality, encoding, concatenation, quality check
* Single JSON configuration file with schema validation
* Cancel and resume at any time with progress saved in the JSON configuration file
* Real-time TUI progress reporting for long-running sequences
* Real-time progress reporting to STDOUT when piping (external script support)
* Per-step commands for individual execution
* VapourSynth filtering with per-stage overrides for encode, scene detection, and target quality inputs
* Supported decoders: `bestsource`, `vs-ffms2`, `lsmash`, `dgdecnv`, `ffms2`
* Supported encoders: `aom`, `rav1e`, `vpx`, `svt-av1`, `avm`, `x264`, `x265`, `vvenc`, `ffmpeg`
* Supported concatenation methods: `mkvmerge`, `ffmpeg`, `ivf`
* Supported quality metrics: `ssimulacra2`, `butteraugli`, `butteraugli-3`, `xpsnr`, `cvvdp`, `vmaf`

> [!NOTE]
> Per-command captures (`media/detect-scenes.avif`, `media/detect-noise.avif`, `media/benchmark.avif`, `media/target-quality.avif`, `media/encode.avif`, `media/concatenate.avif`, `media/quality-check.avif`) will replace the placeholders above when available. Non-TUI commands (`init`, `scale-noise`, `optimize-bitrate`, `scale-speed`, `clean`) have no gallery entry.

## Getting Started

For a complete walkthrough, see the [guide](https://rust-av.github.io/Av1an/guide.html). It covers the end-to-end workflow, the JSON configuration file, temporary and log file layout, and when each command launches a TUI.

### Installation

The Cargo package is **`california-condor`**, and the binary is **`av1an`**.
As of 2026-11-13, the package is unpublished on crates.io. The checked upstream
releases do not yet offer this CLI's ZIPs.
From the root of a `main` checkout containing this workspace, with the
[build prerequisites](docs/compiling.md) installed:

```bash
cargo install --locked --path california-condor --bin av1an
```

**Windows:** this copies only `av1an.exe`, not its required DLLs. Follow the
[Windows runtime/build instructions](docs/installation.md#installation)
instead of using this command alone.

See [Installation](docs/installation.md) for package availability and runtime
dependencies, or [Docker](docs/docker.md) for upstream image policy and local builds.

When a release provides **`av1an-windows-x64.zip`**, extract the whole archive;
`av1an.exe` alone is not an install. Keep its mandatory FFMS2, MSVC CRT and
DirectML DLLs together, plus metric DLLs and the `model/` subdirectory. See the
[Windows bundle layout](docs/installation.md#windows-releases). The loose
`av1an.exe` asset on existing releases is legacy Av1an (`v0`), not this rewrite.

External tools required for decoding, filtering, encoding, metrics, and concatenation:

* [Python][python-download] - Recommended for [VapourSynth][vapoursynth-download]
* [vs-jetpack][vsjetpack] - Installs [VapourSynth][vapoursynth] and plugins for scaling, denoising, debanding, deinterlacing, etc.
* [Vship][vship] - GPU-accelerated metrics for [SSIMULACRA 2][ssimulacra2], [butteraugli][butteraugli], and [ColorVideoVDP][cvvdp]. The native loader admits only the verified 5.1.2 ABI, backend and symbols; a file alone does not prove GPU availability. See [installation and compatibility](../av-metrics-vship/README.md#installing-libvship)
* [libvmaf][libvmaf] - Required only for the `vmaf` metric. Use verified release **v3.2.1**, which reports runtime **3.2.0**; arbitrary newer/distro builds are not guaranteed compatible. See [installation and ABI policy](../av-metrics-vmaf/README.md#prerequisites)
* At least one encoder: [aomenc][aom], [SvtAv1EncApp][svt-av1], [rav1e][rav1e], [avmenc][avm], [vpxenc][vpx], [x264][x264], [x265][x265], [vvenc][vvenc], [FFmpeg][ffmpeg]
* Either [FFmpeg][ffmpeg] or [MKVToolNix][mkvtoolnix] for concatenating encoded scenes

> [!TIP]
> Ensure binaries such as [FFmpeg][ffmpeg], [mkvmerge][mkvtoolnix], or [SVT-AV1][svt-av1] are on `PATH`.

### Usage

Run the full pipeline with no subcommand, or run steps individually. Use `--help` on each command for details (e.g. `av1an --help`, `av1an detect-scenes --help`, `av1an target-quality --help`, `av1an encode --help`).

```bash
$ av1an init "input.mp4" "output.mkv"
$ av1an detect-scenes
$ av1an encode
$ av1an concatenate
```

*Encode a 1080p 10-bit AV1 video with Film Grain Synthesis using rav1e. Use FFMS2 to decode. Downscale scene detection input to 540p for faster detection. Target SSIMULACRA 2 score 85 with the fast profile. Concatenate with mkvmerge.*

```bash
$ av1an \
    --temp "./deletemelater" \
    --logs "./deletemelater/condor.log" \
    --config-file "./deletemelater/config.json" \
    --input "bird takeoff.mp4" \
    --output "bird takeoff.mkv" \
    --decoder "vs-ffms2" \
    --filters "resize:scaler=bilinear;width=1920;height=1080;format=yuv420p10le" \
    --scd-filters "resize:scaler=bilinear;width=540;height=960;" \
    --encoder "rav1e" \
    --params "--speed 4" \
    --photon-noise 800 \
    --target-metric "ssimulacra2" \
    --target 85 \
    --target-profile "fast" \
    --concat "mkvmerge"
```

See [av1an](https://rust-av.github.io/Av1an/commands/condor.html) for all full-run flags, [configuration](https://rust-av.github.io/Av1an/configuration.html) for global flags and `condor.json` validation, [types](https://rust-av.github.io/Av1an/types.html) for Decoder, Encoder, Filters, and other complex types, and the [guide](https://rust-av.github.io/Av1an/guide.html) for step ordering.

### Concurrent jobs and atomic configuration saves

Every job command, including configuration/scene edits and `info` reads, takes a
nonblocking OS advisory lock before loading the configuration and keeps it through
execution, save callbacks and worker joins. A busy-job error names the selected
configuration and its lock; retry after the other process exits. Help, version,
and the unavailable `clean` command do not acquire a job lock.

Existing-job read/edit/run routes reject a missing configuration before creating
directories, sidecars or logs. Only init-create (not `init info/set/reset`) and the
root pipeline route prepare missing config parents. Those creation routes may leave
parents and a sidecar even when later validation rejects the new job; no rollback
or automatic lock-file cleanup is performed. Bare `scenes` and `info` on existing
jobs need write/create access to the sidecar, even though they only read JSON, and
return busy while another process owns the job.

The stable sidecar uses the full filename: `job.json.lock` and `job.other.lock`
are independent. The empty sidecar intentionally remains after exit or a crash;
its existence does **not** mean busy. **Do not delete it to clear a lock**: the OS
releases ownership when the process closes/exits, including a forced termination.
Forced termination can leave native encoder/FFmpeg descendants writing work files;
lock release does not prove that those processes have stopped.
Separate configurations can run concurrently, but still need distinct work and
output paths. Symlink aliases (including existing symlink parents for new files)
resolve to one configuration target. Separately named hardlinks are not unified;
use one configuration pathname per job. Do not retarget aliases or replace/delete
sidecars while jobs are active. Lock/open/unsupported-filesystem errors refuse the
transaction rather than falling back to unprotected writes.

Saves validate and serialize before staging, then write/flush an exclusively owned,
unique sibling temporary file and atomically replace the configuration. Failed
publication leaves the original bytes intact and cleans only the owned temporary
file; a caller's old fixed `.temp` file is not reused. This is atomic publication,
not an fsync/power-loss durability guarantee.
On Unix publication uses the tempfile's `0600` mode (subject to umask), including
when replacing a more widely readable config; existing permissions are not preserved.

Direct Rust callers must acquire `configuration::JobLock` **before** loading and
use `guard.path()` for loads, handlers and saves. Retain the guard through edits,
public `run_*` helpers, all callbacks and joins. These APIs do not acquire a lock
themselves; individually atomic saves cannot protect an already stale snapshot.
Prepare a new config's parent directory explicitly before acquiring its guard;
generic `JobLock::acquire` never creates directories.

```rust,ignore
let guard = california_condor::configuration::JobLock::acquire(path)?;
let (mut config, save_file) =
    california_condor::commands::handlers::load_configuration(Some(guard.path()))?;
config.condor.sequence_config.parallel_encoder.workers = Some(2);
config.save(&save_file)?;
// Keep guard alive if calling run_encoder(&config, &save_file), too.
```

## Testing

From the repository root, with the [build/runtime prerequisites](docs/compiling.md)
installed:

```sh
cargo +stable test --profile ci -p california-condor
cargo +stable test --profile ci -p california-condor --test interrupt -- --nocapture
cargo +stable test --profile ci -p california-condor --features screenshots --test screenshots
python3 -B california-condor/tests/build/schema.py
```

The interruption cases require Linux and Python's PTY support. The standalone
schema acceptance check requires Git, VapourSynth, cached Cargo dependencies and
a matching installed or cached native ONNX Runtime. Before running it, set
`ORT_LIB_PATH` to that runtime's library directory unless pkg-config already finds
it; the offline Cargo build cannot download the runtime. The check builds an
isolated Git fixture and is not part of unittest discovery. Its scratch directory
uses Python's default temporary-directory selection, including `TMPDIR`.

## Gallery

![[av1an](./media/condor.avif)](./media/condor-small.avif) | ![detect-scenes](./media/detect-scenes-small.avif) |
:-: | :-: |
`av1an` | `detect-scenes` |
![detect-noise](./media/detect-noise-small.avif) | ![benchmark](./media/benchmark-small.avif) |
`detect-noise` | `benchmark` |
![target-quality](./media/target-quality-small.avif) | ![encode](./media/encode-small.avif) |
`target-quality` | `encode` |
![concatenate](./media/concatenate-small.avif) | ![quality-check](./media/quality-check-small.avif) |
`concatenate` | `quality-check` |

<!-- Links -->

[aom]: https://aomedia.googlesource.com/aom "Alliance for Open Media AV1"
[avm]: https://github.com/AOMediaCodec/avm "Alliance for Open Media AOM Video Model"
[svt-av1]: https://gitlab.com/AOMediaCodec/SVT-AV1 "Scalable Video Technology for AV1"
[rav1e]: https://github.com/xiph/rav1e "Rust AV1 Encoder"
[vpx]: https://chromium.googlesource.com/webm/libvpx "WebM VP8/VP9"
[x264]: https://www.videolan.org/developers/x264.html "x264"
[x265]: https://www.videolan.org/developers/x265.html "x265"
[vvenc]: https://github.com/fraunhoferhhi/vvenc "Fraunhofer Versatile Video Encoder"
[ffmpeg]: https://ffmpeg.org "FFmpeg"

[python-download]: https://www.python.org/downloads "Python"
[vsjetpack]: https://github.com/Jaded-Encoding-Thaumaturgy/vs-jetpack "vs-jetpack"

[ssimulacra2]: https://github.com/cloudinary/ssimulacra2 "SSIMULACRA 2 - Structural SIMilarity Unveiling Local And Compression Related Artifacts"
[butteraugli]: https://github.com/google/butteraugli "butteraugli - A tool for measuring perceived differences between images"
[cvvdp]: https://github.com/gfxdisp/colorvideovdp "ColorVideoVDP: A visible difference predictor for color images and videos"

[vapoursynth]: https://www.vapoursynth.com "VapourSynth - A video processing framework with simplicity in mind"
[vapoursynth-download]: https://www.vapoursynth.com/doc/installation.html "Installing and Compiling"

[vszip]: https://github.com/dnjulek/vapoursynth-zip "VapourSynth Zig Image Process"
[vship]: https://codeberg.org/Line-fr/Vship "Vship : Fast Metric Computation on GPU"
[libvmaf]: https://github.com/Netflix/vmaf "libvmaf - VMAF (Video Multi-Method Assessment Fusion)"

[mkvtoolnix]: https://mkvtoolnix.download "MKVToolNix"
