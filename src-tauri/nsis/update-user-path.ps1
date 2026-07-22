param(
    [Parameter(Mandatory = $true, Position = 0)]
    [ValidateSet("install", "uninstall")]
    [string]$Action,
    [Parameter(Mandatory = $true, Position = 1)]
    [string]$LauncherDirectory,
    [Parameter(Mandatory = $true, Position = 2)]
    [string]$LegacyCliDirectory
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Update-PathEntries {
    param(
        [AllowEmptyString()]
        [string]$PathValue,
        [Parameter(Mandatory = $true)]
        [string]$LauncherDirectory,
        [Parameter(Mandatory = $true)]
        [string]$LegacyCliDirectory,
        [Parameter(Mandatory = $true)]
        [bool]$Install
    )

    $entries = New-Object "System.Collections.Generic.List[string]"
    if (-not [string]::IsNullOrEmpty($PathValue)) {
        foreach ($entry in $PathValue.Split([char]";")) {
            $isLauncher = [string]::Equals($entry, $LauncherDirectory, [System.StringComparison]::OrdinalIgnoreCase)
            $isLegacyCli = [string]::Equals($entry, $LegacyCliDirectory, [System.StringComparison]::OrdinalIgnoreCase)
            if (-not $isLauncher -and -not $isLegacyCli) {
                $entries.Add($entry)
            }
        }
    }

    if ($Install) {
        $entries.Add($LauncherDirectory)
    }

    return [string]::Join(";", $entries.ToArray())
}

$key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey("Environment")
try {
    $valueExists = @($key.GetValueNames()) -contains "Path"
    $valueKind = if ($valueExists) {
        $key.GetValueKind("Path")
    }
    else {
        [Microsoft.Win32.RegistryValueKind]::ExpandString
    }

    if ($valueKind -ne [Microsoft.Win32.RegistryValueKind]::String -and
        $valueKind -ne [Microsoft.Win32.RegistryValueKind]::ExpandString) {
        throw "The user Path registry value has unsupported type $valueKind"
    }

    $pathValue = [string]$key.GetValue(
        "Path",
        "",
        [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames
    )
    $updatedPath = Update-PathEntries `
        -PathValue $pathValue `
        -LauncherDirectory $LauncherDirectory `
        -LegacyCliDirectory $LegacyCliDirectory `
        -Install ($Action -eq "install")

    $key.SetValue("Path", $updatedPath, $valueKind)
}
finally {
    if ($null -ne $key) {
        $key.Dispose()
    }
}
