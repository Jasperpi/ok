#!/bin/sh
# Installs ok for this user, on any Linux: the program in ~/.local/bin, its
# icon and its entry in the applications menu.
#   curl -fsSL https://jasperpi.github.io/ok/install.sh | sh
set -eu
case "$(uname -m)" in
  x86_64|amd64) ARCH=x86_64 ;;
  aarch64|arm64) ARCH=aarch64 ;;
  *) echo "ok is built for x86-64 and ARM64 computers; this one is $(uname -m)." >&2; exit 1 ;;
esac
URL="https://github.com/Jasperpi/ok/releases/latest/download/ok-linux-$ARCH.tar.gz"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
echo "Downloading ok for $ARCH."
if command -v curl >/dev/null 2>&1; then curl -fsSL "$URL" -o "$TMP/ok.tar.gz"; else wget -qO "$TMP/ok.tar.gz" "$URL"; fi
tar -xzf "$TMP/ok.tar.gz" -C "$TMP"
BIN="$HOME/.local/bin"
mkdir -p "$BIN" "$HOME/.local/share/applications" "$HOME/.local/share/icons/hicolor/256x256/apps"
install -m755 "$TMP/ok/ok" "$BIN/ok"
install -m644 "$TMP/ok/ok.png" "$HOME/.local/share/icons/hicolor/256x256/apps/ok.png"
sed "s|^Exec=ok|Exec=$BIN/ok|" "$TMP/ok/ok.desktop" > "$HOME/.local/share/applications/ok.desktop"
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$HOME/.local/share/applications" >/dev/null 2>&1 || true
echo "ok is installed. Open it from your applications menu, or type ok in a terminal."
case ":$PATH:" in *":$BIN:"*) ;; *) echo "($BIN is not on your PATH yet; the menu entry works either way.)" ;; esac
