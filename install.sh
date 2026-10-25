#!/bin/sh
# Install novarch as a pacman package from the latest GitHub release:
#   curl -fsSL https://raw.githubusercontent.com/Novaturiente/novarch/main/install.sh | sh
# Upgrades an older novarch package in place. Run as your normal user; it calls sudo.
set -eu
# NOVARCH_RELEASE_URL: only for testing against a local build (file:///path/to/dist).
base="${NOVARCH_RELEASE_URL:-https://github.com/Novaturiente/novarch/releases/latest/download}"
pkg="novarch-x86_64.pkg.tar.zst"

die() {
	echo "install.sh: $*" >&2
	exit 1
}
[ "$(id -u)" -ne 0 ] || die "run as your normal user (novarch refuses root; this script calls sudo)"
command -v pacman >/dev/null || die "novarch needs Arch Linux (pacman not found)"
[ "$(uname -m)" = x86_64 ] || die "only x86_64 is supported"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
echo ":: Downloading novarch"
curl -fsSL -o "$tmp/$pkg" "$base/$pkg"
curl -fsSL -o "$tmp/SHA256SUMS" "$base/SHA256SUMS"
(cd "$tmp" && grep " $pkg\$" SHA256SUMS | sha256sum -c -) || die "checksum mismatch"
# --noconfirm: under `curl | sh` stdin is this script, so pacman must not read answers.
echo ":: Installing (sudo may ask for your password)"
sudo pacman -U --needed --noconfirm "$tmp/$pkg"
echo ":: Installed $(novarch --version). Next: novarch sync"
