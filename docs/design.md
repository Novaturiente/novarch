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
| **declared** | union of every `packages` array in the packages folder's shared files plus this host's files (section 2) | what you want installed |
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

**Safe failure:** if `managed.json` is missing, `to_remove` is empty. Losing state means
novarch forgets to remove things, never that it removes the wrong things. If it exists
but does not parse, refuse to run and print its path; never treat it as empty and
overwrite it. Write it atomically (write to a temp file in the same directory, then `std::fs::rename`).

**Adoption:** listing a package that is already installed makes it managed (decision
recorded in `AGENTS.md`). If it was installed as a dependency, run
`sudo pacman -D --asexplicit <pkg>` so it is not later removed as an orphan when its
dependent goes away.

---

## 2. Files on disk

```
~/.config/novarch/config.toml      ($XDG_CONFIG_HOME/novarch) written by the first-run prompt
<packages_dir>/                    any folder the user picks, e.g. inside a dotfiles repo
├── base.toml                      shared: read on every host
├── dev.toml
├── manual-installs.toml           default target for `novarch add`
└── hosts/
    ├── <host-a>/*.toml            read only on host-a
    └── <host-b>/*.toml
~/.local/state/novarch/            ($XDG_STATE_HOME/novarch) per machine, never shared
├── managed.json                   sorted list of real package names
└── novarch.log                    novarch's own log (pacman keeps /var/log/pacman.log)
```

`config.toml`:

```toml
packages_dir = "/home/me/dotfiles/novarch"   # absolute path, ~ expanded when asked
host = "laptop"        # optional; default is the machine's hostname (gethostname)
chaotic_aur = true     # answer to the first-run question (section 4.2)
```

**Hosts:** declared = shared files (top level of `packages_dir`) + `hosts/<host>/*.toml`.
Nothing is tied to particular machines: any number of hosts, each a folder named after
its hostname. `host` in `config.toml` overrides the hostname, so two machines can share a
profile or a machine can use a name other than its hostname. A host with no folder uses
the shared files only; if `hosts/` exists but has no folder for this host, print a
warning naming the host (catches a renamed machine).

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
- Only top-level `*.toml` files count (plus `hosts/<host>/*.toml`); other files are ignored.
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
| `novarch add PKG... [--to FILE] [--no-upgrade]` | Installs the packages, then appends them to `manual-installs.toml` (or `FILE`, relative to `packages_dir`, e.g. `hosts/laptop/dev.toml`) and marks them managed. `--no-upgrade` is the emergency path (section 4.7). |
| `novarch remove PKG...` | Deletes the names from whichever package files contain them, then runs the removal path for them. Refuses packages that are not managed. |
| `novarch status` | Lists explicitly installed packages that no file declares, then offers to write them to a new file so novarch manages them (section 4.9). |
| `novarch list [PKG...]` | Each declared package, the file(s) that declare it, and whether it is installed and managed. With names: only those, plus "not declared" or "dependency of X" (`pacman -Qi` Required By) for names no file lists. |
| `novarch completions bash\|zsh\|fish` | Prints a shell completion script. |

Global flags: `--yes` (passes `--noconfirm`), `--verbose`. `sync` and `remove` also take
`--allow-mass-remove` (section 4.6).

`diff` is not in the original requirements but is the single most useful safety feature:
see what a sync will remove before it happens.

---

## 4. Flows

### 4.0 Config and package lists (every command)

1. **No `config.toml`** (first run): ask for the packages folder with a line editor that
   tab-completes directories like a shell; suggest `~/.config/novarch/packages`. Offer to
   create the folder if it does not exist. Save the answer to `config.toml`.
2. **Check `packages_dir`:** it must exist, be a directory, and contain at least one
   top-level `*.toml`. If not, explain which check failed and ask again (same prompt).
   If the folder exists but has no `*.toml`, also offer the import from 4.9: on an
   existing system that is the normal first step.
   If stdin is not a terminal, refuse instead of asking. Never continue with a missing
   or empty folder: an empty declared set would mean "remove every managed package".
3. **Parse every file.** Each must be valid TOML with exactly one key, `packages`, an
   array of strings. Unknown keys are errors (catches `pakages = [...]`). Errors name the
   file and line. Any error stops the command; never run with a partial set.
4. **Validate names** against makepkg's rule: only ASCII letters, digits and `@ . _ + -`,
   not starting with `-` or `.`. This also stops a name like `--overwrite=*` reaching pacman as an option.
5. **Declared must be non-empty** after merging. Zero names is an error, not a plan.

### 4.1 Preflight (every mutating command)

0. **Interactive only.** Refuse if stdin is not a terminal. There is no unattended mode:
   AUR PKGBUILDs are code run as you, and removals need a human to read the list.
1. **Not root.** `libc::geteuid() == 0` → abort. paru refuses to build AUR packages as
   root ("can't install AUR package as root") and so does makepkg. novarch runs as the
   normal user, uses `sudo pacman ...` for repo work, and lets paru call sudo itself.
2. **Lock check.** If `/var/lib/pacman/db.lck` exists *and* `pgrep -x pacman` finds a
   process, abort with a clear message. If the lock exists with no pacman running, tell
   the user it is probably stale and how to remove it. **Never delete the lock
   automatically.**
3. **Chaotic-AUR** (see 4.2), then the keyring refresh (4.4), then **paru** (see 4.3).
   Chaotic first, so the refresh's `-Sy` picks it up; paru right after, because its
   install is the `-Su` that must follow that `-Sy`.

### 4.2 Chaotic-AUR setup

Detect: a line matching `^\[chaotic-aur\]` in `/etc/pacman.conf`. If absent and
`chaotic_aur` is not yet in `config.toml`, ask first: Chaotic-AUR is a third-party repo
of prebuilt AUR binaries, trusted with root once added. Save the answer.
`chaotic_aur = false` skips this section for good (paru then comes from the fallback in
4.3, and AUR packages are built locally). `--yes` answers yes. If accepted:

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

`paru` found in any `$PATH` entry (`std::env::split_paths`) → done. Otherwise, right
after the keyring refresh's `-Sy` (so these `-Su` calls complete it, no partial upgrade):

1. **With chaotic-aur** (normal case): `sudo pacman -Su --needed paru`.
2. **Fallback** (chaotic declined):
   ```sh
   sudo pacman -Su --needed base-devel git       # base-devel does not include git
   git clone --depth=1 https://aur.archlinux.org/paru-bin.git <tmpdir>
   cd <tmpdir> && makepkg -si --needed
   ```
   `paru-bin` skips a long Rust compile. `<tmpdir>` is `$TMPDIR/novarch-paru-bin-<pid>`,
   deleted afterwards. **Verify it starts** (`paru --version`): `paru-bin` can lag behind
   pacman's libalpm soname (seen 2026-10-25: built for `libalpm.so.15`, pacman 7.1 ships
   `.so.16`). If it fails, `sudo pacman -R paru-bin` (just installed, nothing depends on
   it) and build `paru` from source the same way (`makepkg -sir`), which always matches.

**Source builds need base-devel.** paru does not depend on it. When `sync`/`add` will
build packages that are in no sync repo and `base-devel` is not installed, it is added
to the same `paru -Su` transaction.

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
orphans: offer cleanup, see 4.8
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

**Protected packages** are never removed, only dropped from `managed` with a message:
`base`, `pacman`, `sudo`, `systemd`, `linux` and `linux-*` (kernels, headers, firmware),
bootloaders (`grub`, `refind`, `limine`, `efibootmgr`), `paru`, `paru-bin`, `novarch`,
`novarch-bin`. Deleting `linux` from a list must not uninstall the kernel.

**Confirm before removing.** Print every package with its reason ("managed, no longer
listed in any file") and ask `[y/N]`, even though pacman asks again. With `--yes`, refuse
if more than 10 packages would be removed unless `--allow-mass-remove` is also passed.

```sh
pacman -Qqtt         # installed packages no other package hard-requires
```

- In `to_remove` **and** in `-Qqtt` → nothing depends on it → `sudo pacman -Rs <those>`.
- In `to_remove` but **not** in `-Qqtt` → something still hard-depends on it →
  `sudo pacman -D --asdeps <those>` and drop from `managed`. It stays installed, and
  pacman will clean it up naturally when its dependent is removed. Log this clearly.

Why this works for chains: if managed `A` depends on managed `B` and both are unlisted,
`B` is not in `-Qqtt`, so it is marked as a dependency; then `-Rs A` removes `B` too,
because `-s` removes dependencies that are "not required by other packages and not
explicitly installed".

Why `-Rs` and not `-Rns`: `-n` skips saving `.pacsave` backups of edited config files.
Keeping them costs nothing and saves you from losing config.

Why not the wiki's `pacman -Rns $(pacman -Qdtq)` here: that removes **every** orphan on
the system, including ones novarch never touched. Removal of `to_remove` stays scoped;
system-wide orphans are a separate, opt-in question (4.8).

**Never** pass `-Rc` (cascade) or `-Rdd` (skip dependency checks). Plain `-R` refusing
with "removing X breaks dependency" is the guard that keeps hard dependencies safe.

Optional dependencies do not count as "required": use `-Qtt`, matching the requirement
"unless it is a hard dependency". (`man pacman`: a single `-t` also hides packages that
are only *optionally* required; `-tt` includes them. Checked 2026-10-25.)

### 4.7 add / remove

- `add`: preflight → keyring → `paru -Su --needed <pkgs>` → **only on success**, append to
  the TOML file and to `managed`. Install first so a typo never lands in your config.
  Default file `manual-installs.toml`; `--to FILE` picks another, relative to
  `packages_dir`, and must stay inside it.
- `add --no-upgrade` (emergency install, no full upgrade): preflight → check
  `pacman -Qu` is empty → `paru -S --needed <pkgs>` (no `-y`, no keyring refresh) → same
  save step. `pacman -Qu` empty means the local sync database is no newer than the
  installed system, so installing from it cannot be a partial upgrade. If it is not
  empty (someone ran `-Sy` without `-u`), refuse and tell the user to run plain `add`.
  A stale mirror may 404 on a download; that fails cleanly and nothing is saved.
- `remove`: must be in `managed` (otherwise refuse and say "not managed by novarch; use
  pacman -R"). Delete the name from the TOML file(s), then run 4.6 for just those names.

### 4.8 Orphan cleanup (end of sync, opt-in each time)

After the removal step, compute the safe orphan list:

```sh
pacman -Qdtq    # installed as a dependency, and nothing needs it, not even optionally
```

Then drop protected packages (4.6) and anything declared. If the list is empty, say
nothing. Otherwise print it and ask `Remove N orphaned packages? [y/N]`. Default is no,
and `--yes` does **not** answer it: this is the only step that touches packages novarch
does not own, so a human must say yes every time. On yes: `sudo pacman -Rs <list>`
(never `--noconfirm`, never `-n`, so pacman confirms again and keeps `.pacsave` files).

Why it is safe: single `-t` excludes packages that any installed package requires *or
optionally* requires, so optional features stay. `-Rs` still refuses anything that would
break a hard dependency. What it cannot know: a dependency you use directly (a library
for your own scripts). The prompt is the guard; declare such packages to keep them.

### 4.9 status (bring unlisted packages under novarch)

Unlisted = `pacman -Qqe` (explicitly installed) minus declared. Print them, split into
repo (`pacman -Qqen`) and foreign/AUR (`pacman -Qqem`). Then ask for a file name
(default `preinstalled.toml`) and write them there, sorted, with `# repo` and
`# AUR / foreign` comments. The file must not already exist. The next `sync` adopts them
(they are installed, so nothing is downloaded). Answering no writes nothing.

### 4.10 Concurrency and state backups

- **One run at a time.** Every mutating command takes `flock(LOCK_EX | LOCK_NB)` on
  `~/.local/state/novarch/lock` and refuses if another novarch holds it. The kernel
  drops the lock when the process exits, so it can never go stale.
- **Backups.** Before each `managed.json` write, rotate the old file to
  `managed.json.1` … `managed.json.5`. Undoing a bad sync is copying one back by hand.

---

## 5. Running subprocesses (logs, progress, prompts)

- **Inherit the terminal.** `Command::new(..).status()`: stdin/stdout/stderr are
  inherited by default, no pipes.
  pacman and paru then draw their own progress bars and ask their own `[Y/n]`
  questions (conflicts, provider choice, PKGBUILD review) directly to the user. This
  covers both the "use pacman's progress output" and "allow interaction" requirements
  for free.
- Capture output only for read-only queries (`pacman -Qq`, `-T`, `-Qqtt`, `-Qdtq`, `-Qqe`, `-Qqo`, `-Qu`), with
  `Command::new(..).output()`.
- Check return codes yourself. pacman exits non-zero when the user answers "n"; treat
  that as "aborted by user", stop, and do **not** update `managed.json`.
- `pacman -T` exits 127 when some names are unsatisfied — that is data, not an error.
- `--yes` maps to `--noconfirm`. With paru this also skips the PKGBUILD review prompt
  (unverified exactly; test it). AUR PKGBUILDs are arbitrary code — keep `--yes` opt-in.
- Pass `--sudoloop` to paru so sudo doesn't time out during long AUR builds.
- **Never pipe, prefix or recolour pacman/paru output.** Capturing it (even to tee it
  into a log) turns stdout into a pipe: pacman then drops progress bars, and prompts
  without a trailing newline appear late or not at all. A pty wrapper could fix that but
  is not worth its complexity. pacman already logs every transaction to
  `/var/log/pacman.log`; novarch's log points there.
- **Colour for pacman/paru:** pass `--color auto` (or `never`/`always`, from novarch's
  own `--color`). Stock `/etc/pacman.conf` ships `#Color` commented out, so a fresh
  install would otherwise be colourless. `auto` only colours a tty (`man pacman`).
- **Ctrl-C.** The terminal sends SIGINT to novarch and the child together; pacman
  handles it safely itself. While a child runs, novarch installs a no-op SIGINT handler
  (not `SIG_IGN`: ignored signals stay ignored across `exec`, so the child would become
  unkillable). Afterwards print `✗ interrupted, nothing saved` and exit 130.

### 5.1 What the user sees

novarch's own lines go to stderr, each with an icon and a colour:

| Icon | ASCII | Colour | Meaning |
|---|---|---|---|
| `::` | `::` | bold blue | phase header |
| `➜` | `>` | dim | command about to run |
| `✓` | `ok` | green | step succeeded |
| `⚠` | `!` | yellow | warning, needs attention later |
| `✗` | `x` | bold red | error, novarch stops |
| `?` | `?` | bold cyan | novarch's own question |
| `+` `~` `-` | same | green, blue, red | install, adopt, remove |

- **Phases are numbered:** `:: [3/7] Upgrading system and installing 4 packages`.
  The user always knows where the run is and how much is left.
- **Announce before handing over the terminal.** Print the full command dimmed
  (`➜ sudo pacman -Sy --needed archlinux-keyring`) and, before the first sudo call,
  `sudo may ask for your password`. Silence while sudo waits is what feels "stuck".
  After the child exits: `✓ done in 42s`, or `✗ pacman exited with code 1`.
- **AUR builds:** before paru starts, say how many packages it will build from source
  and that this can take minutes; paru's own build output then streams live.
- **Plan before acting:** `sync`, `add` and `remove` print the plan (`+`, `~`, `-`
  lines, with the file that declares each package) and counts, then ask
  `? Proceed? [Y/n]`. `diff` prints the same plan and stops.
- **Summary at the end:** counts installed/adopted/removed/orphans removed, total time,
  `.pacnew` files, reboot hint, and the log path. One screen, readable after the
  pacman scrollback.
- **Errors say what to do next.** novarch only sees exit codes (output is not captured),
  so hints are per phase: keyring phase failed → "check network, then run
  `novarch upgrade`"; install failed → "nothing was saved; scroll up for pacman's
  error"; lock held → the lock message from 4.1.
- **Quiet queries, visible waits.** Read-only queries are fast and silent; any phase
  that waits on the network (keyring refresh) gets a header first.
- **Colour rules:** `--color auto|always|never` (global), default `auto` = colour only
  when stderr is a tty. `NO_COLOR` set (any value) means `never`. Plain ANSI escape
  codes, no crate.
- **Icon fallback:** use the ASCII column when `TERM=linux` (the Linux console of a fresh
  install cannot draw `✓` or `➜`) or the locale is not UTF-8 (`LANG`/`LC_ALL`).
- `--verbose` also prints each read-only query as a `➜` line.

### 5.2 Log file

`~/.local/state/novarch/novarch.log`, appended (`std::fs::OpenOptions`), no colours or
icons, no logging crate. One line per event with a local timestamp
(`libc::localtime_r`) and the run's pid: each full argv, its exit code and duration,
every plan (install/adopt/remove sets), every answer to a novarch question, and the
final summary. A bad sync can be reconstructed from this file plus
`/var/log/pacman.log`. Rotate to `novarch.log.1` when it passes 1 MiB.

---

## 6. Things to handle (pitfalls)

| Pitfall | Handling |
|---|---|
| **Package groups** (`xorg`, `gnome`) | A group name is not a package, so `pacman -T` reports it missing forever. Expand at load time with one `pacman -Sg <all names>` call and treat members as declared (a name that is also a sync package stays a package). (`base-devel` is a meta *package* now, not a group, so it just works.) |
| **Provides / virtual names** (`sh` → `bash`, `neovim` satisfied by `neovim-git`) | Check with `pacman -T`, record real names via `pacman -Qq <name>`. |
| **Typos / nonexistent names** | Bad characters are caught at load (4.0). Names that are valid but don't exist: paru fails the whole transaction with "target not found"; nothing changes, the error names the package. |
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
    ├── config.rs              config.toml, first-run prompt, host name
    ├── ui.rs                  icons, colours, phases, prompts, log file (section 5)
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
| `libc` | `geteuid()`, `gethostname()`, `isatty()`, `flock()`, `localtime_r()`, SIGINT handler |
| `rustyline` | first-run folder prompt with directory tab completion |
| `clap_complete` | `novarch completions` |

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
2. **Config + lists:** first-run prompt and `config.toml` (`config.rs`); `lists.rs` loads,
   validates (4.0) and merges shared + host files. `novarch diff` prints the declared set.
3. **Query system:** `system.rs` wraps `pacman -T`, `-Qq`, `-Qqtt`. `diff` shows `to_install`.
4. **State + plan:** `state.rs`, `plan.rs` with its tests. `diff` shows all three sets.
5. **Preflight:** root check, lock check.
6. **Install path:** keyring refresh + `paru -Su --needed`, save state with lock and
   backups (4.10). `novarch sync` installs.
7. **Removal path:** sections 4.6 and 4.8. `sync` is now complete.
8. **upgrade, add, remove, status, list** commands (small once 6 and 7 exist).
9. **Bootstrap:** chaotic-aur + paru (test in a container — it edits `/etc/pacman.conf`).
10. **Polish:** group expansion, `.pacnew` report, reboot hint, shell completions.
11. **Release:** GitHub Actions release workflow, `install.sh`, AUR `PKGBUILD` (section 7).

The output style (5.1) and log file (5.2) are built in step 2 as a small `ui.rs`, so
every later step uses them from the start.

---

## 9. Deliberately not in v1

| Skipped | Add when |
|---|---|
| Pre/post hooks, systemd service enabling, dotfiles | Never: novarch manages packages only (decided). |
| Unattended mode (timers, cron, non-terminal runs) | Never (decided); see 4.1 step 0. |
| Btrfs snapshot before sync | You don't use snapper + `snap-pac`, which already snapshots every pacman transaction. |
| Separate `aur = [...]` key | A name exists in both repo and AUR and you need to pick. |
| Checking names exist before install (`pacman -Si` / AUR RPC) | paru's "target not found" is annoying enough to matter. |

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
