[CmdletBinding()]
param(
    [string] $LauncherBinary,
    [string] $ClientBuildDirectory,
    [string] $HelperBinary,
    [string] $StagingRoot,
    [string] $ArchivePath,
    [switch] $KeepArtifacts,
    [switch] $AllowDirtyWorktree
)

$ErrorActionPreference = 'Stop'

$expectedManifest = @(
    'bahamut-launcher.exe'
    'bahamut-update-helper.exe'
    'bahamut-loader.exe'
    'bahamut.dll'
    'plugins/screenshot.dll'
    'plugins/discord-rpc.dll'
    'LICENSE.md'
    'README.md'
    'licenses/MinHook-LICENSE.txt'
    'licenses/Dear-ImGui-LICENSE.txt'
    'licenses/Lua-COPYRIGHT.txt'
    'licenses/Inter-OFL.txt'
    'licenses/Cinzel-OFL.txt'
    'licenses/JetBrainsMono-OFL.txt'
    'licenses/Miniz-LICENSE.txt'
    'addons/chatlogs/addon.toml'
    'addons/chatlogs/chatlogs.lua'
    'addons/zonename/addon.toml'
    'addons/zonename/zonename.lua'
    'addons/packetlogger/addon.toml'
    'addons/packetlogger/packetlogger.lua'
    'addons/combatparser/addon.toml'
    'addons/combatparser/combatparser.lua'
    'addons/distance/addon.toml'
    'addons/distance/distance.lua'
    'addons/targethp/addon.toml'
    'addons/targethp/targethp.lua'
    'addons/fps/addon.toml'
    'addons/fps/fps.lua'
    'addons/pos/addon.toml'
    'addons/pos/pos.lua'
    'addons/wiki/addon.toml'
    'addons/wiki/wiki.lua'
    'addons/targetlines/addon.toml'
    'addons/targetlines/targetlines.lua'
    'scripts/default.txt'
)

$expectedDirectories = @(
    'addons/'
    'addons/chatlogs/'
    'addons/zonename/'
    'addons/packetlogger/'
    'addons/combatparser/'
    'addons/distance/'
    'addons/targethp/'
    'addons/fps/'
    'addons/pos/'
    'addons/wiki/'
    'addons/targetlines/'
    'licenses/'
    'config/'
    'config/addons/'
    'logs/packets/'
    'config/plugins/'
    'config/plugins/screenshot/'
    'logs/'
    'logs/chat/'
    'logs/launcher/'
    'plugins/'
    'plugins/dats/'
    'screenshots/'
    'scripts/'
)

function Get-NormalizedPath([string] $Path) {
    return [System.IO.Path]::GetFullPath($Path).TrimEnd('\', '/')
}

function Get-OfficialOverlayEntries([string] $Root) {
    $rootPath = Get-NormalizedPath $Root
    if (-not (Test-Path -LiteralPath $rootPath -PathType Container)) {
        throw "official overlay package is missing: $rootPath"
    }
    $rootAttributes = [System.IO.File]::GetAttributes($rootPath)
    if (($rootAttributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "official overlay package is a reparse point: $rootPath"
    }
    if (-not (Test-Path -LiteralPath (Join-Path $rootPath 'overlay.toml') -PathType Leaf)) {
        throw "official overlay manifest is missing: $rootPath"
    }

    $prefix = 'plugins/dats/bahamut-dats-overlay/'
    $pending = New-Object 'System.Collections.Generic.Stack[string]'
    $files = New-Object 'System.Collections.Generic.List[string]'
    $directories = New-Object 'System.Collections.Generic.List[string]'
    $directories.Add($prefix.TrimEnd('/') + '/')
    $pending.Push($rootPath)
    while ($pending.Count -gt 0) {
        $current = $pending.Pop()
        foreach ($entry in @(Get-ChildItem -LiteralPath $current -Force)) {
            $attributes = [System.IO.File]::GetAttributes($entry.FullName)
            if (($attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "official overlay package contains a reparse point: $($entry.FullName)"
            }
            $relative = $entry.FullName.Substring($rootPath.Length).TrimStart('\', '/') -replace '\\', '/'
            $packagePath = $prefix + $relative
            if ($entry.PSIsContainer) {
                $directories.Add($packagePath.TrimEnd('/') + '/')
                $pending.Push($entry.FullName)
            } else {
                $files.Add($packagePath)
            }
        }
    }

    return [pscustomobject]@{
        Files = @($files.ToArray() | Sort-Object)
        Directories = @($directories.ToArray() | Sort-Object)
    }
}

function Remove-SafeTree([string] $Path) {
    $fullPath = Get-NormalizedPath $Path
    $tempRoot = Get-NormalizedPath ([System.IO.Path]::GetTempPath())
    if (-not $fullPath.StartsWith($tempRoot + '\', [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to remove a path outside the temporary root: $fullPath"
    }
    if (-not (Test-Path -LiteralPath $fullPath)) {
        return
    }

    # Scan the complete tree before removing anything so junctions are unlinked
    # before ordinary contents are deleted.
    $pending = New-Object 'System.Collections.Generic.Stack[string]'
    $reparsePoints = New-Object 'System.Collections.Generic.List[string]'
    $pending.Push($fullPath)
    while ($pending.Count -gt 0) {
        $current = $pending.Pop()
        foreach ($entry in @(Get-ChildItem -LiteralPath $current -Force)) {
            $attributes = [System.IO.File]::GetAttributes($entry.FullName)
            if (($attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                $reparsePoints.Add($entry.FullName)
            } elseif ($entry.PSIsContainer) {
                $pending.Push($entry.FullName)
            }
        }
    }
    foreach ($reparsePoint in @($reparsePoints | Sort-Object Length -Descending)) {
        Remove-Item -LiteralPath $reparsePoint -Force
    }
    foreach ($entry in @(Get-ChildItem -LiteralPath $fullPath -Force)) {
        Remove-Item -LiteralPath $entry.FullName -Force -Recurse
    }
    Remove-Item -LiteralPath $fullPath -Force
}

function Get-StagingManifest([string] $Root) {
    $rootPath = Get-NormalizedPath $Root
    return @(
        Get-ChildItem -LiteralPath $rootPath -File -Recurse |
            ForEach-Object {
                $_.FullName.Substring($rootPath.Length).TrimStart('\', '/') -replace '\\', '/'
            } |
            Sort-Object
    )
}

function Get-ArchiveManifest([string] $Archive) {
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [System.IO.Compression.ZipFile]::OpenRead((Get-NormalizedPath $Archive))
    try {
        return @(
            $zip.Entries |
                Where-Object { -not $_.FullName.EndsWith('/') } |
                ForEach-Object { $_.FullName.TrimStart('/') -replace '\\', '/' } |
                Sort-Object
        )
    } finally {
        $zip.Dispose()
    }
}

function Get-StagingDirectoryManifest([string] $Root) {
    $rootPath = Get-NormalizedPath $Root
    return @(
        Get-ChildItem -LiteralPath $rootPath -Directory -Recurse |
            ForEach-Object {
                ($_.FullName.Substring($rootPath.Length).TrimStart('\', '/') -replace '\\', '/') + '/'
            } |
            Sort-Object
    )
}

function Get-ArchiveDirectoryManifest([string] $Archive) {
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [System.IO.Compression.ZipFile]::OpenRead((Get-NormalizedPath $Archive))
    try {
        return @(
            $zip.Entries |
                Where-Object { $_.FullName.EndsWith('/') } |
                ForEach-Object { $_.FullName.TrimStart('/') -replace '\\', '/' } |
                Sort-Object
        )
    } finally {
        $zip.Dispose()
    }
}

function New-ReleaseArchive([string] $Root, [string] $Archive) {
    Add-Type -AssemblyName System.IO.Compression
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $rootPath = Get-NormalizedPath $Root
    $archivePath = Get-NormalizedPath $Archive
    if (Test-Path -LiteralPath $archivePath) {
        Remove-Item -LiteralPath $archivePath -Force
    }

    $zip = [System.IO.Compression.ZipFile]::Open(
        $archivePath,
        [System.IO.Compression.ZipArchiveMode]::Create
    )
    try {
        foreach ($directory in @(Get-ChildItem -LiteralPath $rootPath -Directory -Recurse | Sort-Object FullName)) {
            $relative = $directory.FullName.Substring($rootPath.Length).TrimStart('\', '/') -replace '\\', '/'
            $null = $zip.CreateEntry($relative + '/')
        }
        foreach ($file in @(Get-ChildItem -LiteralPath $rootPath -File -Recurse | Sort-Object FullName)) {
            $relative = $file.FullName.Substring($rootPath.Length).TrimStart('\', '/') -replace '\\', '/'
            $null = [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
                $zip,
                $file.FullName,
                $relative,
                [System.IO.Compression.CompressionLevel]::Optimal
            )
        }
    } finally {
        $zip.Dispose()
    }
}

function Assert-ExactManifest([string] $Label, [string[]] $Actual, [string[]] $Expected) {
    $expected = @($Expected | Sort-Object)
    if (($Actual -join "`n") -ne ($expected -join "`n")) {
        throw "$Label manifest mismatch. Expected: $($expected -join ', '). Actual: $($Actual -join ', ')."
    }
}

function Assert-ExactDirectoryManifest([string] $Label, [string[]] $Actual, [string[]] $Expected) {
    $expected = @($Expected | Sort-Object)
    if (($Actual -join "`n") -ne ($expected -join "`n")) {
        throw "$Label directory manifest mismatch. Expected: $($expected -join ', '). Actual: $($Actual -join ', ')."
    }
}

function New-SyntheticReleaseInputs([string] $Root) {
    $launcher = Join-Path $Root 'bahamut-launcher-shell.exe'
    $helper = Join-Path $Root 'bahamut-update-helper.exe'
    $clientBuild = Join-Path $Root 'client-build'
    New-Item -ItemType Directory -Force -Path $clientBuild | Out-Null
    Set-Content -LiteralPath $launcher -Value 'synthetic launcher' -Encoding ascii
    Set-Content -LiteralPath $helper -Value 'synthetic update helper' -Encoding ascii
    Set-Content -LiteralPath (Join-Path $clientBuild 'bahamut-loader.exe') -Value 'synthetic loader' -Encoding ascii
    Set-Content -LiteralPath (Join-Path $clientBuild 'bahamut.dll') -Value 'synthetic client module' -Encoding ascii
    Set-Content -LiteralPath (Join-Path $clientBuild 'screenshot.dll') -Value 'synthetic screenshot plugin' -Encoding ascii
    Set-Content -LiteralPath (Join-Path $clientBuild 'discord-rpc.dll') -Value 'synthetic DiscordRPC plugin' -Encoding ascii
    return @($launcher, $clientBuild, $helper)
}

function windows_release_archive_manifest_and_cleanliness {
    $repoPath = Get-NormalizedPath (Join-Path $PSScriptRoot '..')
    $ownedTempRoots = [System.Collections.Generic.List[string]]::new()
    $inputRoot = $null
    $stagingPath = $null
        $archive = $null
    try {
        if (-not $StagingRoot) {
            $StagingRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("bahamut-release-staging-" + [guid]::NewGuid().ToString('N'))
            $ownedTempRoots.Add($StagingRoot)
        }
        $stagingPath = Get-NormalizedPath $StagingRoot
        if (Test-Path -LiteralPath $stagingPath) {
            if (@(Get-ChildItem -LiteralPath $stagingPath -Force).Count -ne 0) {
                throw "release staging destination is not fresh: $stagingPath"
            }
        } else {
            New-Item -ItemType Directory -Force -Path $stagingPath | Out-Null
        }

        if (-not $LauncherBinary -or -not $ClientBuildDirectory) {
            $inputRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("bahamut-release-inputs-" + [guid]::NewGuid().ToString('N'))
            $ownedTempRoots.Add($inputRoot)
            New-Item -ItemType Directory -Force -Path $inputRoot | Out-Null
            $synthetic = New-SyntheticReleaseInputs $inputRoot
            $LauncherBinary = $synthetic[0]
            $ClientBuildDirectory = $synthetic[1]
            $HelperBinary = $synthetic[2]

            $overlayFixture = Join-Path $inputRoot 'synthetic-overlay'
            $overlayPayload = Join-Path $overlayFixture 'data\1C\59\00'
            New-Item -ItemType Directory -Force -Path $overlayPayload | Out-Null
            Set-Content -LiteralPath (Join-Path $overlayFixture 'overlay.toml') -Value 'synthetic manifest' -Encoding ascii
            Set-Content -LiteralPath (Join-Path $overlayPayload 'CB.DAT') -Value 'synthetic DAT' -Encoding ascii
            $overlayFixtureEntries = Get-OfficialOverlayEntries $overlayFixture
            if ($overlayFixtureEntries.Files -notcontains 'plugins/dats/bahamut-dats-overlay/overlay.toml' -or
                $overlayFixtureEntries.Files -notcontains 'plugins/dats/bahamut-dats-overlay/data/1C/59/00/CB.DAT' -or
                $overlayFixtureEntries.Directories -notcontains 'plugins/dats/bahamut-dats-overlay/data/1C/59/00/') {
                throw 'recursive official overlay inventory did not include its nested payload and directories'
            }
        } else {
            if (-not $HelperBinary) {
                $HelperBinary = Join-Path $repoPath 'target\release\bahamut-update-helper.exe'
            }
        }

        $officialOverlayEntries = Get-OfficialOverlayEntries (Join-Path $repoPath 'plugins/dats/bahamut-dats-overlay')
        $packageExpectedFiles = @($expectedManifest) + @($officialOverlayEntries.Files)
        $packageExpectedDirectories = @($expectedDirectories) + @($officialOverlayEntries.Directories)

        & (Join-Path $PSScriptRoot 'stage-windows-release.ps1') `
            -LauncherBinary $LauncherBinary `
            -ClientBuildDirectory $ClientBuildDirectory `
            -HelperBinary $HelperBinary `
            -Destination $stagingPath | Out-Host

        if (-not $ArchivePath) {
            $ArchivePath = Join-Path ([System.IO.Path]::GetTempPath()) ("bahamut-release-" + [guid]::NewGuid().ToString('N') + '.zip')
            $ownedTempRoots.Add($ArchivePath)
        }
        $archive = Get-NormalizedPath $ArchivePath
        $archiveParent = Split-Path -Parent $archive
        New-Item -ItemType Directory -Force -Path $archiveParent | Out-Null
        New-ReleaseArchive $stagingPath $archive

        Assert-ExactManifest 'staging' (Get-StagingManifest $stagingPath) $packageExpectedFiles
        Assert-ExactManifest 'archive' (Get-ArchiveManifest $archive) $packageExpectedFiles
        Assert-ExactDirectoryManifest 'staging' (Get-StagingDirectoryManifest $stagingPath) $packageExpectedDirectories
        Assert-ExactDirectoryManifest 'archive' (Get-ArchiveDirectoryManifest $archive) $packageExpectedDirectories

        if (-not $AllowDirtyWorktree) {
            $status = @(git -C $repoPath status --porcelain --untracked-files=all)
            if ($status.Count -ne 0) {
                throw "worktree is not clean after release packaging:`n$($status -join "`n")"
            }
        }
        Write-Output 'windows_release_archive_manifest_and_cleanliness: PASS'
    } finally {
        if (-not $KeepArtifacts) {
            foreach ($root in $ownedTempRoots) {
                if ($root -and (Test-Path -LiteralPath $root)) {
                    if ([System.IO.Path]::GetExtension($root) -eq '.zip') {
                        Remove-Item -LiteralPath (Get-NormalizedPath $root) -Force
                    } else {
                        Remove-SafeTree $root
                    }
                }
            }
        }
    }
}

windows_release_archive_manifest_and_cleanliness
