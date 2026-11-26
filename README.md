# Av1an

![Demonstration of the California Condor TUI](./california-condor/media/demo.avif)

[![Discord server](https://discordapp.com/api/guilds/696849974230515794/embed.png)](https://discord.gg/Ar8MvJh)
[![CI tests](https://github.com/rust-av/Av1an/actions/workflows/tests.yml/badge.svg)](https://github.com/rust-av/Av1an/actions/workflows/tests.yml)
[![](https://tokei.rs/b1/github/rust-av/Av1an?category=code)](https://github.com/rust-av/Av1an)

<a href="https://www.buymeacoffee.com/master_of_zen" target="_blank"><img src="https://cdn.buymeacoffee.com/buttons/v2/default-yellow.png" alt="Buy Me A Coffee" style="height: 60px !important;width: 217px !important;" ></a>

Av1an is a video encoding Rust library and command-line tool designed to be as fast as possible, easy to use, and extremely extensible. It supports a wide variety of encoders and tools to produce high quality videos.

## Branch changes and legacy Av1an

The default branch is [`main`](https://github.com/rust-av/Av1an/tree/main).
`main` now includes the Condor rewrite of Av1an, whose executable is `av1an`.
The [`v0` branch](https://github.com/rust-av/Av1an/tree/v0) contains the latest
stable release of legacy Av1an.

## Features

* Support for Windows, Linux, and MacOS
* [Docker builds](./california-condor/docs/docker.md) (published legacy Av1an images may not contain Condor)
* Hyper-scalable video encoding
* Cancel and resume at any time without losing progress
* Real-time progress feedback
* Terminal User Interface (TUI)
* Modular architecture for custom workflows

### Video Filtering

Av1an is built with [FFMS2][ffms2] for simple decoding but also supports [VapourSynth][vapoursynth] for advanced filtering and analysis.

* Decode the input video with either the [FFMS2][ffms2], [BestSource][vs-bestsource], [L-SMASH-Works][vs-lsmash], or [DGDecNV][vs-dgdecnv] VapourSynth plugin
* Pass a VapourSynth Python script (`.vpy` or `.py`) as the input
* Apply custom variables to the VapourSynth script
* Output a specific track/index from the VapourSynth script
* Apply additional VapourSynth filters such as [Trim][vs-trim], [Crop][vs-crop], [Resize][vs-resize], and [Splice][vs-splice]

### Video Encoding

Av1an supports the following encoder executables:

- [aomenc][aom] - Alliance for Open Media reference AV1 encoder
- [SvtAv1EncApp][svt-av1] - Scalable Video Technology for AV1 AV1 encoder
- [rav1e][rav1e] - Rust AV1 encoder
- [avmenc][avm] - Alliance for Open Media AOM Video Model reference AV2 encoder
- [vpxenc][vpx] - WebM VP8/VP9 encoder
- [x264][x264] - x264 H.264 encoder
- [x265][x265] - x265 H.265 encoder
- [vvenc][vvenc] - Fraunhofer Versatile Video Encoder H.266 encoder
- [FFmpeg][ffmpeg] - FFmpeg

### Video Analysis

Av1an can analyze video properties and qualities to improve encoded video quality by:

* Splitting the video into contiguous scenes
* Scaling Photon Noise ISO based on the level of noise/grain in each scene
* Finding the most efficient quantizer for each scene for a given quality target
* Choosing the appropriate encoding speed for each scene

## Getting Started

### Installation

The CLI's Cargo package is **`california-condor`**; its executable is **`av1an`**.
As of 2026-11-13, this package is unpublished on crates.io, and the checked
upstream releases do not provide Condor ZIPs. Use a checkout of `main` containing
this workspace; `v0` contains legacy Av1an.
After installing the [build prerequisites](./california-condor/docs/compiling.md),
run from the repository root:

```bash
cargo install --locked --path california-condor --bin av1an
```

**Windows:** this copies only `av1an.exe`, not its required DLLs. Follow the
[Windows runtime/build instructions](./california-condor/docs/installation.md#installation)
instead of using this command alone.

See [Installation](./california-condor/docs/installation.md) for package availability,
future release bundles and runtime dependencies, and [Docker](./california-condor/docs/docker.md)
for upstream image policy and local Condor builds.

### Usage

#### California Condor CLI

The Av1an CLI, California Condor, uses a single JSON configuration file to manage the entire encoding process. Each step can also be executed individually with its own command. You can use `--help` on each command for details on how to use them (e.g. `av1an --help`, `av1an detect-scenes --help`, `av1an target-quality --help`). For a complete guide on using California Condor, see the [California Condor README](./california-condor/README.md) and [guide](https://rust-av.github.io/Av1an/guide.html). For a quick start, see the example below.

*Encode a 1080p 10-bit AV1 video with Film Grain Synthesis using rav1e. Use FFMS2 to decode the input. Downscale the scene detection input to 540p to detect scenes faster. Target a SSIMULACRA 2 quality score of 85 quickly. Use mkvmerge to concatenate the output video.*

```bash
av1an \
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

#### Andean Condor Library

The Av1an library, Andean Condor, consists of the following components: `Input`, `Output`, `Encoder`, `Scene`, and `Sequence`. These components are managed with a single `Condor` instance. See the [library example](./andean-condor/examples/simple.rs) and [local dependency setup](./california-condor/docs/index.md#library); `andean-condor` is not yet published on crates.io.

##### Library cancellation

The ordinary library methods accept cancellation directly. For a standalone
input open without cancellation controls:

```rust
use andean_condor::core::input::Input;
use andean_condor::utils::cancellation::Cancellation;

let input = Input::ensure_indexed(&input_model, |_| {}, &Cancellation::default())?;
```

For a job, keep one token from `Input::ensure_indexed` through
`AndeanCondor::process_all` (or `process_one`). Clone it for each stage and
share a clone with the application's stop controls:

```rust
use andean_condor::core::{AndeanCondor, input::Input};
use andean_condor::utils::cancellation::Cancellation;

let cancellation = Cancellation::default();
let controls = cancellation.clone(); // Move this clone into the stop handler.

let input = Input::ensure_indexed(&input_model, |_| {}, &cancellation)?;
// Build the pipeline with this input, then run its stages with the same token.
let warnings = pipeline.process_all(progress_tx, cancellation)?;
```

Call `controls.request()?` for each stop request. The first request stops
admitting new work but lets admitted scenes finish all passes. A second request
stops feeds, kills owned child processes, and wakes workers so they can be joined.
Errors retain a typed `Cancelled::Forced` cause.

For manual passes, call `cancellation.admit()?` once per scene and pass that
view to filter setup and every `encode_pass_with_stream` or
`encode_pass_with_input` call. Reuse the same pass statistics where needed.
Keep the original token to admit the next scene.

Native decoder/plugin calls already in progress must finish; arbitrary hung
foreign calls cannot be interrupted safely. Unix children use private process
groups; Windows uses suspended creation and kill-on-close Job Objects (native
Windows acceptance remains pending). Deliberately escaping a managed group/job
is not supported. These library APIs do not register signals or change the CLI's
keyboard, terminal-exit, or exit-code behavior.

Unix hosts must not externally reap managed children or enable `SIGCHLD=SIG_IGN` /
`SA_NOCLDWAIT` while their guards are active. Auto-reap dispositions are rejected
before spawn; detected lost ownership is reported without signaling stale IDs.
This check cannot protect against concurrent external waiters. Use piped or null
stdin for managed children: inherited terminal input may stop a background group.

##### Rust API migration

This is a breaking change for library callers. CLI syntax and saved jobs are
unchanged.

- Drop the `_cancel` suffix from calls and keep the token argument. Calls that
  previously took no token now need one. Convert a legacy `Arc<AtomicBool>` to
  `Cancellation` once per job; the flag itself can only request graceful stop.
  Unlike the old FFMS2 indexing flag, graceful cancellation lets admitted
  indexing finish; use a forced request to interrupt it.
- Implement `Sequence::initialize` with `&Cancellation` and `Sequence::execute`
  with `Cancellation`. Check the token before admitting work, and return
  `Cancelled::Forced` after forced cleanup rather than reporting success.
  Use the token directly in new sequences: `Cancellation::legacy_flag` is
  deprecated and cannot distinguish graceful from forced cancellation.
  Implement `AndeanCondor::{process_one, process_all}` with `Cancellation` by value.
- Pass `ParallelEncoderOptions` to `ParallelEncoder::encode_tasks`,
  `Benchmarker::benchmark_workers`, and `TargetQuality::probe_pass`. The default
  is three total attempts with frame-count validation. Zone encoding takes the
  validation flag directly and still runs only once. `ZonePlan::try_build`
  returns `anyhow::Result`; downcast to `ZoneIneligible` to inspect an
  eligibility failure separately from a probe error or cancellation.
- `Encoder::{supports_capability, container}` now return `Result`: a missing
  executable or forced interruption is distinct from an unsupported capability
  or container. `parameters_with_output` takes `None` for the old output-derived
  stats base or `Some(path)` for an explicit base.
- Pass cancellation to California Condor's `entry_point`, `run_*` functions,
  and configuration/initialization helpers. Only the executable entry point
  installs OS signal handlers. Pass `Cancellation` to `Zoner::run` and
  `&Interrupt` to `TuiApp::handle_ctrl_c`, which returns `Result<()>`.
  Use `Configuration::save` for atomic config saves.
- Call `metrics::vmaf::score_probed_frames` with submission and final-score
  callbacks (use `|_| {}` if you do not need submission progress). Use
  `metrics::vship::score_probed_frames` for Vship. Use `Admission` methods for
  provenance, `EncoderBase` for extension and progress parsing queries, and
  `PhotonNoise::hash_name` for the grain table hash.
- Use `Default` for `VmafConfig` and `VshipConfig`, and select a VMAF model
  file with `.with_model(VmafModel::Path(path))`. Pass both colors to
  `FmetricsScorer::new` and the reset flag to `VshipScorer::submit_pair_raw`.
  Find version and availability probes at the metric crate root.

## Developing

See [Developing and Contributing](https://rust-av.github.io/Av1an/contributing.html) for a guide on developing Av1an and preparing for a Pull Request.

<!-- Links -->

[ffms2]: https://github.com/ffms/ffms2 "FFmpegSource"

[aom]: https://aomedia.googlesource.com/aom "Alliance for Open Media AV1"
[avm]: https://github.com/AOMediaCodec/avm "Alliance for Open Media AOM Video Model"
[svt-av1]: https://gitlab.com/AOMediaCodec/SVT-AV1 "Scalabe Video Technology for AV1"
[rav1e]: https://github.com/xiph/rav1e "Rust AV1 Encoder"
[vpx]: https://chromium.googlesource.com/webm/libvpx "WebM VP8/VP9"
[x264]: https://www.videolan.org/developers/x264.html "x264"
[x265]: https://www.videolan.org/developers/x265.html "x265"
[vvenc]: https://github.com/fraunhoferhhi/vvenc "Fraunhofer Versatile Video Encoder"
[ffmpeg]: https://ffmpeg.org "FFmpeg"

[ssimulacra2]: https://github.com/cloudinary/ssimulacra2 "SSIMULACRA 2 - Structural SIMilarity Unveiling Local And Compression Related Artifacts"
[butteraugli]: https://github.com/google/butteraugli "butteraugli - A tool for measuring perceived differences between images"
[cvvdp]: https://github.com/gfxdisp/colorvideovdp "ColorVideoVDP: A visible difference predictor for color images and videos"

[python-download]: https://www.python.org/downloads "Python"
[vsjetpack]: https://github.com/Jaded-Encoding-Thaumaturgy/vs-jetpack "vs-jetpack"

[vapoursynth]: https://www.vapoursynth.com "VapourSynth - A video processing framework with simplicity in mind"
[vapoursynth-download]: https://www.vapoursynth.com/doc/installation.html "Installing and Compiling"
[vs-trim]: https://www.vapoursynth.com/doc/functions/video/trim.html "Trim"
[vs-crop]: https://www.vapoursynth.com/doc/functions/video/crop_cropabs.html "Crop/CropAbs"
[vs-resize]: https://www.vapoursynth.com/doc/functions/video/resize.html "Resize"
[vs-splice]: https://www.vapoursynth.com/doc/functions/video/splice.html "Splice"

[vs-bestsource]: https://github.com/vapoursynth/bestsource "BestSource"
[vs-lsmash]: https://github.com/HomeOfAviSynthPlusEvolution/L-SMASH-Works "L-SMASH-Works"
[vs-dgdecnv]: https://www.rationalqm.us/dgdecnv/dgdecnv.html "DGDecNV - AVC/HEVC/MPG/VC1 Decoder and Frame Server"

[vszip]: https://github.com/dnjulek/vapoursynth-zip "VapourSynth Zig Image Process"
[vship]: https://codeberg.org/Line-fr/Vship "Vship : Fast Metric Computation on GPU"
[libvmaf]: https://github.com/Netflix/vmaf "libvmaf - VMAF (Video Multi-Method Assessment Fusion)"
[transnetv2]: https://huggingface.co/elya5/transnetv2 "TransNetV2 ONNX export"

[mkvtoolnix]: https://mkvtoolnix.download "MKVToolNix"
