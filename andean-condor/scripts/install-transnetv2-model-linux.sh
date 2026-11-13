#!/bin/bash
# .SYNOPSIS
#     Downloads the TransNetV2 ONNX model for neural scene detection.
#
# .DESCRIPTION
#     av1an never downloads the model at runtime: it searches for
#     `transnetv2.onnx` at, in order,
#
#       1. the `model_path` set on the TransNetV2 scene detection method,
#       2. $TRANSNETV2_MODEL_PATH,
#       3. model/transnetv2.onnx beside the av1an executable,
#       4. transnetv2.onnx beside the av1an executable,
#       5. the per-user cache directory.
#
#     This script downloads the model into the current directory and prints
#     the environment variable that points av1an at it. The download is
#     verified against a pinned sha256 before it is moved into place, so an
#     interrupted download can never pass for the real model.
#
#     Releases already ship the model at model/transnetv2.onnx beside the
#     av1an executable, where it is found with no environment variable set;
#     this script is for source builds and custom layouts.
#
# .ENVIRONMENT
#     DESTINATION              Where to place the model. Defaults to the
#                              current directory.
#     TRANSNETV2_MODEL_URL     Download source override.
#     TRANSNETV2_MODEL_SHA256  Expected digest override.
#
# .EXAMPLE
#     ./install-transnetv2-model-linux.sh
#
# .EXAMPLE
#     DESTINATION=/opt/condor/model ./install-transnetv2-model-linux.sh
set -euo pipefail

ModelUrl="${TRANSNETV2_MODEL_URL:-https://huggingface.co/elya5/transnetv2/resolve/main/transnetv2.onnx}"
ModelSha256="${TRANSNETV2_MODEL_SHA256:-c4d54a682bace32f25136ef83ca2c9d403e8f8193775efeb995172a0d95a8e0c}"
Destination="${DESTINATION:-$PWD}"

step() { echo "==> $1"; }

step 'Downloading the TransNetV2 model'
mkdir -p "$Destination"
partial="$Destination/transnetv2.onnx.part"
curl -sSfL --retry 3 --retry-delay 2 -o "$partial" "$ModelUrl"

step 'Verifying sha256'
# GNU coreutils ships sha256sum; macOS only has shasum.
if command -v sha256sum >/dev/null 2>&1; then
    digest="$(sha256sum "$partial" | awk '{print $1}')"
else
    digest="$(shasum -a 256 "$partial" | awk '{print $1}')"
fi
if [ "$digest" != "$ModelSha256" ]; then
    rm -f "$partial"
    echo "ERROR: the downloaded model does not match the pinned sha256." >&2
    echo "  expected $ModelSha256" >&2
    echo "  got      $digest" >&2
    exit 1
fi

mv -f "$partial" "$Destination/transnetv2.onnx"
echo "    $Destination/transnetv2.onnx  $(wc -c < "$Destination/transnetv2.onnx") bytes, sha256 verified"

echo
echo 'Done.'
echo "  Model : $Destination/transnetv2.onnx"
echo

cat <<EOF
Set this environment variable to use the install:

  export TRANSNETV2_MODEL_PATH='$Destination/transnetv2.onnx'

Add it to your shell rc to persist. Not needed when the model sits at
model/transnetv2.onnx beside the av1an binary, which is where a release
stages it; the scene detector searches there on its own. The model_path on
the TransNetV2 scene detection method in condor.json also wins over the
environment variable.

Verify with:

  av1an detect-scenes --method transnetv2
EOF
