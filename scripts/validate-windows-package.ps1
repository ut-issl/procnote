param(
    [Parameter(Mandatory = $true)]
    [string]$InstallerPath,
    [Parameter(Mandatory = $true)]
    [string]$MsiPath,
    [string]$LegacyInstallerPath
)

$ErrorActionPreference = "Stop"

function Find-Executable {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,
        [string[]]$SearchRoots = @()
    )

    $command = Get-Command $Name -ErrorAction SilentlyContinue
    if ($null -ne $command) {
        return $command.Source
    }

    foreach ($root in $SearchRoots) {
        if ([string]::IsNullOrWhiteSpace($root) -or -not (Test-Path $root)) {
            continue
        }
        $match = Get-ChildItem $root -Recurse -Filter $Name -File |
            Where-Object { $_.FullName.Contains("\x64\") } |
            Sort-Object FullName -Descending |
            Select-Object -First 1
        if ($null -ne $match) {
            return $match.FullName
        }
    }

    throw "Could not find required executable: $Name"
}

function Get-UserPathSnapshot {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Environment")
    if ($null -eq $key) {
        return [pscustomobject]@{
            Exists = $false
            Value = $null
            Kind = $null
        }
    }

    try {
        $exists = @($key.GetValueNames()) -contains "Path"
        if (-not $exists) {
            return [pscustomobject]@{
                Exists = $false
                Value = $null
                Kind = $null
            }
        }

        return [pscustomobject]@{
            Exists = $true
            Value = $key.GetValue("Path", $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
            Kind = $key.GetValueKind("Path")
        }
    }
    finally {
        $key.Dispose()
    }
}

function Set-UserPath {
    param(
        [Parameter(Mandatory = $true)]
        [AllowEmptyString()]
        [string]$Value,
        [Microsoft.Win32.RegistryValueKind]$Kind = [Microsoft.Win32.RegistryValueKind]::ExpandString
    )

    $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey("Environment")
    try {
        $key.SetValue("Path", $Value, $Kind)
    }
    finally {
        $key.Dispose()
    }
}

function Restore-UserPath {
    param(
        [Parameter(Mandatory = $true)]
        [pscustomobject]$Snapshot
    )

    if ($Snapshot.Exists) {
        Set-UserPath -Value ([string]$Snapshot.Value) -Kind $Snapshot.Kind
        return
    }

    $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey("Environment")
    try {
        $key.DeleteValue("Path", $false)
    }
    finally {
        $key.Dispose()
    }
}

function Invoke-Process {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,
        [string[]]$ArgumentList = @(),
        [Parameter(Mandatory = $true)]
        [string]$Description
    )

    $process = Start-Process -FilePath $FilePath -ArgumentList $ArgumentList -PassThru -Wait
    if ($process.ExitCode -ne 0) {
        throw "$Description failed with exit code $($process.ExitCode)"
    }
}

$installer = (Resolve-Path $InstallerPath).Path
$msi = (Resolve-Path $MsiPath).Path
$legacyInstaller = if ([string]::IsNullOrWhiteSpace($LegacyInstallerPath)) {
    $null
}
else {
    (Resolve-Path $LegacyInstallerPath).Path
}
$extractDir = Join-Path $env:RUNNER_TEMP "procnote-nsis-extracted"
$msiExtractDir = Join-Path $env:RUNNER_TEMP "procnote-msi-extracted"
$manifestPath = Join-Path $env:RUNNER_TEMP "procnote.exe.manifest"
Remove-Item $extractDir, $msiExtractDir -Recurse -Force -ErrorAction SilentlyContinue
New-Item $extractDir -ItemType Directory | Out-Null

& 7z x -y "-o$extractDir" $installer | Out-Host
if ($LASTEXITCODE -ne 0) {
    throw "Failed to extract NSIS installer: $installer"
}

$gui = Join-Path $extractDir "procnote.exe"
$launcher = Join-Path $extractDir "bin\procnote.cmd"
$pathUpdater = Join-Path $extractDir "installer\update-user-path.ps1"
$sourceLauncher = Join-Path $PWD "src-tauri\launchers\windows\procnote.cmd"
$sourcePathUpdater = Join-Path $PWD "src-tauri\nsis\update-user-path.ps1"

if (-not (Test-Path $gui -PathType Leaf)) {
    throw "Packaged GUI executable is missing: $gui"
}
$nsisGuiMatches = @(Get-ChildItem $extractDir -Recurse -Filter "procnote.exe" -File)
if ($nsisGuiMatches.Count -ne 1) {
    throw "NSIS must contain exactly one GUI executable; found $($nsisGuiMatches.Count)"
}
if (-not (Test-Path $launcher -PathType Leaf)) {
    throw "Packaged terminal launcher is missing: $launcher"
}
if (-not (Test-Path $pathUpdater -PathType Leaf)) {
    throw "Packaged PATH updater is missing: $pathUpdater"
}
if (Test-Path (Join-Path $extractDir "cli")) {
    throw "Legacy CLI directory is still packaged"
}
if ((Get-FileHash $sourceLauncher).Hash -ne (Get-FileHash $launcher).Hash) {
    throw "Packaged launcher differs from its source file"
}
if ((Get-FileHash $sourcePathUpdater).Hash -ne (Get-FileHash $pathUpdater).Hash) {
    throw "Packaged PATH updater differs from its source file"
}

# An administrative install extracts MSI payloads without changing machine state.
New-Item $msiExtractDir -ItemType Directory | Out-Null
Invoke-Process `
    -FilePath "msiexec.exe" `
    -ArgumentList @("/a", "`"$msi`"", "/qn", "/norestart", "TARGETDIR=`"$msiExtractDir`"") `
    -Description "MSI administrative extraction"

$msiGuiMatches = @(Get-ChildItem $msiExtractDir -Recurse -Filter "procnote.exe" -File)
$msiLauncherMatches = @(Get-ChildItem $msiExtractDir -Recurse -Filter "procnote.cmd" -File)
$msiPathUpdaterMatches = @(Get-ChildItem $msiExtractDir -Recurse -Filter "update-user-path.ps1" -File)
if ($msiGuiMatches.Count -ne 1) {
    throw "MSI must contain exactly one GUI executable; found $($msiGuiMatches.Count)"
}
if ($msiLauncherMatches.Count -ne 1 -or
    -not $msiLauncherMatches[0].FullName.EndsWith("\bin\procnote.cmd", [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "MSI does not contain the terminal launcher under its bin directory"
}
if ($msiPathUpdaterMatches.Count -ne 1 -or
    -not $msiPathUpdaterMatches[0].FullName.EndsWith("\installer\update-user-path.ps1", [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "MSI does not contain the packaged PATH updater under its installer directory"
}
if ((Get-FileHash $gui).Hash -ne (Get-FileHash $msiGuiMatches[0].FullName).Hash) {
    throw "MSI and NSIS contain different GUI executables"
}
if ((Get-FileHash $sourceLauncher).Hash -ne (Get-FileHash $msiLauncherMatches[0].FullName).Hash) {
    throw "MSI launcher differs from its source file"
}
if ((Get-FileHash $sourcePathUpdater).Hash -ne (Get-FileHash $msiPathUpdaterMatches[0].FullName).Hash) {
    throw "MSI PATH updater differs from its source file"
}

$windowsKits = Join-Path ${env:ProgramFiles(x86)} "Windows Kits\10\bin"
$mt = Find-Executable -Name "mt.exe" -SearchRoots @($windowsKits)
& $mt -nologo "-inputresource:$gui;#1" "-out:$manifestPath"
if ($LASTEXITCODE -ne 0) {
    throw "Failed to extract the GUI application manifest"
}
$manifest = Get-Content $manifestPath -Raw
if (-not $manifest.Contains('name="Microsoft.Windows.Common-Controls"') -or -not $manifest.Contains('version="6.0.0.0"')) {
    throw "GUI application manifest does not activate Common Controls v6"
}

$visualStudioRoot = $null
$vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
if (Test-Path $vswhere) {
    $visualStudioRoot = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
}
$dumpbinRoots = if ([string]::IsNullOrWhiteSpace($visualStudioRoot)) { @() } else { @(Join-Path $visualStudioRoot "VC\Tools\MSVC") }
$dumpbin = Find-Executable -Name "dumpbin.exe" -SearchRoots $dumpbinRoots

$headers = (& $dumpbin /headers $gui | Out-String).ToLowerInvariant()
if (-not $headers.Contains("machine (x64)")) {
    throw "Packaged GUI is not an x64 executable"
}
if (-not $headers.Contains("subsystem (windows gui)")) {
    throw "Packaged executable does not use the Windows GUI subsystem"
}

$dependencies = (& $dumpbin /dependents $gui | Out-String).ToUpperInvariant()
# UCRTBASE.DLL and API-MS-WIN-CRT-* are Windows components and are expected:
# Tauri statically links the versioned VC runtime while dynamically linking UCRT.
$redistributableRuntimePrefixes = @(
    "VCRUNTIME",
    "MSVCR1",
    "MSVCP",
    "CONCRT",
    "VCAMP",
    "VCOMP"
)
foreach ($runtimePrefix in $redistributableRuntimePrefixes) {
    if ($dependencies.Contains($runtimePrefix)) {
        throw "Packaged GUI dynamically imports a Visual C++ Redistributable library: $runtimePrefix"
    }
}

$process = Start-Process -FilePath $gui -ArgumentList "--version" -PassThru
if (-not $process.WaitForExit(30000)) {
    $process.Kill()
    throw "Packaged GUI did not complete --version within 30 seconds"
}
if ($process.ExitCode -ne 0) {
    throw "Packaged GUI failed --version with exit code $($process.ExitCode)"
}

$wrapperProcess = Start-Process -FilePath $env:ComSpec -ArgumentList @("/d", "/c", "call `"$launcher`" --version") -PassThru -Wait
if ($wrapperProcess.ExitCode -ne 0) {
    throw "Packaged launcher failed with exit code $($wrapperProcess.ExitCode)"
}
Get-Process -Name "procnote" -ErrorAction SilentlyContinue | Wait-Process -Timeout 30

# Exercise the real NSIS hooks against controlled user-PATH entries. When the
# v0.0.4 installer is supplied, this performs an actual silent upgrade; otherwise
# it creates the legacy directory layout directly.
$installDir = Join-Path $env:LOCALAPPDATA "procnote"
$installedLauncher = Join-Path $installDir "bin\procnote.cmd"
$installedPathUpdater = Join-Path $installDir "installer\update-user-path.ps1"
$uninstaller = Join-Path $installDir "uninstall.exe"
$legacyDir = Join-Path $installDir "cli"
$binDir = Join-Path $installDir "bin"
$pathSnapshot = Get-UserPathSnapshot

if (Test-Path $installDir) {
    throw "Refusing to overwrite an existing validation install: $installDir"
}

$beforeEntry = "C:\procnote-path-test-before"
$legacyNeighbor = "${legacyDir}-tools"
$binNeighbor = "${binDir}-tools"
$afterEntry = "C:\procnote-path-test-after"
$paddingEntries = @("%USERPROFILE%\procnote-path-test-env", "") +
    @(0..39 | ForEach-Object { "C:\procnote-path-test-padding-{0:D3}" -f $_ })
$preservedEntries = @($beforeEntry) + $paddingEntries + @($legacyNeighbor, $binNeighbor, $afterEntry)
$initialEntries = @($beforeEntry) + $paddingEntries + @(
    $legacyDir.ToUpperInvariant(),
    $legacyNeighbor,
    $binDir.ToUpperInvariant(),
    $binNeighbor,
    $binDir,
    $afterEntry
)
$initialPath = $initialEntries -join ";"
$expectedInstalledPath = @($preservedEntries + $binDir) -join ";"
$expectedUninstalledPath = $preservedEntries -join ";"
if ($initialPath.Length -le 1024) {
    throw "PATH lifecycle fixture must exceed NSIS's string limit"
}

try {
    if ($null -ne $legacyInstaller) {
        $expectedLegacyInstallerHash = "CA310856FFF274EEAB13D9698B27C2D1F1CF681733E6861AF99FBEF418A0E7B5"
        if ((Get-FileHash $legacyInstaller).Hash -cne $expectedLegacyInstallerHash) {
            throw "v0.0.4 installer hash does not match the published artifact"
        }

        Set-UserPath -Value $beforeEntry
        Invoke-Process -FilePath $legacyInstaller -ArgumentList @("/S") -Description "v0.0.4 NSIS installation"
        if (-not (Test-Path (Join-Path $legacyDir "procnote.exe") -PathType Leaf)) {
            throw "v0.0.4 installer did not create the legacy CLI executable"
        }
        $legacyPath = Get-UserPathSnapshot
        $expectedLegacyPath = @($beforeEntry, $legacyDir) -join ";"
        if (-not $legacyPath.Exists -or
            $legacyPath.Kind -ne [Microsoft.Win32.RegistryValueKind]::ExpandString -or
            $legacyPath.Value -cne $expectedLegacyPath) {
            throw "v0.0.4 installer produced an unexpected user PATH: $($legacyPath.Value)"
        }
    }
    else {
        New-Item $legacyDir -ItemType Directory -Force | Out-Null
        Set-Content (Join-Path $legacyDir "legacy-marker.txt") "legacy CLI layout"
    }
    Set-UserPath -Value $initialPath

    Invoke-Process -FilePath $installer -ArgumentList @("/S") -Description "current NSIS installation"

    if (-not (Test-Path $installedLauncher -PathType Leaf)) {
        throw "NSIS installation did not install the terminal launcher"
    }
    if (-not (Test-Path $installedPathUpdater -PathType Leaf)) {
        throw "NSIS installation did not install its PATH updater"
    }
    if (Test-Path $legacyDir) {
        throw "NSIS installation did not remove the legacy CLI directory"
    }
    if ((Get-FileHash $sourceLauncher).Hash -ne (Get-FileHash $installedLauncher).Hash) {
        throw "Installed launcher differs from its source file"
    }
    if ((Get-FileHash $sourcePathUpdater).Hash -ne (Get-FileHash $installedPathUpdater).Hash) {
        throw "Installed PATH updater differs from its source file"
    }

    $installedPath = Get-UserPathSnapshot
    if (-not $installedPath.Exists -or
        $installedPath.Kind -ne [Microsoft.Win32.RegistryValueKind]::ExpandString -or
        $installedPath.Value -cne $expectedInstalledPath) {
        throw "NSIS installation produced an unexpected user PATH: $($installedPath.Value)"
    }

    if (-not (Test-Path $uninstaller -PathType Leaf)) {
        throw "NSIS installation did not create an uninstaller"
    }
    Invoke-Process -FilePath $uninstaller -ArgumentList @("/S") -Description "NSIS uninstallation"

    $uninstalledPath = Get-UserPathSnapshot
    if (-not $uninstalledPath.Exists -or
        $uninstalledPath.Kind -ne [Microsoft.Win32.RegistryValueKind]::ExpandString -or
        $uninstalledPath.Value -cne $expectedUninstalledPath) {
        throw "NSIS uninstallation produced an unexpected user PATH: $($uninstalledPath.Value)"
    }
}
finally {
    if (Test-Path $uninstaller -PathType Leaf) {
        $cleanup = Start-Process -FilePath $uninstaller -ArgumentList @("/S") -PassThru -Wait
        if ($cleanup.ExitCode -ne 0) {
            Write-Warning "Validation uninstaller cleanup failed with exit code $($cleanup.ExitCode)"
        }
    }
    Restore-UserPath -Snapshot $pathSnapshot
    Remove-Item $installDir -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host "Validated Windows NSIS/MSI packages and installer lifecycle: $installer; $msi"
