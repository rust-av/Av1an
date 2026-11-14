# av-metrics-vmaf

Native VMAF scoring via [libvmaf], with opt-in CUDA acceleration.

VMAF (Video Multi-Method Assessment Fusion) is Netflix's perceptual quality metric.
This crate binds libvmaf directly and scores frame pairs as they are decoded, so no
temporary files or JSON logs are involved.

Frames come from [`av-decoders`], so a clip can be read from Y4M, FFMS2, FFmpeg or a
VapourSynth script without this crate knowing which.

## Features

- **Per-frame scores**, collected after native finalization. Frame pairs are
  submitted as they are decoded; no score files or JSON logs are written.
- **Opt-in CUDA when it works, CPU otherwise.** With the Cargo `cuda` feature,
  the backend is chosen by constructing a complete scorer, then reported via
  [`VmafScorer::backend`], so a
  silent fallback can never be mistaken for GPU acceleration.
- **Optional dependency.** libvmaf is opened with `dlopen` at runtime, so the crate
  compiles on machines that do not have it. Scoring reports
  [`VmafError::LibraryLoadFailed`] rather than failing the build.
- **No `bindgen`.** The C surface is declared by hand, keeping the build free of a
  libclang dependency.

## Usage

```rust
use av_decoders::Decoder;
use av_metrics_vmaf::{PoolMethod, VmafConfig, VmafScorer, VideoFormat};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
// Requires libvmaf at runtime; see "Building libvmaf" below.
let mut reference = Decoder::from_file("reference.mkv")?;
let mut distorted = Decoder::from_file("distorted.mkv")?;

let details = *reference.get_video_details();
let format = VideoFormat {
    width: details.width as u32,
    height: details.height as u32,
    bit_depth: details.bit_depth as u32,
    chroma_sampling: details.chroma_sampling,
};

let mut scorer = VmafScorer::new(VmafConfig::new().with_threads(4), format)?;
println!("scoring on the {} backend", scorer.backend());

let scores = scorer.score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {})?;
println!("VMAF: {:.3}", VmafScorer::pool(&scores, PoolMethod::Mean)?);
# Ok(())
# }
```

Scores are also emitted through the callback passed to
[`score_decoders`][`VmafScorer::score_decoders`], after successful finalization.
Stock temporal models and asynchronous workers do not guarantee interim score
readiness. `drain_scores(from)` therefore returns empty before `finish()`, without
querying libvmaf. Afterward it reads final scores in index order; advance the
caller-owned cursor by the returned length to avoid duplicate reads.

`finish()` returns the full final vector, including on repeated calls, but flushes
the native context only once on success. Further submissions are rejected before
copying. Flush errors retain their native function/status; final score query errors
retain `vmaf_score_at_index`, the submitted index and raw status. Native C logging
is disabled (`VMAF_LOG_LEVEL_NONE`); failures still propagate through Rust errors.

Av1an's `score_vmaf_frames_with_progress` reports accepted submissions separately
from real final score callbacks. Quality Check and Target Quality use submission
progress during decoding, without inventing scores or counting final scores twice.

## Cargo features

| Feature | Default | Effect |
| --- | --- | --- |
| `cuda` | off | Opt in to CUDA scoring and availability. Also requires CUDA-enabled libvmaf (`-Denable_cuda=true`) and verified allocator support. Without this feature, `Auto` and `CpuOnly` use CPU only; `RequireCuda` returns an actionable feature error before loading libvmaf. |

For example, build the wrapper with `cargo build -p av-metrics-vmaf --features cuda`,
or enable `features = ["cuda"]` on your `av-metrics-vmaf` dependency. Installing a
GPU or CUDA-enabled libvmaf does not enable the Cargo feature. The feature gates
scoring and `cuda_available()`, not raw FFI types or optional symbol lookup.
`cuda_available()` is false without the feature; even when true, device/model
compatibility still requires a complete scorer attempt.

## Prerequisites

libvmaf is **not** vendored. Install it separately:

- [libvmaf](https://github.com/Netflix/vmaf) release **v3.2.1**, whose verified
  archive/package runtime string is **`3.2.0`** (see the ABI policy below).
- Only when building libvmaf itself: a C toolchain, Meson and Ninja. Ordinary
  Rust crate builds need neither libvmaf, its headers, nor a C compiler.
- **CUDA only:** the NVIDIA CUDA Toolkit (tested upstream with CUDA >= 11) plus
  [`nv-codec-headers`](https://github.com/FFmpeg/nv-codec-headers).

### Supported runtime ABI

Before initializing CPU **or** CUDA, the loader requires the exact `vmaf_version()`
string `3.2.0`, verified against release v3.2.1 at
[`f85a853692a8c730d0270cd733c8bb30b5b93b7c`][pinned headers]. The tag and runtime
version differ: that release's [Meson project version][pinned version] is 3.2.0.
Unverified strings (including `3.2.1`, other releases, and git/dev suffixes), null,
and non-UTF8 versions return `VmafError::IncompatibleLibrary`.

Version alone is insufficient: newer development archives can still report
`3.2.0` while enlarging `VmafPicture` with color metadata. The loader also rejects
the presence of **any** of these known new-layout exports, without calling them:

- `vmaf_picture_convert_context_init`
- `vmaf_picture_convert`
- `vmaf_picture_convert_context_close`
- `vmaf_picture_wrap`

The [color/conversion change][color ABI] and [wrapping change][wrap ABI] establish
these markers. Backports exporting them are conservatively rejected too. This is
a bounded compatibility guard, **not binary attestation**: arbitrary modified
binaries that reuse versions and hide markers are unsupported. There is no bypass,
including `CpuOnly`. Reinstall the pinned release archive/package or point
`VMAF_LIB_DIR` at a compatible installation; the error identifies the observed
version/marker. An opened but incompatible library is not silently replaced.

`VmafPicture::pix_fmt` is a raw C integer; `VideoFormat::pixel_format` still returns
a typed domain value. Pool checkouts adopt an owning guard **before** checking
native format metadata. Unknown/unsupported formats are rejected and returned
exactly once, before any host copy/submission; failed fetch outputs remain unowned.
The actual pool format is validated, not overwritten with configured metadata.

## Building libvmaf

libvmaf's own prerequisites are Python >= 3.6, Meson >= 0.56.1, Ninja >= 1.7.1,
NASM >= 2.13.02 (x86 only) and `xxd`.

### Arch Linux

A distro package is an option only if it satisfies the [exact ABI policy](#supported-runtime-abi):

```bash
pacman -S vmaf
```

This installs `libvmaf.so` to `/usr/lib`, which the platform loader finds
at runtime. Arch's package is built **without** CUDA. Package upgrades are not
automatically compatible; use the pinned source below if the installed build is rejected.

### Debian / Ubuntu

`libvmaf-dev` is available in Debian unstable, but its version is not a compatibility
guarantee. On stable, install the build prerequisites, then use the pinned source
instructions below:

```bash
sudo apt-get install -y build-essential meson ninja-build nasm python3-pip xxd
```

### macOS

```bash
brew install meson ninja nasm
```

Then use the pinned source instructions below, not an arbitrary latest release.

### From source, any platform

```bash
# Use the immutable source archive, not a git checkout whose version has a suffix.
curl --fail -L https://github.com/Netflix/vmaf/archive/f85a853692a8c730d0270cd733c8bb30b5b93b7c.tar.gz -o vmaf-v3.2.1.tar.gz
tar -xzf vmaf-v3.2.1.tar.gz
cd vmaf-f85a853692a8c730d0270cd733c8bb30b5b93b7c
meson setup libvmaf/build libvmaf --buildtype release
meson install -C libvmaf/build
```

Useful Meson options:

| Option | Effect |
| --- | --- |
| `-Denable_cuda=true` | Enable CUDA. Requires `nvcc` and `nv-codec-headers`. |
| `-Denable_avx512=true` | Faster SIMD on supported CPUs. Requires NASM >= 2.14. |
| `-Denable_float=true` | Compile the floating-point feature extractors. |
| `-Denable_nvtx=true` | Emit NVTX ranges for Nsight profiling. |
| `-Denable_tools=false` | Skip the `vmaf` CLI, leaving just the library. |
| `-Denable_docs=false`, `-Denable_tests=false` | Trim the build further. |
| `-Denable_asm=false` | Drop the NASM CPUID assembly. |

### Windows — no building required

**libvmaf cannot be built with MSVC.** vcpkg's port declares
`"supports": "!windows"`, and that exclusion is accurate: `src/feature/integer_vif.h`
defines `__builtin_clz` under `#ifdef _MSC_VER`, which MSVC rejects, and several
headers `#include <pthread.h>`, which the MSVC CRT does not provide.

Fortunately **a prebuilt `libvmaf.dll` is published**, so nothing needs compiling.
Pick whichever route suits you:

- **[Manual install](#windows-manual-install-no-msys2)** — the installer downloads
  four pinned `.tar.zst` packages and extracts them with Windows' `tar`. No MSYS2
  toolchain, pacman or compiler is needed.
- **MSYS2** — `pacman -S mingw-w64-x86_64-vmaf` installs
  `mingw64/bin/libvmaf.dll` along with headers, an import library and `pkgconfig`.
  The current package must still satisfy the exact ABI policy; other package
  versions/environments are not automatically compatible with this binding.

Then point `VMAF_LIB_DIR` at the directory containing `libvmaf.dll`.

`VMAF_LIB_DIR` is a **directory**, not a DLL filename. On Windows, a nonempty
override must name an existing directory; invalid overrides are ignored with a
warning. Relative directories are deliberately resolved against the current
directory to absolute paths. Empty overrides remain unset. Unicode paths work.
Keep dependency DLLs beside libvmaf: qualified loads search that directory first,
then safe OS defaults (application directory, user-added DLL directories, System32).
Bare-name fallback uses only those defaults, not ordinary PATH or implicit media
CWD. Selecting `.` explicitly still trusts CWD. No global DLL search setting is
changed. Already-loaded modules, OS redirection, and other components' DLL policies
remain outside this isolation; it is not binary attestation. Linux loading is
unchanged. See the [required native Windows regression gate](../test-support/README.md).

> [!IMPORTANT]
> Every prebuilt Windows DLL is compiled **without CUDA**, so Windows gets CPU-only
> VMAF. libvmaf's CUDA build needs `nvcc` and `nv-codec-headers`, which are not part
> of the MinGW package.

### Windows manual install (no MSYS2)

This crate loads libvmaf **at runtime** with `libloading`, so you need only the
`.dll` files — not MSYS2's toolchain, not the import library, and not a compiler.
Four DLLs are enough, and all four come from MSYS2's ordinary binary packages:

| File | Provided by | Why it is needed |
| --- | --- | --- |
| `libvmaf.dll` | `mingw-w64-x86_64-vmaf` | the library itself |
| `libgcc_s_seh-1.dll` | `mingw-w64-x86_64-libgcc` | GCC unwinder, imported by `libvmaf.dll` |
| `libstdc++-6.dll` | `mingw-w64-x86_64-libstdc++` | C++ runtime, imported by `libvmaf.dll` |
| `libwinpthread-1.dll` | `mingw-w64-x86_64-libwinpthread-git` | pthreads, imported by `libvmaf.dll` |

The last three are hard imports of `libvmaf.dll`: without any one of them the
loader fails the module with Win32 error 126. Load errors retain attempted paths
and OS diagnostics, so check for missing sibling dependencies. The library's only other imports
are `kernel32.dll` and `msvcrt.dll`, which every Windows install already has.

MSYS2 lists the libstdc++ package with its `+` characters percent-encoded, so the
archive is `mingw-w64-x86_64-libstdc%2B%2B-<version>-<rel>-any.pkg.tar.zst`; the
literal `+` returns 404.

`tar` and `curl` are both in the base Windows install, so nothing else is required.
[`scripts/install-libvmaf-windows.ps1`](scripts/install-libvmaf-windows.ps1) does
the whole thing:

```powershell
.\scripts\install-libvmaf-windows.ps1
```

Run from this crate's directory. By default it installs into `.\libvmaf`, downloading
four pinned packages and all nine upstream models (~550 KB), then prints the
environment variables to set.
Each download is SHA-256 checked before extraction/use. Mirrors must serve the
same exact filename and digest; unavailable old packages fail rather than using
newer directory-index entries. Scratch extraction uses a private owned temporary
directory, cleaned on success or failure. Useful switches:

| Switch | Effect |
| --- | --- |
| `-InstallDir <path>` | Install somewhere other than `.\libvmaf` |
| `-LibvmafVersion <ver>` | Select the audited release pin; currently only `3.2.1` is accepted (also the default, or `VMAF_VERSION` if set) |
| `-ModelsOnly` | Refresh the models without re-downloading the DLLs |

Setting the variables is left to you, as a script that mutates the machine's
environment is not something to run by surprise. Either scope it to the session:

```powershell
$env:VMAF_LIB_DIR    = "$PWD\libvmaf\bin"
$env:VMAF_MODEL_PATH = "$PWD\libvmaf\model"
```

or persist it for future shells:

```powershell
[Environment]::SetEnvironmentVariable('VMAF_LIB_DIR',    "$PWD\libvmaf\bin",   'User')
[Environment]::SetEnvironmentVariable('VMAF_MODEL_PATH', "$PWD\libvmaf\model", 'User')
```

Then confirm it works:

```powershell
cargo test -p av-metrics-vmaf
```

`every_model_in_the_search_path_loads` reports which models the installed libvmaf
actually accepts, so a build that cannot use some of them is visible immediately
rather than at scoring time.

#### Which models work

All nine are downloaded, but only four work with the MSYS2 build:

| Model | Usable | How to select |
| --- | --- | --- |
| `vmaf_v0.6.1` | yes | default |
| `vmaf_v0.6.1neg` | yes | `features: ["neg"]` |
| `vmaf_4k_v0.6.1` | yes | `features: ["uhd"]` |
| `vmaf_float_v0.6.1` | **no** | needs `-Denable_float=true` |
| `vmaf_float_v0.6.1neg` | **no** | needs `-Denable_float=true` |
| `vmaf_float_4k_v0.6.1` | **no** | needs `-Denable_float=true` |
| `vmaf_b_v0.6.3` | **no** | not supported by libvmaf 3.2.1 |
| `vmaf_float_b_v0.6.3` | **no** | needs `-Denable_float=true` |

That is a property of the binary, not of the models. libvmaf's meson options
default `enable_float` to **false**, and there is no `enable_bound` option at all
in 3.2.1 — the BOUND extractors were removed, which is why `vmaf_b_v0.6.3` fails
with `could not read model from path` even though the file is present and valid.

The MSYS2 package is therefore built with neither `VMAF_feature_float_adm` nor
`VMAF_feature_bound_adm`, and libvmaf rejects the affected models with `EINVAL`
when it tries to register their extractors:

```console
> dumpbin /exports libvmaf.dll | findstr adm
VMAF_feature_adm2_score      VMAF_feature_adm_scale0    ... present
VMAF_feature_float_adm      VMAF_feature_bound_adm    ... absent
```

The four working models are all reachable by name through `VmafModel`, so
`features: ["neg"]`, `features: ["uhd"]` and their combination
`features: ["uhd", "neg"]` (which selects `vmaf_4k_v0.6.1neg`) resolve on a build
with no built-in models. `features: ["weighted"]` maps to `vmaf_b_v0.6.3` and
**will fail on every stock libvmaf build**, Windows or Linux — use
`features: ["uhd"]`, or pass a model path explicitly, instead.

> [!TIP]
> The script pins package versions for reproducibility. If the canonical host is
> unreachable, it falls back to the `mirrors.dotsrc.org` and `mirror.msys2.org`
> mirrors, which serve byte-identical packages. You will see which one answered:
>
> ```console
>   mingw-w64-x86_64-vmaf-3.2.1-1-any.pkg.tar.zst  <- mirrors.dotsrc.org
> ```
>
> To see what MSYS2 currently offers, list the repository index directly. Note
> that some mirrors refuse directory listings and answer only `403`, so this
> needs the canonical host or one that permits it:
>
> ```powershell
> (curl.exe -s -A "Mozilla/5.0" https://repo.msys2.org/mingw/mingw64/) -match "mingw-w64-x86_64-libgcc-[^"]+\.pkg\.tar\.zst" |
>   Select-Object -Last 1
> ```

> [!NOTE]
> `vmaf.exe`, the import library in `mingw64/lib/` and the headers are optional for
> ordinary crate builds and scoring. Development header verification below does
> require real headers; the installer currently copies only DLLs and models.

Downloaded hashes are audited known-byte pins, not upstream signatures or ABI
attestation. Locally installed libraries/models remain administrator/package-manager
trust. See the [native artifact trust and pin-update guide](../.github/native-artifacts.md)
before changing package/model pins; do not disable checks to install a later release.

Windows native runtime and header-oracle execution remain platform acceptance
gates; Linux verification does not establish Windows ABI execution.

### Models

**Not every libvmaf build includes the VMAF models.** Where they are missing,
`vmaf_model_load("vmaf_v0.6.1")` fails with `EINVAL` and no scoring is possible.
Arch's `vmaf` package and MSYS2's `mingw-w64-*-vmaf` are both built this way.

For stock models, a nonempty `VMAF_MODEL_PATH` selects exactly
`<directory>/<version>.json`, **even if the library has built-in models**. It is
one literal directory, not a model filename or a path list; spaces, Unicode and
path-list separators are not split. An empty value is treated as unset. A missing
directory/file or a native model-loading error fails with the selected path and
override context; no built-in or fallback model is tried instead.

Without that override, the crate first tries the built-in stock model, then looks
for the same `<version>.json` on disk if the built-in load fails. File search order:

1. `VMAF_MODEL_PATH`, if nonempty — exclusive selection, not a fallback hint
2. platform defaults: `/usr/share/model` (Arch), `/usr/share/vmaf/model`
   (Debian, Fedora), `/usr/local/share/model` and `/usr/local/share/vmaf/model`
   on Linux; `/opt/homebrew/share/vmaf/model` and `/usr/local/share/vmaf/model`
   on macOS; `%MSYS2_ROOT%\mingw64\share\vmaf\model` and the `ucrt64` equivalent
   on Windows
3. `model/` beside the running executable — this is what a release layout uses
4. Adjacent `model/` directories inferred from the loaded library or `VMAF_LIB_DIR`

Library-prefix `share/vmaf/model/` directories rank with the system locations in
step 2, ahead of every executable/library-adjacent `model/` directory.

System paths are searched **before** the executable-adjacent ones, so a stray
`vmaf_v0.6.1.json` in a shared or downloads-adjacent binary directory cannot
silently override the distribution's model. `VMAF_MODEL_PATH` is the deliberate
exception, since setting it is an explicit choice.

So for a release that ships `av1an.exe`, `libvmaf.dll` and `model/`, no
configuration is needed. Otherwise point `VMAF_MODEL_PATH` at the directory holding
`vmaf_v0.6.1.json`, or pass the file explicitly with `VmafConfig::with_model_path`.
An explicit per-run file path takes precedence over `VMAF_MODEL_PATH`.

The stock model is a single ~19 KB JSON file from the [libvmaf repository][models].
A model only works if the libvmaf build has the matching feature extractors — see
[which models work](#which-models-work) for why five of the nine are rejected by
the MSYS2 build.

> [!TIP]
> `ModelNotFound` names a failed explicit selection (and the native status when
> loading fails). Without an override, it lists the fallback directories.

> [!NOTE]
> Identical clips do **not** score 100. libvmaf's motion and temporal extractors
> compare consecutive frames, so frame 0 has no predecessor and scores lower — 97.43
> on the reference clip — while every later frame scores exactly 100. The pooled
> mean of an identical pair is therefore around 99.96. Verified against both
> libvmaf's own CLI and FFmpeg's `libvmaf` filter, which agree to six decimals.

If you would rather compile it yourself, MSYS2 with MinGW-w64 is the configuration
upstream documents and tests:

```bash
meson setup libvmaf/build libvmaf --buildtype release --default-library shared
meson install -C libvmaf/build
```

## Configuration

libvmaf is discovered only at runtime, in this order:

1. **`VMAF_LIB_DIR`** — a directory containing the library. Set this when libvmaf is
   somewhere non-standard on the machine running the binary.
2. Bare library names, using the platform loader's search path. Windows includes
   the application directory; on Unix, a bundled library may require a runtime
   loader path such as `LD_LIBRARY_PATH` on Linux or `DYLD_LIBRARY_PATH` on macOS.

```bash
# Set this in the environment of the application using the crate.
export VMAF_LIB_DIR=/opt/libvmaf/lib
```

No native library is probed or required at build time, and no build-machine path
is embedded as a search hint. Set `VMAF_LIB_DIR` when running the binary, not just
when compiling it. A missing or incompatible library is reported when used;
`is_available()` returns `false`.

### Frame selection

Score every frame you care about and let this crate return one score per submitted
pair, positionally. Both `score_frames` and `score_decoders` preserve position: if
libvmaf fails to produce a score for a frame, that is reported as
[`VmafError::MissingScore`] rather than skipped, because dropping an entry would
shift every later score into the wrong slot.

`VmafConfig::validate` rejects `n_subsample > 1`. libvmaf applies that as a gate on
the **spatial** extractors only:

```c
if (!(fex->flags & (VMAF_FEATURE_EXTRACTOR_TEMPORAL | VMAF_FEATURE_EXTRACTOR_PREV_REF)))
    if ((n_subsample > 1) && (index % n_subsample)) continue;
```

The temporal and motion extractors still run on every frame, so the frames in between
end up with only some of their features and report no score at all. To score fewer
frames, submit every *n*-th pair instead:

```rust
// Score every third frame by selecting every third pair.
let every_third: Vec<_> = pairs.iter().step_by(3).cloned().collect();
let scores = scorer.score_frames(&every_third)?;
```

See [Models](#models) for stock-model discovery: a nonempty `VMAF_MODEL_PATH` is
exclusive, while an unset/empty value allows built-in loading followed by
system-before-adjacent fallback. `with_model_path` is independent of that override.

Arch's `vmaf` package installs **all nine** models to `/usr/share/model`, which is
what the Docker image relies on, so Linux needs no download at all.

## Platform support

| Platform | libvmaf availability | VMAF support |
| --- | --- | --- |
| Linux (Arch) | `vmaf` package, ships all nine models | CPU |
| Linux (Debian/Ubuntu) | `libvmaf-dev` (unstable) or source | CPU |
| Linux with CUDA-enabled libvmaf | source, `-Denable_cuda=true` | CPU; CUDA attempted with Cargo `cuda` feature |
| macOS | `brew` or source | CPU |
| Windows | prebuilt DLL from MSYS2 packages, extracted manually | CPU |

GPU acceleration requires **the Cargo `cuda` feature and Linux with a CUDA-enabled
libvmaf**. See `BackendPreference` to require a backend or skip the CUDA attempt.

With that feature enabled, `Auto` selects CUDA only after context creation, both
picture pools, model load, required model features and both initial host-picture
checkouts succeed. A failed
attempt is fully released before fresh CPU setup, including reloading the same
model and registering its features again. `RequireCuda` returns the setup error
without retrying CPU; `CpuOnly` never initializes CUDA. Rejected optional additional
extractors still warn and continue. Required features, not model-name guesses,
decide compatibility; the model and scoring options are never substituted.

Rust copies only into the ordinary **host** picture pool. CUDA's separate
**device** ring is for libvmaf's own upload/extraction path, never a CPU row-copy
destination. In the audited [native setup and submission implementation],
`vmaf_cuda_preallocate_pictures` does not create the host pool used by
`vmaf_fetch_preallocated_picture`, so CUDA scorers configure both pools.

**libvmaf 3.2.1 workaround:** CUDA attempts set native `n_threads=0`. The threaded
submission branch returns before releasing translated device pictures; the native
serial path reaches that cleanup. CPU attempts/fallback keep the caller's requested
worker count, and the public configuration is not rewritten. This is not a claim
that native threaded CUDA is safe; remove the workaround only after verifying a
native fix.

CUDA setup also requires `vmaf_cuda_release` and an allocator-matched native
`free` before allocating any state. On Linux, `free` is resolved in process-global
scope, matching ordinary libvmaf allocator bindings, including coherent
`LD_PRELOAD` allocator overrides. Looking it up only in libvmaf's dependency scope
can select a different allocator. CUDA is unsupported for libvmaf loaded with
`RTLD_DEEPBIND` or built with symbolic/private allocator bindings; use `CpuOnly`
for those builds. This loader does not request deep binding and cannot detect
custom allocator bindings. Other platforms, including custom Windows
CUDA DLLs, currently fail closed: `Auto` uses CPU and `RequireCuda` reports the
missing cleanup support (or the missing Cargo feature if disabled). No Windows
process CRT, MSVCRT or UCRT is guessed; `CpuOnly` does not allocate CUDA state or
call CUDA cleanup.

The ownership contract was checked against [libvmaf 3.2.1's CUDA implementation]
and [state import/close implementation]: import copies the descriptor, after which
the native context alone releases its CUDA resources. Rust frees the original C
wrapper once; before import succeeds, Rust releases the unimported resources too.
Native release is not a wrapper deallocator. Returned state-init errors with an
allocated output are cleaned up; upstream driver assertions are not recoverable
Rust errors, and upstream CUDA function-loader allocations are not fixed here.

Which of the nine models a build can use depends on its feature extractors: the
`float` and BOUND models need options that no packaged build enables. See
[which models work](#which-models-work).

## Testing and benchmarking

**No media files are required.** All test content is generated in memory at runtime
by `tests/common`, as a deterministic y4m stream fed through `av-decoders`'
always-available Y4M path. Content comes from a fixed-seed generator, so runs are
byte-identical across platforms and machines.

```bash
cargo test -p av-metrics-vmaf --no-default-features
cargo test -p av-metrics-vmaf --no-default-features --features cuda
```

The `ffi` integration tests check the actual loader with isolated counted C fixtures: unsupported
versions and each same-version new-ABI marker must reach **zero** init, pool, or
fetch calls. The verified runtime still constructs and scores a marker pair.
Those runtime fixtures run on Unix and require `cc`; they do not execute real CUDA.

The test-only `ffi::tests::abi::real_headers_match_layouts_and_detect_offset_drift` compiles and
executes `tests/fixtures/headers.c` with a C11 GCC/Clang compiler (`CC`, default
`cc`). It compares all shared struct sizes, alignments, field offsets and widths,
including both nested geometries, against **actual public headers**, and compiles
function signature checks. A temporary changed real picture header must fail the
same comparison. Installed CPU headers are selected by `VMAF_INCLUDE_DIR` (parent
of `libvmaf/`) or Unix pkg-config's include directory. CPU packages omitting the
public CUDA header use the [unmodified pinned test copy][header provenance]; the
private release signature uses a documented upstream declaration excerpt.

Without headers/compiler this gate explicitly reports `UNEXECUTED`; setting
`AV_METRICS_REQUIRE_CPU=1` (as CPU CI does) makes missing prerequisites fail,
not silently pass. Explicit invalid `VMAF_INCLUDE_DIR` also fails. Normal crate
builds never compile the probe or download headers.

```bash
AV_METRICS_REQUIRE_CPU=1 cargo +stable test --profile ci -p av-metrics-vmaf --lib ffi::tests::abi -- --nocapture
AV_METRICS_REQUIRE_CPU=1 cargo +stable test --profile ci -p av-metrics-vmaf --test ffi -- --nocapture
```

For the analogous Windows header gate, extract `mingw64/include/libvmaf/` from
the **same pinned** `mingw-w64-x86_64-vmaf-3.2.1-1-any.pkg.tar.zst` used for the
DLL (or use that package's MSYS2 installation). Do not use unrelated latest headers.
No Windows pkg-config is assumed. With a C11 Clang/GCC executable on PATH:

```powershell
$env:VMAF_INCLUDE_DIR = "C:\path\to\extracted\mingw64\include"
$env:CC = "clang"
$env:AV_METRICS_REQUIRE_CPU = "1"
cargo +stable test --profile ci -p av-metrics-vmaf --lib ffi::tests::abi -- --nocapture
```

This compiles the portable C probe and exercises its drift check on that platform;
Windows/macOS execution and real GPU acceptance must still be run there.

Tests that require libvmaf skip themselves when it is unavailable, so the suite is
green without a native installation. On Unix, the ownership cases under
`scorer::ownership` compile a self-contained C ABI fixture with `cc` and run isolated child tests
through the public constructor, submission, and drop paths. It checks ownership
transfer without relying on caller-descriptor zeroing, worker-reference retention,
and failed fetch/read cleanup; it does not execute real CUDA or a native thread
pool. `native_cpu_threaded_matches_serial` separately exercises installed
libvmaf's CPU workers when available.

On Unix, the behavior-named tests under `backend::tests::internal_integration::cuda`
run isolated production `Context` paths
against a C ABI fixture, checking state-init, context-init, import and pool
failures, missing cleanup symbols, CPU fallback and successful teardown. It
counts wrapper allocations/frees separately from resource releases; it does not
execute a CUDA driver. On Linux, `ffi::tests::cleanup_uses_process_allocator_scope`
separately checks the production allocator resolver against a local decoy `free`
and an ordinary global relocation, comparing addresses without calling either.
Hardware acceptance still requires repeated real CUDA
construction/teardown while another independent GPU user remains active, with
no abort or disruption to that user, plus native sanitizer checks. Those gates
must be run on suitable hardware; passing the fixture does not establish them.

`scorer::tests::internal_integration::setup` checks both availability APIs and all three preferences at the native
constructor boundary, with complete symbols and late setup failures. Without the
feature, CUDA entry-point counters stay zero and required CUDA fails before even
calling a failing API loader. With the feature, CUDA selection and fresh CPU retry
retain the complete-attempt contract. Run both Cargo configurations above.

Its individually named full-constructor cases provide non-skipping C-boundary checks
for each setup failure under all three preferences: feature-enabled CUDA attempts
and fresh CPU retry, or feature-disabled CPU/rejection policy, plus model/picture
cleanup, separate host/device allocation and untouched device canaries. Their marker
scores prove submission/flush routing, **not** CPU/CUDA numerical equivalence.

Separate partial-constructor cases check owned partial JSON models,
file fallback, fresh CPU retry and CPU-only feature/fetch failures. The shared
fixture counts each allocation/destruction and checkout/return before close.
The two `blocked_second_constructor_fetch` cases reserve host slots, wait for a genuinely blocked
second fetch, then release the slots through a condition-variable handshake.
They check both successful construction and allocation failure after release,
without real GPU execution. Installed-CPU malformed-model checks additionally
exercise repeated parser errors followed by valid scoring; they are not sanitizer
or leak-detector evidence.

The real comparison gate is compiled in ordinary suites but explicitly reports
itself unexecuted without the environment flag. Run it on a supported CUDA host:

```bash
AV_METRICS_REQUIRE_CUDA=1 cargo +stable test --profile ci -p av-metrics-vmaf \
  --no-default-features --features cuda required_cuda_matches_cpu_on_same_clip -- --nocapture
```

Set `VMAF_TEST_MODEL` if needed. The gate uses the same freshly generated 12-frame
clip and model on CPU and required CUDA, checks backend selection and every score
within 0.01 absolute VMAF points to allow kernel rounding, and fails if CUDA is
missing or rejects the model. Hardware execution of this tolerance remains pending;
the ABI double cannot validate it.

A successful native submission consumes its picture pair. If acquiring the next
pair fails, discard the scorer and create a new one: retrying or reading partial
scores is rejected. Validation failures before submission retain the pair and do
not advance the frame index. Pool exhaustion may block until a picture returns;
it is not a reliable exhaustion error. Pool close can likewise block while any
checked-out pictures remain outstanding. Constructor cleanup returns held pictures
before destroying the model and closing the context, including on a failed second
fetch. JSON model errors can leave owned partial models, which are destroyed before
file fallback or CPU retry; failed context-init and picture-fetch outputs have
different native ownership contracts and are not adopted.

Not every libvmaf build embeds the stock VMAF models. When yours does not, point
`VMAF_TEST_MODEL` at a model JSON (for example `model/vmaf_v0.6.1.json` from the
libvmaf repository) and the scoring tests will use it:

```bash
VMAF_TEST_MODEL=/path/to/vmaf_v0.6.1.json cargo test -p av-metrics-vmaf
```

Benchmarks exist to guide local optimisation work:

```bash
cargo bench -p av-metrics-vmaf
```

They are deliberately **not** run in CI: Criterion adds minutes per run and its
output is noisy on shared runners, so enforcing a regression threshold would produce
flaky failures.

## License

BSD-2-Clause-Patent, matching [libvmaf]. Any binary redistributing libvmaf must also
carry its license text.

[libvmaf]: https://github.com/Netflix/vmaf
[pinned headers]: https://github.com/Netflix/vmaf/blob/f85a853692a8c730d0270cd733c8bb30b5b93b7c/libvmaf/include/libvmaf/picture.h#L38-L46
[pinned version]: https://github.com/Netflix/vmaf/blob/f85a853692a8c730d0270cd733c8bb30b5b93b7c/libvmaf/meson.build#L1-L14
[color ABI]: https://github.com/Netflix/vmaf/commit/0497a0f299706b3124241f6ffc7fffc81d5665eb
[wrap ABI]: https://github.com/Netflix/vmaf/commit/700124a4cc354ac555b49fe14e71e6d27af73a35
[header provenance]: tests/fixtures/headers/README.md
[models]: https://github.com/Netflix/vmaf/tree/v3.2.1/model "libvmaf model files"
[libvmaf 3.2.1's CUDA implementation]: https://github.com/Netflix/vmaf/blob/f85a853692a8c730d0270cd733c8bb30b5b93b7c/libvmaf/src/cuda/common.c#L104-L154
[state import/close implementation]: https://github.com/Netflix/vmaf/blob/f85a853692a8c730d0270cd733c8bb30b5b93b7c/libvmaf/src/libvmaf.c#L187-L195
[native setup and submission implementation]: https://github.com/Netflix/vmaf/blob/f85a853692a8c730d0270cd733c8bb30b5b93b7c/libvmaf/src/libvmaf.c#L161-L936
[`av-decoders`]: https://github.com/rust-av/av-decoders
[`VmafScorer::backend`]: https://docs.rs/av-metrics-vmaf/latest/av_metrics_vmaf/struct.VmafScorer.html#method.backend
[`VmafScorer::score_decoders`]: https://docs.rs/av-metrics-vmaf/latest/av_metrics_vmaf/struct.VmafScorer.html#method.score_decoders
[`VmafError::LibraryLoadFailed`]: https://docs.rs/av-metrics-vmaf/latest/av_metrics_vmaf/enum.VmafError.html
[`VmafError::MissingScore`]: https://docs.rs/av-metrics-vmaf/latest/av_metrics_vmaf/enum.VmafError.html
