#!/bin/sh
# Packages a built ok for Linux four ways, for one processor:
#   ok-linux-<arch>.tar.gz     the program, its icon and menu entry
#   ok-linux-<arch>.AppImage   one file that runs on any distribution
#   ok_<version>_<debarch>.deb for Debian, Ubuntu, Mint, Pop!_OS
#   ok-<version>.<rpmarch>.rpm for Fedora, openSUSE, RHEL
#
# usage: package.sh <binary> <arch: x86_64|aarch64> <version> <out dir> [icons dir]
# The icons dir is what `ok --icons` writes; without it the binary is run
# to make them, which only works for this machine's own processor.
# Needs: tar, dpkg-deb, rpmbuild, and appimagetool on PATH (or APPIMAGETOOL).
set -eu

BIN=$1
ARCH=$2
VERSION=$3
OUT=$4
HERE=$(cd "$(dirname "$0")" && pwd)
WORK=$(mktemp -d)
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)

case "$ARCH" in
  x86_64) DEBARCH=amd64 ;;
  aarch64) DEBARCH=arm64 ;;
  *) echo "unknown arch $ARCH" >&2; exit 1 ;;
esac

if [ -n "${5:-}" ]; then cp -r "$5" "$WORK/icons"; else "$BIN" --icons "$WORK/icons"; fi

# The tarball.
mkdir -p "$WORK/ok"
install -m755 "$BIN" "$WORK/ok/ok"
install -m644 "$WORK/icons/icon_256x256.png" "$WORK/ok/ok.png"
install -m644 "$HERE/ok.desktop" "$WORK/ok/ok.desktop"
install -m644 "$HERE/INSTALL.txt" "$WORK/ok/INSTALL.txt"
tar -C "$WORK" -czf "$OUT/ok-linux-$ARCH.tar.gz" ok

# The files every package puts in the same places.
stage() {
  root=$1
  install -Dm755 "$BIN" "$root/usr/bin/ok"
  install -Dm644 "$HERE/ok.desktop" "$root/usr/share/applications/ok.desktop"
  for size in 16 32 128 256 512; do
    install -Dm644 "$WORK/icons/icon_${size}x${size}.png" "$root/usr/share/icons/hicolor/${size}x${size}/apps/ok.png"
  done
}

# The Debian package.
stage "$WORK/deb"
mkdir -p "$WORK/deb/DEBIAN"
cat > "$WORK/deb/DEBIAN/control" <<EOF
Package: ok
Version: $VERSION
Architecture: $DEBARCH
Maintainer: Jasper Meller <noreply@github.com>
Section: editors
Priority: optional
Homepage: https://github.com/Jasperpi/ok
Recommends: libx11-6, libxcursor1, wl-clipboard | xclip | xsel
Description: A writing tool: one sheet of Gallant
 One window, a character grid drawn in Sun Gallant, and a machine that
 says ok when your words are on disk.
EOF
dpkg-deb --root-owner-group --build "$WORK/deb" "$OUT/ok_${VERSION}_${DEBARCH}.deb"

# The RPM.
stage "$WORK/rpmroot"
mkdir -p "$WORK/rpm/SPECS"
cat > "$WORK/rpm/SPECS/ok.spec" <<EOF
Name: ok
Version: $VERSION
Release: 1
Summary: A writing tool: one sheet of Gallant
License: See /usr/share/doc
URL: https://github.com/Jasperpi/ok
BuildArch: $ARCH
AutoReqProv: no
Requires: libX11

%description
One window, a character grid drawn in Sun Gallant, and a machine that says
ok when your words are on disk.

%install
cp -a $WORK/rpmroot/. %{buildroot}/

%files
/usr/bin/ok
/usr/share/applications/ok.desktop
/usr/share/icons/hicolor/*/apps/ok.png
EOF
rpmbuild --define "_topdir $WORK/rpm" --define "_build_id_links none" --target "$ARCH" -bb "$WORK/rpm/SPECS/ok.spec" >/dev/null
cp "$WORK"/rpm/RPMS/"$ARCH"/ok-*.rpm "$OUT/ok-$VERSION.$ARCH.rpm"

# The AppImage.
APPDIR="$WORK/ok.AppDir"
stage "$APPDIR"
cp "$HERE/ok.desktop" "$APPDIR/ok.desktop"
cp "$WORK/icons/icon_256x256.png" "$APPDIR/ok.png"
cat > "$APPDIR/AppRun" <<'EOF'
#!/bin/sh
HERE=$(dirname "$(readlink -f "$0")")
exec "$HERE/usr/bin/ok" "$@"
EOF
chmod 755 "$APPDIR/AppRun"
TOOL=${APPIMAGETOOL:-appimagetool}
RUNTIME_ARG=""
if [ -n "${APPIMAGE_RUNTIME:-}" ]; then RUNTIME_ARG="--runtime-file $APPIMAGE_RUNTIME"; fi
# shellcheck disable=SC2086
ARCH=$ARCH "$TOOL" --appimage-extract-and-run $RUNTIME_ARG "$APPDIR" "$OUT/ok-linux-$ARCH.AppImage" >/dev/null 2>&1 \
  || ARCH=$ARCH "$TOOL" $RUNTIME_ARG "$APPDIR" "$OUT/ok-linux-$ARCH.AppImage"

rm -rf "$WORK"
ls -la "$OUT"
