#!/bin/sh
# Desinstala Mosaic del usuario actual.
A="$HOME/.local/share"
rm -f "$HOME/.local/bin/mosaic" "$HOME/.local/bin/refs" "$A/applications/mosaic.desktop" "$A/mime/packages/mosaic.xml" \
      "$A/icons/hicolor/256x256/apps/mosaic-board.png" "$A/icons/hicolor/512x512/apps/mosaic-board.png"
update-mime-database "$A/mime" >/dev/null 2>&1 || true
update-desktop-database "$A/applications" >/dev/null 2>&1 || true
echo "Mosaic desinstalado (las preferencias siguen en ~/.config/mosaic)"
