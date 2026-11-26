# av-metrics-fmetrics

Native CPU perceptual quality scoring (SSIMULACRA2, Butteraugli, CVVDP, IW-SSIM, MS-SSIM)
via the [fmetrics](https://github.com/halidecx/fmetrics) C API.

## Features

- SSIMULACRA2, Butteraugli, CVVDP, IW-SSIM and MS-SSIM
- Frames come from `av_decoders`, so a clip can be read from Y4M, FFMS2, FFmpeg or a
  VapourSynth script without this crate knowing which
- Loaded with `dlopen` at runtime, so the crate compiles on machines without it and
  `is_available()` reports `false`
- Scores pairs as they are decoded, so no temporary files and no distortion maps

## Supported runtime ABI

The loader accepts only the exact `fmetrics_version_str()` value `0.1.0`, from
[pinned commit ae87c8e](https://github.com/BoatsMcGee/fmetrics/tree/ae87c8e5607063f0dcc8241b52cb143f6c9ad4ac).
Null, non-UTF8 and other versions return `FmetricsError::IncompatibleLibrary`
before any workspace/context allocation. Rebuild that revision and set
`FMETRICS_LIB_PATH` to its library directory. An opened incompatible binary is
not silently replaced by another candidate, and there is no compatibility override.
This is a bounded ABI policy, not attestation of arbitrary modified binaries.

Native statuses use integer-safe `FmetricsErr`: known associated constants keep
their names; `.0` replaces enum casts. `from_raw` preserves every code, and only
zero is success. Unknown errors retain the original integer in `CallFailed`.
Input-only image formats/colorspaces and display models remain typed enums.
The `ffi` integration tests compile vendored, unmodified public headers with a C11 compiler;
ordinary crate builds need no compiler, headers or fmetrics installation.

## Usage

```rust
use av_decoders::Decoder;
use av_metrics_fmetrics::{ColorInfo, FmetricsConfig, FmetricsMetric, FmetricsScorer};

let config = FmetricsConfig::new(FmetricsMetric::Ssimulacra2);

let mut reference = Decoder::from_file("reference.mkv")?;
let mut distorted = Decoder::from_file("distorted.mkv")?;

let reference_color = ColorInfo::from_details(reference.get_video_details())?;
let distorted_color = ColorInfo::from_details(distorted.get_video_details())?;
let mut scorer = FmetricsScorer::new(config, reference_color, distorted_color)?;

// Submit pairs as they are decoded; the caller owns frame lifetime.
let score = scorer.submit_pair(&reference_planes, &distorted_planes)?;
println!("SSIMULACRA2: {}", score.require(FmetricsMetric::Ssimulacra2)?);
```

### Choosing a metric

| Metric        | Direction        | Notes                                 |
| ------------- | ---------------- | ------------------------------------- |
| `Ssimulacra2` | higher is better | Perceptual, matches libvship's naming |
| `Butteraugli` | lower is better  | A distance, not a quality measure     |
| `Cvvdp`       | higher is better | Temporal; `cvvdp_jod` is the headline |
| `Iwssim`      | higher is better | Needs at least 16 px per dimension    |
| `Msssim`      | higher is better | Needs at least 16 px per dimension    |

`FmetricsMetric::prefers_lower_is_better()` states the direction, so pooling does
not have to be re-derived at each call site.

### Butteraugli norms

Low-level `FmetricsConfig` with no `butteraugli_pnorm` and raw
`FmetricsButteraugliOptions::default()` both use **finite p=3**, intensity 203.
This differs from Andean's `QualityMetric::BUTTERAUGLI { norm: None, .. }`, which
requests the plugin's **infinity norm**. fmetrics 0.1.0 has no infinity mode or
sentinel: `i32::MAX` is just a large finite exponent, not an approximation we use.
Andean excludes fmetrics for infinity and rejects direct fmetrics driver calls
with that request. Use libvship/the VS plugin, or explicitly select a finite norm;
without an infinity-capable runtime the job must fail, not substitute p=3.

Use `with_pnorm(3)` (or another positive `i32`) for a finite request. Both builder
and public-field configurations reject explicit zero/negative exponents before
library loading/workspace allocation. The pinned native reduction computes powers
at p, 2p and 4p; no smaller upstream maximum is specified. Large positive exponents
can overflow: a native OK with NaN or either infinity returns
`FmetricsError::NonFiniteButteraugli`, never a successful `FrameScore` or Andean
score callback. The workspace lease is returned on this error.

The finite-options routing matrix and real-C ABI sinks check these contracts, not native
Butteraugli numerical accuracy or GPU/plugin equivalence.

### CVVDP temporal modes

CVVDP defaults to **60 fps** when `with_frame_rate` is not called. Supply the
source rate when known; explicit values must be finite and satisfy
**0 < fps <= 100000**. Zero (including negative zero), negatives, NaN, infinities
and larger rates return `InvalidConfiguration` before library loading or native
allocation, including when temporal processing is disabled. Direct writes to the
public `frame_rate` field receive the same validation. Non-CVVDP metrics ignore it.
Andean always forwards its explicit source rate, including invalid values, rather
than replacing it with the default.

The default and inclusive bound follow the
[pinned Vship plugin](https://codeberg.org/Line-fr/Vship/src/commit/5a627933274196219f782b3b74e13fad47cd6370/src/HIP/cvvdp/vapoursynth.hpp#L191-L195).
Unlike the plugin's metadata sanitization, explicit invalid input is rejected.
The bound limits FPS-driven temporal ring growth; it does not guarantee enough
memory for every accepted rate and resolution.

```rust
# use av_metrics_fmetrics::{FmetricsConfig, FmetricsMetric};
let config = FmetricsConfig::new(FmetricsMetric::Cvvdp).with_frame_rate(24.0);
```

By default (`disable_temporal=false`), one context sees every submitted frame in
order and returns native accumulated JOD/quality values unchanged. Use
`with_disable_temporal(true)` for independent frames: after each successful
compute the wrapper clears **both history and accumulated score**, matching
Vship's VapourSynth `disableTemporal` behavior. Reset failures return errors, not
successful scores.

These ordered `FrameScore::cvvdp_jod` outputs are cumulative prefixes: the last
JOD is the whole submitted session's result. Do not average, minimize or maximize
the prefixes as though they were independent frame scores. `cvvdp_quality` stays
a separate native field, not a substitute for JOD. This crate has no sequence
quality-pooling API; callers reduce the returned scores. With temporal processing
disabled, the returned frame values are independent and may use the caller's
configured per-frame statistic. Andean's Quality Check/Target Quality consumers
use the final prefix of each fresh scene session while retaining all prefixes
for progress and saved vectors; direct clip-wide scoring still covers one session.

`with_reset_on_discontinuity(true)` is a separate, default-on source-gap policy;
neither builder changes the other. Selection owners know the actual source
indices. fmetrics 0.1.0 has **no history-only reset**: public `reset_temporal()`
clears accumulation too. Andean therefore rejects enabled-temporal CVVDP with
noncontiguous selections before context creation, scoring or callbacks, allowing
its existing backend fallback. Contiguous or disabled-temporal CVVDP still works.
The CVVDP plugin fallback cannot perform history-only gap resets either and
rejects that selection too; native Vship supports it. Neither fallback silently
changes the requested temporal semantics.
Low-level callers may explicitly choose `with_reset_on_discontinuity(false)` and
intentionally carry history across selected source gaps; the scorer itself has
no source indices. Do not use the combined reset as a history-only substitute.

Stateful C ABI tests prove wrapper/reset/driver behavior, not CVVDP numeric
accuracy. Frame-rate tests also check native FPS arguments and rejection before
native probes/allocations using harmless ABI sinks, not real temporal rings.
Real Vship-vs-plugin parity requires a supported GPU and plugin. On that host, run:

```sh
AV_METRICS_REQUIRE_CVVDP_PARITY=1 cargo +stable test --profile ci -p andean-condor --test metrics metrics::plugin_parity::tests::cvvdp_native_and_plugin_scores_agree_with_temporal_processing_enabled_and_disabled -- --exact --nocapture
```

Missing runtime/plugin fails that required gate; ordinary suites report it UNEXECUTED.

### Colour

fmetrics takes **interleaved RGB**, not planar YUV, so every pair is converted
before submission. That makes the conversion part of the measurement: a wrong
matrix silently changes every score. `ColorInfo::from_details` honors each input's
decoded range and BT.601, BT.709 or BT.2020 non-constant-luminance matrix.
`FmetricsScorer::new` takes a separate `ColorInfo` for each input. Pass the same
value twice only when the two inputs have the same color properties.

Only absent/`Unspecified` values use defaults: limited range, BT.709 above 650
lines (BT.601 otherwise), and sRGB transfer/BT.709 primaries. BT.709, SMPTE 170M,
and BT.2020 10/12-bit SDR transfers are interpreted using BT.1886 gamma 2.4,
linearized and encoded as sRGB. Existing sRGB and linear RGB retain their encoded
values and matching native tags. `ColorInfo::transfer` describes the source;
`ColorInfo::colorspace` describes the output. Only BT.709 primaries are supported;
other gamuts and unsupported transfers (including PQ/HLG) return
`UnsupportedFormat`. An HDR flag or display model is not an input decoder.
`colorspace_from_details` now returns a `Result`; `ColorInfo::colorspace` carries
the supported encoding through to the native image descriptor.

av-decoders 0.12 populates color metadata through FFMS2/FFmpeg. For VapourSynth,
get the backend with `get_vapoursynth_impl()`, then call its
`get_extended_video_details()` to read the first output frame's properties;
ordinary decoder details omit them. Y4M has no color metadata
in this decoder and therefore uses the defaults. These are clip-level properties;
per-frame changes and mastering-display/content-light-level rendering are not
implemented here. Chroma reconstruction remains nearest-cell sampling.
The metric converter uses published `yuvxyb` 0.6.0 (MIT, Rust 1.95 minimum), with
root default features disabled. Its transitive `yuvxyb-math` 0.1.1 still enables
its default `fastmath` polynomial powers; approximate math is **not** all disabled.
Independent numeric regressions cover the matrix, meaningful source depth and
SDR transfer conversion. FMA target features may change last-bit results in the
library's floating-point/fastmath path; high-depth SDR gray regressions allow
four u16 codes (<0.007% full scale) against standard gamma-2.4 + piecewise sRGB.
No dependency fork or alternate handwritten path exists.

yuvxyb clamps normalized luma to [0, 1] and chroma to [-0.5, 0.5] **before** the
matrix, including samples outside nominal limited range. Invalid stored codes
outside the meaningful source depth are rejected. Valid odd 4:2:0/4:2:2 input is
edge-padded for the library, then cropped to the original dimensions without
resizing. Source plane geometry and readable spans are checked before copying
into owned frames. `PlaneSource<'a>`/`PlaneSet<'a>` have private geometry and borrow
live storage: use fallible `PlaneSource::from_bytes`, `from_u16`, or
`PlaneSet::from_frame(&frame)`. Strides (including negative bottom-up strides)
and origins are **bytes**, even for u16. Zero geometry, unsupported sample sizes,
short strides/buffers, invalid origins and offset/native-width overflow return
`MalformedFrame`. `PlaneSet::new` groups checked planes; `yuv_to_rgb` is safe,
checks depth/storage/peak and chroma agreement, and returns owned `RgbImage`.
Unsupported color/depth/peak/library conversions return `UnsupportedFormat`.

`RgbView::new(&bytes, width, height, wide)` now returns a `Result`: packed RGB
must cover every row and fit native `c_uint` geometry. Wide RGB uses native-endian
u16 samples: borrowed byte slices must be u16-aligned or return `MalformedFrame`;
owned wide images use `Vec<u16>` to guarantee alignment for native uint16_t reads.
Their byte views retain native byte order, not a portable little-endian encoding.
Planar u16 input still supports unaligned origins via unaligned reads. `RgbSource` is sealed.
`as_img` on an image/view returns `BorrowedImg<'_>`, keeping storage borrowed
until its last use. `as_raw()` exposes the unchanged `repr(C)` `FmetricsImg` for
unsafe native interoperability, not for safe submission. Raw plane adapters use
`unsafe PlaneSource::from_raw_parts(pointer, actual_span_bytes, width, height,
stride_bytes, origin_bytes, sample_bytes)`; callers must prove the actual live
allocation, initialized visible samples and immutable lifetime. Checked required
extent is not proof of allocation length. Padding is never read or exposed as a
slice: Andean carries native allocation lengths and VS's guaranteed span through
the last visible row, excluding unpromised final padding. Borrowed views cannot
escape frame-owned callbacks.

TransNetV2 keeps its model-specific fractional box-average preprocessing; routing
it through an integer-only YUV interface would change quantization/filter order.
Unrecognized raw numeric metadata may already be collapsed to `None` by the
decoder; this API cannot distinguish that from an absent property.

Bit depth maps directly: 8-bit becomes `RGB_UINT8`, and 10- or 12-bit is
up-converted to `RGB_UINT16` rather than truncated, which would discard precision
the source has. Other depths are rejected rather than silently rescaled.

## Prerequisites

- Rust 1.97 or newer
- fmetrics, built with Zig 0.16.0

fmetrics is distributed as a source library rather than a prebuilt install.
The installers build immutable commit
`ae87c8e5607063f0dcc8241b52cb143f6c9ad4ac` from `BoatsMcGee/fmetrics`, carrying the
shared-library target and Windows portability patch absent upstream.
`FMETRICS_BRANCH=add-windows-shared-build` is descriptive only: moving or deleting
that branch does not select the source or invalidate an otherwise fetchable pin.
`-Commit` (or `FMETRICS_COMMIT`) must be a full 40-hex commit and HEAD must match it.
Windows additionally requires ancestry from upstream base
`e8bf3cfe06fb78f51864804b2f7003667d4c5a71` and
`patches/0001-windows-portability.patch` before compilation. A different pin/repository
is an explicit source-trust decision, not automatic ABI compatibility.

## Installing fmetrics

Unlike libvship and libvmaf there is no prebuilt Windows binary and no Linux
package to install, so this is a source build. It needs exactly **Zig 0.16.0** by
default and Git; 0.17 is refused because it removed `b.build_root`, which the pinned
`build.zig` uses.

Both installers require successful `zig version` output equal to the expected
pin before fetching source/dependencies, building or changing the destination.
`-ZigPath` on Windows (or `PATH` on Linux) selects the executable, not an exemption.
An administrator can deliberately set `FMETRICS_ZIG_VERSION` to another exact
0.16 patch; the 0.16 compatibility ceiling and exact-match check still apply.
That override is caller authorization, not proof the alternate toolchain works.
See the [native pin-update guide](../.github/native-artifacts.md#zig-build-toolchain).

Run the following scripts from this crate's directory. They use private owned
temporary checkouts and clean them on success/failure, without adopting an old
same-PID checkout or deleting a destination `dl` directory.

### Windows

```powershell
winget install --id zig.zig --version 0.16.0 --exact
.\scripts\install-fmetrics-windows.ps1
```

The script fetches a pinned commit, runs `zig build --fetch` to populate the
dependency tree, patches fcvvdp (it calls POSIX `sysconf()`, which does not
compile on Windows), builds `fmetrics.dll`, and installs it to `$PWD\fmetrics`.

`-Destination` chooses another directory; the script prints the
`SetEnvironmentVariable` line that points the crate at it.

On Windows, `FMETRICS_LIB_PATH` and `FMETRICS_LIB_DIR` name **existing directories**,
not DLL files. Invalid nonempty overrides are ignored with a warning; empty ones
are unset. Relative directories deliberately select a path relative to CWD and
become absolute, preserving Unicode. Keep private dependency DLLs beside fmetrics:
qualified loads search siblings before safe defaults (application directory,
user-added DLL directories, System32). Bare fallback uses only those defaults,
not ordinary PATH or implicit media CWD; selecting `.` explicitly still trusts CWD.
Failures retain attempted paths and original OS diagnostics, including missing
dependencies. No global DLL search settings change. Already-loaded modules, OS
redirection, and other components' DLL policy remain outside this isolation; it
does not attest binaries. Linux loading is unchanged. See the
[required native Windows regression gate](../test-support/README.md).

### Linux

Needs exact Zig 0.16.0 by default, Git and a C compiler:

```sh
bash ./scripts/install-fmetrics-linux.sh
```

The script fetches a pinned commit, builds the static library, and links
`libfmetrics.so` by hand — `zig build -Dshared=true` is broken on ELF and yields
a library exporting nothing.

`-Destination` chooses another directory; the script prints the `export` line
that points the crate at it. `-Quiet` suppresses that.

macOS is untested: the hand-link uses `--whole-archive`, a GNU ld and lld flag.

## Discovery order

At runtime the library is looked for in:

1. `FMETRICS_LIB_PATH`, then `FMETRICS_LIB_DIR` — explicit runtime directory overrides
2. The directory holding the running executable
3. Platform system directories
4. Bare names, letting the platform loader's own search path apply

Staging the library beside the executable is therefore enough. No native library
is probed or required at build time, and no build-machine path is embedded as a
search hint. Set the directory overrides in the running application's environment,
not just when compiling. A missing or incompatible library is reported when used;
`is_available()` returns `false`.

## Availability

```rust
# use av_metrics_fmetrics::is_available;
if !is_available() {
    // No fmetrics on this machine; scoring is unavailable.
}
```

Cheap to call: the library is resolved once and cached.

## Testing

```sh
cargo test -p av-metrics-fmetrics
```

Tests that need the library skip themselves when it is absent, so the suite passes
on a machine without it.

## Benchmarks

One example reports three numbers, because one cannot answer the question a
caller has:

- **end-to-end** — decode and score a pair as the scorer is actually driven.
  Comparable against another engine's end-to-end figure.
- **library only** — score pre-converted frames, so decode and conversion are
  out of the timed region. This is what says whether the library itself
  parallelises; end-to-end cannot, because decode does not.
- **conversion only** — the YUV to RGB conversion on its own, which is roughly
  half of fmetrics' per-frame cost and cannot be inferred from the other two.

```sh
# Everything, single-threaded.
cargo run --release -p av-metrics-fmetrics --example bench -- ref.mkv dist.mkv

# Through 12 threads, 200 frames per thread.
cargo run --release -p av-metrics-fmetrics --example bench -- ref.mkv dist.mkv 12 200
```

Scores are printed with the timings throughout: a speedup that changes the
numbers is not a speedup. `identical_to_serial` in the library-only section
asserts that concurrent scoring through distinct workspaces is bit-identical to a
serial run, which is the property the whole threading design rests on.

## Licence

BSD-2-Clause-Patent, matching the rest of the workspace. fmetrics itself is
Apache-2.0; redistributing its binary requires shipping its `LICENSE` and an
attribution notice.
