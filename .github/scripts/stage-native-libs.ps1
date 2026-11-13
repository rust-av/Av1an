<#
.SYNOPSIS
    Stages the native libraries a Windows release needs.

.DESCRIPTION
    av1an.exe opens libvmaf, libvship and fmetrics with `dlopen`, so the ones it
    uses have to be beside the executable before packaging:

      libvmaf.dll          dlopened by av-metrics-vmaf; absence only costs VMAF.
      libgcc_s_seh-1.dll   \
      libstdc++-6.dll       |  imported by libvmaf.dll, which fails to load
      libwinpthread-1.dll  /   without all three.
      libvship.dll         dlopened by av-metrics-vship; absent costs the native
                           vship metrics, which fall back to the VapourSynth
                           plugin. The Vulkan build imports only vulkan-1.dll and
                           KERNEL32.dll, so it needs no vendor runtime.
      fmetrics.dll         dlopened by av-metrics-fmetrics; absent costs the CPU
                           fallback for SSIMULACRA2, Butteraugli and CVVDP on a
                           machine with no usable GPU.

    fmetrics has no prebuilt Windows release, so the crate's own install script
    compiles it from a pinned commit, which needs Zig on the runner.

    FFMS2 is dynamically linked: ffms2.dll is mandatory even for --version.
    Copy it from FFMS_LIB_DIR, the same pinned package used by ffms2-sys.
    The x64 5.0 MSVC package DLL imports only Windows inbox DLLs.
    The selected Rust/ORT MSVC build also needs app-local VC++ runtime DLLs,
    copied from the installed Visual Studio default x64 CRT redistributable.
    Cargo also stages ORT/DirectML DLLs when the selected build produces them;
    package-release.ps1 includes those root DLLs as byte files.

    The TransNetV2 scene detection model is fetched into `model/` beside the
    executable the same way the VMAF model JSONs are: the scene detector
    searches `model/transnetv2.onnx` relative to itself and never downloads
    anything at runtime.

.PARAMETER StageDir
    Directory the release is assembled in. Defaults to `target\release`.

.PARAMETER SkipVmaf
    Skip the libvmaf stage. Only useful when testing another stage alone.

.PARAMETER SkipModels
    Skip fetching the VMAF model JSON files.

.PARAMETER SkipFmetrics
    Skip building fmetrics. The stage then has no CPU fallback, and only the
    VapourSynth plugin path can score SSIMULACRA2, Butteraugli and CVVDP.

.PARAMETER SkipTransNetV2
    Skip fetching the TransNetV2 scene detection model.
#>
[CmdletBinding()]
param(
    [Parameter()]
    [string] $StageDir = 'target\release',

    [Parameter()]
    [switch] $SkipVmaf,

    [Parameter()]
    [switch] $SkipModels,

    [Parameter()]
    [switch] $SkipFmetrics,

    [Parameter()]
    [switch] $SkipTransNetV2
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# The Docker image, the Linux staging script and the install scripts all read
# this same file, so every entry point builds the same pins. Any value may also
# be overridden from the environment.
$CicdEnv = @{}
foreach ($line in Get-Content (Join-Path $PSScriptRoot '..\.env')) {
    if ($line -match '^\s*#' -or $line -notmatch '\S') { continue }
    if ($line -match '^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.+?)\s*$') {
        $CicdEnv[$Matches[1]] = $Matches[2]
    }
}

function Get-Pin([string] $Name) {
    <#
    .SYNOPSIS
        Returns one value from .github/.env, failing if it is absent.
    #>
    if (-not $CicdEnv.ContainsKey($Name)) {
        throw "Missing $Name in .github/.env."
    }
    return $CicdEnv[$Name]
}

# Some mirrors reject requests without a User-Agent with 403.
$UserAgent = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64)'

# Pinned so every run stages the same bytes.
$TransNetV2ModelUrl = Get-Pin 'TRANSNETV2_MODEL_URL'
$TransNetV2ModelSha256 = Get-Pin 'TRANSNETV2_MODEL_SHA256'
$VmafVersion = Get-Pin 'VMAF_VERSION'
if ($VmafVersion -ne '3.2.1') { throw "Verified VMAF package/model pins unavailable for $VmafVersion." }
$Msys2Packages = @(
    "mingw-w64-x86_64-vmaf-$VmafVersion-1-any.pkg.tar.zst",
    'mingw-w64-x86_64-libgcc-16.2.0-4-any.pkg.tar.zst',
    'mingw-w64-x86_64-libstdc%2B%2B-16.2.0-4-any.pkg.tar.zst',
    'mingw-w64-x86_64-libwinpthread-git-12.0.0.r747.g1a99f8514-1-any.pkg.tar.zst'
)
$Msys2Hashes = @{}
$Msys2PinNames = @('VMAF_PACKAGE_SHA256', 'MSYS2_LIBGCC_SHA256', 'MSYS2_LIBSTDCPP_SHA256', 'MSYS2_LIBWINPTHREAD_SHA256')
for ($i = 0; $i -lt $Msys2Packages.Count; $i++) {
    $Msys2Hashes[$Msys2Packages[$i]] = Get-Pin $Msys2PinNames[$i]
}

# Tried in turn: the primary mirror refuses connections on some networks.
$Msys2Repos = @(
    'https://repo.msys2.org/mingw/mingw64/',
    'https://mirrors.dotsrc.org/msys2/mingw/mingw64/',
    'https://mirror.msys2.org/mingw/mingw64/'
)

$VshipVersion = Get-Pin 'VSHIP_VERSION'
if ($VshipVersion -ne 'v5.1.2') { throw "Verified Vship Vulkan pin unavailable for $VshipVersion; use an independently audited pin." }

# Every model libvmaf ships. The first four are what the MSYS2 build accepts; the
# rest need a libvmaf built with -Denable_float=true or with the BOUND extractors.
$VmafModels = @(
    'vmaf_v0.6.1', 'vmaf_v0.6.1neg', 'vmaf_4k_v0.6.1', 'vmaf_4k_v0.6.1neg',
    'vmaf_b_v0.6.3', 'vmaf_float_v0.6.1', 'vmaf_float_v0.6.1neg',
    'vmaf_float_4k_v0.6.1', 'vmaf_float_b_v0.6.3'
)
$VmafModelBase = "https://raw.githubusercontent.com/Netflix/vmaf/v$VmafVersion/model/"

function Write-Step {
    param([Parameter(Mandatory)][string] $Message)
    Write-Host "==> $Message" -ForegroundColor Cyan
}

function Invoke-Download {
    <#
    .SYNOPSIS
        Downloads one URI, failing loudly with the curl exit code.
    #>
    param(
        [Parameter(Mandatory)][string] $Uri,
        [Parameter(Mandatory)][string] $OutFile,
        [Parameter(Mandatory)][string] $Sha256,
        [Parameter()] [int] $Retries = 3
    )

    if ($Sha256 -notmatch '^[0-9a-fA-F]{64}$') { throw 'An independently verified SHA-256 is required.' }
    $partial = "$OutFile.part.$([guid]::NewGuid().ToString('N'))"
    try {
        curl.exe -sSfL --proto '=https' --proto-redir '=https' -A $UserAgent --retry $Retries --retry-delay 2 -o $partial $Uri 2>$null
        if ($LASTEXITCODE -ne 0) { throw "Failed to download $Uri (curl exit $LASTEXITCODE)." }
        if ((Get-FileHash -Algorithm SHA256 -LiteralPath $partial).Hash -ne $Sha256) {
            throw "SHA-256 mismatch for $Uri; refusing to stage bytes."
        }
        $size = (Get-Item -LiteralPath $partial).Length
        Move-Item -LiteralPath $partial -Destination $OutFile -Force
        Write-Host ("    {0}  {1:N0} bytes, sha256 verified" -f (Split-Path $OutFile -Leaf), $size)
    } finally {
        if (Test-Path -LiteralPath $partial) { Remove-Item -LiteralPath $partial -Force }
    }
}

function Assert-Staged {
    <#
    .SYNOPSIS
        Fails when any named file is absent from the stage directory.
    #>
    param(
        [Parameter(Mandatory)][string[]] $Names,
        [Parameter(Mandatory)][string] $Directory
    )

    $missing = $Names | Where-Object { -not (Test-Path (Join-Path $Directory $_)) }
    if ($missing) {
        throw "Missing after staging: $($missing -join ', ')"
    }

    Write-Host "    all $($Names.Count) required files present" -ForegroundColor DarkGray
}

New-Item -ItemType Directory -Force -Path $StageDir | Out-Null

Write-Step 'Staging FFMS2 from the configured build package'
if (-not $env:FFMS_LIB_DIR) { throw 'FFMS_LIB_DIR must point to the pinned FFMS2 package x64 directory.' }
$ffms2 = Join-Path $env:FFMS_LIB_DIR 'ffms2.dll'
if (-not (Test-Path $ffms2 -PathType Leaf) -or (Get-Item $ffms2).Length -eq 0) {
    throw "Missing or empty FFMS2 runtime: $ffms2"
}
Copy-Item -LiteralPath $ffms2 -Destination (Join-Path $StageDir 'ffms2.dll') -Force

Write-Step 'Staging the selected Visual Studio x64 CRT redistributable'
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
if (-not (Test-Path $vswhere -PathType Leaf)) { throw "Required installed VS locator missing: $vswhere" }
$vsInstall = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if ($LASTEXITCODE -ne 0 -or -not $vsInstall) { throw 'vswhere could not locate the installed x64 MSVC build tools.' }
$vsInstall = $vsInstall.Trim()
$redistVersionFile = Join-Path $vsInstall 'VC\Auxiliary\Build\Microsoft.VCRedistVersion.default.txt'
$redistVersion = (Get-Content -LiteralPath $redistVersionFile -Raw).Trim()
if ($redistVersion -notmatch '^\d+\.\d+\.\d+(\.\d+)?$') { throw "Invalid VS redist version in $redistVersionFile" }
$redistArch = Join-Path $vsInstall "VC\Redist\MSVC\$redistVersion\x64"
$crtDirs = @(Get-ChildItem -LiteralPath $redistArch -Directory -Filter 'Microsoft.VC14*.CRT')
if ($crtDirs.Count -ne 1) { throw "Expected one selected x64 CRT redistributable directory in $redistArch" }
$crtDir = $crtDirs[0].FullName
Write-Host "    VS: $vsInstall; redist version: $redistVersion; CRT source: $crtDir"
$MsvcRuntimeNames = @('vcruntime140.dll', 'vcruntime140_1.dll', 'msvcp140.dll')
foreach ($name in $MsvcRuntimeNames) {
    $runtime = Join-Path $crtDir $name
    if (-not (Test-Path $runtime -PathType Leaf) -or (Get-Item $runtime).Length -eq 0) {
        throw "Missing or empty required MSVC runtime in selected VS redist: $runtime"
    }
}
# C++ runtime companions (e.g. msvcp140_2.dll/atomic_wait) come from this same
# CRT distribution, never System32 or PATH. Keep their mutually matching bytes.
foreach ($runtime in Get-ChildItem -LiteralPath $crtDir -File -Filter '*.dll') {
    if ($runtime.Length -eq 0) { throw "Empty MSVC CRT companion: $($runtime.FullName)" }
    Copy-Item -LiteralPath $runtime.FullName -Destination (Join-Path $StageDir $runtime.Name) -Force
}

if (-not $SkipVmaf) {
    Write-Step 'Fetching libvmaf and its runtime dependencies'

    $vmafStage = Join-Path $StageDir 'vmaf-stage'
    New-Item -ItemType Directory -Force -Path $vmafStage, (Join-Path $StageDir 'model') | Out-Null

    try {
        foreach ($package in $Msys2Packages) {
            $archive = Join-Path $vmafStage $package
            $failures = [System.Collections.Generic.List[string]]::new()

            foreach ($repo in $Msys2Repos) {
                try {
                    Invoke-Download -Uri "$repo$package" -OutFile $archive -Sha256 $Msys2Hashes[$package] -Retries 2
                    Write-Host "    $package <- $repo"
                    $failures.Clear()
                    break
                } catch {
                    $failures.Add($_.Exception.Message)
                }
            }

            if ($failures.Count -gt 0) {
                throw "Could not fetch $package from any MSYS2 mirror:`n  $($failures -join "`n  ")"
            }

            # Extract only the DLLs; the package also carries headers, an import
            # library and pkgconfig, none of which the runtime needs.
            tar -xf $archive -C $vmafStage 'mingw64/bin/*.dll'
            if ($LASTEXITCODE -ne 0) { throw "tar failed to extract $package." }
        }

        Copy-Item (Join-Path $vmafStage 'mingw64/bin/*.dll') $StageDir -Force

        Assert-Staged -Names @(
            'libvmaf.dll'
            'libgcc_s_seh-1.dll'
            'libstdc++-6.dll'
            'libwinpthread-1.dll'
        ) -Directory $StageDir
    } finally {
        # Staging space is only needed while the archives are in flight.
        if (Test-Path $vmafStage) { Remove-Item -Recurse -Force $vmafStage }
    }
}

Write-Step 'Staging libvship (existing candidate or pinned Vulkan package)'

$vshipStaging = Join-Path $StageDir 'libvship.dll'
if (Test-Path -LiteralPath $vshipStaging -PathType Leaf) {
    Write-Host "Preserving local candidate $vshipStaging; native runtime admission is still required, not a new hash-verified download."
} else {
    # Reuse the standalone installer's discovery/preservation and integrity boundary.
    $vshipWork = Join-Path $StageDir "vship-stage-$([guid]::NewGuid().ToString('N'))"
    try {
        $vshipCandidate = & ./av-metrics-vship/scripts/install-libvship-windows.ps1 -Destination $vshipWork -Version ($VshipVersion.TrimStart('v')) -Variant vulkan -ExpectedSha256 (Get-Pin 'VSHIP_VULKAN_SHA256')
        if ($vshipCandidate.Downloaded) {
            Write-Host '    staging the SHA-256 verified package'
        } else {
            Write-Host '    staging a local candidate, not a new hash-verified download; native runtime admission is still required'
        }
        Copy-Item -LiteralPath $vshipCandidate.Path -Destination $vshipStaging
    } finally {
        if (Test-Path -LiteralPath $vshipWork) { Remove-Item -LiteralPath $vshipWork -Recurse -Force }
    }
}
if ((Get-FileHash -Algorithm SHA256 -LiteralPath $vshipStaging).Hash -ne (Get-Pin 'VSHIP_VULKAN_SHA256')) {
    throw "Release Vship DLL $vshipStaging does not match the pinned $VshipVersion Vulkan SHA-256; refusing archive/publication. Local runtime reuse is separate from release integrity."
}

if (-not $SkipModels) {
    Write-Step 'Fetching VMAF models'

    $modelDir = Join-Path $StageDir 'model'
    New-Item -ItemType Directory -Force -Path $modelDir | Out-Null

    foreach ($model in $VmafModels) {
        Invoke-Download -Uri "$VmafModelBase$model.json" -OutFile (Join-Path $modelDir "$model.json") -Sha256 (Get-Pin "VMAF_MODEL_$($model.Replace('.', '_'))_SHA256")
    }
}

if (-not $SkipTransNetV2) {
    Write-Step 'Fetching the TransNetV2 scene detection model'

    $modelDir = Join-Path $StageDir 'model'
    New-Item -ItemType Directory -Force -Path $modelDir | Out-Null

    # Downloaded to a .part file and verified before it is moved into place, so
    # an interrupted fetch can never pass for the real model.
    $model = Join-Path $modelDir 'transnetv2.onnx'
    $partial = "$model.part"
    Invoke-Download -Uri $TransNetV2ModelUrl -OutFile $partial -Sha256 $TransNetV2ModelSha256

    if ((Get-FileHash -Algorithm SHA256 $partial).Hash.ToLowerInvariant() -ne $TransNetV2ModelSha256) {
        Remove-Item $partial -ErrorAction SilentlyContinue
        throw "The downloaded TransNetV2 model does not match the pinned SHA-256 ($TransNetV2ModelSha256)."
    }

    Move-Item -Force $partial $model
    Write-Host ("    transnetv2.onnx  {0:N0} bytes, sha256 verified" -f (Get-Item $model).Length)
}

if (-not $SkipFmetrics) {
    # The crate's own install script does the work, so there is one implementation
    # of the build rather than two that can drift apart.
    Write-Step 'Building fmetrics from source'

    # Hand the pins over in the environment; the install script keeps working
    # standalone when they are absent.
    $env:FMETRICS_COMMIT        = Get-Pin 'FMETRICS_COMMIT'
    $env:FMETRICS_REPO          = Get-Pin 'FMETRICS_REPO'
    $env:FMETRICS_BRANCH        = Get-Pin 'FMETRICS_BRANCH'
    $env:FMETRICS_UPSTREAM_BASE = Get-Pin 'FMETRICS_UPSTREAM_BASE'
    $env:FMETRICS_ZIG_VERSION   = Get-Pin 'FMETRICS_ZIG_VERSION'

    & ./av-metrics-fmetrics/scripts/install-fmetrics-windows.ps1 -Destination $StageDir
    if ($LASTEXITCODE -ne 0) {
        throw 'the fmetrics build failed.'
    }
}

Write-Step 'Verifying the staged release'

# Only what this run was asked to produce: the Skip switches exist so one stage can
# be exercised alone, and asserting on a skipped library made them always throw.
$required = @('ffms2.dll', 'libvship.dll') + $MsvcRuntimeNames
if (-not $SkipVmaf) {
    $required += @('libvmaf.dll', 'libgcc_s_seh-1.dll', 'libstdc++-6.dll', 'libwinpthread-1.dll')
}
if (-not $SkipFmetrics) {
    $required += 'fmetrics.dll'
}
if (-not $SkipTransNetV2) {
    $required += 'model/transnetv2.onnx'
}

Assert-Staged -Names $required -Directory $StageDir

Write-Host 'All native dependencies staged.' -ForegroundColor Green
