# novarch — design

Declarative package management for Arch Linux, in Rust. You list packages in TOML
files; `novarch sync` makes the system match, using `pacman` for repo packages and
`paru` for AUR packages. It only ever removes packages it manages, so everything else
on the system (base install, packages you install by hand) is left alone.

Facts below were checked on 2026-09-26 against pacman 7.1.0, paru 2.1.0, the Arch Wiki
and the Chaotic-AUR README. Sources are linked in [References](#references).

---

## 1. Core model

Three sets drive everything:

| Set | Where it comes from | Meaning |
|---|---|---|
| **declared** | union of every `packages` array in `~/.config/novarch/packages/*.toml` | what you want installed |
| **managed** | `~/.local/state/novarch/managed.json` | what novarch has taken ownership of |
| **installed** | `pacman -Qq` / `pacman -T` | what the system actually has |

A `sync` computes:

```
to_install = declared that pacman -T reports as not satisfied
to_adopt   = declared that are already installed but not in managed
to_remove  = managed - declared            (only these can ever be removed)
```

After a successful run: `managed = (managed | declared) - removed`.

**Why a state file?** Tools like decman and pacdef treat *every* explicitly installed
package that is not listed as something to remove. That violates the requirement "do not
touch packages installed some other way". Recording what novarch owns gives a clean
boundary: a package you install by hand with `pacman -S` is never in `managed`, so it is
never removed.

**Safe failure:** if `managed.json` is deleted or corrupted, `to_remove` is empty.
Losing state means novarch forgets to remove things, never that it removes the wrong
things. Write it atomically (write to a temp file in the same directory, then `std::fs::rename`).

**Adoption:** listing a package that is already installed makes it managed (decision
recorded in `AGENTS.md`). If it was installed as a dependency, run
`sudo pacman -D --asexplicit <pkg>` so it is not later removed as an orphan when its
dependent goes away.

---

## 2. Files on disk

```
~/.config/novarch/                 ($XDG_CONFIG_HOME/novarch)
└── packages/
    ├── base.toml
    ├── dev.toml
    └── added.toml                 default target for `novarch add`
~/.local/state/novarch/            ($XDG_STATE_HOME/novarch)
├── managed.json                   sorted list of real package names
└── novarch.log                    novarch's own log (pacman keeps /var/log/pacman.log)
```

Package file format, one key only:

```toml
# ~/.config/novarch/packages/dev.toml
packages = [
  "git",
  "neovim",
  "visual-studio-code-bin",   # AUR (or chaotic-aur) — no need to mark it, paru resolves it
]
```

- Files are read in sorted order and merged; a name in two files is fine.
- One list for repo and AUR packages: `paru -S` resolves both, so separating them would
  only add bookkeeping. Add an `aur = [...]` key later only if a name exists in both and
  you need to force one source (`paru --aur` / `--repo`).
- Use **toml_edit** for reading and writing. It keeps comments and formatting when
  `novarch add` appends a name, which the serde-based `toml` crate does not.

---

## 3. Commands

| Command | What it does |
|---|---|
| `novarch sync` | Full reconcile: preflight, keyring, upgrade + install in one transaction, adopt, remove unlisted managed packages. |
| `novarch diff` | Dry run. Prints `to_install`, `to_adopt`, `to_remove`. Changes nothing. |
| `novarch upgrade` | Keyring refresh + `paru -Syu`. Does not touch the lists. |
| `novarch add PKG... [--to FILE]` | Installs the packages, then appends them to `added.toml` (or `FILE`) and marks them managed. |
| `novarch remove PKG...` | Deletes the names from whichever package files contain them, then runs the removal path for them. Refuses packages that are not managed. |

Global flags: `--yes` (passes `--noconfirm`), `--verbose`.

`diff` is not in the original requirements but is the single most useful safety feature:
see what a sync will remove before it happens.

---

## 4. Flows

### 4.1 Preflight (every mutating command)

1. **Not root.** `libc::geteuid() == 0` → abort. paru refuses to build AUR packages as
   root ("can't install AUR package as root") and so does makepkg. novarch runs as the
   normal user, uses `sudo pacman ...` for repo work, and lets paru call sudo itself.
2. **Lock check.** If `/var/lib/pacman/db.lck` exists *and* `pgrep -x pacman` finds a
   process, abort with a clear message. If the lock exists with no pacman running, tell
   the user it is probably stale and how to remove it. **Never delete the lock
   automatically.**
3. **Chaotic-AUR** (see 4.2), then **paru** (see 4.3). Chaotic first, because it makes
   installing paru a plain `pacman -S`.

### 4.2 Chaotic-AUR setup

Detect: a line matching `^\[chaotic-aur\]` in `/etc/pacman.conf`. If absent:

```sh
sudo pacman-key --recv-key 3056513887B78AEB --keyserver keyserver.ubuntu.com
sudo pacman-key --lsign-key 3056513887B78AEB
sudo pacman -U 'https://cdn-mirror.chaotic.cx/chaotic-aur/chaotic-keyring.pkg.tar.zst' \
               'https://cdn-mirror.chaotic.cx/chaotic-aur/chaotic-mirrorlist.pkg.tar.zst'
```

Then back up `/etc/pacman.conf` to `/etc/pacman.conf.novarch.bak` and append **at the end**
(first repo listed wins on name clashes, so chaotic-aur must come after the official repos):

```ini
[chaotic-aur]
Include = /etc/pacman.d/chaotic-mirrorlist
```

Write it with `sudo tee -a /etc/pacman.conf`; novarch itself never runs as root. The next
step of every flow is a `-Sy`, which picks the new repo up.

### 4.3 paru bootstrap

`paru` found in any `$PATH` entry (`std::env::split_paths`) → done. Otherwise:

1. **With chaotic-aur** (normal case): `sudo pacman -S --needed paru`.
2. **Fallback** (chaotic setup failed or user disabled it later):
   ```sh
   sudo pacman -S --needed base-devel git        # base-devel does not include git
   git clone https://aur.archlinux.org/paru-bin.git <tmpdir>
   cd <tmpdir> && makepkg -si
   ```
   `paru-bin` skips a long Rust compile. Use `tempfile::TempDir` (deleted on drop).

### 4.4 Keyring refresh

Before any upgrade:

```sh
sudo pacman -Sy --needed $(pacman -Qqo /usr/share/pacman/keyrings)
```

`pacman -Qqo /usr/share/pacman/keyrings` lists every installed keyring package
(`archlinux-keyring`, and also `chaotic-keyring`, `cachyos-keyring` on derivatives)
without hardcoding names — and without catching `gnome-keyring`, which a `*-keyring`
name match would. `--needed` makes it a no-op when current.

The Arch Wiki says `-Sy <keyring>` followed immediately by `-Su` "is not considered a
partial upgrade". That is why the next step **must** be a `-Su`.

### 4.5 sync

```
preflight
keyring refresh                              (-Sy happens here)
paru -Su --needed [--sudoloop] <to_install>  one transaction: upgrade + install
adopt:   sudo pacman -D --asexplicit <to_adopt that are installed as deps>
remove:  see 4.6
save managed.json
report .pacnew files and reboot hint
```

**Partial upgrades are unsupported on Arch.** Never run `-Sy` and then `-S pkg`
without `-u`. Installing inside the same `-Su` transaction as the upgrade is the only
safe pattern, and it also means a sync always leaves the system fully up to date.

If `to_install` is empty this degenerates to `paru -Su`, which is exactly `upgrade`.

Record the **real** package name in `managed.json`, not the declared one:
`pacman -Qq sh` prints `bash`, because pacman resolves provides. So after install, map
each declared name through `pacman -Qq <name>`.

### 4.6 Removal

Input: `to_remove` (managed but no longer declared), filtered to what is still
installed (a conflict prompt during install may already have removed some).

```sh
pacman -Qqt          # installed packages no other package requires
```

- In `to_remove` **and** in `-Qqt` → nothing depends on it → `sudo pacman -Rs <those>`.
- In `to_remove` but **not** in `-Qqt` → something still hard-depends on it →
  `sudo pacman -D --asdeps <those>` and drop from `managed`. It stays installed, and
  pacman will clean it up naturally when its dependent is removed. Log this clearly.

Why this works for chains: if managed `A` depends on managed `B` and both are unlisted,
`B` is not in `-Qqt`, so it is marked as a dependency; then `-Rs A` removes `B` too,
because `-s` removes dependencies that are "not required by other packages and not
explicitly installed".

Why `-Rs` and not `-Rns`: `-n` skips saving `.pacsave` backups of edited config files.
Keeping them costs nothing and saves you from losing config.

Why not the wiki's `pacman -Rns $(pacman -Qdtq)`: that removes **every** orphan on the
system, including ones novarch never touched. Scope removal to `to_remove`.

**Never** pass `-Rc` (cascade) or `-Rdd` (skip dependency checks). Plain `-R` refusing
with "removing X breaks dependency" is the guard that keeps hard dependencies safe.

Optional dependencies do not count as "required" (`-Qt`, not `-Qtt`), matching the
requirement "unless it is a hard dependency".

### 4.7 add / remove

- `add`: preflight → keyring → `paru -Su --needed <pkgs>` → **only on success**, append to
  the TOML file and to `managed`. Install first so a typo never lands in your config.
- `remove`: must be in `managed` (otherwise refuse and say "not managed by novarch; use
  pacman -R"). Delete the name from the TOML file(s), then run 4.6 for just those names.

---

## 5. Running subprocesses (logs, progress, prompts)

- **Inherit the terminal.** `Command::new(..).status()`: stdin/stdout/stderr are
  inherited by default, no pipes.
  pacman and paru then draw their own progress bars and ask their own `[Y/n]`
  questions (conflicts, provider choice, PKGBUILD review) directly to the user. This
  covers both the "use pacman's progress output" and "allow interaction" requirements
  for free.
- Capture output only for read-only queries (`pacman -Qq`, `-T`, `-Qqt`, `-Qqo`), with
  `Command::new(..).output()`.
- Check return codes yourself. pacman exits non-zero when the user answers "n"; treat
  that as "aborted by user", stop, and do **not** update `managed.json`.
- `pacman -T` exits 127 when some names are unsatisfied — that is data, not an error.
- `--yes` maps to `--noconfirm`. With paru this also skips the PKGBUILD review prompt
  (unverified exactly; test it). AUR PKGBUILDs are arbitrary code — keep `--yes` opt-in.
- Pass `--sudoloop` to paru so sudo doesn't time out during long AUR builds.

**novarch's own messages:** plain `eprintln!` with a pacman-like prefix
(`:: novarch: resolving package lists...`); add colour later only if it helps. Log by
appending lines to `~/.local/state/novarch/novarch.log` (`std::fs::OpenOptions`, append
mode); no logging crate. Log every command novarch runs (the full argv)
and every plan (install/adopt/remove sets) so a bad sync can be reconstructed.

---

## 6. Things to handle (pitfalls)

| Pitfall | Handling |
|---|---|
| **Package groups** (`xorg`, `gnome`) | A group name is not a package, so `pacman -T` reports it missing forever. Expand at load time with `pacman -Sgq <name>` and treat members as declared. (`base-devel` is a meta *package* now, not a group, so it just works.) |
| **Provides / virtual names** (`sh` → `bash`, `neovim` satisfied by `neovim-git`) | Check with `pacman -T`, record real names via `pacman -Qq <name>`. |
| **Typos / nonexistent names** | paru fails the whole transaction with "target not found". Fine for v1: nothing changes, error names the package. |
| **`-git` variants and conflicts** | pacman asks "X and Y are in conflict. Remove Y?" interactively. Install before removing, and re-check installed state before removal. |
| **IgnorePkg** in pacman.conf | `-Su` skips them; respect it, don't try to override. |
| **`.pacnew` files** | After a sync, list `/etc/**/*.pacnew` (or suggest `pacdiff` from `pacman-contrib`). Report only, never merge. |
| **Kernel upgraded** | If `linux*` was upgraded, print "reboot recommended". |
| **Derivatives** (CachyOS, EndeavourOS) | Extra repos and keyrings; the keyring query in 4.4 handles them. CachyOS often ships paru and chaotic already — detection makes both steps no-ops. |
| **Pacman already running** | Preflight lock check. paru also waits on the lock itself. |
| **Arch news with manual interventions** | paru can show news before upgrading (`NewsOnUpgrade` in `paru.conf`). Mention it in the README rather than reimplementing. |

---

## 7. Project layout

```
novarch/
├── AGENTS.md
├── Cargo.toml
├── docs/design.md             this file
└── src/
    ├── main.rs                clap CLI, one function per command
    ├── lists.rs               load/merge/append TOML package files (toml_edit)
    ├── state.rs               managed.json load/save (atomic)
    ├── plan.rs                pure: (declared, installed, managed) -> Plan; #[cfg(test)] tests
    ├── system.rs              every external command: pacman, paru, sudo, pacman-key
    └── bootstrap.rs           chaotic-aur + paru setup
```

`plan.rs` is the only piece of real logic and it is pure (no external commands), so it is
the one thing with tests (`cargo test`). Everything else is thin glue around pacman/paru
and is tested by running it.

**Dependencies (crates):** add each one in the milestone that first needs it.

| Crate | For |
|---|---|
| `clap` (derive) | CLI parsing |
| `toml_edit` | read + append package files, keeping comments |
| `serde_json` | `managed.json` |
| `libc` | `geteuid()` for the root check |
| `tempfile` | paru-bin build directory (bootstrap fallback) |

Development needs only `rustup`/`cargo`: `cargo build`, `cargo test`,
`cargo run -- diff`.

**Distribution:** the release binary needs only glibc, so it runs on a fresh Arch system
(the `base` package does not include Python or any other runtime).

- GitHub Releases: a workflow builds `cargo build --release` for
  `x86_64-unknown-linux-gnu` on every `v*` tag and attaches the binary. An `install.sh`
  fetched with `curl` downloads it to `~/.local/bin/novarch`.
- AUR: a `PKGBUILD` (`novarch-bin` from the release binary) makes novarch a normal
  package, so novarch can then manage itself.

**Testing mutating commands safely:** use a throwaway container,
`docker run -it --rm archlinux:latest`, or a VM. Don't develop `sync` against your
daily system until `diff` output looks right.

---

## 8. Build order

Each milestone is usable on its own.

1. **Skeleton:** `Cargo.toml`, `main.rs` with clap, `novarch --help` works.
2. **Read lists:** `lists.rs` loads and merges `packages/*.toml`. `novarch diff` prints the declared set.
3. **Query system:** `system.rs` wraps `pacman -T`, `-Qq`, `-Qqt`. `diff` shows `to_install`.
4. **State + plan:** `state.rs`, `plan.rs` with its tests. `diff` shows all three sets.
5. **Preflight:** root check, lock check.
6. **Install path:** keyring refresh + `paru -Su --needed`, save state. `novarch sync` installs.
7. **Removal path:** section 4.6. `sync` is now complete.
8. **upgrade, add, remove** commands (small once 6 and 7 exist).
9. **Bootstrap:** chaotic-aur + paru (test in a container — it edits `/etc/pacman.conf`).
10. **Polish:** group expansion, `.pacnew` report, reboot hint, file logging.
11. **Release:** GitHub Actions release workflow, `install.sh`, AUR `PKGBUILD` (section 7).

---

## 9. Deliberately not in v1

| Skipped | Add when |
|---|---|
| `novarch import` (bootstrap lists from `pacman -Qqe`) | You set up on an existing machine and don't want to type the lists. |
| Per-host files (`hosts/<hostname>.toml`) | You use the same config on two machines. |
| Pre/post hooks, systemd service enabling, dotfiles | Never, probably — out of scope for a package manager. |
| Btrfs snapshot before sync | You don't use snapper + `snap-pac`, which already snapshots every pacman transaction. |
| Separate `aur = [...]` key | A name exists in both repo and AUR and you need to pick. |
| Name validation before install | paru's "target not found" is annoying enough to matter. |
| A config file (`config.toml`) | There is a second setting worth having. |

---

## References

- Chaotic-AUR setup: <https://github.com/chaotic-aur/.github/blob/main/profile/README.md>
- paru: <https://github.com/Morganamilo/paru> and `man paru`
- Package signing / keyring: <https://wiki.archlinux.org/title/Pacman/Package_signing>
- Partial upgrades: <https://wiki.archlinux.org/title/System_maintenance#Partial_upgrades_are_unsupported>
- Pacman (lock file, removal, repo order): <https://wiki.archlinux.org/title/Pacman>
- Prior art: [decman](https://github.com/kiviktnm/decman) (Python, removes all unlisted explicit packages),
  [aconfmgr](https://github.com/CyberShadow/aconfmgr) (most careful removal model),
  [metapac](https://github.com/ripytide/metapac) (rejects groups, has `unmanaged` bootstrap),
  [pacdef](https://github.com/steven-omaha/pacdef) (archived).
