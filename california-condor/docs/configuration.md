# Configuration

Global flags and `condor.json` handling. See [guide](./guide.md) for workflow, [av1an](./commands/condor.md) for full-run flags, [type reference](./types.md) for complex types, and [Configuration](./configuration/index.md) for the `condor.json` schema.

For the full `condor.json` struct reference, see [Configuration](./configuration/index.md).

## Global Flags

| Name                                              | Flag            | Type   | Default              |
| ------------------------------------------------- | --------------- | ------ | -------------------- |
| [Config File](#config-file---config-file)         | `--config-file` | Path   | `./condor.json`      |
| [Temporary](#temporary---temp)                    | `--temp`        | Path   | Input file name hash |
| [Log File](#log-file---logs)                      | `--logs`        | Path   | `./logs/condor.log`  |
| [Verbose](#verbose---verbose)                     | `--verbose`     |        |                      |
| [Version](#version--v---version)                   | `-v`, `--version` |      |                      |

## Config File `--config-file`

Path to the configuration file.

### Default

If not specified, `./condor.json` in the current directory is used.

### Examples

- `> av1an --config-file ./deletemelater/config.json -i input.mp4 -o output.mkv`
- `> av1an init input.mp4 output.mkv --config-file ./config.json` (via `--config-file` global on `init`)

## Temporary `--temp`

Path to the temporary directory for scenes and intermediate encodes.

### Default

If not specified, a directory named with a hash of the input file name is created in the current working directory.

### Examples

- `> av1an --temp ./temp -i input.mp4 -o output.mkv`
- `> av1an --temp /mnt/working_bird_folder -i input.mp4 -o output.mkv`

## Log File `--logs`

Path to the log file.

### Default

If not specified, logs to `./logs/condor.log`.

### Examples

- `> av1an --logs ./deletemelater/condor.log -i input.mp4 -o output.mkv`

## Verbose `--verbose`

Enable verbose output and logging.

## Version `-v`, `--version`

Display encoder, VapourSynth plugin, and quality metric library installation
information. With `--verbose`, prints extended details.

### Quality Metric Libraries

The `Quality Metric Libraries` section reports `libvship`, `fmetrics` and
`libvmaf`, which are loaded at runtime and may be absent. It is listed
separately from the VapourSynth plugins because the two are independent:
libvship ships as the `libvship` VapourSynth plugin, so that plugin can be
marked installed while the native C API is still unloadable — and it is the
native library, not the plugin, that decides whether these metrics score on
the GPU.

Under `--verbose`, a loaded library also reports its version. libvship adds
its compute backend and device name, and libvmaf adds whether it was built
with CUDA support plus the presence of each stock VMAF model, since a libvmaf
without its models compiled in loads but cannot score.

### Compute Devices

When `--verbose` is used and libvship enumerated any devices, each is listed
under the library with the index that selects it:

```
✓ libvship: 5.1.0 (backend: Vulkan, device: NVIDIA GeForce RTX 4090)
  Compute Devices
  ----------------------------------------------
  [0] AMD Radeon Graphics (integrated)
  [1] NVIDIA GeForce RTX 4090, 24 GiB VRAM (default)
```

The index in brackets is the `gpu_id` to supply when the default is not the
device you want — a discrete GPU is normally preferred, but the default cannot
know that your machine has a better part for the job. `(default)` marks the
device used when none is configured.

Set it on the metric itself in `condor.json`, for any of the GPU metrics:

```json
"metric": {
  "SSIMULACRA2": {
    "target_range": [74.0, 76.0],
    "resolution": null,
    "threads": null,
    "gpu_id": 1
  }
}
```

Leaving `gpu_id` unset lets libvship choose, which is what it prefers. Note
that a VapourSynth `gpu_id` and the index above refer to the same libvship
device ordering.

## condor.json

Created by [init](./commands/init.md). Contains `input`, `output`, `temp`, and `condor` sequence state. Filters live on each input inside `condor`, not at the top level. Each step loads, updates, and saves it, enabling cancel and resume.

The file includes a `$schema` URL baked via `CONDOR_SCHEMA_URL` in `build.rs`:
clean exact `v`-prefixed SemVer builds use their own tag, while nonexact/dirty
builds use `latest`. GitHub Actions fork builds use the valid `GITHUB_REPOSITORY`;
local builds default to upstream. This selects a URL, not a guarantee that a
schema asset has been published. See [schema generation](compiling.md#configuration-schema).

### Result revisions and resume

Successful Target Quality probes, Quality Check measurements and finalized parallel
chunks carry a processing revision and structured input/settings context. Old JSON
still loads for inspection and offline edits. Settings-only jobs need no result
certification; saving or upgrading JSON never certifies old measurements or encodes.
Previous successful tests do **not** certify preexisting encodes, including results
from earlier fmetrics or CVVDP math/conversion behavior.

Execution refuses legacy, future-revision or changed-context results before run
handlers resave the job. It preserves original records, chunks and final output.
Changing a probe's source, filters, scene, encoder, metric, model or backend also
withholds its derived quantizer/chunk eligibility, even when encoding or concatenation
is called directly. Parallel Encoder still runs its configured decode/frame-count
checks on matching chunks. Saved-job concatenation checks provenance and regular,
nonempty chunk files without decoding every chunk a second time.
Target/budget-only edits retain raw TQ measurements for current-policy reselection;
dependent chunks must match that selection policy. Worker/retry changes do not
invalidate pixels. Speed Scaler records its own downstream preset transition so
original TQ measurements remain usable; arbitrary probe-option/preset edits are not
whitelisted. QC reference precedence is its explicit input, then the Parallel
Encoder input override, then the root input.

x264/x265 Target Quality now probes scenes independently to match final scene
encodes and avoid unsafe x265 zone retries, instead of sharing a zone-encoded probe
stream. This costs the grouped-probe optimization. Earlier x264/x265 probe records
lack the `independent-scenes-1` context
marker and cannot resume, including their derived selections and chunks. Preserve
the original files and recompute using the new-job or explicit-reset procedure
below; do not add the marker to old records. Other encoders retain revision-1
compatibility, and matching current x264/x265 records still resume.

Certified chunks that disappear or are truncated, malformed certified QC score
counts/values, and a missing previously measured QC output are **refused**, not
automatically withdrawn, repaired or remeasured. Records and files remain untouched;
use the explicit reset/new-job procedure below. A fresh QC job with no measurement
record still warns normally when its output is missing.

SerialEncoder and raw zone outputs do not issue saved-job chunk certificates.
Passing them to the saved-job `SceneConcatenator` sequence now refuses, even if
their frame counts match. This is an intentional Rust API behavior break; raw
caller-owned muxing remains separate from saved-job resume.

Prefer a new job with a different config and empty work directory when recomputing.
For an explicit metadata reset, inspect the affected scene first, then use the
applicable commands (replace `0` with its index):

```sh
av1an scenes 0 reset target-quality
av1an scenes 0 reset quality-check
av1an scenes 0 reset parallel-encoder
```

These reset **records only**; they do not remove physical chunks. Preserve or move
old chunks yourself before recomputing into an empty work directory. Do not copy a
new revision/context onto old results to bypass admission.

Source identity uses canonical path, length and modification time, not a content
digest: ordinary replacement is detected, but same-length edits preserving timestamps
are outside this non-adversarial check. Inline script text, arguments, declared
decoder/index settings and filters are recorded; hidden files read by scripts/plugins
cannot be inferred. VMAF custom/override model SHA-256 content identities and stock
fallback candidates are recorded; `VMAF_MODEL_PATH` remains exclusive. Native backend versions and plugin
file/function admission identities are checked, not universal native-binary attestation.
Plugins loaded only by a private script may conservatively refuse resume if their
eligibility cannot be established without evaluating that script. Provenance does
not certify frame callback order; the existing QC plugin arrival-order limitation
remains.

### External Validation

An editor with JSON Schema support can use `$schema` for completion and validation
when that asset is available. For unpublished source/fork builds, generate the
schema from the same checkout using the [local command](compiling.md#configuration-schema)
and point your editor at that file; a missing release asset is not a config error.
