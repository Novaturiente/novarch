#!/usr/bin/env bash
# Scenario tests, run as user `tester` inside the container (see setup.sh).
# Each novarch run happens in a pseudo-terminal (`script`) because mutating commands
# require one; answers to novarch's own questions are fed on stdin.
set -u
MODE="${MODE:-chaotic}"
PK="$HOME/pk"
STATE="$HOME/.local/state/novarch"
CFG="$HOME/.config/novarch/config.toml"
pass=0 fail=0 n=0 LAST="" RC=0

clean() {
    # strip CSI colour codes and OSC sequences (sudo emits OSC 3008 session markers)
    tr -d '\r' <"$1" | sed -e 's/\x1b\[[0-9;?]*[a-zA-Z]//g' -e 's/\x1b\][^\x1b]*\x1b\\//g'
}

# How many novarch questions have been shown so far: lines starting with "? " that carry
# an answer hint like [Y/n], [y/N] or [preinstalled.toml].
asked() { clean "$1" | grep -cE '^\? .*\[[^]]*\]'; }
# pacman's own questions (`:: ... [Y/n]`).
pacman_asked() { clean "$1" | grep -cE '^:: .*\[(Y/n|y/N)\]'; }

# nv "<a1|a2|...>" args... : run novarch in a pty, typing each answer only once its
# question is on screen (pacman discards input typed ahead of its own prompts, like a
# real terminal). `p:<a>` answers pacman's next question instead of novarch's.
# Keeps cleaned output in $LAST and the exit code in $RC.
nv() {
    local answers="$1"
    shift
    n=$((n + 1))
    local log="/results/$(printf '%02d' $n)-${1}.log"
    local fifo
    fifo=$(mktemp -u)
    mkfifo "$fifo"
    timeout 3600 script -qec "novarch --color never $*" /dev/null <"$fifo" >"$log.raw" 2>&1 &
    local pid=$!
    exec 8>"$fifo"
    local i=0 a
    if [ -n "$answers" ]; then
        IFS='|' read -ra list <<<"$answers"
        for a in "${list[@]}"; do
            if [[ $a == p:* ]]; then
                a=${a#p:}
                local base
                base=$(pacman_asked "$log.raw")
                while kill -0 $pid 2>/dev/null && [ "$(pacman_asked "$log.raw")" -le "$base" ]; do sleep 0.3; done
            else
                i=$((i + 1))
                while kill -0 $pid 2>/dev/null && [ "$(asked "$log.raw")" -lt $i ]; do sleep 0.3; done
            fi
            kill -0 $pid 2>/dev/null || break # exited without asking (writing would SIGPIPE)
            sleep 0.3
            [ "$a" = "<enter>" ] && a=""
            printf '%s\n' "$a" >&8
        done
    fi
    wait $pid
    RC=$?
    exec 8>&-
    rm -f "$fifo"
    clean "$log.raw" >"$log"
    rm -f "$log.raw"
    LAST=$(cat "$log")
    echo "    \$ novarch $* -> exit $RC ($(basename "$log"))"
}

ok() { pass=$((pass + 1)); echo "  ok   $1"; }
bad() {
    fail=$((fail + 1))
    echo "  FAIL $1"
    echo "$LAST" | tail -15 | sed 's/^/       | /'
}
check() { if eval "$2"; then ok "$1"; else bad "$1"; fi; }
installed() { pacman -Q "$1" >/dev/null 2>&1; }
explicit() { pacman -Qqe "$1" >/dev/null 2>&1; }
managed() { grep -q "\"$1\"" "$STATE/managed.json" 2>/dev/null; }
said() { grep -qF -- "$1" <<<"$LAST"; }
list() {
    local f="$1"
    shift
    { echo "packages = ["; for p in "$@"; do echo "  \"$p\","; done; echo "]"; } >"$PK/$f"
}

echo "--- setup ($MODE)"
mkdir -p "$PK" "$(dirname "$CFG")"
echo "packages_dir = \"$PK\"" >"$CFG"
[ "$MODE" = fallback ] && echo "chaotic_aur = false" >>"$CFG"
AUR=pacman-cleanup-hook # tiny AUR package: chaotic binary or built from source by paru
sudo pacman -S --noconfirm --asdeps jq >/dev/null # jq + oniguruma installed as deps
list base.toml tree jq oniguruma sudo sh "$AUR"

echo "--- guards"
sudo touch /var/lib/pacman/db.lck
nv "" sync --yes
check "stale pacman lock refuses" '[ $RC -ne 0 ] && said "probably stale" && [ -e /var/lib/pacman/db.lck ]'
sudo rm /var/lib/pacman/db.lck
mkdir -p "$STATE"
exec 9>"$STATE/lock" # hold the lock in this shell, like a running novarch would
flock 9
nv "" sync --yes
check "second novarch refuses while lock is held" '[ $RC -ne 0 ] && said "already running"'
exec 9>&-
LAST=$(sudo -n novarch sync 2>&1 </dev/null)
check "root refuses" 'said "do not run novarch as root"'

echo "--- first sync: bootstrap, install, adopt"
nv "n" sync --yes
check "sync exit 0" '[ $RC -eq 0 ]'
check "paru installed and starts" 'paru --version >/dev/null 2>&1'
if [ "$MODE" = chaotic ]; then
    check "chaotic-aur in pacman.conf" 'grep -q "^\[chaotic-aur\]" /etc/pacman.conf'
    check "pacman.conf backup" '[ -e /etc/pacman.conf.novarch.bak ]'
    check "answer saved in config" 'grep -q "chaotic_aur = true" "$CFG"'
else
    check "no chaotic-aur in pacman.conf" '! grep -q "^\[chaotic-aur\]" /etc/pacman.conf'
fi
check "tree installed" 'installed tree'
check "AUR package installed" "installed $AUR"
check "jq adopted and now explicit" 'explicit jq && managed jq'
check "sh recorded as bash" 'managed bash && ! managed sh'
check "everything managed" 'managed tree && managed sudo && managed oniguruma && managed '"$AUR"
check "plan was printed" 'said "to install" && said "to adopt"'
check "numbered steps" 'said "[1/5]" && said "[5/5] Report"'

nv "" diff
check "diff is clean after sync" 'said "0 to install, 0 to adopt, 0 to remove"'

echo "--- removal, protected, chain, orphans"
sudo pacman -S --noconfirm --asdeps figlet >/dev/null # a system orphan novarch does not own
list base.toml sh "$AUR"
nv "" diff
check "diff shows removals and protected sudo" 'said "- jq" && said "- oniguruma" && said "- tree" && said "sudo is protected"'
nv "y|p:y" sync --yes
check "tree, jq removed" '[ $RC -eq 0 ] && ! installed tree && ! installed jq'
check "oniguruma (still required by jq) removed with jq" '! installed oniguruma'
check "required one was marked asdeps" 'said "oniguruma is still required"'
check "sudo kept but no longer managed" 'installed sudo && ! managed sudo'
check "orphan figlet offered and removed on yes" 'said "figlet" && ! installed figlet'
check "removed ones not managed" '! managed tree && ! managed jq'
check "managed.json backups" '[ -e "$STATE/managed.json.1" ]'

sudo pacman -S --noconfirm --asdeps figlet >/dev/null
nv "<enter>" sync --yes
check "orphan prompt defaults to no" '[ $RC -eq 0 ] && said "Remove these orphaned" && installed figlet'
sudo pacman -Rs --noconfirm figlet >/dev/null

echo "--- mass removal guard"
MANY=(bc ed zip nano cowsay sl cmatrix time words tree jq)
list many.toml "${MANY[@]}"
nv "n" sync --yes
check "11 packages installed" '[ $RC -eq 0 ] && installed cmatrix && installed bc'
rm "$PK/many.toml"
nv "n" sync --yes
check "--yes refuses removing 11" '[ $RC -ne 0 ] && said "allow-mass-remove" && installed cmatrix'
nv "n" sync --yes --allow-mass-remove
check "--allow-mass-remove removes them" '[ $RC -eq 0 ] && ! installed cmatrix && ! installed bc'

echo "--- add"
nv "" add --yes bc
check "add installs and appends to manual-installs.toml" '[ $RC -eq 0 ] && installed bc && grep -q "\"bc\"" "$PK/manual-installs.toml" && managed bc'
before=$(cat "$PK/manual-installs.toml")
nv "" add --yes no-such-package-xyz
check "failed add leaves files alone" '[ $RC -ne 0 ] && [ "$(cat "$PK/manual-installs.toml")" = "$before" ] && ! managed no-such-package-xyz'
nv "" add --yes --to ../escape.toml ed
check "--to outside the folder refused" '[ $RC -ne 0 ] && said "inside the packages folder" && ! installed ed'
nv "" add --yes --no-upgrade --to hosts/test/extra.toml ed
check "add --no-upgrade into a host file" '[ $RC -eq 0 ] && installed ed && grep -q "\"ed\"" "$PK/hosts/test/extra.toml"'
nv "" add --yes -- "'--overwrite=*'"
check "option-like name refused" 'said "invalid package name"'

echo "--- remove"
nv "" remove --yes bc
check "remove uninstalls and edits the file" '[ $RC -eq 0 ] && ! installed bc && ! grep -q "\"bc\"" "$PK/manual-installs.toml" && ! managed bc'
nv "" remove --yes bash-completion
check "remove refuses unmanaged" 'said "not managed by novarch"'

echo "--- status, list, corrupt state, upgrade"
nv "y|<enter>" status
check "status writes preinstalled.toml" 'grep -q "\"base\"" "$PK/preinstalled.toml"'
rm "$PK/preinstalled.toml"
nv "" list sh "$AUR" zzz
check "list shows provides and unknowns" 'said "(as bash)" && said "not installed, not declared"'
cp "$STATE/managed.json" /tmp/m.json
echo '{broken' >"$STATE/managed.json"
nv "" diff
check "corrupt managed.json stops the run" '[ $RC -ne 0 ] && said "is corrupt"'
cp /tmp/m.json "$STATE/managed.json"
nv "" upgrade --yes
check "upgrade" '[ $RC -eq 0 ] && said "upgrade finished"'
check "log written" 'grep -q "exec: sudo pacman" "$STATE/novarch.log"'

echo "--- result ($MODE): $pass passed, $fail failed"
[ "$fail" -eq 0 ]
