#!/bin/bash
# .SYNOPSIS
#     Stages the native libraries a Linux release needs beside the binary.
#
# .DESCRIPTION
#     The Linux mirror of stage-native-libs.ps1.
#
#     Three libraries travel with the release:
#       * libvmaf.so, from the distro `vmaf` package or the pinned source.
#       * libvship.so, built from a pinned commit; upstream publishes no Linux
#         binaries.
#       * libfmetrics.so, built the same way: the CPU fallback for SSIMULACRA2,
#         Butteraugli and CVVDP.
#
#     The TransNetV2 scene detection model is fetched into `model/` beside the
#     executable the same way the VMAF models are staged: the scene detector
#     searches `model/transnetv2.onnx` relative to itself and never downloads
#     anything at runtime.
#
#     FFMS2 is dynamically linked and requires the host libffms2 shared library.
#     VapourSynth is loaded at runtime only for inputs/paths that use it.
#
# .PARAMETER StageDir
#     Where to place the libraries. Defaults to `target/release`, beside the
#     binary cargo is about to produce.
#
# .EXAMPLE
#     ./.github/scripts/stage-native-libs.sh target/release
set -euo pipefail

# Load the shared pins (Vship, fmetrics) from the single CI/CD file so this and
# the Docker image cannot drift. The install scripts are standalone and pick
# these up from the environment.
# shellcheck source=/dev/null
set -a
. "$(dirname "${BASH_SOURCE[0]}")/../.env"
set +a

StageDir="${1:-target/release}"
DownloadVerified="$(dirname "${BASH_SOURCE[0]}")/download-verified.sh"
if [ "$VMAF_VERSION" != 3.2.1 ]; then
    echo "ERROR: no verified VMAF package/model pins for $VMAF_VERSION; audit the new version first." >&2
    exit 1
fi

step() { echo "==> $1"; }

# model/ is gitignored and Ubuntu ships no `vmaf` package, so the pinned set
# is fetched.
fetch_vmaf_models() {
    local destination="$1"
    local base="https://raw.githubusercontent.com/Netflix/vmaf/v${VMAF_VERSION:?VMAF_VERSION missing from .github/.env}/model"
    local models=(
        vmaf_v0.6.1 vmaf_v0.6.1neg vmaf_4k_v0.6.1 vmaf_4k_v0.6.1neg
        vmaf_b_v0.6.3 vmaf_float_v0.6.1 vmaf_float_v0.6.1neg
        vmaf_float_4k_v0.6.1 vmaf_float_b_v0.6.3
    )
    echo "No local VMAF models; fetching the pinned $VMAF_VERSION set"
    for model in "${models[@]}"; do
        local pin="VMAF_MODEL_${model//./_}_SHA256"
        bash "$DownloadVerified" "${!pin:?model SHA256 missing}" "$destination/$model.json" "$base/$model.json"
    done
}

step 'Staging VMAF models'
mkdir -p "$StageDir/model"

MODEL_DIR="${GITHUB_WORKSPACE:-$(pwd)}/model"
if [ ! -f "$MODEL_DIR/vmaf_v0.6.1.json" ]; then
    for candidate in /usr/share/model /usr/share/vmaf/model /usr/local/share/model; do
        if [ -f "$candidate/vmaf_v0.6.1.json" ]; then
            MODEL_DIR="$candidate"
            break
        fi
    done
fi

if [ -f "$MODEL_DIR/vmaf_v0.6.1.json" ]; then
    echo "Model directory: $MODEL_DIR"
    cp -v "$MODEL_DIR"/*.json "$StageDir/model/"
else
    fetch_vmaf_models "$StageDir/model"
fi

# The TransNetV2 model is downloaded, verified against the pinned sha256 and
# only then moved into place, so an interrupted fetch cannot pass for the
# real model.
step 'Staging the TransNetV2 scene detection model'
partial="$StageDir/model/transnetv2.onnx.part"
curl -sSfL --retry 3 --retry-delay 2 -o "$partial" "${TRANSNETV2_MODEL_URL:?TRANSNETV2_MODEL_URL missing from .github/.env}"
digest="$(sha256sum "$partial" | awk '{print $1}')"
if [ "$digest" != "${TRANSNETV2_MODEL_SHA256:?TRANSNETV2_MODEL_SHA256 missing from .github/.env}" ]; then
    rm -f "$partial"
    echo "ERROR: the downloaded TransNetV2 model does not match the pinned sha256." >&2
    echo "  expected $TRANSNETV2_MODEL_SHA256" >&2
    echo "  got      $digest" >&2
    exit 1
fi
mv -f "$partial" "$StageDir/model/transnetv2.onnx"
echo "    $StageDir/model/transnetv2.onnx  $(wc -c < "$StageDir/model/transnetv2.onnx") bytes, sha256 verified"

# libvmaf must share av1an's glibc; build the pinned source when the host
# ships none.
step 'Staging libvmaf'
build_libvmaf() {
    set -a
    . "$(dirname "${BASH_SOURCE[0]}")/../.env"
    set +a
    local work
    work="$(mktemp -d "${TMPDIR:-/tmp}/vmaf-build-XXXXXX")"
    trap 'rm -rf "$work"' RETURN
    bash "$DownloadVerified" "${VMAF_SOURCE_SHA256:?VMAF_SOURCE_SHA256 missing}" "$work/vmaf.tar.gz" \
        "https://github.com/Netflix/vmaf/archive/refs/tags/v${VMAF_VERSION:?VMAF_VERSION missing from .github/.env}.tar.gz"
    mkdir "$work/src" && tar -xzf "$work/vmaf.tar.gz" -C "$work/src" --strip-components=1
    # vmaf 3.x builds with meson, not make; float extractors serve the *_float_* models.
    meson setup "$work/build" "$work/src/libvmaf" --buildtype release --prefix "$work/install" --libdir lib -Denable_float=true -Denable_tests=false -Denable_docs=false -Denable_tools=false
    ninja -C "$work/build"
    ninja -C "$work/build" install
    cp -v "$work"/install/lib/libvmaf.so* "$StageDir/"
}

vmaf_path="$(ldconfig -p | awk -F ' => ' '/libvmaf\.so/ { if (!path) path = $2 } END { print path }')"
if [ -n "$vmaf_path" ]; then
    cp -v "$vmaf_path" "$StageDir/"
else
    build_libvmaf
fi

# Each install script pins its own upstream commit, the same default the image
# builds.
step 'Building libvship'
bash ./av-metrics-vship/scripts/install-libvship-linux.sh -Destination "$StageDir" -Quiet

step 'Building fmetrics'
bash ./av-metrics-fmetrics/scripts/install-fmetrics-linux.sh -Destination "$StageDir" -Quiet

# `ls` fails the step if any of the three is missing.
echo
echo 'Staged:'
ls -l "$StageDir"/libvmaf.so* "$StageDir"/libvship.so "$StageDir"/libfmetrics.so "$StageDir"/model/transnetv2.onnx
