#!/usr/bin/env bash
# Run by GoReleaser after it builds the executable (see .goreleaser.yaml):
# makes what the release ships beside it, in build/.
#   build/tex-seed/  the LaTeX packages the app starts with (src/backend/embedded.rs);
#                    made once by the executable itself, which downloads them,
#                    then reused (CI caches it). SEED=dir copies one instead.
#   build/lib/       the ICU libraries the executable links, whose version
#                    differs between distributions; it finds them by its RPATH.
# Then checks that a new document compiles with them alone, offline.
set -euo pipefail
bin="$1"
cd "$(dirname "$0")/.."
mkdir -p build

if [ ! -d build/tex-seed/bundles ]; then
    if [ -n "${SEED:-}" ]; then
        cp -r "$SEED" build/tex-seed
    else
        "$bin" --build-tex-seed build/tex-seed
    fi
fi

rm -rf build/lib
mkdir -p build/lib
ldd "$bin" | awk '/libicu/ { print $3 }' | while read -r lib; do
    cp -L "$lib" build/lib/
    # And what it loads in turn (libicuuc → libicudata).
    ldd "$lib" | awk '/libicu/ { print $3 }' | xargs -r -I{} cp -L {} build/lib/
done
ls build/lib

check=$(mktemp -d)
trap 'rm -rf "$check"' EXIT
# The default font size, unlike the templates (12pt).
cat > "$check/main.tex" <<'TEX'
\documentclass{article}
\usepackage[T1]{fontenc}
\usepackage[utf8]{inputenc}
\usepackage[brazil]{babel}
\usepackage{lmodern,amsmath,graphicx,hyperref}
\begin{document}
Olá — “mundo” \today. $\int_0^1 x\,dx$
\end{document}
TEX
mkdir -p "$check/app/lib"
cp "$bin" "$check/app/"
cp build/lib/* "$check/app/lib/"
run=("$check/app/tectec" --compile "$check/main.tex")
if command -v unshare >/dev/null && unshare -rn true 2>/dev/null; then
    run=(unshare -rn "${run[@]}")
fi
TECTEC_TEX_CACHE="$check/cache" TECTEC_TEX_SEED="$PWD/build/tex-seed" "${run[@]}" >/dev/null
echo "offline check: ok"
