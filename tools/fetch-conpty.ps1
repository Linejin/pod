# Downloads Microsoft's ConPTY package (conpty.dll + OpenConsole.exe, MIT)
# into vendor/conpty/. podd's build.rs copies it next to the built binaries;
# pod-holder then uses it instead of the inbox ConPTY (see docs/03 §6).
#
#   powershell -ExecutionPolicy Bypass -File tools/fetch-conpty.ps1 [-Arch x64|arm64|x86]
#
# The package version and SHA-256 are pinned; bump both together.
param([string]$Arch = "x64")
$ErrorActionPreference = "Stop"

$Version = "1.25.260930003"
$Sha256 = "02B07B349AF66D801159BDF9E440D4A1CE78BB951F37FC8609731665AFDAE7EE"
$Url = "https://api.nuget.org/v3-flatcontainer/microsoft.windows.console.conpty/$Version/microsoft.windows.console.conpty.$Version.nupkg"

$Root = Split-Path -Parent $PSScriptRoot
$Dest = Join-Path $Root "vendor\conpty"
$Tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("pod-conpty-" + [guid]::NewGuid())
New-Item -ItemType Directory -Force $Tmp | Out-Null
try {
    $Zip = Join-Path $Tmp "conpty.zip"
    Write-Host "downloading Microsoft.Windows.Console.ConPTY $Version"
    Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile $Zip
    $Hash = (Get-FileHash -Algorithm SHA256 $Zip).Hash
    if ($Hash -ne $Sha256) { throw "SHA-256 mismatch: got $Hash, expected $Sha256" }
    Expand-Archive -Path $Zip -DestinationPath (Join-Path $Tmp "x")

    New-Item -ItemType Directory -Force (Join-Path $Dest $Arch) | Out-Null
    Copy-Item (Join-Path $Tmp "x\runtimes\win-$Arch\native\conpty.dll") (Join-Path $Dest "conpty.dll") -Force
    Copy-Item (Join-Path $Tmp "x\build\native\runtimes\$Arch\OpenConsole.exe") (Join-Path $Dest "$Arch\OpenConsole.exe") -Force
    Set-Content -Encoding utf8 (Join-Path $Dest "VERSION.txt") @(
        "Microsoft.Windows.Console.ConPTY $Version ($Arch)",
        "License: MIT (https://github.com/microsoft/terminal/blob/main/LICENSE)",
        "Source: $Url",
        "SHA-256 of the .nupkg: $Sha256"
    )
    Write-Host "installed into $Dest"
} finally {
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
}
