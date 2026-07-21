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

function Invoke-CapturedProcess {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,
        [string[]]$ArgumentList = @(),
        [int]$TimeoutMilliseconds = 30000
    )

    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $FilePath
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    foreach ($argument in $ArgumentList) {
        $startInfo.ArgumentList.Add($argument)
    }

    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    try {
        if (-not $process.Start()) {
            throw "Failed to start captured process: $FilePath"
        }
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit($TimeoutMilliseconds)) {
            $process.Kill($true)
            throw "Process did not exit within $TimeoutMilliseconds ms: $FilePath"
        }

        return [pscustomobject]@{
            ExitCode = $process.ExitCode
            Stdout = $stdout.GetAwaiter().GetResult()
            Stderr = $stderr.GetAwaiter().GetResult()
        }
    }
    finally {
        $process.Dispose()
    }
}

function Assert-NoRedistributableRuntime {
    param(
        [Parameter(Mandatory = $true)]
        [string]$BinaryPath,
        [Parameter(Mandatory = $true)]
        [string]$Description,
        [Parameter(Mandatory = $true)]
        [string]$DumpbinPath
    )

    $dependencies = (& $DumpbinPath /dependents $BinaryPath | Out-String).ToUpperInvariant()
    # UCRTBASE.DLL and API-MS-WIN-CRT-* are Windows components and are expected.
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
            throw "$Description dynamically imports a Visual C++ Redistributable library: $runtimePrefix"
        }
    }

    return $dependencies
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
$launcher = Join-Path $extractDir "bin\procnote.exe"
$pathUpdater = Join-Path $extractDir "installer\update-user-path.ps1"
$sourceLauncher = Join-Path $PWD "src-tauri\launchers\bin\procnote-launcher.exe"
$sourcePathUpdater = Join-Path $PWD "src-tauri\nsis\update-user-path.ps1"

$metadataJson = & cargo metadata --no-deps --format-version 1 | Out-String
if ($LASTEXITCODE -ne 0) {
    throw "Could not read Cargo package metadata"
}
$metadata = $metadataJson | ConvertFrom-Json
$launcherPackages = @($metadata.packages | Where-Object { $_.name -eq "procnote-launcher" })
if ($launcherPackages.Count -ne 1) {
    throw "Expected exactly one procnote-launcher package in Cargo metadata"
}
$expectedVersionOutput = "procnote $($launcherPackages[0].version)"

if (-not (Test-Path $gui -PathType Leaf)) {
    throw "Packaged GUI executable is missing: $gui"
}
if (-not (Test-Path $launcher -PathType Leaf)) {
    throw "Packaged terminal launcher is missing: $launcher"
}
$nsisExecutables = @(Get-ChildItem $extractDir -Recurse -Filter "procnote.exe" -File)
if ($nsisExecutables.Count -ne 2) {
    throw "NSIS must contain one GUI and one console launcher; found $($nsisExecutables.Count) procnote executables"
}
if (-not (Test-Path $pathUpdater -PathType Leaf)) {
    throw "Packaged PATH updater is missing: $pathUpdater"
}
if (Test-Path (Join-Path $extractDir "cli")) {
    throw "Legacy CLI directory is still packaged"
}
if (Get-ChildItem $extractDir -Recurse -Filter "procnote.cmd" -File) {
    throw "Obsolete command-script launcher is still packaged"
}
if ((Get-FileHash $sourceLauncher).Hash -ne (Get-FileHash $launcher).Hash) {
    throw "Packaged launcher differs from its freshly built source file"
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

$msiExecutables = @(Get-ChildItem $msiExtractDir -Recurse -Filter "procnote.exe" -File)
$msiLauncherMatches = @($msiExecutables | Where-Object {
        $_.FullName.EndsWith("\bin\procnote.exe", [System.StringComparison]::OrdinalIgnoreCase)
    })
$msiGuiMatches = @($msiExecutables | Where-Object {
        -not $_.FullName.EndsWith("\bin\procnote.exe", [System.StringComparison]::OrdinalIgnoreCase)
    })
$msiPathUpdaterMatches = @(Get-ChildItem $msiExtractDir -Recurse -Filter "update-user-path.ps1" -File)
if ($msiGuiMatches.Count -ne 1) {
    throw "MSI must contain exactly one GUI executable; found $($msiGuiMatches.Count)"
}
if ($msiLauncherMatches.Count -ne 1) {
    throw "MSI does not contain exactly one console launcher under its bin directory"
}
if (Get-ChildItem $msiExtractDir -Recurse -Filter "procnote.cmd" -File) {
    throw "MSI still contains the obsolete command-script launcher"
}
if ($msiPathUpdaterMatches.Count -ne 1 -or
    -not $msiPathUpdaterMatches[0].FullName.EndsWith("\installer\update-user-path.ps1", [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "MSI does not contain the packaged PATH updater under its installer directory"
}
if ((Get-FileHash $gui).Hash -ne (Get-FileHash $msiGuiMatches[0].FullName).Hash) {
    throw "MSI and NSIS contain different GUI executables"
}
if ((Get-FileHash $sourceLauncher).Hash -ne (Get-FileHash $msiLauncherMatches[0].FullName).Hash) {
    throw "MSI launcher differs from its freshly built source file"
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

$guiHeaders = (& $dumpbin /headers $gui | Out-String).ToLowerInvariant()
if (-not $guiHeaders.Contains("machine (x64)")) {
    throw "Packaged GUI is not an x64 executable"
}
if (-not $guiHeaders.Contains("subsystem (windows gui)")) {
    throw "Packaged GUI does not use the Windows GUI subsystem"
}

$launcherHeaders = (& $dumpbin /headers $launcher | Out-String).ToLowerInvariant()
if (-not $launcherHeaders.Contains("machine (x64)")) {
    throw "Packaged launcher is not an x64 executable"
}
if (-not $launcherHeaders.Contains("subsystem (windows cui)")) {
    throw "Packaged launcher does not use the Windows console subsystem"
}

$null = Assert-NoRedistributableRuntime -BinaryPath $gui -Description "Packaged GUI" -DumpbinPath $dumpbin
$launcherDependencies = Assert-NoRedistributableRuntime `
    -BinaryPath $launcher `
    -Description "Packaged launcher" `
    -DumpbinPath $dumpbin
if ($launcherDependencies.Contains("COMCTL32.DLL")) {
    throw "Console launcher unexpectedly imports the Common Controls GUI library"
}

$process = Start-Process -FilePath $gui -ArgumentList "--version" -PassThru
if (-not $process.WaitForExit(30000)) {
    $process.Kill()
    throw "Packaged GUI did not complete --version within 30 seconds"
}
if ($process.ExitCode -ne 0) {
    throw "Packaged GUI failed --version with exit code $($process.ExitCode)"
}

$versionResult = Invoke-CapturedProcess -FilePath $launcher -ArgumentList @("--version")
if ($versionResult.ExitCode -ne 0 -or
    $versionResult.Stdout.Trim() -cne $expectedVersionOutput -or
    -not [string]::IsNullOrEmpty($versionResult.Stderr)) {
    throw "Packaged launcher produced unexpected --version output: $($versionResult.Stdout) $($versionResult.Stderr)"
}

$helpResult = Invoke-CapturedProcess -FilePath $launcher -ArgumentList @("--help")
if ($helpResult.ExitCode -ne 0 -or
    -not $helpResult.Stdout.Contains("Usage: procnote [WORKSPACE]") -or
    -not $helpResult.Stdout.Contains("--version") -or
    -not [string]::IsNullOrEmpty($helpResult.Stderr)) {
    throw "Packaged launcher produced unexpected --help output: $($helpResult.Stdout) $($helpResult.Stderr)"
}

$invalidResult = Invoke-CapturedProcess -FilePath $launcher -ArgumentList @("--not-a-procnote-option")
if ($invalidResult.ExitCode -ne 2 -or
    -not $invalidResult.Stderr.Contains("unexpected argument '--not-a-procnote-option'") -or
    -not [string]::IsNullOrEmpty($invalidResult.Stdout)) {
    throw "Packaged launcher did not report an invalid argument in the foreground"
}

if (Get-Process -Name "procnote" -ErrorAction SilentlyContinue) {
    throw "A procnote process remained after terminal-only launcher commands"
}

# Exercise the real NSIS hooks against controlled user-PATH entries. When the
# v0.0.4 installer is supplied, this performs an actual silent upgrade; otherwise
# it creates the legacy directory layout directly.
$installDir = Join-Path $env:LOCALAPPDATA "procnote"
$installedLauncher = Join-Path $installDir "bin\procnote.exe"
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
        throw "Installed launcher differs from its freshly built source file"
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

    $originalProcessPath = $env:Path
    try {
        $env:Path = [Environment]::ExpandEnvironmentVariables([string]$installedPath.Value)
        $resolvedCommands = @(Get-Command "procnote" -CommandType Application -ErrorAction Stop)
        if ($resolvedCommands.Count -ne 1 -or
            -not [string]::Equals(
                $resolvedCommands[0].Source,
                $installedLauncher,
                [System.StringComparison]::OrdinalIgnoreCase
            )) {
            throw "A fresh terminal PATH did not resolve the installed launcher: $($resolvedCommands.Source)"
        }

        $installedVersionResult = Invoke-CapturedProcess -FilePath $resolvedCommands[0].Source -ArgumentList @("--version")
        if ($installedVersionResult.ExitCode -ne 0 -or
            $installedVersionResult.Stdout.Trim() -cne $expectedVersionOutput -or
            -not [string]::IsNullOrEmpty($installedVersionResult.Stderr)) {
            throw "Installed launcher produced unexpected --version output"
        }
    }
    finally {
        $env:Path = $originalProcessPath
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
