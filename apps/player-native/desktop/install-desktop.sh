#!/usr/bin/env sh
# Registers NIR Player and the `.nirpak` MIME type for the current Linux desktop user.
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
PLAYER_BIN="$SCRIPT_DIR/player-native"
if [ ! -x "$PLAYER_BIN" ]; then
  echo "error: player-native binary not found next to install-desktop.sh" >&2
  exit 1
fi

DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
APP_DIR="$DATA_HOME/applications"
MIME_DIR="$DATA_HOME/mime/packages"
mkdir -p "$APP_DIR" "$MIME_DIR"

sed "s|^Exec=.*|Exec=\"$PLAYER_BIN\" %f|" "$SCRIPT_DIR/nir-player.desktop" > "$APP_DIR/nir-player.desktop"
cp "$SCRIPT_DIR/nir-nirpak.xml" "$MIME_DIR/nir-nirpak.xml"

if command -v update-mime-database >/dev/null 2>&1; then
  update-mime-database "$DATA_HOME/mime" || true
fi
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$APP_DIR" || true
fi

echo "Installed NIR Player desktop entry & .nirpak MIME association:"
echo "  $APP_DIR/nir-player.desktop"
echo "  $MIME_DIR/nir-nirpak.xml"
