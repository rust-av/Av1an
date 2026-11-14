# Header provenance

`libvmaf/libvmaf_cuda.h` is unmodified from Netflix/vmaf release v3.2.1,
commit [`f85a853692a8c730d0270cd733c8bb30b5b93b7c`](https://github.com/Netflix/vmaf/blob/f85a853692a8c730d0270cd733c8bb30b5b93b7c/libvmaf/include/libvmaf/libvmaf_cuda.h).
CPU-only packages may omit it. The probe prefers installed public headers,
then uses this copy for CUDA declarations only. It requires installed
`picture.h`, `libvmaf.h`, `model.h`, `feature.h`, and generated `version.h`.

`cuda_release.h` contains the verbatim private `vmaf_cuda_release` declaration
from [`libvmaf/src/cuda/common.h`](https://github.com/Netflix/vmaf/blob/f85a853692a8c730d0270cd733c8bb30b5b93b7c/libvmaf/src/cuda/common.h#L70).
The full private header requires the CUDA SDK; this excerpt verifies only the
opaque-pointer function signature, not CUDA state internals or GPU behavior.
Both files retain their BSD+Patent license notices; [LICENSE](LICENSE) is copied
from the same upstream commit.

The release's [Meson project and soname](https://github.com/Netflix/vmaf/blob/f85a853692a8c730d0270cd733c8bb30b5b93b7c/libvmaf/meson.build#L1-L14)
are **3.2.0**, despite the v3.2.1 tag. The generated header reports 3/2/0.
The probe checks those macros and compares actual C sizes, alignments, offsets,
and field widths with Rust. This is a header oracle, not binary attestation.
Its negative fixture inserts `unsigned abi_offset_drift` after `VmafPicture.bpc`
in a temporary copy of the real header; the same comparison must detect it.
No installed headers are modified, and ordinary crate builds never use this probe.
