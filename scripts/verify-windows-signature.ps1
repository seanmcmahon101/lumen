param(
    [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string[]] $Path,
    [Parameter(Mandatory)][string] $ProductVersion,
    [string] $PublisherThumbprint
)

$ErrorActionPreference = 'Stop'
foreach ($file in $Path) {
    $resolved = (Resolve-Path -LiteralPath $file).Path
    # Give the uninstaller template an EXE extension for Windows' SIP selection.
    $temporary = $null
    try {
        $check = $resolved
        if ([IO.Path]::GetExtension($resolved) -in '.e32', '.e64') {
            $temporary = Join-Path ([IO.Path]::GetTempPath()) ("lumen-verify-$([guid]::NewGuid()).exe")
            Copy-Item -LiteralPath $resolved -Destination $temporary
            $check = $temporary
        }
        $signature = Get-AuthenticodeSignature -LiteralPath $check
        if ($signature.Status -ne 'Valid' -or $signature.SignatureType -ne 'Authenticode') {
            throw "Invalid or missing embedded Authenticode signature on $file ($($signature.Status))"
        }
        if (!$signature.TimeStamperCertificate) { throw "Missing trusted timestamp on $file" }
        if ($PublisherThumbprint -and $signature.SignerCertificate.Thumbprint -ne $PublisherThumbprint) {
            throw "Unexpected signing certificate on $file"
        }
        $info = [Diagnostics.FileVersionInfo]::GetVersionInfo($check)
        if ($info.ProductName -cne 'Lumen' -or $info.ProductVersion -cne $ProductVersion) {
            throw "Unexpected product metadata on $file"
        }
        Write-Host "Verified $file - $($signature.SignerCertificate.Subject)"
    } finally {
        if ($temporary) { Remove-Item -LiteralPath $temporary -Force }
    }
}
