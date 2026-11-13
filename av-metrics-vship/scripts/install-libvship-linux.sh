#!/bin/bash
# .SYNOPSIS
#     Builds libvship from source and installs the shared library for Linux.
#
# .DESCRIPTION
#     Installs libvship so av-metrics-vship can load it at runtime with
#     `libloading`. Unlike Windows, where each release publishes a prebuilt DLL
#     per backend, upstream publishes no Linux assets at all, so this compiles
#     from source.
#
#     `Backend` selects the compute backend. Vulkan is the default because its
#     only imports are libvulkan.so.1 and libc/libstdc++/libm/libgcc_s, so one
#     build runs against any working NVIDIA, AMD or Intel Vulkan driver and needs
#     no vendor redistributable. `nvidia` and `amd` require the CUDA and ROCm
#     runtimes respectively.
#
#     The library is installed by hand rather than via `make install`, which
#     depends on an `uninstall` target and would also symlink into
#     $(PREFIX)/lib/vapoursynth. Only the shared library is needed, because the
#     crate `dlopen`s it.
#
#     This script is standalone: it needs no repository checkout and nothing
#     beside itself. The release pipeline overrides the pins below with
#     environment variables instead of editing the defaults.
#
# .PARAMETER Destination
#     Where to place `libvship.so`. Defaults to `$PWD/vship`.
#
# .PARAMETER Backend
#     Which upstream build to compile: `vulkan`, `cuda` or `amd`.
#
# .PARAMETER Version
#     libvship release tag to build. Defaults to the pinned tag below.
#
# .PARAMETER Commit
#     The commit to build. Defaults to the pinned revision of `Version`.
#
# .PARAMETER Quiet
#     Suppress the environment-variable guidance printed at the end.
#
# .EXAMPLE
#     ./install-libvship-linux.sh
#
# .EXAMPLE
#     ./install-libvship-linux.sh -Backend cuda -Destination /usr/lib
set -euo pipefail

# Defaults, overridable from the environment so the release pipeline can pass
# its pins without editing this script.
Destination="$PWD/vship"
Backend="${VSHIP_BACKEND:-vulkan}"
Version="${VSHIP_VERSION:-v5.1.2}"
Commit="${VSHIP_COMMIT:-5a627933274196219f782b3b74e13fad47cd6370}"
Repository="${VSHIP_REPO:-https://codeberg.org/Line-fr/Vship.git}"
Quiet=0

while [ $# -gt 0 ]; do
    case "$1" in
        -Destination) Destination="$2"; shift 2 ;;
        -Backend)     Backend="$2";     shift 2 ;;
        -Version)     Version="$2";     shift 2 ;;
        -Commit)      Commit="$2";      shift 2 ;;
        -Quiet)       Quiet=1;          shift ;;
        -h|--help)    awk 'NR > 1 { if (/^#/) print; else exit }' "$0"; exit 0 ;;
        *)            echo "Unknown argument: $1" >&2; exit 2 ;;
    esac
done

case "$Backend" in
    vulkan) make_backend=Vulkan; runtime='any working Vulkan driver'; tool=clang++ ;;
    cuda)   make_backend=Cuda;   runtime='the NVIDIA CUDA runtime';   tool=nvcc ;;
    amd)    make_backend=HIP;     runtime='the AMD ROCm runtime';      tool=hipcc ;;
    *)      echo "Backend must be vulkan, cuda or amd, not '$Backend'." >&2; exit 2 ;;
esac

library=libvship.so

work="$(mktemp -d "${TMPDIR:-/tmp}/vship-build-XXXXXX")"
trap 'rm -rf "$work"' EXIT

step() { echo "==> $1"; }

require() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "$2" >&2
        exit 1
    }
}

require git  "git not found; it is needed to fetch a pinned commit."
require make "make not found; it drives the upstream Makefile."

if [ "$make_backend" = Vulkan ]; then
    require clang "clang not found; the Vulkan backend is built with clang++."
    if [ -z "${VULKAN_SDK:-}" ] && [ ! -f /usr/include/vulkan/vulkan.h ]; then
        echo "vulkan.h not found. Install vulkan-headers, or set VULKAN_SDK." >&2
        exit 1
    fi
else
    # hipcc or nvcc replaces clang++ as the compiler for these backends.
    require "$tool" "$tool not found; the $Backend backend needs the $runtime."
fi

# The tag is verified against a pinned commit because a release tag can be moved.
step "Fetching Vship $Version"
git clone --quiet --branch "$Version" --depth 1 "$Repository" "$work"

actual="$(git -C "$work" rev-parse HEAD)"
if [ "$actual" != "$Commit" ]; then
    echo "$Version is at $actual, not the expected $Commit. Rebase, or pass" >&2
    echo "-Commit to build a different one deliberately." >&2
    exit 1
fi

step "Building $library (BACKEND=$make_backend)"
# The .spv shaders are committed upstream and embedded by the `shaderEmbedder`
# target, so slangc is only needed to rebuild them, not to build the library.
make -C "$work" build "BACKEND=$make_backend"

mkdir -p "$Destination"
target="$Destination/$library"
install -m755 "$work/$library" "$target"

# A failed build can still leave a valid-looking file behind, so the API is
# checked rather than assumed. Counted into variables rather than piped to
# `grep -q`, which exits at the first match and leaves nm killed by SIGPIPE --
# under `pipefail` that is indistinguishable from "no match".
exports="$(nm -D --defined-only "$target" | grep -c ' T Vship_' || true)"
symbols="$(nm -D --defined-only "$target" | grep -c 'Vship_GetVersion' || true)"
if [ "$symbols" -eq 0 ] || [ "$exports" -lt 10 ]; then
    echo "$target exports $exports Vship_* symbols and no Vship_GetVersion." >&2
    echo "The build did not produce a usable library." >&2
    exit 1
fi

size="$(stat -c %s "$target")"

echo
echo 'Done.'
echo "  $library : $target ($size bytes, $exports exports)"
echo "  version  : $Version ($Backend, needs $runtime)"
echo "  commit   : $Commit"

if [ "$Quiet" -eq 0 ]; then
    cat <<EOF

Set this environment variable to use the install:

  export VSHIP_LIB_DIR='$Destination'

Not needed when the library sits beside the av1an binary, which is where a
release stages it; the crate searches there on its own.

Verify with:

  cargo test -p av-metrics-vship

Tests needing the library skip themselves when it is absent, so a pass alone
does not prove it loaded.
EOF
fi
