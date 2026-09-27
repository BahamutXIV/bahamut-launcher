[CmdletBinding()]
param(
    [ValidateSet('Debug', 'Release')]
    [string] $Configuration = 'Debug'
)

$ErrorActionPreference = 'Stop'

$repositoryRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$outRoot = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot 'out'))
$release = $Configuration -eq 'Release'

function Get-NormalizedPath([string] $Path) {
    return [System.IO.Path]::GetFullPath($Path).TrimEnd('\', '/')
}

function Get-SafeTreeEntries([string] $Root) {
    $rootPath = Get-NormalizedPath $Root
    $rootAttributes = [System.IO.File]::GetAttributes($rootPath)
    if (($rootAttributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Refusing to follow a reparse-point staging path: $rootPath"
    }

    $pending = New-Object 'System.Collections.Generic.Stack[string]'
    $entries = New-Object 'System.Collections.Generic.List[System.IO.FileSystemInfo]'
    $pending.Push($rootPath)
    while ($pending.Count -gt 0) {
        $current = $pending.Pop()
        foreach ($entry in @(Get-ChildItem -LiteralPath $current -Force)) {
            $attributes = [System.IO.File]::GetAttributes($entry.FullName)
            if (($attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Refusing to follow a reparse-point staging entry: $($entry.FullName)"
            }
            $entries.Add($entry)
            if ($entry.PSIsContainer) {
                $pending.Push($entry.FullName)
            }
        }
    }
    return @($entries.ToArray())
}

function Remove-SafeStagingTree([string] $Path) {
    $fullPath = Get-NormalizedPath $Path
    $normalizedOutRoot = Get-NormalizedPath $outRoot
    if ($fullPath -eq $normalizedOutRoot -or
        -not $fullPath.StartsWith($normalizedOutRoot + '\', [System.StringComparison]::OrdinalIgnoreCase) -or
        (Split-Path -Parent $fullPath) -ne $normalizedOutRoot) {
        throw "Refusing to remove a non-staging output path: $fullPath"
    }
    if (-not (Test-Path -LiteralPath $fullPath)) {
        return
    }
    Get-SafeTreeEntries $fullPath | Out-Null
    Remove-Item -LiteralPath $fullPath -Force -Recurse
}

function Assert-SafePublishPath([string] $Path) {
    $fullPath = Get-NormalizedPath $Path
    $normalizedOutRoot = Get-NormalizedPath $outRoot
    if ($fullPath -ne $normalizedOutRoot -and
        -not $fullPath.StartsWith($normalizedOutRoot + '\', [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to publish outside the repository output root: $fullPath"
    }

    $current = $fullPath
    while ($true) {
        $item = Get-Item -LiteralPath $current -Force -ErrorAction SilentlyContinue
        if ($null -ne $item) {
            $attributes = [System.IO.File]::GetAttributes($current)
            if (($attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Refusing to publish through a reparse-point path: $current"
            }
        }
        if ($current -eq $normalizedOutRoot) {
            break
        }
        $parent = Split-Path -Parent $current
        if ($parent -eq $current) {
            throw "Could not resolve publish path: $fullPath"
        }
        $current = $parent
    }
}

function Publish-StagedPackage([string] $StagingPath, [string] $DestinationPath, [bool] $RemoveLegacyPrerequisites) {
    $normalizedStagingPath = Get-NormalizedPath $StagingPath
    $normalizedDestinationPath = Get-NormalizedPath $DestinationPath
    $entries = @(Get-SafeTreeEntries $normalizedStagingPath)

    foreach ($directory in @($entries | Where-Object { $_.PSIsContainer } | Sort-Object FullName)) {
        $relative = $directory.FullName.Substring($normalizedStagingPath.Length).TrimStart('\', '/')
        $destinationDirectory = Join-Path $normalizedDestinationPath $relative
        Assert-SafePublishPath $destinationDirectory
        New-Item -ItemType Directory -Force -Path $destinationDirectory | Out-Null
    }
    foreach ($file in @($entries | Where-Object { -not $_.PSIsContainer } | Sort-Object FullName)) {
        $relative = $file.FullName.Substring($normalizedStagingPath.Length).TrimStart('\', '/')
        $destinationFile = Join-Path $normalizedDestinationPath $relative
        # Preserve the user-owned startup command; release inventory owns bundled overlay files.
        if ($relative -eq 'scripts\default.txt' -and
            (Test-Path -LiteralPath $destinationFile -PathType Leaf)) {
            continue
        }
        Assert-SafePublishPath $destinationFile
        if (Test-Path -LiteralPath $destinationFile -PathType Container) {
            throw "package file path is a directory: $destinationFile"
        }
        $destinationParent = Split-Path -Parent $destinationFile
        New-Item -ItemType Directory -Force -Path $destinationParent | Out-Null
        Copy-Item -LiteralPath $file.FullName -Destination $destinationFile -Force
    }
    if ($RemoveLegacyPrerequisites) {
        $obsoleteDirectory = Join-Path $normalizedDestinationPath 'prerequisites'
        Assert-SafePublishPath $obsoleteDirectory
        $existingDirectory = Get-Item -LiteralPath $obsoleteDirectory -Force -ErrorAction SilentlyContinue
        if ($null -ne $existingDirectory) {
            if (-not $existingDirectory.PSIsContainer) {
                throw "obsolete prerequisite path is not a directory: $obsoleteDirectory"
            }
            foreach ($fileName in @('vc_redist.x86.exe', 'MicrosoftEdgeWebView2Setup.exe')) {
                $obsoleteFile = Join-Path $obsoleteDirectory $fileName
                Assert-SafePublishPath $obsoleteFile
                $existingFile = Get-Item -LiteralPath $obsoleteFile -Force -ErrorAction SilentlyContinue
                if ($null -ne $existingFile) {
                    if ($existingFile.PSIsContainer) {
                        throw "obsolete prerequisite path is not a file: $obsoleteFile"
                    }
                    Remove-Item -LiteralPath $obsoleteFile -Force
                }
            }
            if (@(Get-ChildItem -LiteralPath $obsoleteDirectory -Force).Count -eq 0) {
                Remove-Item -LiteralPath $obsoleteDirectory
            }
        }
    }
}

if ($release) {
    $metadata = cargo metadata --manifest-path (Join-Path $repositoryRoot 'Cargo.toml') `
        --no-deps --format-version 1 |
        ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) {
        throw 'cargo metadata failed'
    }
    $version = ($metadata.packages |
        Where-Object { $_.name -eq 'bahamut-launcher-shell' } |
        Select-Object -First 1).version
    if (-not $version) {
        throw 'could not resolve the launcher package version'
    }
    $destination = Join-Path $outRoot (Join-Path 'release' $version)
    $clientBuildRoot = Join-Path $outRoot 'client-release'
    $cargoProfile = 'release'
    $helperBinary = Join-Path $repositoryRoot 'target\release\bahamut-update-helper.exe'
} else {
    $destination = Join-Path $outRoot 'dev'
    $clientBuildRoot = Join-Path $outRoot 'client-debug'
    $cargoProfile = 'debug'
    $helperBinary = $null
}

$destination = [System.IO.Path]::GetFullPath($destination)
if (-not $destination.StartsWith($outRoot + '\', [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "package destination escaped the repository output root: $destination"
}

cmake -S (Join-Path $repositoryRoot 'client') `
    -B $clientBuildRoot `
    -G 'Visual Studio 17 2022' `
    -A Win32
if ($LASTEXITCODE -ne 0) {
    throw 'client module configuration failed'
}

cmake --build $clientBuildRoot --config $Configuration
if ($LASTEXITCODE -ne 0) {
    throw 'client module build failed'
}

$cargoArguments = @(
    'build'
    '--manifest-path'
    (Join-Path $repositoryRoot 'Cargo.toml')
    '-p'
    'bahamut-launcher-shell'
)
if ($release) {
    $cargoArguments += '--release'
}
& cargo @cargoArguments
if ($LASTEXITCODE -ne 0) {
    throw 'launcher build failed'
}
if ($release) {
    & cargo build --manifest-path (Join-Path $repositoryRoot 'Cargo.toml') --release --bin bahamut-update-helper
    if ($LASTEXITCODE -ne 0) {
        throw 'update helper build failed'
    }
}

Assert-SafePublishPath $outRoot
New-Item -ItemType Directory -Force -Path $outRoot | Out-Null
$stagingPath = Join-Path $outRoot ("package-staging-" + [guid]::NewGuid().ToString('N'))
$stagingCreated = $false
try {
    New-Item -ItemType Directory -Path $stagingPath | Out-Null
    $stagingCreated = $true
    & (Join-Path $PSScriptRoot 'stage-windows-release.ps1') `
        -LauncherBinary (Join-Path $repositoryRoot "target\$cargoProfile\bahamut-launcher-shell.exe") `
        -ClientBuildDirectory (Join-Path $clientBuildRoot $Configuration) `
        -HelperBinary $helperBinary `
        -Destination $stagingPath | Out-Host
    Publish-StagedPackage $stagingPath $destination $release
} finally {
    if ($stagingCreated -and (Test-Path -LiteralPath $stagingPath)) {
        Remove-SafeStagingTree $stagingPath
    }
}

Write-Output "Bahamut Launcher package: $destination"
