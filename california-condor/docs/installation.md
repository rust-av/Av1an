# Installation

The Av1an CLI's Cargo package is **`california-condor`** and its binary is
**`av1an`**. As checked on 2026-11-13, `california-condor` and the library
`andean-condor` are not published on crates.io.
Use a source checkout of `main` containing this workspace. After installing the
[build prerequisites](./compiling.md), run from that checkout's root:

```bash
cargo install --locked --path california-condor --bin av1an
```

This installs `av1an` into Cargo's binary directory (normally `~/.cargo/bin` or
`%USERPROFILE%\.cargo\bin`); keep it on `PATH`. For a build without installation,
see [Compiling](./compiling.md).

**Windows: `cargo install` alone is incomplete.** It copies only `av1an.exe`,
not the mandatory runtime DLLs. Prefer the [Windows source-build route](./compiling.md#compiling-on-windows)
and run `target\release\av1an.exe` with its runtime files kept beside it:

- Copy `ffms2.dll` from the selected FFMS2 MSVC x64 installation (`FFMS_LIB_DIR`)
  into `target\release`.
- Keep the actual build-produced `DirectML.dll` and any additional ORT DLLs that
  the build places in `target\release`; do not substitute another ORT version.
- Stage `vcruntime140.dll`, `vcruntime140_1.dll`, `msvcp140.dll` and companions
  from the selected installed Visual Studio default-version x64 CRT
  redistributable. Do not take runtime DLLs from System32 or an unrelated PATH.
- Keep optional native metric DLLs with their runtime dependencies beside the
  executable when using those metrics. Keep VMAF and TransNetV2 models in `model/`
  beside it, or configure their explicit runtime model paths.

If relocating the executable, copy those actual runtime files and the needed
`model/` directory with it; copying only `av1an.exe` is not an installation.
Do not rely on temporary `cargo install` build artifacts being retained to obtain
these DLLs. The [Windows bundle layout](#windows-releases) below describes the
required files, not an already available download.

As checked on 2026-11-13, the AUR's `av1an-git` package provides legacy Av1an,
not this Condor rewrite. The shared `av1an` executable name does not establish
which implementation a package contains. No Arch package for the rewrite is
recommended here; use the source build above.

The checked [upstream releases](https://github.com/rust-av/Av1an/releases)
(latest plus the four recent versioned releases, 2026-11-13) do not provide
`av1an-windows-x64.zip` or `av1an-linux-x64.zip`; `latest` contains only legacy
`av1an.exe`, not this CLI. **Use source until a release supplies the Condor archives.**
The layouts below describe the configured packaging contract, not assets already
available for download. The release coordinator is configured to publish both ZIPs
and one `configuration.schema.json`, from the same source SHA with one release body,
only after both platform gates pass. Actual cloud publication and native Windows
acceptance remain pending.

For upstream image policy and local Condor builds, see [Docker](docker.md).

#### Windows releases

When a release provides **`av1an-windows-x64.zip`**, download and extract the whole
archive into one directory. The layout matters, because Condor finds parts of
itself by relative path:

```text
av1an/
├── av1an.exe
├── ffms2.dll              mandatory native decoder/runtime dependency
├── vcruntime140.dll       mandatory app-local MSVC C/C++ runtimes
├── vcruntime140_1.dll      |
├── msvcp140.dll           /  plus companions from the same VS CRT distribution
├── DirectML.dll           mandatory for the pinned Windows ONNX Runtime build
├── onnxruntime*.dll       if a selected build produces additional shared ORT DLLs
├── libvmaf.dll            VMAF
├── libvship.dll           SSIMULACRA2, Butteraugli and CVVDP on the GPU
├── fmetrics.dll           CPU fallback metrics
├── libgcc_s_seh-1.dll     \  runtime dependencies of libvmaf
├── libstdc++-6.dll        |
├── libwinpthread-1.dll   /
├── model/                 VMAF models, and the TransNetV2 scene detection model
└── configuration.schema.json
```

Avoid these incomplete installations:

- **Leaving `model/` out or flattening it.** The VMAF models and the TransNetV2
  scene detection model must be in a `model` subdirectory beside `av1an.exe`.
  Without the required model, that scoring/detection path is unavailable.
- **Copying only the executable.** The configured release publishes ZIPs and a
  schema, not loose `av1an.exe` or runtime DLL assets. Keep the bundle together.

**Keep the runtime bundle together.** `ffms2.dll` is dynamically linked and
mandatory, even for `av1an --version`. The pinned FFMS2 5.0 MSVC x64 DLL imports
only Windows system DLLs; it does not need separate FFmpeg codec DLLs. Keep the
mandatory MSVC C/C++ runtimes and their bundled companions together; these are
app-local files, not a requirement to install the VC++ Redistributable on the
host. Release staging copies them from the selected installed Visual Studio
default-version x64 CRT redistributable and logs its exact source/version, never
from System32 or PATH. The pinned Windows ONNX Runtime links its C++ library
statically but still imports the bundled `DirectML.dll`. Keep any additional ORT
runtime DLLs produced by other selected builds. A missing process-loader
dependency can prevent Condor from starting at all.

The metric libraries (`libvmaf.dll`, `libvship.dll`, `fmetrics.dll`) are loaded
on demand; their absence or failure disables their native metric path. This
does not make FFMS2 or other process-loader dependencies optional. GPU metrics
still require a compatible GPU/driver (including the Vulkan loader for libvship);
bundling DLLs does not guarantee GPU availability. VMAF and TransNetV2 also need
their respective files in `model/`.

The configured Windows release CI gate checks the actual bundled PE imports, then
extracts the ZIP into clean scratch and runs `--version` plus native FFMS2 frame decoding without
the build directory on PATH. This describes the required gate, not a completed
native Windows run. Use the ZIP, not a loose executable.

#### Linux releases

When a release provides **`av1an-linux-x64.zip`**, download and extract the whole
archive into one directory — same layout as Windows, with `.so` libraries
beside the executable and the VMAF and TransNetV2 models in `model/`. The archive keeps the
executable bit on `av1an` and sets an `$ORIGIN` runpath for adjacent metric
libraries. After satisfying the host dependencies below, check `./av1an --version`.

**x86_64 only.** There is no aarch64 build: on arm64 hosts the executable fails
with `cannot execute: required file not found`, because the archive ships one
x86-64 ELF. Build from source on those machines.

The Linux executable is **not** self-sufficient: the host provides

* **`libffms2`**, which `av1an` links against directly. Without it the loader
  fails before the program starts (`error while loading shared libraries:
  libffms2.so.5`), even for `--version`:
  * Debian / Ubuntu: `apt install libffms2-5`
  * Fedora: `dnf install ffms2`
  * Arch: `pacman -S ffms2`
* **A compatible glibc runtime** — the current Ubuntu 24.04 build target implies
  glibc 2.39 as the baseline. Recheck the actual artifact if the runner changes;
  this is not a portable/musl build or support for older glibc hosts.
* **an encoder and a concatenator** from the list below, plus `mkvmerge` or
  `ffmpeg`.
* **VapourSynth and its plugins** only for VapourSynth inputs, for `xpsnr`
  (which has no native implementation), and as the fallback when a metric's
  native library is missing or fails. That fallback also needs the FFmpegSource
  plugin (`vs-ffms2`); FFMS2 inputs scored by `libvmaf`, `libvship` or
  `libfmetrics` need none.

The metric libraries are staged on the same Ubuntu 24.04 runner, but that alone
does not guarantee every metric, GPU driver or transitive runtime works wherever
`av1an` starts. The Linux archive smoke gate checks CPU metric discovery; GPU and
other platform acceptance must run in their own environments.

Run without a terminal — a CI job, or output redirected to a file — and `av1an`
skips the TUI on its own, printing JSON progress events on stdout.

For the unpublished Rust library, use the [Andean Condor local path dependency](index.md#library).

## External tools and optional metrics

Av1an uses several external tools for decoding, filtering, and encoding video. For a quick start, install the following:

* [Python][python-download] - Recommended for [VapourSynth][vapoursynth-download]
* [vs-jetpack][vsjetpack] - Installs [VapourSynth][vapoursynth] and a convenient collection of VapourSynth plugins and Python modules for scaling, denoising, debanding, deinterlacing, metrics, etc.
* [Vship][vship] - Native GPU metrics require the verified **5.1.2** ABI, known backend, required symbols and a usable device. A local DLL is not proof of compatibility. The configured Windows bundle includes `libvship.dll`; for source installs and the unavailable official AMD 5.1.2 archive, see [`av-metrics-vship`](../../av-metrics-vship/README.md#installing-libvship)
* [libvmaf][libvmaf] - Required only for `vmaf`. Use release **v3.2.1** (runtime version **3.2.0**); arbitrary newer or distro builds are not guaranteed compatible. The configured bundle includes the library and models; see the [ABI and install policy](../../av-metrics-vmaf/README.md#prerequisites)
* [fmetrics](../../av-metrics-fmetrics/README.md#installing-fmetrics) - Optional CPU metrics, with runtime ABI **0.1.0**, an immutable source pin and exact Zig **0.16.0** by default
* [TransNetV2][transnetv2] - The neural network behind `detect-scenes --method transnetv2`. The configured bundle includes its ONNX model in `model/`; for source builds, run [`andean-condor/scripts/install-transnetv2-model-windows.ps1`](../../andean-condor/scripts/install-transnetv2-model-windows.ps1) on Windows or [`andean-condor/scripts/install-transnetv2-model-linux.sh`](../../andean-condor/scripts/install-transnetv2-model-linux.sh) on Linux/macOS — either downloads the model into the current directory and prints the `TRANSNETV2_MODEL_PATH` to set
* At least one of the following encoder binaries: [aomenc][aom], [SvtAv1EncApp][svt-av1], [rav1e][rav1e], [avmenc][avm], [x264][x264], [x265][x265], [vvenc][vvenc], [FFmpeg][ffmpeg]
* Either [FFmpeg][ffmpeg] or [MKVToolNix][mkvtoolnix] for concatenating the encoded scenes

> [!TIP]
> Make sure binaries like [FFmpeg][ffmpeg], [mkvmerge][mkvtoolnix], or [SVT-AV1][svt-av1] are added to your PATH.


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
