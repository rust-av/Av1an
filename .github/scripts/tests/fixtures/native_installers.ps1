# Offline native ownership fixtures. DLL/archive/model payloads are inert text.
param([ValidateSet('', 'missing-archive', 'missing-pin', 'invalid-pin', 'good', 'hash')][string] $PackageCase = '')
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
foreach ($name in @('VSHIP_PLUGIN_PATH', 'VSHIP_LIB_DIR', 'VSSCRIPT_PATH', 'VAPOURSYNTH_EXTRA_PLUGIN_PATH', 'VSHIP_VERSION', 'VMAF_VERSION')) {
    [Environment]::SetEnvironmentVariable($name, '')
}
$env:PATH = ''
$sha = '770e607624d689265ca6c44884d0807d9b054d23c473c106c72be9de08b7376c' # SHA256('good')
$global:Mode = 'good'
$global:Unpacked = 0
$global:Requests = [System.Collections.Generic.List[string]]::new()
function curl.exe {
    if ($PackageCase) {
        $url = [string]$args[-1]
        $Requests.Add($url)
        if ($url.EndsWith('/')) {
            # A lookup regression gets a plausible newer listing, never a network.
            $global:LASTEXITCODE = 0
            return '<a href="mingw-w64-x86_64-vmaf-99.0.0-1-any.pkg.tar.zst">newer VMAF</a><a href="mingw-w64-x86_64-libgcc-99.0.0-1-any.pkg.tar.zst">newer runtime</a>'
        }
        if ($PackageCase -eq 'missing-archive') {
            $global:LASTEXITCODE = 22 # curl -f: HTTP 404 on every exact archive URL.
            return
        }
    }
    $out = $args[[array]::IndexOf($args, '-o') + 1]
    $out = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($out)
    $failure = if ($args[-1] -like '*.json' -and $Mode -like 'model-*') { $Mode.Substring(6) } else { $Mode }
    [IO.File]::WriteAllText($out, $(if ($failure -eq 'hash') { 'bad' } else { 'good' }))
    $global:LASTEXITCODE = if ($failure -eq 'http') { 22 } else { 0 }
}
function Expand-Archive {
    param($Path, $DestinationPath, [switch]$Force)
    $global:Unpacked++
    if ([IO.File]::ReadAllText($Path) -ne 'good') { throw 'unverified ZIP reached unpack' }
    if ($Mode -eq 'extract') { throw 'installer fixture extraction failure' }
    if ($Mode -ne 'missing') {
        [IO.File]::WriteAllText((Join-Path $DestinationPath 'libvship_NVIDIA.dll'), 'inert Vship DLL')
    }
}
function tar {
    $global:Unpacked++
    $archive = $args[[array]::IndexOf($args, '-xf') + 1]
    if ([IO.File]::ReadAllText($archive) -ne 'good') { throw 'unverified tar reached unpack' }
    if ($Mode -eq 'extract') { $global:LASTEXITCODE = 79; return }
    $stage = $args[[array]::IndexOf($args, '-C') + 1]
    $bin = Join-Path $stage 'mingw64/bin'
    New-Item -ItemType Directory -Path $bin -Force | Out-Null
    foreach ($dll in @('libvmaf.dll', 'libgcc_s_seh-1.dll', 'libstdc++-6.dll', 'libwinpthread-1.dll')) {
        [IO.File]::WriteAllText((Join-Path $bin $dll), 'inert VMAF DLL')
    }
    $global:LASTEXITCODE = 0
}

function Assert-Preserved([string] $Destination) {
    if ([IO.File]::ReadAllText((Join-Path $Destination 'dl/sentinel')) -ne 'preexisting dl') { throw 'destination dl changed' }
    if (@(Get-ChildItem -LiteralPath (Join-Path $Destination 'dl') -Force).Count -ne 1) { throw 'destination dl was used as scratch' }
    if ([IO.File]::ReadAllText((Join-Path $Destination 'sentinel')) -ne 'unrelated destination') { throw 'unrelated sentinel changed' }
    if (@(Get-ChildItem -LiteralPath ([IO.Path]::GetTempPath()) -Force).Count -ne 0) { throw 'owned scratch leaked' }
    if (@(Get-ChildItem -LiteralPath $Destination -Recurse -Force | Where-Object { $_.Name -like '*.part.*' }).Count) { throw 'partial download leaked' }
}
function New-Destination([string] $Name) {
    $destination = Join-Path $env:FIXTURE $Name
    New-Item -ItemType Directory -Path (Join-Path $destination 'dl') -Force | Out-Null
    [IO.File]::WriteAllText((Join-Path $destination 'dl/sentinel'), 'preexisting dl')
    [IO.File]::WriteAllText((Join-Path $destination 'sentinel'), 'unrelated destination')
    return $destination
}

$vship = Join-Path $env:REPO 'av-metrics-vship/scripts/install-libvship-windows.ps1'
if (-not $PackageCase) {
foreach ($variant in @('nvidia', 'vulkan')) {
    $destination = New-Destination "vship $variant"
    $modes = if ($variant -eq 'nvidia') { @('http', 'hash', 'extract', 'missing', 'good') } else { @('http', 'hash', 'good') }
    foreach ($global:Mode in $modes) {
        $global:Unpacked = 0
        $accepted = $false
        try { $result = & $vship -Destination $destination -Variant $variant -ExpectedSha256 $sha; $accepted = $true }
        catch {
            if ($Mode -eq 'good' -or $_.Exception.Message -notmatch 'Failed to download|SHA-256 mismatch|installer fixture extraction failure|archive does not contain') { throw }
        }
        if ($accepted -ne ($Mode -eq 'good')) { throw "wrong Vship acceptance: $variant/$Mode" }
        if ($Unpacked -ne $(if ($variant -eq 'nvidia' -and $Mode -in @('extract', 'missing', 'good')) { 1 } else { 0 })) { throw 'Vship unpack before verification' }
        Assert-Preserved $destination
        $target = Join-Path $destination 'vship.dll'
        if ($accepted) {
            $expected = if ($variant -eq 'nvidia') { 'inert Vship DLL' } else { 'good' }
            if (-not $result.Downloaded -or $result.Path -ne $target -or [IO.File]::ReadAllText($target) -ne $expected) { throw 'Vship output/name incorrect' }
        } elseif (Test-Path -LiteralPath $target) { throw 'Vship published rejected bytes' }
    }
}
}

# Substitute only test pins via the native AST; retain production Save-File,
# validation, extraction, installation and finally control flow verbatim.
$vmaf = Join-Path $env:REPO 'av-metrics-vmaf/scripts/install-libvmaf-windows.ps1'
$errors = $null; $tokens = $null
$ast = [Management.Automation.Language.Parser]::ParseFile($vmaf, [ref]$tokens, [ref]$errors)
if ($errors.Count) { throw ($errors | Out-String) }
$source = [IO.File]::ReadAllText($vmaf)
$assignments = $ast.FindAll({param($node)
    $node -is [Management.Automation.Language.AssignmentStatementAst] -and $node.Left.Extent.Text -in @('$PackageHashes', '$ModelHashes')
}, $true) | Sort-Object { $_.Extent.StartOffset } -Descending
if (@($assignments).Count -ne 2) { throw 'fixture did not locate both pin maps' }
foreach ($assignment in $assignments) {
    $table = $assignment.Right.Find({param($node) $node -is [Management.Automation.Language.HashtableAst]}, $true)
    $entries = @($table.KeyValuePairs | ForEach-Object { "'$($_.Item1.Value)' = '$sha'" })
    if ($PackageCase -and $assignment.Left.Extent.Text -eq '$PackageHashes') {
        if ($PackageCase -eq 'missing-pin') { $entries = @($entries | Select-Object -Skip 1) }
        if ($PackageCase -eq 'invalid-pin') { $entries[0] = $entries[0].Replace($sha, 'invalid') }
    }
    $replacement = $assignment.Left.Extent.Text + ' = @{' + ($entries -join '; ') + '}'
    $source = $source.Remove($assignment.Extent.StartOffset, $assignment.Extent.EndOffset - $assignment.Extent.StartOffset).Insert($assignment.Extent.StartOffset, $replacement)
}
$fixtureInstaller = [scriptblock]::Create($source)
if ($PackageCase) {
    $packages = @(
        'mingw-w64-x86_64-vmaf-3.2.1-1-any.pkg.tar.zst',
        'mingw-w64-x86_64-libgcc-16.2.0-4-any.pkg.tar.zst',
        'mingw-w64-x86_64-libstdc%2B%2B-16.2.0-4-any.pkg.tar.zst',
        'mingw-w64-x86_64-libwinpthread-git-12.0.0.r747.g1a99f8514-1-any.pkg.tar.zst'
    )
    $repos = @('https://repo.msys2.org/mingw/mingw64/', 'https://mirrors.dotsrc.org/msys2/mingw/mingw64/', 'https://mirror.msys2.org/mingw/mingw64/')
    $destination = New-Destination "package $PackageCase"
    New-Item -ItemType Directory -Path (Join-Path $destination 'model'), (Join-Path $destination 'bin') | Out-Null
    [IO.File]::WriteAllText((Join-Path $destination 'model/vmaf_v0.6.1.json'), 'existing model')
    [IO.File]::WriteAllText((Join-Path $destination 'bin/sentinel'), 'existing bin')
    $global:Mode = switch ($PackageCase) { 'missing-archive' { 'http' } 'hash' { 'hash' } default { 'good' } }
    $failure = $null
    try { & $fixtureInstaller -InstallDir $destination }
    catch { $failure = $_.Exception.Message }
    $expectedRequests = @()
    $expectedFailure = switch ($PackageCase) {
        'missing-pin' { "Verified pin unavailable for $($packages[0])." }
        'invalid-pin' { 'An independently verified SHA-256 is required.' }
        { $_ -in @('missing-archive', 'hash') } {
            $expectedRequests = @($repos | ForEach-Object { "$_$($packages[0])" })
            "Failed to download from any source:`n" + (($expectedRequests | ForEach-Object { "  $_ -> HTTP/download failure or SHA-256 mismatch" }) -join "`n")
        }
    }
    if ($PackageCase -eq 'good') {
        if ($failure) { throw $failure }
        $expectedRequests = @($packages | ForEach-Object { "$($repos[0])$_" })
        $models = @('vmaf_v0.6.1', 'vmaf_v0.6.1neg', 'vmaf_4k_v0.6.1', 'vmaf_4k_v0.6.1neg', 'vmaf_b_v0.6.3', 'vmaf_float_v0.6.1', 'vmaf_float_v0.6.1neg', 'vmaf_float_4k_v0.6.1', 'vmaf_float_b_v0.6.3')
        $expectedRequests += @($models | ForEach-Object { "https://raw.githubusercontent.com/Netflix/vmaf/v3.2.1/model/$_.json" })
    } elseif ($failure -cne $expectedFailure) { throw "Unexpected package $PackageCase failure: $failure" }
    if (($Requests -join "`n") -cne ($expectedRequests -join "`n")) { throw "Package requested an index, unpinned package or wrong source: $($Requests -join ', ')" }
    if ($Unpacked -ne $(if ($PackageCase -eq 'good') { 4 } else { 0 })) { throw 'Package unpack before verified download' }
    Assert-Preserved $destination
    if ([IO.File]::ReadAllText((Join-Path $destination 'bin/sentinel')) -ne 'existing bin') { throw 'existing bin changed' }
    $dlls = @(Get-ChildItem -LiteralPath (Join-Path $destination 'bin') -Filter '*.dll' -File)
    $models = @(Get-ChildItem -LiteralPath (Join-Path $destination 'model') -Filter '*.json' -File)
    if ($dlls.Count -ne $(if ($PackageCase -eq 'good') { 4 } else { 0 }) -or $models.Count -ne $(if ($PackageCase -eq 'good') { 9 } else { 1 })) { throw 'Package installed rejected/missing packages' }
    $expectedModel = if ($PackageCase -eq 'good') { 'good' } else { 'existing model' }
    if ([IO.File]::ReadAllText((Join-Path $destination 'model/vmaf_v0.6.1.json')) -ne $expectedModel) { throw 'Package changed model destination incorrectly' }
    Write-Host "Package $PackageCase verified"
    return
}
foreach ($modelsOnly in @($false, $true)) {
    $destination = New-Destination "vmaf modelsOnly $modelsOnly"
    New-Item -ItemType Directory -Path (Join-Path $destination 'model'), (Join-Path $destination 'bin') | Out-Null
    [IO.File]::WriteAllText((Join-Path $destination 'model/vmaf_v0.6.1.json'), 'existing model')
    [IO.File]::WriteAllText((Join-Path $destination 'bin/sentinel'), 'existing bin')
    $modes = if ($modelsOnly) { @('http', 'hash', 'good') } else { @('http', 'hash', 'extract', 'model-http', 'model-hash', 'good') }
    foreach ($global:Mode in $modes) {
        $global:Unpacked = 0
        $accepted = $false
        try { & $fixtureInstaller -InstallDir $destination -ModelsOnly:$modelsOnly; $accepted = $true }
        catch {
            if ($Mode -eq 'good' -or $_.Exception.Message -notmatch 'Failed to download from any source|tar failed to extract') { throw }
        }
        if ($accepted -ne ($Mode -eq 'good')) { throw "wrong VMAF acceptance: $modelsOnly/$Mode" }
        $expectedUnpacks = if ($modelsOnly -or $Mode -in @('http', 'hash')) { 0 } elseif ($Mode -eq 'extract') { 1 } else { 4 }
        if ($Unpacked -ne $expectedUnpacks) { throw 'VMAF unpack before verification' }
        Assert-Preserved $destination
        $dlls = @(Get-ChildItem -LiteralPath (Join-Path $destination 'bin') -Filter '*.dll' -File)
        $models = @(Get-ChildItem -LiteralPath (Join-Path $destination 'model') -Filter '*.json' -File)
        if ([IO.File]::ReadAllText((Join-Path $destination 'bin/sentinel')) -ne 'existing bin') { throw 'existing bin changed' }
        if ($dlls.Count -ne $(if (-not $modelsOnly -and ($accepted -or $Mode -like 'model-*')) { 4 } else { 0 })) { throw 'VMAF DLL output incorrect' }
        if ($models.Count -ne $(if ($accepted) { 9 } else { 1 })) { throw 'VMAF model output incorrect' }
        if (-not $accepted -and [IO.File]::ReadAllText((Join-Path $destination 'model/vmaf_v0.6.1.json')) -ne 'existing model') { throw 'rejected model replaced existing file' }
        foreach ($file in $dlls + @($models | Where-Object { $accepted })) {
            if ([IO.File]::ReadAllText($file.FullName) -ne $(if ($file.Extension -eq '.dll') { 'inert VMAF DLL' } else { 'good' })) { throw 'VMAF output bytes incorrect' }
        }
    }
}

# Force a collision at the create boundary without changing production GUIDs.
# A failed New-Item must never enter cleanup or delete the preexisting file.
$global:Collision = $null
function New-Item {
    [CmdletBinding()]
    param([string] $ItemType, [string[]] $Path, [switch] $Force)
    if ($Path.Count -eq 1 -and $Path[0].StartsWith([IO.Path]::GetTempPath())) {
        $global:Collision = $Path[0]
        [IO.File]::WriteAllText($Collision, 'preexisting collision')
    }
    Microsoft.PowerShell.Management\New-Item @PSBoundParameters
}
$global:Mode = 'good'
foreach ($installer in @($vship, $fixtureInstaller)) {
    $destination = New-Destination "collision $([guid]::NewGuid().ToString('N'))"
    $parameters = if ($installer -is [string]) {
        @{ Destination = $destination; Variant = 'nvidia'; ExpectedSha256 = $sha }
    } else { @{ InstallDir = $destination } }
    $failed = $false
    try { & $installer @parameters } catch { $failed = $true }
    if (-not $failed -or -not $Collision -or [IO.File]::ReadAllText($Collision) -ne 'preexisting collision') { throw 'failed creation claimed/deleted another owner file' }
    Remove-Item -LiteralPath $Collision -Force
    $global:Collision = $null
    Assert-Preserved $destination
}
