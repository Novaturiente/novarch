#!/usr/bin/env bash
# Release check in a throwaway container: runs the CI build steps, then installs an old
# novarch package and upgrades it with the new one through install.sh.
# Usage: tests/container/release.sh [old-novarch-binary]   (docker; network)
# With an old binary (e.g. ~/.dotfiles/system/novarch) it is packaged as novarch 0.2.0
# first, to prove the new package replaces it cleanly.
set -euo pipefail
cd "$(dirname "$0")/../.."
old="${1:-}"
mounts=(-v "$PWD:/src:ro")
[ -n "$old" ] && mounts+=(-v "$(realpath "$old"):/old-novarch:ro")
docker run --rm "${mounts[@]}" archlinux:latest bash -euo pipefail -c '
pacman -Syu --noconfirm --needed base-devel git rust sudo >/tmp/setup.log 2>&1
mkdir /work && tar -C /src --exclude=./target --exclude=./.git -cf - . | tar -C /work -xf -
cd /work
echo "--- CI steps"
cargo test --locked --quiet 2>&1 | grep "test result"
cargo build --release --locked --quiet
useradd -m builder && echo "builder ALL=(ALL) NOPASSWD: ALL" >/etc/sudoers.d/builder
chown -R builder /work
su builder -c "cd /work && packaging/build-pkg.sh" | tail -6
if [ -e /old-novarch ]; then
  echo "--- old novarch 0.2.0 package"
  mkdir /old && cp /old-novarch /old/novarch && chown -R builder /old
  printf "%s\n" "pkgname=novarch" "pkgver=0.2.0" "pkgrel=1" "arch=(x86_64)" "options=(!debug)" \
    "package() { install -Dm755 \"\$startdir/novarch\" \"\$pkgdir/usr/bin/novarch\"; install -Dm644 /dev/null \"\$pkgdir/usr/share/man/man1/novarch.1.gz\"; }" >/old/PKGBUILD
  su builder -c "cd /old && makepkg --nodeps --noconfirm >/dev/null 2>&1"
  pacman -U --noconfirm /old/novarch-0.2.0-1-x86_64.pkg.tar.zst >/dev/null
  echo "installed: $(pacman -Q novarch), runs: $(novarch --version)"
fi
echo "--- install.sh (from the local dist/)"
su builder -c "NOVARCH_RELEASE_URL=file:///work/dist sh /work/install.sh" 2>&1 | grep -v "^ *(\|^warning: directory permissions" | tail -6
echo "--- result"
pacman -Q novarch
novarch --version
pacman -Qlq novarch | grep -E "bin/novarch|completion|site-functions|vendor_completions|licenses/novarch/LICENSE$"
[ ! -e /usr/share/man/man1/novarch.1.gz ] && echo "old man page removed with the old package"
echo "--- install.sh as root must refuse:"
NOVARCH_RELEASE_URL=file:///work/dist sh /work/install.sh 2>&1 | tail -1 || true
'
