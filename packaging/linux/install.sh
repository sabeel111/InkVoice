#!/bin/sh
# Installs InkVoice for the current user (no root needed).
#   ./install.sh              install / update
#   ./install.sh --uninstall  remove the app (your books and voices are kept)
set -e
cd "$(dirname "$0")"
SHARE="${XDG_DATA_HOME:-$HOME/.local/share}"
BIN="$HOME/.local/bin"
APPS="$SHARE/applications"
ICON="$SHARE/icons/hicolor/256x256/apps"

if [ "$1" = "--uninstall" ]; then
  rm -f "$BIN/inkvoice" "$APPS/inkvoice.desktop" "$ICON/inkvoice.png"
  echo "InkVoice removed. Your books and voices are still in $SHARE/inkvoice (delete it to free the space)."
  exit 0
fi

install -Dm755 inkvoice "$BIN/inkvoice"
install -Dm644 inkvoice.desktop "$APPS/inkvoice.desktop"
install -Dm644 inkvoice.png "$ICON/inkvoice.png"
update-desktop-database "$APPS" >/dev/null 2>&1 || true
gtk-update-icon-cache -q "$SHARE/icons/hicolor" >/dev/null 2>&1 || true

echo "InkVoice installed. Start it from your app launcher, or run: inkvoice"
case ":$PATH:" in *":$BIN:"*) ;; *) echo "Note: $BIN is not on your PATH (the launcher entry still works)." ;; esac
