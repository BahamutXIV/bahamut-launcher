[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string] $LauncherBinary,

    [Parameter(Mandatory)]
    [string] $ClientBuildDirectory,


    [string] $HelperBinary,

    [Parameter(Mandatory)]
    [string] $Destination
)

$ErrorActionPreference = 'Stop'

$repositoryRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$launcherPath = (Resolve-Path -LiteralPath $LauncherBinary).Path
$clientBuildPath = (Resolve-Path -LiteralPath $ClientBuildDirectory).Path
$destinationPath = [System.IO.Path]::GetFullPath($Destination)
$officialOverlaySource = Join-Path $repositoryRoot 'plugins/dats/bahamut-dats-overlay'

if (Test-Path -LiteralPath $destinationPath) {
    $existing = @(Get-ChildItem -LiteralPath $destinationPath -Force)
    if ($existing.Count -ne 0) {
        throw "release staging destination is not empty: $destinationPath"
    }
}

$licenseDestination = Join-Path $destinationPath 'licenses'
$chatlogsAddonDestination = Join-Path $destinationPath 'addons\chatlogs'
$zonenameAddonDestination = Join-Path $destinationPath 'addons\zonename'
$packetloggerAddonDestination = Join-Path $destinationPath 'addons\packetlogger'
$combatParserAddonDestination = Join-Path $destinationPath 'addons\combatparser'
$distanceAddonDestination = Join-Path $destinationPath 'addons\distance'
$targetHpAddonDestination = Join-Path $destinationPath 'addons\targethp'
$fpsAddonDestination = Join-Path $destinationPath 'addons\fps'
$posAddonDestination = Join-Path $destinationPath 'addons\pos'
$wikiAddonDestination = Join-Path $destinationPath 'addons\wiki'
New-Item -ItemType Directory -Force -Path $licenseDestination | Out-Null
foreach ($directory in @(
    $chatlogsAddonDestination
    $zonenameAddonDestination
    $packetloggerAddonDestination
    $combatParserAddonDestination
    $distanceAddonDestination
    $targetHpAddonDestination
    $fpsAddonDestination
    $posAddonDestination
    $wikiAddonDestination
    (Join-Path $destinationPath 'plugins\dats')
    (Join-Path $destinationPath 'config\addons')
    (Join-Path $destinationPath 'config\plugins')
    (Join-Path $destinationPath 'config\plugins\screenshot')
    (Join-Path $destinationPath 'scripts')
    (Join-Path $destinationPath 'logs\launcher')
    (Join-Path $destinationPath 'logs\chat')
    (Join-Path $destinationPath 'logs\packets')
    (Join-Path $destinationPath 'screenshots')
)) {
    New-Item -ItemType Directory -Force -Path $directory | Out-Null
}

$copies = @(
    @{ Source = $launcherPath; Destination = Join-Path $destinationPath 'bahamut-launcher.exe' }
    @{ Source = Join-Path $clientBuildPath 'bahamut-loader.exe'; Destination = Join-Path $destinationPath 'bahamut-loader.exe' }
    @{ Source = Join-Path $clientBuildPath 'bahamut.dll'; Destination = Join-Path $destinationPath 'bahamut.dll' }
    @{ Source = Join-Path $clientBuildPath 'screenshot.dll'; Destination = Join-Path $destinationPath 'plugins\screenshot.dll' }
    @{ Source = Join-Path $clientBuildPath 'discord-rpc.dll'; Destination = Join-Path $destinationPath 'plugins\discord-rpc.dll' }
    @{ Source = Join-Path $repositoryRoot 'LICENSE.md'; Destination = Join-Path $destinationPath 'LICENSE.md' }
    @{ Source = Join-Path $repositoryRoot 'docs/getting-started.md'; Destination = Join-Path $destinationPath 'README.md' }
    @{ Source = Join-Path $repositoryRoot 'client/vendor/minhook/LICENSE.txt'; Destination = Join-Path $licenseDestination 'MinHook-LICENSE.txt' }
    @{ Source = Join-Path $repositoryRoot 'client/vendor/imgui/LICENSE.txt'; Destination = Join-Path $licenseDestination 'Dear-ImGui-LICENSE.txt' }
    @{ Source = Join-Path $repositoryRoot 'client/vendor/lua/COPYRIGHT'; Destination = Join-Path $licenseDestination 'Lua-COPYRIGHT.txt' }
    @{ Source = Join-Path $repositoryRoot 'client/vendor/miniz/LICENSE'; Destination = Join-Path $licenseDestination 'Miniz-LICENSE.txt' }
    @{ Source = Join-Path $repositoryRoot 'src-tauri/ui/assets/licenses/Inter-OFL.txt'; Destination = Join-Path $licenseDestination 'Inter-OFL.txt' }
    @{ Source = Join-Path $repositoryRoot 'src-tauri/ui/assets/licenses/Cinzel-OFL.txt'; Destination = Join-Path $licenseDestination 'Cinzel-OFL.txt' }
    @{ Source = Join-Path $repositoryRoot 'src-tauri/ui/assets/licenses/JetBrainsMono-OFL.txt'; Destination = Join-Path $licenseDestination 'JetBrainsMono-OFL.txt' }
    @{ Source = Join-Path $repositoryRoot 'addons/chatlogs/addon.toml'; Destination = Join-Path $chatlogsAddonDestination 'addon.toml' }
    @{ Source = Join-Path $repositoryRoot 'addons/chatlogs/chatlogs.lua'; Destination = Join-Path $chatlogsAddonDestination 'chatlogs.lua' }
    @{ Source = Join-Path $repositoryRoot 'addons/zonename/addon.toml'; Destination = Join-Path $zonenameAddonDestination 'addon.toml' }
    @{ Source = Join-Path $repositoryRoot 'addons/zonename/zonename.lua'; Destination = Join-Path $zonenameAddonDestination 'zonename.lua' }
    @{ Source = Join-Path $repositoryRoot 'addons/packetlogger/addon.toml'; Destination = Join-Path $packetloggerAddonDestination 'addon.toml' }
    @{ Source = Join-Path $repositoryRoot 'addons/packetlogger/packetlogger.lua'; Destination = Join-Path $packetloggerAddonDestination 'packetlogger.lua' }
    @{ Source = Join-Path $repositoryRoot 'addons/combatparser/addon.toml'; Destination = Join-Path $combatParserAddonDestination 'addon.toml' }
    @{ Source = Join-Path $repositoryRoot 'addons/combatparser/combatparser.lua'; Destination = Join-Path $combatParserAddonDestination 'combatparser.lua' }
    @{ Source = Join-Path $repositoryRoot 'addons/distance/addon.toml'; Destination = Join-Path $distanceAddonDestination 'addon.toml' }
    @{ Source = Join-Path $repositoryRoot 'addons/distance/distance.lua'; Destination = Join-Path $distanceAddonDestination 'distance.lua' }
    @{ Source = Join-Path $repositoryRoot 'addons/targethp/addon.toml'; Destination = Join-Path $targetHpAddonDestination 'addon.toml' }
    @{ Source = Join-Path $repositoryRoot 'addons/targethp/targethp.lua'; Destination = Join-Path $targetHpAddonDestination 'targethp.lua' }
    @{ Source = Join-Path $repositoryRoot 'addons/fps/addon.toml'; Destination = Join-Path $fpsAddonDestination 'addon.toml' }
    @{ Source = Join-Path $repositoryRoot 'addons/fps/fps.lua'; Destination = Join-Path $fpsAddonDestination 'fps.lua' }
    @{ Source = Join-Path $repositoryRoot 'addons/pos/addon.toml'; Destination = Join-Path $posAddonDestination 'addon.toml' }
    @{ Source = Join-Path $repositoryRoot 'addons/pos/pos.lua'; Destination = Join-Path $posAddonDestination 'pos.lua' }
    @{ Source = Join-Path $repositoryRoot 'addons/wiki/addon.toml'; Destination = Join-Path $wikiAddonDestination 'addon.toml' }
    @{ Source = Join-Path $repositoryRoot 'addons/wiki/wiki.lua'; Destination = Join-Path $wikiAddonDestination 'wiki.lua' }
    @{ Source = Join-Path $repositoryRoot 'scripts/default.txt'; Destination = Join-Path $destinationPath 'scripts\default.txt' }
)

if ($HelperBinary) {
    $copies += @{
        Source = (Resolve-Path -LiteralPath $HelperBinary).Path
        Destination = Join-Path $destinationPath 'bahamut-update-helper.exe'
    }
}

foreach ($copy in $copies) {
    if (-not (Test-Path -LiteralPath $copy.Source -PathType Leaf)) {
        throw "release staging source is missing: $($copy.Source)"
    }
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $copy.Destination) | Out-Null
    Copy-Item -LiteralPath $copy.Source -Destination $copy.Destination
}

if (-not (Test-Path -LiteralPath $officialOverlaySource -PathType Container)) {
    throw "official overlay package is missing: $officialOverlaySource"
}
$overlayAttributes = [System.IO.File]::GetAttributes($officialOverlaySource)
if (($overlayAttributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
    throw "official overlay package is a reparse point: $officialOverlaySource"
}
$overlayDestination = Join-Path $destinationPath 'plugins\dats\bahamut-dats-overlay'
New-Item -ItemType Directory -Force -Path $overlayDestination | Out-Null
$pendingOverlayDirectories = New-Object 'System.Collections.Generic.Stack[string]'
$pendingOverlayDirectories.Push($officialOverlaySource)
while ($pendingOverlayDirectories.Count -gt 0) {
    $currentDirectory = $pendingOverlayDirectories.Pop()
    foreach ($entry in @(Get-ChildItem -LiteralPath $currentDirectory -Force)) {
        $attributes = [System.IO.File]::GetAttributes($entry.FullName)
        if (($attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "official overlay package contains a reparse point: $($entry.FullName)"
        }
        $relative = $entry.FullName.Substring($officialOverlaySource.Length).TrimStart('\', '/')
        $destinationEntry = Join-Path $overlayDestination $relative
        if ($entry.PSIsContainer) {
            New-Item -ItemType Directory -Force -Path $destinationEntry | Out-Null
            $pendingOverlayDirectories.Push($entry.FullName)
        } else {
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent $destinationEntry) | Out-Null
            Copy-Item -LiteralPath $entry.FullName -Destination $destinationEntry
        }
    }
}

$stagedFiles = Get-ChildItem -LiteralPath $destinationPath -File -Recurse |
    Sort-Object FullName
foreach ($file in $stagedFiles) {
    $relative = $file.FullName.Substring($destinationPath.Length).TrimStart('\', '/')
    Write-Output "$relative ($($file.Length) bytes)"
}
