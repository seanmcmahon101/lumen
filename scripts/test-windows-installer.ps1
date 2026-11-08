param(
    [Parameter(Mandatory)][string] $Setup,
    [Parameter(Mandatory)][string] $App
)

$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true') {
    throw 'Run this install/upgrade/uninstall test only on a disposable GitHub Actions runner'
}
$setupPath = (Resolve-Path -LiteralPath $Setup).Path
$appPath = (Resolve-Path -LiteralPath $App).Path
$version = [Diagnostics.FileVersionInfo]::GetVersionInfo($appPath).ProductVersion
$publisher = (Get-AuthenticodeSignature -LiteralPath $appPath).SignerCertificate.Thumbprint
$install = Join-Path $env:LOCALAPPDATA 'Lumen'
$shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'Lumen.lnk'
$setupArgs = '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART'

function Assert-InstalledSignatures {
    & "$PSScriptRoot/verify-windows-signature.ps1" -Path "$install/lumen.exe", "$install/unins000.exe" -ProductVersion $version -PublisherThumbprint $publisher
    if ((Get-FileHash "$install/lumen.exe").Hash -ne (Get-FileHash $appPath).Hash) {
        throw 'Installed app differs from the signed build'
    }
}

function Start-RunningLumen {
    Copy-Item "$env:SystemRoot/System32/PING.EXE" "$install/lumen.exe" -Force
    Start-Process "$install/lumen.exe" -ArgumentList '-n', '600', '127.0.0.1' -WindowStyle Hidden
    Start-Sleep -Seconds 1
    if (!(Get-Process lumen -ErrorAction SilentlyContinue)) { throw 'Stand-in lumen.exe did not start' }
}

$process = Start-Process $setupPath -ArgumentList $setupArgs -Wait -PassThru -WindowStyle Hidden
if ($process.ExitCode -ne 0 -or !(Test-Path "$install/lumen.exe") -or !(Test-Path $shortcut)) {
    throw 'Per-user installation failed'
}
Assert-InstalledSignatures
Start-RunningLumen
$process = Start-Process $setupPath -ArgumentList $setupArgs -Wait -PassThru -WindowStyle Hidden
if ($process.ExitCode -ne 0 -or (Get-Process lumen -ErrorAction SilentlyContinue)) {
    throw 'Upgrade over a running Lumen failed'
}
Assert-InstalledSignatures
Start-RunningLumen
$process = Start-Process "$install/unins000.exe" -ArgumentList $setupArgs -Wait -PassThru -WindowStyle Hidden
if ($process.ExitCode -ne 0 -or (Test-Path "$install/lumen.exe") -or (Test-Path $shortcut)) {
    throw 'Per-user uninstall failed'
}
