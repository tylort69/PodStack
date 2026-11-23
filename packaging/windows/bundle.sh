#!/usr/bin/env bash
# Collects a release build of PodStack and the GTK runtime it needs into one folder that runs on a Windows machine
# without MSYS2. Run it in an MSYS2 UCRT64 shell after `cargo build --release -p podstack`.
#
#   packaging/windows/bundle.sh <output folder>
#
# Layout (the GTK convention, so GLib finds share/ and lib/ next to bin/):
#   bin/podstack.exe and the DLLs it loads, lib/gdk-pixbuf-2.0 (image loaders), share/glib-2.0/schemas,
#   share/icons (the icons GTK's own widgets draw: arrows, checkmarks, the spinner).
set -euo pipefail
out=${1:?usage: bundle.sh <output folder>}
prefix=${MINGW_PREFIX:-/ucrt64}

rm -rf "$out"
mkdir -p "$out/bin" "$out/lib" "$out/share/glib-2.0/schemas" "$out/share/icons"
cp target/release/podstack.exe "$out/bin/"

# Every DLL a binary loads from the MSYS2 prefix, repeated over what was copied so loaders' own DLLs come along.
copy_dlls() {
    ldd "$@" | awk -v p="$prefix/" 'index($3, p) == 1 { print $3 }' | sort -u | while read -r dll; do
        cp -n "$dll" "$out/bin/"
    done
}

cp -r "$prefix/lib/gdk-pixbuf-2.0" "$out/lib/"
copy_dlls "$out/bin/podstack.exe" "$out"/lib/gdk-pixbuf-2.0/2.10.0/loaders/*.dll

cp "$prefix/share/glib-2.0/schemas/gschemas.compiled" "$out/share/glib-2.0/schemas/"
cp -r "$prefix/share/icons/hicolor" "$prefix/share/icons/Adwaita" "$out/share/icons/"
# The window icon, found by the app id like on Linux.
install -Dm644 assets/icon.png "$out/share/icons/hicolor/512x512/apps/io.github.tylort69.PodStack.png"
# GTK trusts the theme's icon cache, which doesn't know the icon just added.
gtk4-update-icon-cache -q -t -f "$out/share/icons/hicolor"
cp LICENSE.md "$out/"

echo "bundled $(ls "$out/bin" | wc -l) files in bin, $(du -sh "$out" | cut -f1) in all"
