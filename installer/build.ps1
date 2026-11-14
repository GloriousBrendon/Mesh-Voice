# Builds Mesh and its helpers in release, then dist\Mesh-Setup-<version>.exe:
# the one file a friend installs (SPEC-packaging). Runs on the Windows PC,
# not the Linux box: it needs the MSVC toolchain and Inno Setup 6
# (winget install JRSoftware.InnoSetup --scope user).
# mesh-node comes from a Mesh-Node checkout next to this one, or $env:MESH_NODE.
# It stops, with no setup made, if that checkout is missing, if any build
# fails, or if either checkout has changes to tracked files (P2).
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
$node = if ($env:MESH_NODE) { $env:MESH_NODE } else { Join-Path (Split-Path $root) 'Mesh-Node' }
if (-not (Test-Path "$node\Cargo.toml")) { throw "no Mesh-Node checkout at $node (set MESH_NODE)" }
foreach ($repo in $root, $node) {
    $changed = git -C $repo status --porcelain --untracked-files=no
    if ($LASTEXITCODE) { throw "git status failed in $repo" }
    if ($changed) { throw "uncommitted changes in $repo`n$($changed -join "`n")" }
}
$appCommit = git -C $root rev-parse --short HEAD
$nodeCommit = git -C $node rev-parse --short HEAD

# The C runtime linked into each binary, so a PC without Microsoft's runtime
# runs them (D4). --target keeps the flag off build scripts and proc macros.
$target = 'x86_64-pc-windows-msvc'
$oldFlags = $env:RUSTFLAGS
$env:RUSTFLAGS = '-C target-feature=+crt-static'
try {
    foreach ($dir in $root, "$root\mesh-wa", "$root\mesh-dc", $node) {
        Push-Location $dir
        try {
            if ($dir -eq $root) { cargo build --release --target $target -p mesh-app }
            elseif ($dir -eq $node) { cargo build --release --target $target -p mesh-node }
            else { cargo build --release --target $target }
            if ($LASTEXITCODE) { throw "cargo build failed in $dir" }
        } finally { Pop-Location }
    }
} finally { $env:RUSTFLAGS = $oldFlags }

$iscc = @("$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe", "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe") |
    Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw 'Inno Setup 6 not found' }
& $iscc /Q "/DNode=$node" "/DTarget=$target" "/DAppCommit=$appCommit" "/DNodeCommit=$nodeCommit" "$PSScriptRoot\mesh.iss"
if ($LASTEXITCODE) { throw 'ISCC failed' }
$setup = Get-ChildItem "$root\dist\Mesh-Setup-*.exe" | Sort-Object LastWriteTime | Select-Object -Last 1
"$($setup.FullName)  $($setup.Length) bytes  app $appCommit, engine $nodeCommit"
"SHA-256 $((Get-FileHash -Algorithm SHA256 $setup.FullName).Hash)"
