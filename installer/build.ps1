# Builds Mesh and both helpers in release, then dist\Mesh-Setup-<version>.exe.
# Needs Inno Setup 6 (winget install JRSoftware.InnoSetup --scope user).
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
foreach ($dir in $root, "$root\mesh-wa", "$root\mesh-dc") {
    Push-Location $dir
    try {
        if ($dir -eq $root) { cargo build --release -p mesh-app } else { cargo build --release }
        if ($LASTEXITCODE) { throw "cargo build failed in $dir" }
    } finally { Pop-Location }
}
$iscc = @("$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe", "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe") |
    Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw 'Inno Setup 6 not found' }
& $iscc /Q "$PSScriptRoot\mesh.iss"
if ($LASTEXITCODE) { throw 'ISCC failed' }
Get-ChildItem "$root\dist\Mesh-Setup-*.exe" | Sort-Object LastWriteTime | Select-Object -Last 1 FullName, Length
