<#
.SYNOPSIS
    Downloads the TransNetV2 ONNX model for neural scene detection.

.DESCRIPTION
    av1an never downloads the model at runtime: it searches for
    `transnetv2.onnx` at, in order,

      1. the `model_path` set on the TransNetV2 scene detection method,
      2. `$env:TRANSNETV2_MODEL_PATH`,
      3. `model/transnetv2.onnx` beside the av1an executable,
      4. `transnetv2.onnx` beside the av1an executable,
      5. the per-user cache directory.

    This script downloads the model into the current directory and prints the
    environment variable that points av1an at it. The download is verified
    against a pinned SHA-256 before it is moved into place, so an interrupted
    download can never pass for the real model.

    Releases already ship the model at `model/transnetv2.onnx` beside
    `av1an.exe`, where it is found with no environment variable set; this
    script is for source builds and custom layouts.

.PARAMETER Destination
    Directory the model is placed in. Defaults to the current directory.

.PARAMETER ModelUrl
    Download source. Defaults to the pinned Hugging Face URL, or to
    `$env:TRANSNETV2_MODEL_URL` when that is set.

.PARAMETER ModelSha256
    Expected SHA-256 of the model. Defaults to the pinned digest, or to
    `$env:TRANSNETV2_MODEL_SHA256` when that is set.

.EXAMPLE
    .\install-transnetv2-model-windows.ps1

.EXAMPLE
    .\install-transnetv2-model-windows.ps1 -Destination C:\condor\model
#>
[CmdletBinding()]
param(
    [Parameter()]
    [string] $Destination = (Get-Location).Path,

    [Parameter()]
    [string] $ModelUrl = 'https://huggingface.co/elya5/transnetv2/resolve/main/transnetv2.onnx',

    [Parameter()]
    [string] $ModelSha256 = 'c4d54a682bace32f25136ef83ca2c9d403e8f8193775efeb995172a0d95a8e0c'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# The environment overrides keep the pins in one place with the CI staging
# (see .github/.env); a parameter beats both.
if ($env:TRANSNETV2_MODEL_URL) { $ModelUrl = $env:TRANSNETV2_MODEL_URL }
if ($env:TRANSNETV2_MODEL_SHA256) { $ModelSha256 = $env:TRANSNETV2_MODEL_SHA256 }

Write-Host 'Downloading the TransNetV2 model...' -ForegroundColor Cyan
New-Item -ItemType Directory -Force -Path $Destination | Out-Null

$model = Join-Path $Destination 'transnetv2.onnx'
$partial = "$model.part"
curl.exe -sSfL --retry 3 --retry-delay 2 -o $partial $ModelUrl
if ($LASTEXITCODE -ne 0) {
    Remove-Item $partial -ErrorAction SilentlyContinue
    throw "Failed to download $ModelUrl (curl exit $LASTEXITCODE)."
}

$hash = (Get-FileHash -Algorithm SHA256 $partial).Hash.ToLowerInvariant()
if ($hash -ne $ModelSha256) {
    Remove-Item $partial -ErrorAction SilentlyContinue
    throw "The downloaded model does not match the pinned SHA-256 ($ModelSha256); got $hash."
}

Move-Item -Force $partial $model
Write-Host ("  {0}  {1:N0} bytes, sha256 verified" -f $model, (Get-Item $model).Length)

Write-Host ''
Write-Host 'Done.' -ForegroundColor Green
Write-Host "  Model : $model"
Write-Host ''
Write-Host 'Set this environment variable to use the install:'
Write-Host ''
Write-Host "  [Environment]::SetEnvironmentVariable('TRANSNETV2_MODEL_PATH', '$model', 'User')" -ForegroundColor Yellow
Write-Host ''
Write-Host 'Those persist for future shells. For this session only, use:'
Write-Host ''
Write-Host "  `$env:TRANSNETV2_MODEL_PATH = '$model'"
Write-Host ''
Write-Host 'Not needed when the model sits at model/transnetv2.onnx beside av1an.exe,'
Write-Host 'which is where a release stages it; the scene detector searches there on'
Write-Host 'its own. The model_path on the TransNetV2 scene detection method in'
Write-Host 'condor.json also wins over the environment variable.'
Write-Host ''
Write-Host 'Verify with:'
Write-Host ''
Write-Host '  av1an detect-scenes --method transnetv2'
