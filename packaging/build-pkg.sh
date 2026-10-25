#!/usr/bin/env bash
# Build the release assets in dist/ from target/release/novarch:
#   novarch-<version>-1-x86_64.pkg.tar.zst, novarch-x86_64.pkg.tar.zst (same file, stable
#   name for install.sh), novarch-x86_64 (bare binary), install.sh, SHA256SUMS.
# Run as a normal user on Arch (makepkg refuses root).
set -euo pipefail
cd "$(dirname "$0")/.."
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cp target/release/novarch LICENSE "$work/"
sed "s/@VERSION@/$version/" packaging/PKGBUILD >"$work/PKGBUILD"
(cd "$work" && PKGDEST="$work/out" makepkg --nodeps --force --noconfirm)
rm -rf dist
mkdir dist
pkg="novarch-$version-1-x86_64.pkg.tar.zst"
cp "$work/out/$pkg" dist/
cp "$work/out/$pkg" dist/novarch-x86_64.pkg.tar.zst
cp target/release/novarch dist/novarch-x86_64
cp install.sh dist/
(cd dist && sha256sum -- * >SHA256SUMS)
ls -l dist
