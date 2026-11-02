#!/bin/bash
# Packages release builds of Mesh and mesh-wa as an AppImage and a tarball.
# Build them first:
#
#   cargo build --release -p mesh-app
#   cargo build --release --manifest-path mesh-wa/Cargo.toml
#   installer/package-appimage.sh 0.1.0 dist
#
# Writes dist/Mesh-<version>-x86_64.AppImage and dist/mesh-<version>-linux-x86_64.tar.gz.
# Only Mesh's own binaries go in: audio, Wayland/X11, xkbcommon, Vulkan and glibc
# come from the host. mesh-dc stays out (it needs the Discord VPN wrapper).
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
version=${1:?usage: package-appimage.sh <version> [outdir]}
out=$(mkdir -p "${2:-$root/dist}" && cd "${2:-$root/dist}" && pwd)
app="$root/target/release/mesh-app"
wa="$root/mesh-wa/target/release/mesh-wa"
icon="$root/crates/mesh-app/assets/mesh.png"
for f in "$app" "$wa" "$icon"; do [[ -f $f ]] || { echo "missing $f (build first)" >&2; exit 1; }; done

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

desktop() {
  cat <<EOF
[Desktop Entry]
Type=Application
Name=Mesh
GenericName=Chat client
Comment=Matrix, WhatsApp and Discord in one lightweight client
Exec=$1
Icon=mesh
StartupWMClass=mesh
Terminal=false
Categories=Network;InstantMessaging;Chat;
EOF
}

# AppImage. Mesh finds mesh-wa next to its own binary, so both go in usr/bin.
dir="$work/Mesh.AppDir"
install -Dm755 "$app" "$dir/usr/bin/mesh"
install -Dm755 "$wa" "$dir/usr/bin/mesh-wa"
install -Dm644 "$icon" "$dir/usr/share/icons/hicolor/256x256/apps/mesh.png"
install -Dm644 "$icon" "$dir/mesh.png"
desktop mesh > "$dir/mesh.desktop"
cat > "$dir/AppRun" <<'EOF'
#!/bin/sh
here=$(dirname "$(readlink -f "$0")")
exec "$here/usr/bin/mesh" "$@"
EOF
chmod 755 "$dir/AppRun"

tool="$work/appimagetool"
curl -fsSL -o "$tool" \
  https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
chmod 755 "$tool"
# --appimage-extract-and-run: CI runners and containers often have no FUSE.
ARCH=x86_64 VERSION="$version" "$tool" --appimage-extract-and-run --no-appstream \
  "$dir" "$out/Mesh-$version-x86_64.AppImage"

# Tarball, for people who'd rather put the binaries on their PATH themselves.
tar_dir="$work/mesh-$version"
install -Dm755 "$app" "$tar_dir/mesh"
install -Dm755 "$wa" "$tar_dir/mesh-wa"
install -Dm644 "$icon" "$tar_dir/mesh.png"
desktop mesh > "$tar_dir/mesh.desktop"
install -Dm644 "$root/README.md" "$tar_dir/README.md"
install -Dm644 "$root/theme.example.toml" "$tar_dir/theme.example.toml"
tar -C "$work" -czf "$out/mesh-$version-linux-x86_64.tar.gz" "mesh-$version"

ls -lh "$out"
