# av-metrics-vship

Native [libvship] perceptual quality scoring — SSIMULACRA2, Butteraugli and CVVDP — with GPU acceleration.

libvship is the GPU-accelerated successor to the standalone Butteraugli and SSIMULACRA2 tools, and adds CVVDP. This crate binds its C API directly and scores frame pairs as they are decoded, so no temporary files and no distortion maps are involved.

Frames come from [`av-decoders`], so a clip can be read from Y4M, FFMS2, FFmpeg or a VapourSynth script without this crate knowing which.

## Features

- **Per-pair scores, straight from `Vship_ComputeHandler`.** Unlike the libvmaf binding, there is no sliding window and no flush: the value for a pair is final the moment the call returns.
- **One owned handler per scorer.** All metrics submit synchronously; no unused handler pool or round-robin dispatch is needed.
- **Temporal CVVDP is handled correctly.** It builds a single handler, sees pairs in index order, and calls `Vship_Reset` at an index discontinuity so a scene break is not scored as a motion event.
- **Finds libvship where you actually have it.** libvship ships as a VapourSynth plugin, not a standalone library, so discovery looks in `VSHIP_PLUGIN_PATH`, `VSHIP_LIB_DIR`, `<dir(VSSCRIPT_PATH)>/plugins` and `VAPOURSYNTH_EXTRA_PLUGIN_PATH` before falling back to the loader's own search path.
- **Optional dependency.** libvship is opened with `dlopen` at runtime, so the crate compiles on machines that do not have it and `is_available()` returns `false` rather than the build failing.
- **No `bindgen`.** The C surface is declared by hand, keeping the build free of a libclang dependency.

## Supported runtime ABI

Before any GPU probe or handler allocation, the loader requires version **5.1.2**
from [pinned commit 5a627933](https://codeberg.org/Line-fr/Vship/src/commit/5a627933274196219f782b3b74e13fad47cd6370)
and a known backend: HIP=0, CUDA=1, Vulkan=2 or CPU=3. Other versions or backend
integers return `VshipError::IncompatibleLibrary`; reinstall the pinned build.
An opened incompatible binary is not silently replaced. There is no override;
version admission is a bounded ABI policy, not attestation of modified binaries.

C statuses, returned version backends and score tags are raw C integers.
`VshipBackend::try_from(raw)` checks domain conversion; known backend names are
unchanged. Only status zero is success; `CallFailed` preserves every other code.
Successful computes with a wrong/unknown score tag return `InvalidScoreTag`
before interpreting scores. Input-only colorspace/init enums remain typed.
The `ffi` integration tests compile unmodified pinned public headers (MIT NON-AI license
included) and C ABI sinks without GPU SDKs. These are ABI tests, not GPU accuracy
tests; ordinary crate builds require neither a C compiler nor native headers.

## Usage

```rust
use av_decoders::Decoder;
use av_metrics_vship::{PoolMethod, VideoFormat, VshipConfig, VshipMetric, VshipScorer};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
// Requires libvship and a usable GPU at runtime; see "Prerequisites" below.
let mut reference = Decoder::from_file("reference.mkv")?;
let mut distorted = Decoder::from_file("distorted.mkv")?;

let reference_format = VideoFormat::from_details(reference.get_video_details());
let distorted_format = VideoFormat::from_details(distorted.get_video_details());
let mut scorer =
    VshipScorer::new(VshipConfig::new(), reference_format, distorted_format, None)?;

let scores = scorer.score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {})?;
let pooled = VshipScorer::pool(&scores, VshipMetric::Ssimulacra2, PoolMethod::Mean)?;
println!("SSIMULACRA2: {pooled:.4}");
# Ok(())
# }
```

`VshipScorer::new` takes the reference and the encode format separately, plus an optional `(width, height)` target that becomes libvship's `target_width` and `target_height`. The two sides need not share a format, but they must end up the same size: libvship does not resize implicitly, so a 1080p reference against a 720p encode needs the target set (see [Differing input sizes](#differing-input-sizes-need-an-explicit-target)).

The callback passed to `score_decoders` fires once per pair, which is useful for reporting progress while a long encode is being scored. Every returned [`FrameScore`] is complete at that point, so no flush follows the loop.

A scorer is neither `Send` nor `Sync`: construct, use and drop it on the same thread. Each scorer owns one opaque native handler and submits synchronously through `&mut self`.

Distinct scorers may live on different threads, but pinned libvship 5.1.2 writes
a shared global error object even when their native handles differ. This binding
serializes device probes, initialization, compute, reset and destruction across
its safe APIs, including each call's short diagnostic. Progress callbacks run
outside that lock. This limits concurrent native throughput across scorers;
relaxing it requires verified upstream thread-local or synchronized error state.
Scorer ownership and borrowed-plane lifetimes are unchanged.

Public `ffi::VshipApi` function pointers bypass this guard. Raw callers must
coordinate error-state access and prevent overlap with safe APIs themselves.
The lock does not coordinate independent native bindings, VapourSynth plugins or
duplicate crate versions. Short status messages do not read global error details;
the safe API does not retrieve `GetDetailedLastError`.

### Borrowed plane uploads

`PlaneSource<'a>` borrows storage through fallible `from_bytes`, `from_u16` or
`from_plane` adapters. `PlaneSet<'a>` remains a three-plane array; its private
geometry cannot be changed after validation. Raw callers must use unsafe
`from_raw_parts` with an **actual readable allocation length**, keeping that
allocation live throughout the borrow and synchronous native compute/upload.
No Rust byte slice is constructed over foreign uninitialized padding.

Width/height count samples; stride, origin and span count bytes. Uploads require
positive dimensions and stride, `stride >= width * bytes_per_sample`, and
`origin + (height - 1) * stride + row_bytes <= readable_len`. Inter-row padding
is uploaded; final-row trailing padding is **not** required. Bottom-up/negative
strides are rejected. Row pitch must be sample-aligned (`stride % bytes_per_sample == 0`),
including for raw constructors on every backend. Odd host byte origins remain valid.
Dimensions/stride must fit native C `int`; the HIP launch expression additionally
requires `width * height <= INT_MAX - 255` per plane.

Every plane on both sides must match its configured storage width before
submission counters, handler resets or compute change: 8-bit uses one byte,
9/10/12/14/16-bit uses two. A typed `Frame<u8>` cannot supply configured 10-bit
data. Chroma rounds up: 1919×1079 4:2:0 requires 960×540, not 959×539.
Callers tracking external frame gaps can use `submit_pair_raw_with_reset` to
request the same history reset only after both sides pass preflight; Av1an uses
this to preserve its existing discontinuity policy without resetting invalid pairs.

The prefix contract is pinned to Vship
[`5a627933` HIP upload](https://codeberg.org/Line-fr/Vship/src/commit/5a627933274196219f782b3b74e13fad47cd6370/src/HIP/gpuColorToLinear/vshipColor.hpp),
[Vulkan upload](https://codeberg.org/Line-fr/Vship/src/commit/5a627933274196219f782b3b74e13fad47cd6370/src/Vulkan/gpuColorToLinear/vshipColor.hpp)
and [HIP sample conversion](https://codeberg.org/Line-fr/Vship/src/commit/5a627933274196219f782b3b74e13fad47cd6370/src/HIP/gpuColorToLinear/anyDepthToFloat.hpp).
Although HIP handles odd wide pitch, this revision's
[Vulkan dispatch](https://codeberg.org/Line-fr/Vship/src/commit/5a627933274196219f782b3b74e13fad47cd6370/src/Vulkan/gpuColorToLinear/anyDepthToFloat.hpp#L26)
inverts the flag consumed by its
[sample shader](https://codeberg.org/Line-fr/Vship/src/commit/5a627933274196219f782b3b74e13fad47cd6370/src/Vulkan/gpuColorToLinear/shaders/anyDepthToFloat.slang#L48-L78).
Wide odd pitches are therefore conservatively rejected until a corrected Vulkan
dispatch is the supported minimum and odd-pitch uploads pass the real GPU gate.
Pure checks and C ABI sinks run without a GPU; they do not prove GPU upload
completion or metric accuracy. On a supported GPU host, run `just test-gpu`;
`gpu_ten_bit_scorer_rejects_u8_frame_without_submitting` checks typed rejection;
`gpu_padded_reference_upload_matches_packed_score` and
`gpu_padded_distorted_upload_matches_packed_score` compare each padded side with
packed uploads, using nonflat Y/U/V patterns and distinct padding sentinels.
All three fail, rather than skip, when required mode has no working runtime.

Check availability before constructing one, if the outcome is not already known:

```rust
use av_metrics_vship::{device_name, is_available, libvship_version};

if is_available() {
    let version = libvship_version().unwrap_or("unknown version");
    let device = device_name().unwrap_or_default();
    println!("libvship {version} on {device}");
}
```

### Choosing a metric

| Metric | Direction | Temporal | Per-pair score |
| --- | --- | --- | --- |
| `VshipMetric::Ssimulacra2` | higher is better | no | yes |
| `VshipMetric::Butteraugli` | lower is better | no | yes |
| `VshipMetric::Cvvdp` | higher is better | **yes** | a running mean over everything the handler has seen |

Butteraugli reports three norms per pair — `norm_q`, `norm_3` and `norm_inf` — of which `norm_q` is the one matching the configured `q_norm`. Fields the metric does not produce are `None` rather than zero, so a zero is never mistaken for a real score.

### CVVDP is accumulated, not per-frame

CVVDP is a video metric: libvship folds each frame into a single accumulator and
returns the running mean over everything it has seen so far. Upstream is explicit
that **only the last score should be used**, and every frame must still be
computed. Consequences for callers:

- **Do not average the per-frame values.** Averaging a sequence of prefix means
  over-weights the earliest frames. On a 756-frame clip the mean of the running
  means came out at 8.1455 where the final value is 9.0475.
- **Take the last value.** `VshipScorer::pool` with `VshipMetric::Cvvdp` returns
  the final cumulative value regardless of `PoolMethod`, without sorting the
  prefixes. Missing final values are errors, not replaced by earlier prefixes.
- **Use the source frame rate when known.** Omitting `with_fps` uses **60 fps**;
  explicit values must be finite and satisfy **0 < fps <= 100000**. Zero
  (including negative zero), negatives, NaN, infinities and larger rates return
  `InvalidConfiguration` before library loading, device probes or handler
  allocation, even with temporal processing disabled. Other metrics ignore FPS.
- **`reset_temporal` clears the frame history, not the score.** Per the C header,
  `Vship_Reset` *"empties temporal filter history (but not score accumulation)"*, so
  the running mean continues across a scene break. `reset_score` clears the
  accumulator as well, and is what a caller needs if each scene's score is to cover
  only that scene.

Measured against the VapourSynth plugin on the same clip and the same libvship
build, per-frame CVVDP agrees exactly once `fps` is supplied.

### Pooling

`VshipScorer::pool` assumes cumulative CVVDP (the native default) and returns its
last ordered value. Its static signature has no temporal-mode flag. For
SSIMULACRA2 and Butteraugli it still applies the requested `PoolMethod`:

| Method | Result |
| --- | --- |
| `PoolMethod::Mean` | Arithmetic mean over the frames that produced a value. |
| `PoolMethod::Min` | The lowest value. |
| `PoolMethod::Max` | The highest value. |

`Min` and `Max` are literal extremes rather than best and worst; which one is preferable follows `VshipMetric::higher_is_better`. Pooling a list with no values in it fails with `VshipError::InvalidConfiguration`.

For **independent CVVDP frames** (`with_disable_temporal(true)`), explicitly use
the existing `PoolMethod::apply` path, not `VshipScorer::pool`:

```rust
# use av_metrics_vship::{FrameScore, PoolMethod, VshipMetric};
# fn independent(scores: &[FrameScore]) -> Result<f64, av_metrics_vship::VshipError> {
let values: Vec<f64> = scores.iter().filter_map(|s| s.value(VshipMetric::Cvvdp)).collect();
PoolMethod::Mean.apply(&values) // Min and Max work here too.
# }
```

Raw submissions and callbacks still return every prefix. Andean's scene-level
consumers select the final value of each fresh temporal scene session; they do
not combine those scene results into a clip-wide CVVDP score.

### Configuration

| Builder | Default | Effect |
| --- | --- | --- |
| `with_metric` | `Ssimulacra2` | Which metric to compute. |
| `with_gpu_id` | auto | Device index. Unset by default, so a discrete GPU is chosen at run time; see [Device selection](#device-selection). |
| `with_handler_threads` | `4` | Stored legacy request, clamped to at least one. Ignored by synchronous scoring for every metric; `handler_count()` reports the actual count of one. |
| `with_q_norm` | `2` | Butteraugli's norm exponent. Must be positive. |
| `with_intensity_multiplier` | `203.0` | Butteraugli's peak display brightness in nits. Must be positive and finite. |
| `with_display_model` | `standard_fhd` | CVVDP's display model key. Must be non-empty. Fails on an interior NUL. |
| `with_display_model_json` | empty | Path to a JSON display-configuration override, or empty for none. Passed to libvship verbatim. |
| `with_resize_to_display` | `false` | Whether CVVDP scales both inputs to the model's display resolution. |
| `with_fps` | `60.0` | CVVDP frame rate. Must be finite and satisfy `0 < fps <= 100000`, including with temporal processing disabled. Explicit zero is invalid; non-CVVDP metrics ignore it. |
| `with_reset_on_discontinuity` | `true` | Separate source-gap policy: the selection owner requests a history-only reset before the post-gap frame, preserving accumulated score. |
| `with_disable_temporal` | `false` | Independent CVVDP frames: after each successful compute, clear history then accumulated score, matching the VapourSynth plugin. Reset errors propagate before returning the captured score. Does not change the gap policy. |

Source indices belong to the selection owner, not the scorer's submission count.
Andean classifies actual selected-index gaps with checked arithmetic; disabled
mode needs no extra gap reset. Direct raw callers can request a history-only
reset using `submit_pair_raw_with_reset`, which preflights both inputs first.
Native CVVDP values are returned unchanged; perceptual pooling is separate work.

The 60 fps default and inclusive upper bound follow the
[pinned plugin](https://codeberg.org/Line-fr/Vship/src/commit/5a627933274196219f782b3b74e13fad47cd6370/src/HIP/cvvdp/vapoursynth.hpp#L191-L195).
Unlike its metadata sanitization, the wrapper rejects invalid explicit input.
Andean always forwards the explicit source rate, retaining fractional FPS within
f32 precision; it never replaces invalid metadata with the low-level default.
The bound limits FPS-driven temporal ring growth, not total memory at arbitrary
resolutions. Frame-rate checks exercise real constructors/drivers with harmless C ABI
sinks to verify native FPS arguments and zero probes/allocations on rejection;
they do not allocate real high-FPS rings or prove GPU accuracy.

Andean's CVVDP plugin fallback also receives the original selected source indices.
With temporal processing enabled, it rejects gaps before plugin lookup/scoring:
the plugin cannot reset history alone while preserving accumulation. Use native
Vship for that gap policy, a contiguous selection, or independent frames with
`disable_temporal=true`. Native Vship's supported gap handling is unchanged.

The stateful C ABI checks prove reset ordering and ownership, not CVVDP
accuracy. Real native-vs-VapourSynth parity is covered by Andean's plugin-parity
test. Run it on a supported GPU/plugin host with:

```sh
AV_METRICS_REQUIRE_CVVDP_PARITY=1 cargo +stable test --profile ci -p andean-condor --test metrics metrics::plugin_parity::tests::cvvdp_native_and_plugin_scores_agree_with_temporal_processing_enabled_and_disabled -- --exact --nocapture
```

That required gate fails if the runtime/plugin is missing; ordinary suites report
it UNEXECUTED. Its provisional tolerance is not evidence of measured accuracy.

The default legacy request [`DEFAULT_HANDLER_THREADS`] remains 4 for compatibility. `handler_threads()` returns that stored request, and Av1an's `threads` setting still maps onto it, but neither creates workers nor changes the actual handler count of one. Metric option defaults are unchanged.

`submitted()` counts successful frames, including any required post-frame resets;
failed compute/reset calls do not advance it or invoke the score callback. Both
inputs pass preflight before native state changes. Public resets and raw submissions
with `reset_temporal=true` are native no-ops for stateless metrics.

`with_display_model` and `with_display_model_json` return `Result`, because both strings cross into C and an interior NUL cannot be represented there. `VshipConfig::validate` checks the remaining constraints without touching libvship, so an unusable configuration is reported before a device is claimed.

### Colour

`VideoFormat` describes each side separately, and `colorspace_from_details` maps an `av_decoders::VideoDetails` onto libvship's `Vship_Colorspace_t`:

- **Bit depth** becomes `Vship_Sample_t`. libvship's values are non-contiguous — 8, 9, 10, 12, 14 and 16 bits are enum values 2, 3, 5, 7, 9 and 11 — so every variant is declared with an explicit discriminant and nothing is converted sequentially. A depth with no enum value (11 or 13, say) is reported as `UnsupportedFormat` rather than guessed.
- **Chroma sampling** becomes `Vship_ChromaSubsample_t`: 4:2:0 is `{1, 1}`, 4:2:2 is `{1, 0}` and 4:4:4 is `{0, 0}`. Monochrome has no chroma planes at all, so it is rejected.
- **Color metadata** is preserved independently in each `VideoFormat`. Explicit full/limited range, supported matrices, chroma locations, transfer functions (including native PQ/HLG) and primaries override defaults. Values without a native equivalent return `UnsupportedFormat` before device discovery; reserved values never become BT.709 silently. BT.2020 SDR transfers map to native BT.709 as in libvship's FFMS2 adapter.
- Only absent/`Unspecified` values use defaults: limited range, left-sited chroma, BT.709 transfer/primaries, and BT.709 matrix above 650 lines (BT.601 otherwise). The family remains planar YUV; RGB input is rejected.
- **Av1an's `resolution`** becomes `target_width` and `target_height`. An absent target resolution is libvship's `-1` "do not scale" sentinel, which is why no plane is resized here — libvship scales each input internally.

`VideoFormat::colorspace` performs the same mapping from a `VideoFormat` alone, for callers that have geometry rather than a decoder.

av-decoders 0.12 populates metadata through FFMS2/FFmpeg. For VapourSynth, get the
backend with `get_vapoursynth_impl()`, then call its `get_extended_video_details()`
to read the first output frame's properties; ordinary decoder details omit them. Y4M color properties are
unknown. Mastering-display/content-light-level metadata and per-frame metadata
changes are not applied by this binding. `VideoFormat` adds the five optional
decoder color fields; struct-literal callers must supply them (or use `from_details`).
Unrecognized raw numeric metadata may already be collapsed to `None` by the
decoder; this API cannot distinguish that from an absent property.

### Device selection

Leaving `with_gpu_id` unset does not mean device 0. A machine with more than one
GPU usually pairs a discrete part with an integrated one, and the discrete part
is the one that can run these metrics at a useful rate, so libvship's
`integrated` flag is used to prefer it:

- **Discrete first.** The highest-indexed device reporting `integrated == 0` wins,
  which on a typical laptop or desktop is the only discrete GPU.
- **Integrated only as a last resort.** If every device reports integrated — an
  iGPU-only machine, or a desktop with no discrete part — the last device
  enumerated is used, so scoring still works.
- **Cached.** The choice is resolved once and kept in a `OnceLock`. A machine's
  GPU configuration cannot change while the process runs, so re-enumerating on
  every request would be pure waste.
- **Never used when `integrated` is unavailable.** A backend that does not fill
  the field reports zero, which reads as discrete, so such a build simply keeps
  the first device.

`VshipConfig::gpu_id` reports what was configured (`GPU_UNSET` when nothing was),
`has_explicit_gpu_id` distinguishes the two cases, and `gpu_id_or_default`
returns the resolved index. `VshipScorer::gpu_id` reports the device a scorer was
actually built for. The module-level `default_gpu_id()` exposes the same choice
on its own, as an `Option` that is `None` when libvship reports no devices.

### Differing input sizes need an explicit target

libvship performs **no geometry-equality check** between the two sides. It converts
each per its own colorspace and then fails `Vship_InitHandler` with
`DifferingInputType` (status 4) if the two results differ in size:

```text
Vship received 2 videos with different properties.
(Advice) verify that they have the same width, height and length
```

Scoring a 1080p reference against a 720p encode therefore requires naming the
common size explicitly, through either `target_width`/`target_height` on the
encode or on the source. FFVship avoids this by always telling libvship to scale
the encode to the source, which is the behaviour to copy; passing no target at
all (`-1`, the "do not scale" sentinel) leaves both inputs at their native sizes
and fails unless they already match.

## Native or VapourSynth plugin

Av1an ships both paths for SSIMULACRA2, Butteraugli and CVVDP and picks between them at runtime. The C API is preferred wherever `is_available()` is true, because it scores pairs as they are decoded rather than through a filter graph. The decision is `native_supported`, and it falls back to the plugin branch when the metric is one libvship does not implement, when no libvship or no usable device is found, or when native scoring fails at any point. Every fallback is logged rather than propagated, so behaviour on a machine without libvship is unchanged.

VMAF and XPSNR are unaffected: VMAF is served by the separate libvmaf binding and XPSNR by the `vszip` plugin, neither of which libvship provides.

## Prerequisites

libvship is **not** vendored, and it has no CPU fallback in the builds that are published — it is a GPU metric. Install it separately:

- [libvship](https://codeberg.org/Line-fr/Vship) **5.1.2** from the verified source pin;
  later releases are not admitted automatically. The loader also checks the
  backend and required symbols before GPU admission.
- A GPU and the driver matching the libvship build you installed: a Vulkan driver for the `vulkan` build, the NVIDIA CUDA runtime for `nvidia`, or the AMD ROCm runtime for `amd`.

> [!NOTE]
> FFMS2 is unrelated to libvship, which neither needs nor loads it. Condor links
> FFMS2 dynamically: its Windows archive ships `ffms2.dll` beside `av1an.exe`,
> and Linux requires the host `libffms2` shared library. These decoder dependencies
> are separate from this crate's optional metric library.

## Installing libvship

### Windows — no building required

libvship cannot be built with MSVC. The verified v5.1.2 release offers Vulkan and
NVIDIA binaries, but **no official AMD archive**. This crate loads the DLL at
runtime with `libloading`; no import library, headers or compiler are needed for
those binaries. Matching driver/runtime dependencies are still required.

From this crate's directory, [`scripts/install-libvship-windows.ps1`](scripts/install-libvship-windows.ps1)
first looks for an installed candidate and otherwise downloads a pinned binary:

```powershell
.\scripts\install-libvship-windows.ps1
```

The standalone installer stages downloads as `vship.dll`:

| Variant | Upstream asset | Runtime needed |
| --- | --- | --- |
| `vulkan` (default) | `libvship_VULKAN.dll` | any working Vulkan driver |
| `nvidia` | `libvship_NVIDIA.zip` → `libvship_NVIDIA.dll` | the NVIDIA CUDA runtime |
| `amd` | no official v5.1.2 archive/pin; use a compatible local build | the AMD ROCm runtime (`amdhip64_6.dll`) |

The configured Condor release bundle instead stages the Vulkan DLL as
`libvship.dll`; the loader accepts both names.

If an installed candidate is found, the script preserves it and the destination,
downloads nothing, and reports **no package installed**. Set `VSHIP_LIB_DIR` to
that directory if needed. File presence is not ABI or GPU proof: the loader still
checks version 5.1.2, backend, symbols and device capability.

With no candidate, the exact Vulkan/NVIDIA downloads are SHA-256 checked before
use. An unavailable pin (including AMD 5.1.2) fails before download/destination
creation, with local-install guidance. `-ExpectedSha256` authorizes independently
audited bytes for a deliberate version/variant URL; it does not bypass ABI
admission or make an absent official archive exist. Downloads use owned partial
files and private ZIP scratch, cleaned on failure/success without deleting a
pre-existing destination `dl` directory. See the [trust and pin-update guide](../.github/native-artifacts.md).

`vulkan` is the default because its only imports are `vulkan-1.dll` and `KERNEL32.dll`: it runs against any working NVIDIA, AMD or Intel Vulkan driver and needs no vendor redistributable. `nvidia` and `amd` do require the CUDA or ROCm runtime respectively.

Select a variant with `-Variant`, and install elsewhere with `-Destination`.

Setting the environment variable is left to you, as a script that mutates the machine's environment is not something to run by surprise. Either scope it to the session:

```powershell
$env:VSHIP_PLUGIN_PATH = "$PWD\vship"
```

or persist it for future shells:

```powershell
[Environment]::SetEnvironmentVariable('VSHIP_PLUGIN_PATH', "$PWD\vship", 'User')
```

On Windows, `VSHIP_PLUGIN_PATH` and `VSHIP_LIB_DIR` name **existing directories**,
not DLL files. Invalid nonempty overrides are ignored with a warning; empty ones
are unset. Relative directories are explicit selections and become absolute,
preserving Unicode. Keep private dependency DLLs beside libvship: qualified loads
search siblings before safe defaults (application directory, user-added DLL
directories, System32). Bare fallback searches only those defaults, not ordinary
PATH or implicit media CWD. An explicitly selected `.` still trusts CWD.
`VSSCRIPT_PATH=vsscript.dll` selects no directory; use a qualified DLL path such
as `./runtime/vsscript.dll` to discover its sibling plugins. Extra plugin paths
are explicit directories separated by `;`; empty entries are ignored and invalid
directories warn. No global DLL search settings change. Already-loaded modules,
OS redirection, and other components' DLL policy are not isolated or attested.
Linux loader flags and normal search behavior are unchanged. See the [required native Windows regression gate](../test-support/README.md).

Then confirm it works:

```powershell
cargo test -p av-metrics-vship
```

### Already using the VapourSynth plugin

An installed VapourSynth plugin can supply the native library, but does not prove
its C ABI or GPU compatibility. The plugin may sit in
`<vs package>/vapoursynth/plugins/libvship.dll`, outside the loader's normal search
path, so the environment-derived directories below are probed. The C API path is
preferred only when its own admission checks succeed.

### Linux and macOS

There is no distribution-agnostic libvship build to install. Upstream publishes release assets for Windows only, so on Linux the C API must come from a source build:

```sh
bash ./scripts/install-libvship-linux.sh
```

`-Backend` selects `vulkan` (default), `cuda` or `amd`. Vulkan is the default
because its only imports are `libvulkan.so.1` and libc/libstdc++/libm/libgcc_s,
so one build runs against any NVIDIA, AMD or Intel driver; the other two need the
CUDA and ROCm runtimes. `-Destination` chooses the install directory and `-Quiet`
suppresses the printed `export` line.

Run from this crate's directory. The installer verifies the immutable 5.1.2 source
commit before building and uses private owned scratch, including cleanup on early
failure. An existing destination `dl` directory is not used or removed.

Vulkan initialization is lazy in the pinned libvship 5.1.2, so the library can
be loaded on a machine without a Vulkan driver and report itself unavailable.

Set `VSHIP_LIB_DIR` or `VSHIP_PLUGIN_PATH` in the environment of the running
application to select that directory. Neither variable needs to be set when
compiling: no native library is probed or required at build time, and no
build-machine path is embedded as a search hint. If runtime discovery or device
admission fails, native scoring reports unavailability and callers can use the
VapourSynth plugin path instead.


## Discovery order

VapourSynth plugins are often outside the loader's search path. Runtime candidates are tried most specific first:

1. `VSHIP_PLUGIN_PATH` — the user's explicit override.
2. `VSHIP_LIB_DIR` — an alternative explicit runtime directory override.
3. `<dir(VSSCRIPT_PATH)>/plugins`, then `<dir(VSSCRIPT_PATH)>` — the VapourSynth runtime that owns the plugin, and its own directory.
4. Every entry of `VAPOURSYNTH_EXTRA_PLUGIN_PATH` — VapourSynth's own extra-plugin path list, `;`-separated on Windows and `:`-separated elsewhere.
5. The platform system directories (`C:\Windows\System32`, `C:\Windows\SysWOW64`).
6. The bare file names `libvship.dll`, `vship.dll` (or `.dylib` / `.so` elsewhere), so the OS loader's own search path applies as a last resort.

Each directory is tried under both names, because an installation may use either.
`ffi::library_candidates` returns the full list in this order, which is what the
discovery tests assert against. For bundled releases, Windows' loader includes
the application directory. Unix bare-name loading may need a runtime loader path
such as `LD_LIBRARY_PATH` on Linux or `DYLD_LIBRARY_PATH` on macOS pointing at the
staged library directory; copying a library beside the executable alone does not
add that directory to the Unix loader's search path.

On every platform, an absent, empty or bare-filename `VSSCRIPT_PATH` selects no
directory: `vsscript.dll` does not add the current directory or `./plugins` to
the candidates. To select local plugins, use a directory override or an explicitly
qualified script path such as `./runtime/vsscript.dll`. Windows qualifies that
directory to an absolute path; other platforms retain normal relative-path semantics.

## Availability

`is_available()` needs more than a successful `dlopen`: `Vship_GetVersion` must answer and `Vship_GPUFullCheck` must pass on a device. Since libvship 5.1.2, a missing Vulkan driver is reported through the C API instead of aborting during load. Missing runtime libraries for other backends can still prevent loading. Any unavailable library or device returns `false`, so callers can use the VapourSynth plugin path instead.

The resolved default device's **successful** `Vship_GPUFullCheck` is cached and
shared by free/associated availability and version calls and default scorer
constructors. Concurrent first requests perform one successful probe. Failed
capability checks may retry; they are not cached as permanent absence. Existing
library-loading and default-device selection caches are unchanged; this does not
add hotplug or enumeration-failure recovery.

An explicit `with_gpu_id(...)` always validates that index afresh, including
explicit device 0 or the default index. Every scorer still creates its own native
handler. Initialization and compute failures remain errors, independently of
cached availability, and do not invalidate a successful capability check.
`devices()`, `device_info()`, and `device_name()` may still query fresh metadata.
`VshipScorer::backend()` reports which backend the loaded build targets.

## Testing

```powershell
cargo test -p av-metrics-vship
```

Everything that does not need libvship always runs: Rust layout mirrors and literal
discriminant checks, colourspace-mapping units, and compiler-backed pinned-header
checks. Filesystem discovery and private native-call transaction checks live under
`ffi::tests::internal_integration`; discovery uses owned inputs without changing
the runner's environment or working directory. Native ABI fixtures retain
child-only environment settings to exercise the production loader.

The `support` test target registers shared fixture self-checks once, separating
pure generator rules from Y4M decoder integrations. Other consumers retain the
same generator without registering those checks again.

Tests that need libvship are **skipped cleanly** when `is_available()` is false. CI has no GPU and no libvship binary, so the skip is the expected outcome there rather than a failure.

All test input is generated in memory as a y4m byte stream and fed to `av-decoders`' always-available Y4M path. No media files are committed, and the suite runs anywhere, including environments without FFMS2 or FFmpeg installed.

## Benchmarks

```powershell
cargo bench -p av-metrics-vship
```

Criterion benchmarks cover throughput at several resolutions, cost per metric, and decoding alone for comparison. They are deliberately **not** run in CI: Criterion adds minutes per run and its output is noisy on shared runners, so enforcing a regression threshold would produce flaky failures. The scoring benchmarks skip themselves when libvship is unavailable.

## Licence

BSD-2-Clause-Patent, matching the rest of Av1an.

[libvship]: https://codeberg.org/Line-fr/Vship
[`av-decoders`]: https://github.com/rust-av/av-decoders
[`DEFAULT_HANDLER_THREADS`]: https://docs.rs/av-metrics-vship/latest/av_metrics_vship/constant.DEFAULT_HANDLER_THREADS.html
[`FrameScore`]: https://docs.rs/av-metrics-vship/latest/av_metrics_vship/struct.FrameScore.html
