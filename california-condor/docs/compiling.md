# Compiling Condor

You can natively build Condor on Linux and Windows. Cross-compilation is not supported.

The repository is a Cargo workspace: `california-condor` is the `av1an` CLI, `andean-condor` is the Rust library it is built on, and `av-metrics-vmaf`, `av-metrics-vship` and `av-metrics-fmetrics` are the native quality-metric libraries.

Start at the root of a checkout that contains `california-condor/Cargo.toml` and
the workspace `Cargo.toml` on `main`. As of 2026-11-13 these Condor crates are
unpublished; the legacy Av1an `v0` branch or its `av1an.exe` release is not a
substitute for this source tree. The commands below do not clone or select a revision.
Use stable Rust **1.97.0 or newer** on both platforms.

## Compiling on Linux

To compile Condor from source, you need the following dependencies:

- [Rust](https://www.rust-lang.org) (version 1.97.0 or higher)
- [NASM](https://www.nasm.us)
- [clang/LLVM](https://llvm.org)
- [FFmpeg](https://ffmpeg.org)
- [VapourSynth](https://www.vapoursynth.com)
- [FFMS2](https://github.com/ffms/ffms2) development files — `av1an` links FFMS2 dynamically; the shared `libffms2` library is also required at runtime

On Arch Linux, you can install these dependencies by running

```sh
pacman -S --needed rust nasm clang ffmpeg vapoursynth ffms2
```

Installation instructions on other distros will vary. The CI runner uses the
Debian/Ubuntu names: `nasm patchelf libclang-dev libffms2-dev libvulkan-dev
meson ninja-build`.

After installing the dependencies, build the CLI:

```sh
cargo build --locked --release -p california-condor --bin av1an
```

The resulting binary is `./target/release/av1an`.

If FFMS2 is not where the build expects it, point the build at it:

```sh
FFMS_LIB_DIR=/usr/lib/x86_64-linux-gnu FFMS_INCLUDE_DIR=/usr/include cargo build --locked --release -p california-condor --bin av1an
```

## Compiling on Windows

The loose `av1an.exe` asset on the checked upstream `latest` release is legacy
Av1an (`v0`), not this rewrite.
Until a release offers `av1an-windows-x64.zip`, build this source tree; see
[Installation](./installation.md#windows-releases) for the conditional bundle layout.

If you want to build the binary yourself, you will need the following dependencies:

- [Microsoft Visual C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools) - this is a dependency for Rust
- [The stable Rust MSVC toolchain](https://www.rust-lang.org/tools/install) (1.97.0 or newer)
- [VapourSynth](https://github.com/vapoursynth/vapoursynth/releases/latest) — the CI installs it from Python instead: `uv pip install vapoursynth==79`, which provides the headers and `VSSCRIPT_PATH`
- [NASM](https://nasm.us)
- [FFmpeg](https://ffmpeg.org)
- [LLVM](https://releases.llvm.org)
- [FFMS2](https://github.com/ffms/ffms2/releases) — headers and import library for the build, plus the dynamically linked `ffms2.dll` beside `av1an.exe` at runtime

### FFMS2 setup

- Extract the MSVC build of FFMS2 (the CI uses `ffms2-5.0-msvc.7z`) to a directory of your choice.
- Create a new environment variable named `FFMS_LIB_DIR` and set it to that directory's `x64` folder.
  - (For example, set `FFMS_LIB_DIR` to `C:\ffms2\x64`)
- Create a new environment variable named `FFMS_INCLUDE_DIR` and set it to that directory's `include` folder.
  - (For example, set `FFMS_INCLUDE_DIR` to `C:\ffms2\include`)

Open a command prompt or PowerShell window at the workspace root described above:

```powershell
cargo build --locked --release -p california-condor --bin av1an
```

The resulting binary is `target\release\av1an.exe`.

The source-built executable also needs `ffms2.dll`, app-local MSVC runtimes from
the selected installed Visual Studio x64 CRT redistributable, `DirectML.dll` from
the pinned ORT build, and any other build-produced ORT runtime DLLs. Metric DLLs
are loaded on demand; VMAF and TransNetV2 need their models in `model/` or explicit
runtime overrides. Copying only `av1an.exe` is not an installation.
`package-release.ps1` requires Python. See [Installation](./installation.md) for
the configured bundle layout and the still-required native Windows gate.

The Windows build embeds `av1an.exe.manifest` through `embed-resource`, which declares the executable `longPathAware`. Without it, paths longer than `MAX_PATH` fail inside VapourSynth with "Source path does not exist"; this happens automatically, so no extra step is needed.

## Optional native metrics

These libraries are loaded at runtime, not required to compile the CLI. Use the
verified ABI pins: [VMAF v3.2.1 / runtime 3.2.0](../../av-metrics-vmaf/README.md#supported-runtime-abi),
[Vship 5.1.2](../../av-metrics-vship/README.md#supported-runtime-abi), and
[fmetrics 0.1.0](../../av-metrics-fmetrics/README.md#supported-runtime-abi).
Building fmetrics requires exact Zig **0.16.0** by default; the installers verify
its reported version before fetching/building. Follow each installer's pin policy,
not an arbitrary newer native release. See the [native artifact trust/update guide](../../.github/native-artifacts.md).

## Configuration schema

`build.rs` bakes the `$schema` URL into the CLI and generated schema. A clean exact
`v`-prefixed SemVer tag uses that tag's `configuration.schema.json`; nonexact,
dirty or untagged builds use `latest`. In GitHub Actions, a valid
`GITHUB_REPOSITORY` selects that repository's releases (including forks); other
builds default to `rust-av/Av1an`. This URL contract does **not** mean the asset has
been published, especially for this source branch or a build-only fork run.
Regenerate the schema locally from the same source revision with

```sh
cargo run --locked --release -p california-condor --bin generate-schema > california-condor/configuration.schema.json
```

## Documentation screenshots

The [Screenshots](screenshots.md) page is generated from the live interface,
not taken by hand: every image is rendered offscreen through the same
`TuiApp::render` code the terminal uses, so it cannot drift from the real
interface. To regenerate them after changing the TUI:

```sh
cargo run --release --features screenshots --bin gen_screenshots
```

This writes light and dark 1920x1080 AVIFs into
`california-condor/docs/media/tui` (gitignored). The `screenshots` feature keeps
the image/font/AVIF dependencies out of the shipping binary. Pick a specific
monospace face with `CONDOR_SCREENSHOT_FONT=/path/to/font.ttf` for byte-stable
output across machines.

The same render paths are exercised as an integration test — it renders every
screen in both themes in memory and asserts the dimensions, catching panics and
layout regressions in CI:

```sh
cargo test --features screenshots --test screenshots
```

GitHub Actions regenerates the media on every deploy of the documentation site
(see `.github/workflows/mdbook.yml`), so the published page always matches the
interface on `master`.
