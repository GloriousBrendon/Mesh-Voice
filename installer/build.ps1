# Builds Mesh and its helpers in release, then dist\Mesh-Setup-<version>.exe.
# mesh-node comes from a Mesh-Node checkout next to this one, or $env:MESH_NODE;
# without one, the installer leaves it out (Mesh runs without its own network).
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
$node = if ($env:MESH_NODE) { $env:MESH_NODE } else { Join-Path (Split-Path $root) 'Mesh-Node' }
if (Test-Path "$node\Cargo.toml") {
    Push-Location $node
    try {
        cargo build --release -p mesh-node
        if ($LASTEXITCODE) { throw "cargo build failed in $node" }
    } finally { Pop-Location }
} else {
    Write-Warning "mesh-node left out: no Mesh-Node checkout at $node (set MESH_NODE)"
}
$iscc = @("$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe", "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe") |
    Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw 'Inno Setup 6 not found' }
& $iscc /Q "/DNode=$node" "$PSScriptRoot\mesh.iss"
if ($LASTEXITCODE) { throw 'ISCC failed' }
Get-ChildItem "$root\dist\Mesh-Setup-*.exe" | Sort-Object LastWriteTime | Select-Object -Last 1 FullName, Length
