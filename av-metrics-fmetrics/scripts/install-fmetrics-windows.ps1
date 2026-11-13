<#
.SYNOPSIS
    Builds fmetrics from source and installs the shared library for Windows.

.DESCRIPTION
    Installs fmetrics so av-metrics-fmetrics can load it at runtime with
    `libloading`. Unlike libvship and libvmaf, fmetrics publishes no prebuilt
    Windows binaries and nothing installable from a package mirror, so this
    fetches a pinned commit and compiles it with Zig.

    That makes this the one install script here that needs a toolchain: Zig
    0.16.0 and Git. Zig 0.17 is refused because it removed `b.build_root`, which
    the pinned `build.zig` still uses.

    Built from BoatsMcGee/fmetrics, which adds what upstream lacks: a
    shared-library target to load at all, and a patch making fcvvdp compile on
    Windows, where it calls POSIX-only `sysconf()`.

    The library is installed as `fmetrics.dll`, the first name `library_names`
    offers on Windows.

.PARAMETER Destination
    Where to place `fmetrics.dll`. Defaults to `$PWD\fmetrics`.

.PARAMETER Commit
    The full commit ID to build. Defaults to the standalone pin below.

.PARAMETER ZigPath
    `zig` to look up on `PATH`, or a path to `zig.exe`.

.EXAMPLE
    .\install-fmetrics-windows.ps1

.EXAMPLE
    .\install-fmetrics-windows.ps1 -Destination C:\tools\fmetrics -Verbose
#>
[CmdletBinding()]
param(
    [Parameter()]
    [string] $Destination = (Join-Path $PWD 'fmetrics'),

    [Parameter()]
    [string] $Commit = 'ae87c8e5607063f0dcc8241b52cb143f6c9ad4ac',

    [Parameter()]
    [string] $ZigPath = 'zig'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# Defaults, overridable from the environment so the release pipeline can pass
# its pins without editing this script. It stays standalone: with nothing set,
# these literals are what a direct run uses.
$Repository   = if ($env:FMETRICS_REPO)          { $env:FMETRICS_REPO }          else { 'https://github.com/BoatsMcGee/fmetrics.git' }
# Descriptive only; fetching the immutable pin does not require this branch.
$PatchRef     = if ($env:FMETRICS_BRANCH)        { $env:FMETRICS_BRANCH }        else { 'add-windows-shared-build' }
$UpstreamBase = if ($env:FMETRICS_UPSTREAM_BASE) { $env:FMETRICS_UPSTREAM_BASE } else { 'e8bf3cfe06fb78f51864804b2f7003667d4c5a71' }
$RequiredZigVersion = if ($env:FMETRICS_ZIG_VERSION) { $env:FMETRICS_ZIG_VERSION } else { '0.16.0' }

if (-not $PSBoundParameters.ContainsKey('Commit') -and $env:FMETRICS_COMMIT) {
    $Commit = $env:FMETRICS_COMMIT
}
if ($Commit -notmatch '\A[0-9a-fA-F]{40}\z') {
    throw "Expected a full 40-hex commit ID, not '$Commit'."
}
$Commit = $Commit.ToLowerInvariant()

$dllName = 'fmetrics.dll'
$target = Join-Path $Destination $dllName

function Write-Step([string] $Message) {
    Write-Host "==> $Message" -ForegroundColor Cyan
}

function Resolve-Zig {
    <#
    .SYNOPSIS
        Locates zig.exe and checks the exact expected version.

    .DESCRIPTION
        `-ZigPath` may be a bare name to look up on PATH, or a path to zig.exe.
        FMETRICS_ZIG_VERSION deliberately overrides the expected pin, not the
        executable's reported version. The source still requires 0.16.
    #>
    if ($RequiredZigVersion -cnotmatch '\A0\.16\.(0|[1-9][0-9]*)\z') {
        throw "Expected FMETRICS_ZIG_VERSION '$RequiredZigVersion' must be an exact 0.16 patch: 0.17 removed b.build_root."
    }
    $command = Get-Command $ZigPath -ErrorAction SilentlyContinue
    if ($command) {
        $exe = $command.Source
    } elseif (Test-Path $ZigPath) {
        $exe = (Resolve-Path $ZigPath).Path
    } else {
        throw "zig not found at '$ZigPath'. Install exactly Zig $RequiredZigVersion from https://ziglang.org/download/, or pass -ZigPath with the path to zig.exe."
    }

    $version = [string]::Join("`n", @(& $exe version 2>$null))
    if ($LASTEXITCODE -ne 0 -or $version -cne $RequiredZigVersion) {
        $actual = if ($version) { $version } else { '<no version>' }
        throw "zig '$actual' found, but expected exactly $RequiredZigVersion (successful zig version); refusing fetch/build."
    }

    Write-Host "    zig $version" -ForegroundColor DarkGray
    return $exe
}

# --- toolchain ---

$zig = Resolve-Zig

if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
    throw 'git not found; it is needed to fetch a pinned commit.'
}

# Staged under the system temp directory rather than beside the destination, so a
# checkout never lands in a directory that is about to be packaged.
$work = Join-Path ([IO.Path]::GetTempPath()) "fmetrics-build-$([guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $work | Out-Null
try {
    Write-Step "Fetching fmetrics commit $Commit (descriptive branch: $PatchRef)"
    & git init --quiet $work
    if ($LASTEXITCODE -ne 0) { throw 'git init failed for fmetrics checkout.' }
    # Full history is needed for the upstream ancestry check below.
    & git -C $work fetch --quiet --no-tags -- $Repository $Commit
    if ($LASTEXITCODE -ne 0) { throw "Failed to fetch fmetrics commit $Commit from $Repository." }
    & git -C $work checkout --quiet --detach $Commit --
    if ($LASTEXITCODE -ne 0) { throw "Failed to check out fmetrics commit $Commit." }

    $head = & git -C $work rev-parse --verify 'HEAD^{commit}'
    if ($LASTEXITCODE -ne 0 -or -not $head) { throw "Failed to verify fmetrics HEAD for commit $Commit." }
    $head = $head.Trim()
    if ($head -ne $Commit) {
        throw "fmetrics HEAD is $head, not the expected commit $Commit."
    }

    & git -C $work merge-base --is-ancestor $UpstreamBase $Commit
    if ($LASTEXITCODE -ne 0) {
        throw "$Commit is not built on the upstream $UpstreamBase. Rebase before reinstalling."
    }

    # --- dependency patch ---

    # fcvvdp is fetched by Zig into zig-pkg/, outside the repository tree, and
    # calls POSIX-only sysconf() on Windows. Zig extracts dependencies without
    # their .git directory, so the patch is applied from the repository root with
    # --directory.
    $patch = "$work/patches/0001-windows-portability.patch"
    if (-not (Test-Path $patch)) {
        throw "the $Commit checkout has no patches/0001-windows-portability.patch; the shared-library work moved."
    }

    Write-Step 'Fetching dependencies'
    Push-Location $work
    try {
        # `--fetch` populates zig-pkg/ without compiling, so fcvvdp can be
        # patched before it is built.
        & $zig build --fetch
        if ($LASTEXITCODE -ne 0) { throw 'fetching the Zig dependencies failed.' }
    } finally {
        Pop-Location
    }

        $dep = Get-ChildItem (Join-Path $work 'zig-pkg') -Directory -Filter 'fcvvdp-*' |
            Select-Object -First 1
        if (-not $dep) { throw 'zig-pkg/fcvvdp-* was not populated.' }

        Write-Step 'Patching fcvvdp for Windows'
        & git -C $work apply --directory="zig-pkg/$($dep.Name)" $patch
        if ($LASTEXITCODE -ne 0) { throw 'applying the fcvvdp patch failed.' }

    # --- build ---

    # `-Dshared=true` is the fork's addition; without it Zig installs only a
    # static library, which `dlopen` cannot use.
    Write-Step 'Building fmetrics.dll'
    Push-Location $work
    try {
        & $zig build --release=fast -Dshared=true
        if ($LASTEXITCODE -ne 0) { throw 'the fmetrics build failed.' }
    } finally {
        Pop-Location
    }

    $built = Join-Path $work 'zig-out\bin\fmetrics.dll'
    if (-not (Test-Path $built)) {
        throw "the build succeeded but $built does not exist; the output layout may have changed."
    }

    # --- install ---

    New-Item -ItemType Directory -Force -Path $Destination | Out-Null
    Copy-Item $built $target -Force
} finally {
    # The checkout is several hundred megabytes with its dependency tree.
    if (Test-Path -LiteralPath $work) { Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue }
}

$size = [math]::Round((Get-Item $target).Length / 1KB)

Write-Host ''
Write-Host 'Done.' -ForegroundColor Green
Write-Host "  fmetrics.dll : $target ($size KB)"
Write-Host "  commit       : $Commit"
Write-Host ''
Write-Host 'Set this environment variable to use the install:' -ForegroundColor Cyan
Write-Host ''
Write-Host "  [Environment]::SetEnvironmentVariable('FMETRICS_LIB_DIR', '$Destination', 'User')" -ForegroundColor Yellow
Write-Host ''
Write-Host 'That persists for future shells. For this session only, use:'
Write-Host ''
Write-Host "  `$env:FMETRICS_LIB_DIR = '$Destination'"
Write-Host ''
Write-Host 'Not needed when the DLL sits beside av1an.exe, which is where a'
Write-Host 'release stages it; the crate searches there on its own.'
Write-Host ''
Write-Host 'Verify with:'
Write-Host ''
Write-Host '  cargo test -p av-metrics-fmetrics'
Write-Host ''
Write-Host 'Those tests skip themselves when the library is absent, so a pass'
Write-Host 'alone does not prove it loaded.'
Write-Host ''
