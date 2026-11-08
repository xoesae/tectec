#!/usr/bin/env bash
# Installs or updates TecTec to the latest release, for the current user, in
# ~/.local (no root needed). Run it again any time to update.
#
#   curl -fsSL https://raw.githubusercontent.com/xoesae/tectec/main/packaging/install.sh | bash
#   ./install.sh            the same, from a downloaded copy
#   ./install.sh --local    installs the files beside this script instead
#                           (from an extracted release archive, offline)
#
# PREFIX=/somewhere          where to install (default ~/.local)
# TECTEC_VERSION=0.2.0       a given release instead of the latest
# GITHUB_TOKEN=...           only needed while the repository is private,
#                            or to avoid GitHub's API rate limit
set -euo pipefail
repo="xoesae/tectec"
prefix="${PREFIX:-$HOME/.local}"
version_file="$prefix/share/tectec/VERSION"

die() { echo "install.sh: $*" >&2; exit 1; }

# Copies the extracted release in $1 into $prefix.
install_from() {
    local src="$1" version="$2"
    [ -x "$src/tectec" ] || die "no tectec executable in $src"
    mkdir -p "$prefix/bin" "$prefix/lib" "$prefix/share/tectec" "$prefix/share/applications" "$prefix/share/icons/hicolor/scalable/apps"
    # Copied beside it, then renamed over it: works while TecTec is open
    # (cp onto a running executable fails with "Text file busy").
    cp "$src/tectec" "$prefix/bin/.tectec.new"
    chmod 755 "$prefix/bin/.tectec.new"
    mv -f "$prefix/bin/.tectec.new" "$prefix/bin/tectec"
    # Replaced whole, so nothing a previous version shipped is left behind.
    rm -rf "$prefix/lib/tectec" "$prefix/share/tectec/tex-seed"
    cp -r "$src/lib" "$prefix/lib/tectec"
    cp -r "$src/tex-seed" "$prefix/share/tectec/"
    cp "$src/tectec.desktop" "$prefix/share/applications/"
    cp "$src/tectec.svg" "$prefix/share/icons/hicolor/scalable/apps/"
    echo "$version" > "$version_file"
    command -v update-desktop-database >/dev/null && update-desktop-database -q "$prefix/share/applications" 2>/dev/null || true
    echo "TecTec $version installed in $prefix/bin/tectec"
    case ":$PATH:" in
        *":$prefix/bin:"*) ;;
        *) echo "Note: $prefix/bin is not in your PATH." ;;
    esac
}

if [ "${1:-}" = "--local" ]; then
    here="$(cd "$(dirname "$0")" && pwd)"
    # The archive's folder is tectec-<version>-linux-x86_64.
    version="$(basename "$here" | sed -n 's/^tectec-\(.*\)-linux-x86_64$/\1/p')"
    install_from "$here" "${version:-unknown}"
    exit 0
fi

[ "$(uname -s)-$(uname -m)" = "Linux-x86_64" ] || die "releases are for Linux x86_64 only"
command -v curl >/dev/null || die "curl is needed"
command -v tar >/dev/null || die "tar is needed"
command -v sha256sum >/dev/null || die "sha256sum is needed"

auth=()
[ -n "${GITHUB_TOKEN:-}" ] && auth=(-H "Authorization: Bearer $GITHUB_TOKEN")

if [ -n "${TECTEC_VERSION:-}" ]; then
    tag="v${TECTEC_VERSION#v}"
else
    tag="$(curl -fsSL "${auth[@]}" -H "Accept: application/vnd.github+json" \
        "https://api.github.com/repos/$repo/releases/latest" \
        | sed -n 's/^ *"tag_name": *"\([^"]*\)".*/\1/p' | head -n1)" \
        || die "could not reach GitHub for the latest release"
    [ -n "$tag" ] || die "no published release found for $repo"
fi
version="${tag#v}"

installed="$(cat "$version_file" 2>/dev/null || true)"
if [ "$installed" = "$version" ] && [ -x "$prefix/bin/tectec" ]; then
    echo "TecTec $version is already installed (the latest)."
    exit 0
fi
echo "Downloading TecTec $version${installed:+ (installed: $installed)}..."

name="tectec-$version-linux-x86_64"
base="https://github.com/$repo/releases/download/$tag"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -fL --progress-bar "${auth[@]}" -o "$tmp/$name.tar.xz" "$base/$name.tar.xz" \
    || die "could not download $base/$name.tar.xz"
curl -fsSL "${auth[@]}" -o "$tmp/checksums.txt" "$base/checksums.txt" \
    || die "could not download the checksums"
(cd "$tmp" && grep " $name.tar.xz\$" checksums.txt | sha256sum -c --quiet -) \
    || die "checksum mismatch for $name.tar.xz"
tar -xJf "$tmp/$name.tar.xz" -C "$tmp"
install_from "$tmp/$name" "$version"
