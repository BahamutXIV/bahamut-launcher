[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

$repositoryRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$testRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("bahamut-package-regression-" + [guid]::NewGuid().ToString('N'))
$fixtureRoot = Join-Path $testRoot 'fixture'
$fixtureScripts = Join-Path $fixtureRoot 'scripts'
$fixtureBuildScript = Join-Path $fixtureScripts 'build-windows-package.ps1'
$fixtureStageScript = Join-Path $fixtureScripts 'stage-windows-release.ps1'
$destination = Join-Path $fixtureRoot 'out\dev'
$previousBuildMode = [System.Environment]::GetEnvironmentVariable('BAHAMUT_SYNTHETIC_BUILD_MODE', 'Process')
$previousStageMode = [System.Environment]::GetEnvironmentVariable('BAHAMUT_SYNTHETIC_STAGE_MODE', 'Process')

function Get-TreeSnapshot([string] $Root) {
    $rootPath = (Resolve-Path -LiteralPath $Root).Path
    $directories = @(Get-ChildItem -LiteralPath $rootPath -Directory -Recurse |
        ForEach-Object { $_.FullName.Substring($rootPath.Length).TrimStart('\', '/') -replace '\\', '/' } |
        Sort-Object)
    $files = @(Get-ChildItem -LiteralPath $rootPath -File -Recurse |
        ForEach-Object {
            $relative = $_.FullName.Substring($rootPath.Length).TrimStart('\', '/') -replace '\\', '/'
            "$relative=$((Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash)"
        } |
        Sort-Object)
    return @($directories + $files)
}

function Assert-Equal([string] $Label, [object] $Expected, [object] $Actual) {
    if ($Expected -ne $Actual) {
        throw "$Label mismatch. Expected '$Expected', actual '$Actual'."
    }
}

function Remove-SafeTestRoot([string] $Root) {
    $fullPath = [System.IO.Path]::GetFullPath($Root).TrimEnd('\', '/')
    $tempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd('\', '/')
    if ($fullPath -eq $tempRoot -or
        -not $fullPath.StartsWith($tempRoot + '\', [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to remove an unexpected test root: $fullPath"
    }
    if (-not (Test-Path -LiteralPath $fullPath)) {
        return
    }

    $rootAttributes = [System.IO.File]::GetAttributes($fullPath)
    if (($rootAttributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Refusing to follow a reparse-point test root: $fullPath"
    }
    $pending = New-Object 'System.Collections.Generic.Stack[string]'
    $pending.Push($fullPath)
    while ($pending.Count -gt 0) {
        $current = $pending.Pop()
        foreach ($entry in @(Get-ChildItem -LiteralPath $current -Force)) {
            $attributes = [System.IO.File]::GetAttributes($entry.FullName)
            if (($attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Refusing to follow a reparse-point test entry: $($entry.FullName)"
            }
            if ($entry.PSIsContainer) {
                $pending.Push($entry.FullName)
            }
        }
    }
    Remove-Item -LiteralPath $fullPath -Recurse -Force
}

function cmake {
    $Arguments = @($args)

    if ($env:BAHAMUT_SYNTHETIC_BUILD_MODE -eq 'FailCmake') {
        $global:LASTEXITCODE = 1
        return
    }
    if ($Arguments -contains '--build') {
        $buildIndex = [Array]::IndexOf($Arguments, '--build')
        $buildRoot = $Arguments[$buildIndex + 1]
        $configurationIndex = [Array]::IndexOf($Arguments, '--config')
        $configuration = $Arguments[$configurationIndex + 1]
        New-Item -ItemType Directory -Force -Path (Join-Path $buildRoot $configuration) | Out-Null
    }
    $global:LASTEXITCODE = 0
}

function cargo {
    $Arguments = @($args)

    if ($env:BAHAMUT_SYNTHETIC_BUILD_MODE -eq 'FailCargo') {
        $global:LASTEXITCODE = 1
        return
    }
    if ($Arguments[0] -eq 'metadata') {
        $global:LASTEXITCODE = 0
        return '{"packages":[{"name":"bahamut-launcher-shell","version":"1.0.0"}]}'
    }
    $manifestIndex = [Array]::IndexOf($Arguments, '--manifest-path')
    $manifestRoot = Split-Path -Parent $Arguments[$manifestIndex + 1]
    $profile = if ($Arguments -contains '--release') { 'release' } else { 'debug' }
    $targetDirectory = Join-Path $manifestRoot (Join-Path 'target' $profile)
    New-Item -ItemType Directory -Force -Path $targetDirectory | Out-Null
    Set-Content -LiteralPath (Join-Path $targetDirectory 'bahamut-launcher-shell.exe') -Value 'new launcher' -Encoding ascii
    if ($Arguments -contains 'bahamut-update-helper') {
        Set-Content -LiteralPath (Join-Path $targetDirectory 'bahamut-update-helper.exe') -Value 'new helper' -Encoding ascii
    }
    $global:LASTEXITCODE = 0
}

try {
    New-Item -ItemType Directory -Force -Path $fixtureScripts, (Join-Path $fixtureRoot 'client'), $destination | Out-Null
    Set-Content -LiteralPath (Join-Path $fixtureRoot 'Cargo.toml') -Value '[workspace]' -Encoding ascii
    Copy-Item -LiteralPath (Join-Path $repositoryRoot 'scripts\build-windows-package.ps1') -Destination $fixtureBuildScript
    Set-Content -LiteralPath $fixtureStageScript -Encoding utf8 -Value @'
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string] $LauncherBinary,
    [Parameter(Mandatory)][string] $ClientBuildDirectory,
    [string] $HelperBinary,
    [Parameter(Mandatory)][string] $Destination
)

$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force -Path (Join-Path $Destination 'plugins') | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $Destination 'plugins\dats\bahamut-dats-overlay') | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $Destination 'scripts') | Out-Null
if ($env:BAHAMUT_SYNTHETIC_STAGE_MODE -eq 'Fail') {
    Set-Content -LiteralPath (Join-Path $Destination 'partial.txt') -Value 'partial' -Encoding ascii
    throw 'synthetic staging failure'
}
Set-Content -LiteralPath (Join-Path $Destination 'bahamut-launcher.exe') -Value 'new launcher' -Encoding ascii
Set-Content -LiteralPath (Join-Path $Destination 'plugins\screenshot.dll') -Value 'new screenshot' -Encoding ascii
Set-Content -LiteralPath (Join-Path $Destination 'plugins\dats\bahamut-dats-overlay\overlay.toml') -Value 'official overlay' -Encoding ascii
New-Item -ItemType Directory -Force -Path (Join-Path $Destination 'plugins\dats\bahamut-dats-overlay\data\1C\59\00') | Out-Null
Set-Content -LiteralPath (Join-Path $Destination 'plugins\dats\bahamut-dats-overlay\data\1C\59\00\CB.DAT') -Value 'official DAT' -Encoding ascii
Set-Content -LiteralPath (Join-Path $Destination 'scripts\default.txt') -Value 'seed startup' -Encoding ascii
if ($env:BAHAMUT_SYNTHETIC_STAGE_MODE -eq 'Collision') {
    Set-Content -LiteralPath (Join-Path $Destination 'file-collision.dll') -Value 'payload' -Encoding ascii
}
'@

    $stageValidationDestination = Join-Path $testRoot 'stage-validation'
    $stageValidationBuild = Join-Path $fixtureRoot 'stage-build'
    New-Item -ItemType Directory -Force -Path $stageValidationDestination, $stageValidationBuild | Out-Null
    foreach ($fileName in @('bahamut-loader.exe', 'bahamut.dll', 'screenshot.dll', 'discord-rpc.dll')) {
        Set-Content -LiteralPath (Join-Path $stageValidationBuild $fileName) -Value "test $fileName" -Encoding ascii
    }
    $stageValidationLauncher = Join-Path $fixtureRoot 'stage-launcher.exe'
    Set-Content -LiteralPath $stageValidationLauncher -Value 'test launcher' -Encoding ascii
    & (Join-Path $repositoryRoot 'scripts\stage-windows-release.ps1') `
        -LauncherBinary $stageValidationLauncher `
        -ClientBuildDirectory $stageValidationBuild `
        -Destination $stageValidationDestination | Out-Null
    if (-not (Test-Path -LiteralPath (Join-Path $stageValidationDestination 'plugins\dats\bahamut-dats-overlay\overlay.toml') -PathType Leaf)) {
        throw 'release staging omitted the official overlay manifest'
    }

    $stalePackageFiles = [ordered]@{
        'bahamut-launcher.exe' = 'old launcher'
        'plugins/screenshot.dll' = 'old screenshot'
    }
    $preservedFiles = [ordered]@{
        'config/bahamut.ini' = 'keep config'
        'logs/launcher/keep.log' = 'keep log'
        'cache/state.bin' = 'keep cache'
        'screenshots/old.png' = 'keep screenshot'
        'addons/custom/addon.toml' = 'keep addon'
        'plugins/custom.dll' = 'keep plugin'
        'plugins/dats/custom/overlay.toml' = 'custom overlay'
        'prerequisites/vc_redist.x86.exe' = 'keep dev VC++ installer'
        'prerequisites/MicrosoftEdgeWebView2Setup.exe' = 'keep dev WebView2 installer'
    }
    foreach ($entry in @($stalePackageFiles.GetEnumerator()) + @($preservedFiles.GetEnumerator())) {
        $path = Join-Path $destination ($entry.Key -replace '/', '\')
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $path) | Out-Null
        Set-Content -LiteralPath $path -Value $entry.Value -Encoding ascii
    }

    $beforeFailure = Get-TreeSnapshot $destination
    $env:BAHAMUT_SYNTHETIC_BUILD_MODE = 'FailCmake'
    $env:BAHAMUT_SYNTHETIC_STAGE_MODE = 'Success'
    $failureObserved = $false
    $failureMessage = ''
    try {
        . $fixtureBuildScript -Configuration Debug
    } catch {
        $failureObserved = $true
        $failureMessage = $_.Exception.Message
    }
    if (-not $failureObserved -or $failureMessage -notlike '*client module configuration failed*') {
        throw "CMake failure was not propagated as expected: $failureMessage"
    }
    Assert-Equal 'destination after CMake failure' ($beforeFailure -join "`n") ((Get-TreeSnapshot $destination) -join "`n")

    $env:BAHAMUT_SYNTHETIC_BUILD_MODE = 'FailCargo'
    $failureObserved = $false
    $failureMessage = ''
    try {
        . $fixtureBuildScript -Configuration Debug
    } catch {
        $failureObserved = $true
        $failureMessage = $_.Exception.Message
    }
    if (-not $failureObserved -or $failureMessage -notlike '*launcher build failed*') {
        throw "Cargo failure was not propagated as expected: $failureMessage"
    }
    Assert-Equal 'destination after Cargo failure' ($beforeFailure -join "`n") ((Get-TreeSnapshot $destination) -join "`n")

    $env:BAHAMUT_SYNTHETIC_BUILD_MODE = 'Success'
    $env:BAHAMUT_SYNTHETIC_STAGE_MODE = 'Fail'
    $failureObserved = $false
    $failureMessage = ''
    try {
        . $fixtureBuildScript -Configuration Debug
    } catch {
        $failureObserved = $true
        $failureMessage = $_.Exception.Message
    }
    if (-not $failureObserved -or $failureMessage -notlike '*synthetic staging failure*') {
        throw "Staging failure was not propagated as expected: $failureMessage"
    }
    Assert-Equal 'destination after staging failure' ($beforeFailure -join "`n") ((Get-TreeSnapshot $destination) -join "`n")
    if (@(Get-ChildItem -LiteralPath (Join-Path $fixtureRoot 'out') -Directory -Filter 'package-staging-*').Count -ne 0) {
        throw 'staging failure left a temporary package directory'
    }

    $env:BAHAMUT_SYNTHETIC_STAGE_MODE = 'Success'
    . $fixtureBuildScript -Configuration Debug
    Assert-Equal 'published launcher' 'new launcher' (Get-Content -Raw (Join-Path $destination 'bahamut-launcher.exe')).Trim()
    Assert-Equal 'published screenshot' 'new screenshot' (Get-Content -Raw (Join-Path $destination 'plugins\screenshot.dll')).Trim()
    Assert-Equal 'seeded startup script' 'seed startup' (Get-Content -Raw (Join-Path $destination 'scripts\default.txt')).Trim()
    Assert-Equal 'published official overlay' 'official overlay' (Get-Content -Raw (Join-Path $destination 'plugins\dats\bahamut-dats-overlay\overlay.toml')).Trim()
    Assert-Equal 'published official overlay DAT' 'official DAT' (Get-Content -Raw (Join-Path $destination 'plugins\dats\bahamut-dats-overlay\data\1C\59\00\CB.DAT')).Trim()
    foreach ($entry in $preservedFiles.GetEnumerator()) {
        $path = Join-Path $destination ($entry.Key -replace '/', '\')
        Assert-Equal "preserved $($entry.Key)" $entry.Value (Get-Content -Raw $path).Trim()
    }
    if (@(Get-ChildItem -LiteralPath (Join-Path $fixtureRoot 'out') -Directory -Filter 'package-staging-*').Count -ne 0) {
        throw 'successful package publish left a temporary package directory'
    }

    Set-Content -LiteralPath (Join-Path $destination 'scripts\default.txt') -Value 'keep startup command' -Encoding ascii
    Set-Content -LiteralPath (Join-Path $destination 'plugins\dats\bahamut-dats-overlay\overlay.toml') -Value 'locally modified official overlay' -Encoding ascii
    . $fixtureBuildScript -Configuration Debug
    Assert-Equal 'preserved startup script' 'keep startup command' (Get-Content -Raw (Join-Path $destination 'scripts\default.txt')).Trim()
    Assert-Equal 'replaced official overlay' 'official overlay' (Get-Content -Raw (Join-Path $destination 'plugins\dats\bahamut-dats-overlay\overlay.toml')).Trim()
    Assert-Equal 'preserved custom overlay' 'custom overlay' (Get-Content -Raw (Join-Path $destination 'plugins\dats\custom\overlay.toml')).Trim()

    $blockedFile = Join-Path $destination 'file-collision.dll'
    New-Item -ItemType Directory -Path $blockedFile | Out-Null
    $env:BAHAMUT_SYNTHETIC_STAGE_MODE = 'Collision'
    $failureObserved = $false
    try {
        . $fixtureBuildScript -Configuration Debug
    } catch {
        if ($_.Exception.Message -notlike 'package file path is a directory:*') {
            throw
        }
        $failureObserved = $true
    }
    if (-not $failureObserved) {
        throw 'destination file/directory collision was not rejected'
    }
    if (Test-Path -LiteralPath (Join-Path $blockedFile 'file-collision.dll')) {
        throw 'publication nested a payload inside the conflicting directory'
    }

    $env:BAHAMUT_SYNTHETIC_STAGE_MODE = 'Success'
    $releaseDestination = Join-Path $fixtureRoot 'out\release\1.0.0'
    $releasePrerequisites = Join-Path $releaseDestination 'prerequisites'
    New-Item -ItemType Directory -Force -Path $releasePrerequisites | Out-Null
    Set-Content -LiteralPath (Join-Path $releasePrerequisites 'vc_redist.x86.exe') -Value 'old VC++ installer' -Encoding ascii
    Set-Content -LiteralPath (Join-Path $releasePrerequisites 'MicrosoftEdgeWebView2Setup.exe') -Value 'old WebView2 installer' -Encoding ascii
    $releaseCollision = Join-Path $releaseDestination 'file-collision.dll'
    New-Item -ItemType Directory -Path $releaseCollision | Out-Null
    $env:BAHAMUT_SYNTHETIC_STAGE_MODE = 'Collision'
    $failureObserved = $false
    try {
        . $fixtureBuildScript -Configuration Release
    } catch {
        if ($_.Exception.Message -notlike 'package file path is a directory:*') {
            throw
        }
        $failureObserved = $true
    }
    if (-not $failureObserved -or
        (Get-Content -Raw (Join-Path $releasePrerequisites 'vc_redist.x86.exe')).Trim() -ne 'old VC++ installer' -or
        (Get-Content -Raw (Join-Path $releasePrerequisites 'MicrosoftEdgeWebView2Setup.exe')).Trim() -ne 'old WebView2 installer') {
        throw 'failed Release publish removed the old prerequisite installers'
    }
    Remove-Item -LiteralPath $releaseCollision
    $env:BAHAMUT_SYNTHETIC_STAGE_MODE = 'Success'
    . $fixtureBuildScript -Configuration Release
    if (Test-Path -LiteralPath $releasePrerequisites) {
        throw 'obsolete prerequisite installers survived Release package publication'
    }

    Write-Output 'test-windows-package: PASS'
} finally {
    if ($null -eq $previousBuildMode) {
        Remove-Item Env:BAHAMUT_SYNTHETIC_BUILD_MODE -ErrorAction SilentlyContinue
    } else {
        $env:BAHAMUT_SYNTHETIC_BUILD_MODE = $previousBuildMode
    }
    if ($null -eq $previousStageMode) {
        Remove-Item Env:BAHAMUT_SYNTHETIC_STAGE_MODE -ErrorAction SilentlyContinue
    } else {
        $env:BAHAMUT_SYNTHETIC_STAGE_MODE = $previousStageMode
    }
    Remove-SafeTestRoot $testRoot
}
