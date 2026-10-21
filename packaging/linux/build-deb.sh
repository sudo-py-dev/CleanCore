#!/bin/sh
# Build the .deb from release binaries: packaging/linux/build-deb.sh <version> <bin-dir> <out-dir>
# Installs cleancore and cleancore-gui to /usr/bin, plus a menu entry and an icon.
set -eu
version="$1"
bin="$2"
out="$3"
here="$(cd "$(dirname "$0")" && pwd)"
repo="$here/../.."
root="$(mktemp -d)"
chmod 755 "$root"
trap 'rm -rf "$root"' EXIT

install -Dm755 "$bin/cleancore" "$root/usr/bin/cleancore"
install -Dm755 "$bin/cleancore-gui" "$root/usr/bin/cleancore-gui"
install -Dm644 "$here/cleancore.desktop" "$root/usr/share/applications/cleancore.desktop"
install -Dm644 "$repo/assets/logo.svg" "$root/usr/share/icons/hicolor/scalable/apps/cleancore.svg"
install -Dm644 "$repo/LICENSE" "$root/usr/share/doc/cleancore/copyright"

# Shared libraries the binaries link against, as package dependencies.
mkdir -p "$root/debian"
printf 'Source: cleancore\n' > "$root/debian/control"
depends="$(cd "$root" && dpkg-shlibdeps -O usr/bin/cleancore usr/bin/cleancore-gui 2>/dev/null | sed -n 's/^shlibs:Depends=//p')"
rm -rf "$root/debian"

size="$(du -sk "$root" | cut -f1)"
mkdir -p "$root/DEBIAN"
cat > "$root/DEBIAN/control" <<CONTROL
Package: cleancore
Version: $version
Section: utils
Priority: optional
Architecture: amd64
Maintainer: sudo-py-dev <https://github.com/sudo-py-dev/CleanCore>
Installed-Size: $size
Depends: ${depends:-libc6}
Recommends: curl, policykit-1 | pkexec
Homepage: https://github.com/sudo-py-dev/CleanCore
Description: Fast uninstaller and system cleaner
 Uninstalls programs and removes what they leave behind, cleans broken
 launchers, startup entries, orphaned settings and junk files, and backs
 up everything before it is deleted.
CONTROL

mkdir -p "$out"
dpkg-deb --root-owner-group --build "$root" "$out/cleancore_${version}_amd64.deb"
