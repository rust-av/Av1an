<#
.SYNOPSIS
    Downloads a prebuilt libvmaf and its VMAF models for Windows, without MSYS2.

.DESCRIPTION
    Installs libvmaf so av-metrics-vmaf can load it at runtime with `libloading`.
    Only the DLL files are needed: no MSYS2 toolchain, no import library, no
    headers and no compiler. `tar` and `curl.exe` both ship with Windows, so
    this script is self-contained.

    The four DLLs come from ordinary MSYS2 binary packages and must sit beside
    each other: `libvmaf.dll` imports the other three directly.

        libvmaf.dll          <- mingw-w64-x86_64-vmaf
        libgcc_s_seh-1.dll   <- mingw-w64-x86_64-libgcc       (GCC unwinder)
        libstdc++-6.dll      <- mingw-w64-x86_64-libstdc++   (C++ runtime)
        libwinpthread-1.dll  <- mingw-w64-x86_64-libwinpthread-git

    libvmaf's only other imports are kernel32.dll and msvcrt.dll, which every
    Windows install already provides. All nine upstream VMAF models are fetched;
    four are selectable by name through Av1an's `features` option.

.PARAMETER InstallDir
    Where to place `bin` and `model`. Defaults to `$PWD\libvmaf`.

.PARAMETER LibvmafVersion
    Audited release pin to fetch. Currently only 3.2.1 is accepted (runtime 3.2.0).
    Defaults to 3.2.1, or to `$env:VMAF_VERSION` when that is set. Other versions
    fail before files change; exact package/model hashes must be audited first.

.PARAMETER ModelsOnly
    Skip the DLL download and only fetch the models. Useful for refreshing
    models when libvmaf is already installed.

.EXAMPLE
    .\install-libvmaf-windows.ps1

.EXAMPLE
    .\install-libvmaf-windows.ps1 -InstallDir C:\tools\libvmaf -Verbose
#>
[CmdletBinding()]
param(
    [Parameter()]
    [string] $InstallDir = (Join-Path $PWD 'libvmaf'),

    [Parameter()]
    [string] $LibvmafVersion = '3.2.1',

    [Parameter()]
    [switch] $ModelsOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# Precedence: -LibvmafVersion, then $env:VMAF_VERSION (e.g. .github/.env), then
# the default in the param block.
if (-not $PSBoundParameters.ContainsKey('LibvmafVersion') -and $env:VMAF_VERSION) {
    $LibvmafVersion = $env:VMAF_VERSION
}
if ($LibvmafVersion -ne '3.2.1') {
    throw "Verified package/model pins unavailable for VMAF $LibvmafVersion. Audit the exact version before installing; no files changed."
}

# Standalone known-byte pins: no checkout or adjacent manifest is required.
# Keep these exact filenames/models in sync with .github/.env and native-artifacts.md.
$PackageHashes = @{
    'mingw-w64-x86_64-vmaf-3.2.1-1-any.pkg.tar.zst' = 'ae55f009377b3bd447a466109de70650e65296cff329d6bc699e6b74612e3f28'
    'mingw-w64-x86_64-libgcc-16.2.0-4-any.pkg.tar.zst' = 'd615f6a8536ca16b1f049daea1fa440a3b7a405449ec0e93ad6106db43682d54'
    'mingw-w64-x86_64-libstdc%2B%2B-16.2.0-4-any.pkg.tar.zst' = '3d4c3faf4c2c5c7a851ff12214ddcbf8c0d6df0964fc1d3ebd8c38f227183034'
    'mingw-w64-x86_64-libwinpthread-git-12.0.0.r747.g1a99f8514-1-any.pkg.tar.zst' = '38e0a33fc8b3b6b761d4958fa69afef6e0804e4ef7ea64018004da02f9b2294f'
}
$ModelHashes = @{
    'vmaf_v0.6.1' = '5950d61fa1f861bd45d8149d80539ed9f3376cfc2495b8f0fa8e9f57cb131ee3'
    'vmaf_v0.6.1neg' = '1fcda193aaa2a3136471b7516b3c5dc344546c05fcde2d58609f8c60a2387392'
    'vmaf_4k_v0.6.1' = '73b187001309703c89d57cf58baab01660bd11e4ea6fac62bc064c5f5da6dac8'
    'vmaf_4k_v0.6.1neg' = 'f92048bf94401a5b915f551eb248709275c61df5f20d062a3ae010dcf4d4e8fc'
    'vmaf_b_v0.6.3' = '34f620dbaff662fe7d33dd80326553c2cc3910e4135349ba4c367171d3a3cfbd'
    'vmaf_float_v0.6.1' = '26573aa333346f1109226e7283969e8b3581119a1ab7f18897767351dd963ff8'
    'vmaf_float_v0.6.1neg' = '8ac8e9bd6153e57fed4e9809c49711ea7654da2a14ea701423187905e914597a'
    'vmaf_float_4k_v0.6.1' = '2af34f3068174720a35126807dbe8d0b19c5143fee328a9fc2331c15c54e4433'
    'vmaf_float_b_v0.6.3' = 'e32df73119386b124836910956fe8e0b0583a35638d2c518719e5a5069d0248b'
}

# MSYS2's canonical host first, then mirrors carrying the same tree, tried in
# turn: the primary refuses connections on some networks.
$Repos = @(
    'https://repo.msys2.org/mingw/mingw64/',
    'https://mirrors.dotsrc.org/msys2/mingw/mingw64/',
    'https://mirror.msys2.org/mingw/mingw64/'
)

# Some mirrors reject requests that carry no User-Agent with 403.
$UserAgent = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64)'

$ModelBase = "https://raw.githubusercontent.com/Netflix/vmaf/v$LibvmafVersion/model/"

# Only these exact archives have verified pins; do not substitute latest filenames.
$LibvmafPackage = "mingw-w64-x86_64-vmaf-$LibvmafVersion-1-any.pkg.tar.zst"
$RuntimePackages = @(
    'mingw-w64-x86_64-libgcc-16.2.0-4-any.pkg.tar.zst',
    # The `+` is percent-encoded: MSYS2 lists the file with an encoded name, and
    # the literal character 404s.
    'mingw-w64-x86_64-libstdc%2B%2B-16.2.0-4-any.pkg.tar.zst',
    'mingw-w64-x86_64-libwinpthread-git-12.0.0.r747.g1a99f8514-1-any.pkg.tar.zst'
)

# Every model libvmaf v3.2.1 ships. The four in the first group work with the
# MSYS2 build and map to a name in av-metrics-vmaf's `VmafModel`, so they resolve
# automatically. The rest need a libvmaf built with -Denable_float=true, or, for
# `vmaf_b_v0.6.3`, one that still has the BOUND extractors.
$Models = @(
    # Selectable by name through Av1an's `features` option.
    'vmaf_v0.6.1',        # default
    'vmaf_v0.6.1neg',     # features: ["neg"]
    'vmaf_4k_v0.6.1',     # features: ["uhd"]
    'vmaf_4k_v0.6.1neg',  # `model` by path

    # Require a libvmaf built with -Denable_float=true, or an older release for
    # the BOUND models.
    'vmaf_b_v0.6.3',
    'vmaf_float_v0.6.1',
    'vmaf_float_v0.6.1neg',
    'vmaf_float_4k_v0.6.1',
    'vmaf_float_b_v0.6.3'
)

function Save-File {
    <#
    .SYNOPSIS
        Downloads the first URI that responds, returning which one worked.

    .DESCRIPTION
        Requests only exact pinned filenames, never directory listings or newer
        packages. A response is accepted only after its expected digest matches.
    #>
    param(
        [Parameter(Mandatory)][string[]] $Uri,
        [Parameter(Mandatory)][string] $OutFile,
        [Parameter(Mandatory)][string] $Sha256
    )

    if ($Sha256 -notmatch '^[0-9a-fA-F]{64}$') { throw 'An independently verified SHA-256 is required.' }
    $errors = [System.Collections.Generic.List[string]]::new()
    $partial = "$OutFile.part.$([guid]::NewGuid().ToString('N'))"

    try {
    foreach ($candidate in $Uri) {
        # -A is required by some mirrors, which reject requests without a
        # User-Agent with 403. -f makes curl fail on 4xx/5xx instead of writing
        # the error page to disk, which would otherwise fail much later with a
        # confusing parse error.
        curl.exe -sSfL --proto '=https' --proto-redir '=https' -A $UserAgent --retry 2 --retry-delay 1 -o $partial $candidate 2>$null

        if ($LASTEXITCODE -eq 0 -and (Test-Path -LiteralPath $partial) -and
            (Get-FileHash -Algorithm SHA256 -LiteralPath $partial).Hash -eq $Sha256) {
            Move-Item -LiteralPath $partial -Destination $OutFile -Force
            return $candidate
        }

        $errors.Add("  $candidate -> HTTP/download failure or SHA-256 mismatch")
    }

    throw "Failed to download from any source:`n$($errors -join "`n")"
    } finally {
        if (Test-Path -LiteralPath $partial) { Remove-Item -LiteralPath $partial -Force }
    }
}

$BinDir = Join-Path $InstallDir 'bin'
$ModelDir = Join-Path $InstallDir 'model'
$StageDir = Join-Path ([IO.Path]::GetTempPath()) "vmaf-build-$([guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $StageDir | Out-Null
try {

New-Item -ItemType Directory -Force -Path $BinDir, $ModelDir | Out-Null

if (-not $ModelsOnly) {
    $packages = @($LibvmafPackage) + $RuntimePackages
    Write-Host "Downloading $($packages.Count) packages..." -ForegroundColor Cyan

    $usedRepos = [System.Collections.Generic.HashSet[string]]::new()

    foreach ($package in $packages) {
        if (-not $PackageHashes.ContainsKey($package)) { throw "Verified pin unavailable for $package." }
        $archive = Join-Path $StageDir $package
        # One candidate per mirror; Save-File stops at the first that responds.
        $candidates = $Repos | ForEach-Object { "$_$package" }
        $used = Save-File -Uri $candidates -OutFile $archive -Sha256 $PackageHashes[$package]
        $null = $usedRepos.Add(([uri]$used).Host)
        Write-Host ("  {0}  <- {1}" -f $package, ([uri]$used).Host)

        # Extract only the DLLs; the package also carries headers, an import
        # library, pkgconfig and vmaf.exe, none of which this crate needs.
        tar -xf $archive -C $StageDir 'mingw64/bin/*.dll' 2>$null
        if ($LASTEXITCODE -ne 0) { throw "tar failed to extract $package." }
    }

    $staged = Join-Path $StageDir 'mingw64/bin'
    if (-not (Test-Path $staged)) {
        throw "No DLLs were extracted from any of: $($usedRepos -join ', ')"
    }

    # All four must land together: the loader resolves libvmaf.dll's imports
    # from its own directory.
    Copy-Item (Join-Path $staged '*.dll') $BinDir -Force

    $expected = @('libvmaf.dll', 'libgcc_s_seh-1.dll', 'libstdc++-6.dll', 'libwinpthread-1.dll')
    $missing = $expected | Where-Object { -not (Test-Path (Join-Path $BinDir $_)) }
    if ($missing) {
        throw "Missing after extraction: $($missing -join ', '). These are required."
    }

    Write-Host "Installed $($expected.Count) DLLs to $BinDir" -ForegroundColor Green
}

Write-Host "Downloading $($Models.Count) VMAF models..." -ForegroundColor Cyan

foreach ($model in $Models) {
    $destination = Join-Path $ModelDir "$model.json"
    $null = Save-File -Uri "$ModelBase$model.json" -OutFile $destination -Sha256 $ModelHashes[$model]
    Write-Host ("  {0,-26} {1,8:N0} bytes" -f $model, (Get-Item $destination).Length)
}

} finally {
    if (Test-Path -LiteralPath $StageDir) { Remove-Item -LiteralPath $StageDir -Recurse -Force }
}

$totalModels = Get-ChildItem $ModelDir -Filter *.json | Measure-Object Length -Sum

Write-Host ''
Write-Host 'Done.' -ForegroundColor Green
Write-Host "  DLLs   : $BinDir"
Write-Host "  Models : $ModelDir ($($totalModels.Count) files, $([math]::Round($totalModels.Sum / 1KB)) KB)"
Write-Host ''
Write-Host 'Model availability with the MSYS2 build:' -ForegroundColor Cyan
Write-Host '  usable    : vmaf_v0.6.1, vmaf_v0.6.1neg, vmaf_4k_v0.6.1, vmaf_4k_v0.6.1neg'
Write-Host '  need build: the vmaf_float_* models (-Denable_float=true)'
Write-Host '  unusable  : vmaf_b_v0.6.3 -- libvmaf 3.2.1 removed the BOUND'
Write-Host '              extractors, so this one fails on any stock build'
Write-Host ''
Write-Host 'Set these environment variables to use the install:'
Write-Host ''
Write-Host "  [Environment]::SetEnvironmentVariable('VMAF_LIB_DIR', '$BinDir', 'User')" -ForegroundColor Yellow
Write-Host "  [Environment]::SetEnvironmentVariable('VMAF_MODEL_PATH', '$ModelDir', 'User')" -ForegroundColor Yellow
Write-Host ''
Write-Host 'Those persist for future shells. For this session only, use:'
Write-Host ''
Write-Host "  `$env:VMAF_LIB_DIR = '$BinDir'"
Write-Host "  `$env:VMAF_MODEL_PATH = '$ModelDir'"
Write-Host ''
Write-Host 'Verify with:'
Write-Host ''
Write-Host '  cargo test -p av-metrics-vmaf'
Write-Host ''
Write-Host 'The `configured_model_directory_reports_load_failures_and_checks_finite_scores_for_supported_models` test reports exactly which'
Write-Host 'models this libvmaf build accepts.'
