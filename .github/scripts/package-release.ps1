<#
.SYNOPSIS
    Packs the staged release into a single zip with the layout the binaries need.

.DESCRIPTION
    The release assets form a directory, not a pile of loose files, because the
    runtime resolves some of them by relative path:

      * VMAF models must sit in `model/` beside the executable. av-metrics-vmaf
        searches `<exe dir>/model`, so a flat layout silently fails to find them
        and VMAF is reported unavailable with no other symptom.
      * The native libraries must sit beside the executable, because libvmaf.dll
        imports its MinGW runtime DLLs and neither it nor libvship.dll can be
        found anywhere else. fmetrics.dll has no dependencies of its own but is
        still found by relative path. FFMS2 is dynamically linked and mandatory.
        Include build-produced ORT/DirectML/runtime DLLs beside it as well.

.PARAMETER StageDir
    Directory holding the staged release. Defaults to `target\release`.

.PARAMETER OutputPath
    Where to write the archive. Defaults to `target\av1an-windows-x64.zip`.

.PARAMETER SkipSchema
    Leave `configuration.schema.json` out of the archive.
#>
[CmdletBinding()]
param(
    [Parameter()]
    [string] $StageDir = 'target\release',

    [Parameter()]
    [string] $OutputPath = 'target\av1an-windows-x64.zip',

    [Parameter()]
    [string] $SchemaPath = 'california-condor/configuration.schema.json',

    [Parameter()]
    [switch] $SkipSchema
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# Everything the runtime loads by relative path. `model/` is a directory rather
# than a file, which is the whole reason this archive exists.
$Contents = @(
    'av1an.exe'
    'ffms2.dll'
    'vcruntime140.dll'
    'vcruntime140_1.dll'
    'msvcp140.dll'
    'DirectML.dll'
    'libvmaf.dll'
    'libvship.dll'
    'fmetrics.dll'
    'libgcc_s_seh-1.dll'
    'libstdc++-6.dll'
    'libwinpthread-1.dll'
    'model'
)

# Cargo's copy-dylibs puts ORT/DirectML beside the executable, sometimes as
# symlinks into its cache. Add only root DLLs, not the build tree; Add-ZipFile
# reads their bytes so the archive never retains a build-cache symlink.
$Contents += @(Get-ChildItem $StageDir -File -Filter '*.dll' |
    Where-Object { $_.Name -notin $Contents } | ForEach-Object { $_.Name })

function Write-Step {
    param([Parameter(Mandatory)][string] $Message)
    Write-Host "==> $Message" -ForegroundColor Cyan
}

Write-Step 'Checking the staged release'

$missing = $Contents | Where-Object { -not (Test-Path (Join-Path $StageDir $_)) }
if ($missing -contains 'DirectML.dll') {
    throw 'Missing DirectML.dll: the pinned Windows ort-sys static ORT build imports it. Restore its copy-dylibs output beside av1an.exe before packaging; do not use a host/System32 copy.'
}
if ($missing) {
    throw "Missing from $StageDir : $($missing -join ', ')"
}

$schema = $null
if (-not $SkipSchema -and (Test-Path $SchemaPath)) {
    $schema = (Resolve-Path $SchemaPath).Path
    Write-Host "    including $SchemaPath"
}

$models = Get-ChildItem (Join-Path $StageDir 'model') -Filter *.json
Write-Host "    $($Contents.Count) entries plus $($models.Count) model files"

Write-Step "Writing $OutputPath"

New-Item -ItemType Directory -Force -Path (Split-Path $OutputPath -Parent) | Out-Null
if (Test-Path $OutputPath) { Remove-Item -Force $OutputPath }

# Written entry by entry so every entry lands at the archive root.
Add-Type -AssemblyName System.IO.Compression.FileSystem

function Add-ZipFile {
    <#
    .SYNOPSIS
        Copies one file into the open archive under a given entry name.

    .DESCRIPTION
        Uses the stream API, which Windows PowerShell 5.1 supports.
    #>
    param(
        [Parameter(Mandatory)][string] $Source,
        [Parameter(Mandatory)][string] $EntryName,
        [Parameter(Mandatory)][System.IO.Compression.ZipArchive] $Archive
    )

    $entry = $Archive.CreateEntry($EntryName, [System.IO.Compression.CompressionLevel]::Optimal)
    $target = $entry.Open()
    $input = [System.IO.File]::OpenRead($Source)
    try {
        $input.CopyTo($target)
    } finally {
        $input.Dispose()
        $target.Dispose()
    }
}

$archive = [System.IO.Compression.ZipFile]::Open($OutputPath, 'Create')
$expected = @()
try {
    foreach ($entry in $Contents) {
        $source = Join-Path $StageDir $entry
        if (Test-Path $source -PathType Container) {
            foreach ($file in Get-ChildItem $source -Recurse -File) {
                $relative = $file.FullName.Substring((Get-Item $source).FullName.Length + 1).Replace('\', '/')
                $name = "$entry/$relative"
                Add-ZipFile -Source $file.FullName -EntryName $name -Archive $archive
                $expected += $name
            }
        } else {
            Add-ZipFile -Source $source -EntryName $entry -Archive $archive
            $expected += $entry
        }
    }

    if ($schema) {
        # Sits at the archive root, where `av1an init` looks for it.
        Add-ZipFile -Source $schema -EntryName (Split-Path $schema -Leaf) -Archive $archive
        $expected += Split-Path $schema -Leaf
    }
} finally {
    $archive.Dispose()
}

Write-Step 'Verifying the archive'

Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip = [System.IO.Compression.ZipFile]::OpenRead((Resolve-Path $OutputPath).Path)
try {
    $names = $zip.Entries | ForEach-Object { $_.FullName }
} finally { $zip.Dispose() }

# A model outside model/ would resolve to nothing at runtime, so the layout is
# asserted rather than assumed.
$flatModels = $names | Where-Object { $_ -match '^vmaf_.*\.json$' }
if ($flatModels) {
    throw "Models landed at the archive root, where they cannot be found: $($flatModels -join ', ')"
}

# Shared with offline fixtures and the mandatory clean-extracted Windows gate.
python -B (Join-Path $PSScriptRoot 'verify-windows-bundle.py') $OutputPath --expect $expected
if ($LASTEXITCODE -ne 0) { throw 'Windows archive verification failed.' }

$size = [math]::Round((Get-Item $OutputPath).Length / 1MB, 2)
Write-Host "    $($names.Count) entries, $size MB" -ForegroundColor DarkGray
Write-Host "Archive ready: $OutputPath" -ForegroundColor Green
