#!/bin/sh
# Empaqueta el binario de Linux x86_64 ya compilado como bundle Flatpak (sin flatpak-builder).
# Uso: flatpak/build.sh <mosaic-linux-x86_64.tar.gz> <versión> <salida.flatpak>
# Requiere: flatpak y org.freedesktop.Platform/Sdk 25.08 instalados.
set -e
ID=io.github.apexdesk.Mosaic
TAR=$(realpath "$1"); VER=$2; OUT=$(realpath -m "$3")
HERE=$(cd "$(dirname "$0")" && pwd)
W=$(mktemp -d); trap 'rm -rf "$W"' EXIT
tar xzf "$TAR" -C "$W"
cd "$W"
flatpak build-init build $ID org.freedesktop.Sdk org.freedesktop.Platform 25.08
F=build/files
mkdir -p $F/bin $F/share/applications $F/share/metainfo $F/share/mime/packages \
  $F/share/icons/hicolor/256x256/apps $F/share/icons/hicolor/512x512/apps
install -m755 mosaic $F/bin/mosaic
cp "$HERE/../assets/icon-256.png" $F/share/icons/hicolor/256x256/apps/$ID.png
cp "$HERE/../assets/icon-512.png" $F/share/icons/hicolor/512x512/apps/$ID.png
cp "$HERE/$ID.desktop" $F/share/applications/
sed "s/mosaic-board/$ID/" "$HERE/../packaging/mosaic-mime.xml" > $F/share/mime/packages/$ID.xml
sed "s/@VERSION@/$VER/; s/@DATE@/$(date -u +%F)/" "$HERE/$ID.metainfo.xml" > $F/share/metainfo/$ID.metainfo.xml
flatpak build-finish build --command=mosaic --share=ipc --share=network --socket=wayland --socket=fallback-x11 \
  --device=dri --socket=pulseaudio --filesystem=home --filesystem=xdg-run/gvfs
flatpak build-export repo build
flatpak build-bundle repo "$OUT" $ID
