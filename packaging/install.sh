#!/bin/sh
# Instala Mosaic para el usuario actual (sin sudo): binario, lanzador, iconos y tipo de archivo .refs.
# `refs` queda como alias del binario (el nombre anterior).
set -e
cd "$(dirname "$0")/.."
cargo build --release
B="$HOME/.local/bin"; A="$HOME/.local/share"
mkdir -p "$B" "$A/applications" "$A/mime/packages"
install -m 755 target/release/mosaic "$B/mosaic"
ln -sf mosaic "$B/refs"
for s in 256 512; do
  mkdir -p "$A/icons/hicolor/${s}x${s}/apps"
  # nombre de icono único: el tema Papirus trae un «mosaic» de otra aplicación y los temas mandan
  install -m 644 "assets/icon-$s.png" "$A/icons/hicolor/${s}x${s}/apps/mosaic-board.png"
  rm -f "$A/icons/hicolor/${s}x${s}/apps/refs-engine.png" "$A/icons/hicolor/${s}x${s}/apps/mosaic.png"
done
# restos de cuando se llamaba refs
rm -f "$A/applications/refs-engine.desktop" "$A/mime/packages/refs-engine.xml"
install -m 644 packaging/mosaic.desktop "$A/applications/mosaic.desktop"
install -m 644 packaging/mosaic-mime.xml "$A/mime/packages/mosaic.xml"
update-mime-database "$A/mime" >/dev/null 2>&1 || true
update-desktop-database "$A/applications" >/dev/null 2>&1 || true
gtk-update-icon-cache -f -t "$A/icons/hicolor" >/dev/null 2>&1 || true
xdg-mime default mosaic.desktop application/x-mosaic-board 2>/dev/null || true
echo "Mosaic instalado en $B/mosaic (alias: refs)"
