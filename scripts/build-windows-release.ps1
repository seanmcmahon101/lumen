param([Parameter(Mandatory)][string] $Tag)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

rustup update stable --no-self-update
$metadata = cargo +stable metadata --locked --no-deps --format-version 1 | ConvertFrom-Json
$package = @($metadata.packages | Where-Object name -EQ 'lumen')[0]
$version = $package.version
if ($Tag -cne "v$version" -or $version -cnotmatch '^[0-9]+\.[0-9]+\.[0-9]+$') {
    throw 'Release tag must match the stable version in Cargo.toml'
}

# SignPath Foundation requires product name and version metadata on every PE.
# Link a VERSIONINFO resource without adding dependencies to historical tags.
$sdkRoot = (Get-ItemProperty 'HKLM:/SOFTWARE/Microsoft/Windows Kits/Installed Roots').KitsRoot10
$resourceCompiler = Get-ChildItem "$sdkRoot/bin/*/x64/rc.exe" |
    Sort-Object { [version]$_.Directory.Parent.Name } -Descending | Select-Object -First 1
if (!$resourceCompiler) { throw 'Windows SDK resource compiler not found' }
$output = New-Item -ItemType Directory -Force 'target/windows-resource'
$resource = Join-Path $output.FullName 'lumen.res'
$numeric = $version.Replace('.', ',') + ',0'
@"
1 VERSIONINFO
FILEVERSION $numeric
PRODUCTVERSION $numeric
FILEOS 0x40004
FILETYPE 1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "Sean McMahon\0"
      VALUE "FileDescription", "Lumen Windows launcher\0"
      VALUE "FileVersion", "$version.0\0"
      VALUE "InternalName", "lumen\0"
      VALUE "OriginalFilename", "lumen.exe\0"
      VALUE "ProductName", "Lumen\0"
      VALUE "ProductVersion", "$version.0\0"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x0409, 1200
  END
END
"@ | Set-Content (Join-Path $output.FullName 'lumen.rc') -Encoding utf8
& $resourceCompiler.FullName /nologo "/fo$resource" (Join-Path $output.FullName 'lumen.rc')

cargo +stable test --locked --release --target x86_64-pc-windows-msvc
cargo +stable rustc --locked --release --target x86_64-pc-windows-msvc --bin lumen -- -C "link-arg=$resource"
$app = (Resolve-Path 'target/x86_64-pc-windows-msvc/release/lumen.exe').Path
$info = [Diagnostics.FileVersionInfo]::GetVersionInfo($app)
if ($info.ProductName -cne 'Lumen' -or $info.ProductVersion -cne "$version.0") {
    throw 'Built app is missing the required Windows product metadata'
}
"APP_VERSION=$version" >> $env:GITHUB_ENV
"APP_EXE=$app" >> $env:GITHUB_ENV
"WINDOWS_SDK_BIN=$($resourceCompiler.Directory.FullName)" >> $env:GITHUB_ENV
