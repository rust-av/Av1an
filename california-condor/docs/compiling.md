# Compiling Condor

You can natively build Condor on Linux and Windows. Cross-compilation is not supported.

The repository is a Cargo workspace: `california-condor` is the `condor` CLI, `andean-condor` is the Rust library it is built on, and `av-metrics-vmaf`, `av-metrics-vship` and `av-metrics-fmetrics` are the native quality-metric libraries.

## Compiling on Linux

To compile Condor from source, you need the following dependencies:

- [Rust](https://www.rust-lang.org) (version 1.97.0 or higher)
- [NASM](https://www.nasm.us)
- [clang/LLVM](https://llvm.org)
- [FFmpeg](https://ffmpeg.org)
- [VapourSynth](https://www.vapoursynth.com)
- [FFMS2](https://github.com/ffms/ffms2) development files — `condor` links FFMS2 statically, so the headers and import library are build inputs

On Arch Linux, you can install these dependencies by running

```sh
pacman -S --needed rust nasm clang ffmpeg vapoursynth ffms2
```

Installation instructions on other distros will vary. The CI runner uses the
Debian/Ubuntu names: `nasm patchelf libclang-dev libffms2-dev libvulkan-dev
meson ninja-build`.

After installing the dependencies, build the CLI:

```sh
git clone https://github.com/master-of-zen/Av1an && cd Av1an
cargo build --release --bin condor
```

The resulting binary is `./target/release/condor`.

If FFMS2 is not where the build expects it, point the build at it:

```sh
FFMS_LIB_DIR=/usr/lib/x86_64-linux-gnu FFMS_INCLUDE_DIR=/usr/include cargo build --release --bin condor
```

## Compiling on Windows

If you just want a current build of Condor that is newer than the last official release, you can find a pre-built binary of the current `master` branch at https://github.com/master-of-zen/Av1an/releases/tag/latest.

If you want to build the binary yourself, you will need the following dependencies:

- [Microsoft Visual C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools) - this is a dependency for Rust
- [The Rust toolchain](https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe)
- [VapourSynth](https://github.com/vapoursynth/vapoursynth/releases/latest) — the CI installs it from Python instead: `uv pip install vapoursynth==79`, which provides the headers and `VSSCRIPT_PATH`
- [NASM](https://nasm.us)
- [FFmpeg](https://ffmpeg.org)
- [LLVM](https://releases.llvm.org)
- [FFMS2](https://github.com/ffms/ffms2/releases) — headers and import library only; FFMS2 is linked into `condor.exe` statically

### FFMS2 setup

- Extract the MSVC build of FFMS2 (the CI uses `ffms2-5.0-msvc.7z`) to a directory of your choice.
- Create a new environment variable named `FFMS_LIB_DIR` and set it to that directory's `x64` folder.
  - (For example, set `FFMS_LIB_DIR` to `C:\ffms2\x64`)
- Create a new environment variable named `FFMS_INCLUDE_DIR` and set it to that directory's `include` folder.
  - (For example, set `FFMS_INCLUDE_DIR` to `C:\ffms2\include`)

Then, either clone the repository by running

```sh
git clone https://github.com/master-of-zen/Av1an
```

Or download and extract the [source code](https://github.com/master-of-zen/Av1an/archive/refs/heads/master.zip) manually.

Open a command prompt or PowerShell window inside the cloned repository/extracted ZIP folder and run `cargo build --release --bin condor`. If this command executes successfully with no errors, `condor.exe` will be in the folder `target\release`.

The release archive also needs the metric libraries and the VMAF and TransNetV2 models beside the executable; see [Installation](./installation.md) for the layout the released archives ship with.

The Windows build embeds `condor.exe.manifest` through `embed-resource`, which declares the executable `longPathAware`. Without it, paths longer than `MAX_PATH` fail inside VapourSynth with "Source path does not exist"; this happens automatically, so no extra step is needed.

## Configuration schema

The `$schema` URL in `condor.json` points at a release asset. Regenerate the schema from the configuration structs with

```sh
cargo run --release --bin generate-schema > california-condor/configuration.schema.json
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
