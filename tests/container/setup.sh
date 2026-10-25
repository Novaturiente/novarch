#!/usr/bin/env bash
# Runs as root inside the container only: prepare a normal user with passwordless sudo,
# then run the cases as that user (novarch refuses root).
set -euo pipefail
mode="$1"
# The docker image ships without pacman's local signing key; a real install has one
# (pacstrap runs this). Chaotic-AUR setup needs it for `pacman-key --lsign-key`.
pacman-key --init >/results/setup.log 2>&1
pacman -Syu --noconfirm --needed sudo >>/results/setup.log 2>&1
useradd -m tester
echo 'tester ALL=(ALL) NOPASSWD: ALL' >/etc/sudoers.d/tester
chmod 777 /results
su - tester -c "MODE=$mode bash /tests/cases.sh"
