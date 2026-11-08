#!/usr/bin/env bash
# Installs TecTec for the current user, in ~/.local (no root needed);
# PREFIX=/somewhere to choose.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
prefix="${PREFIX:-$HOME/.local}"
mkdir -p "$prefix/bin" "$prefix/lib/tectec" "$prefix/share/tectec" "$prefix/share/applications" "$prefix/share/icons/hicolor/scalable/apps"
cp "$here/tectec" "$prefix/bin/"
cp "$here"/lib/* "$prefix/lib/tectec/"
rm -rf "$prefix/share/tectec/tex-seed"
cp -r "$here/tex-seed" "$prefix/share/tectec/"
cp "$here/tectec.desktop" "$prefix/share/applications/"
cp "$here/tectec.svg" "$prefix/share/icons/hicolor/scalable/apps/"
echo "TecTec installed in $prefix/bin/tectec"
