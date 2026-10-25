# AGENTS.md

novarch: a Rust CLI that manages Arch Linux packages declaratively. Package names live
in TOML files in a user-chosen packages folder (shared files + `hosts/<host>/`); `novarch sync` installs them via
pacman/paru and removes packages that novarch manages but that are no longer listed.
Full design, flows and pitfalls: `docs/design.md`. Read it before changing behavior.

## Status

Steps 1-2 done: CLI skeleton; `config.rs` (config.toml, first-run folder prompt, host),
`lists.rs` (load + validate + merge), `ui.rs` (icons, colours, log). `novarch diff`
prints the declared set; other commands print "not implemented".
Next milestone: step 3 of "Build order" in `docs/design.md`.

## Stack

- Rust (edition 2024), `clap` derive (CLI). Planned crates and the milestone that adds
  each: `docs/design.md` section 7. Add a crate only when its milestone needs it.
- No runtime dependency besides glibc: releases are a single binary (GitHub Releases +
  curl `install.sh`, later AUR). Chosen over Python because Arch `base` has no Python.
- Build/run: `cargo build`, `cargo run -- <command>`.

## Decisions (do not re-litigate without the user)

- **Ownership via state file.** `~/.local/state/novarch/managed.json` records what
  novarch owns. Only `managed - declared` is ever removed. Packages installed any other
  way are never touched.
- **Adoption.** A listed package that is already installed becomes managed.
- **One `packages = [...]` list** per TOML file for repo and AUR alike; paru resolves both.
- **Chaotic-AUR is offered, not forced:** asked once, answer saved in `config.toml`;
  paru comes from it, or from the paru-bin fallback when declined.
- **Packages only.** No services, dotfiles or hooks. **No unattended mode.**
- **Hosts are generic:** any number of `hosts/<name>/` folders; never special-case
  particular machines.
- `add --no-upgrade` is the emergency path; default target file `manual-installs.toml`.

## Invariants (breaking these can damage a user's system)

- Never run as root. Use `sudo pacman` for repo work; paru calls sudo itself.
- Never `-Sy` without `-u` in the same run. Installs go in the same `paru -Su --needed`
  transaction as the upgrade.
- Never pass `-Rc`, `-Rdd`, or `--nodeps` to removals. Use `-Rs`, scoped to `to_remove` only.
- System-wide orphan cleanup only as the end-of-sync prompt (`docs/design.md` 4.8):
  `pacman -Qdtq` (single `-t`) minus protected and declared, default No, never answered
  by `--yes`, never `--noconfirm`.
- Mutating commands hold `flock` on `~/.local/state/novarch/lock`; rotate
  `managed.json` backups (`.1`..`.5`) before every write.
- Never delete `/var/lib/pacman/db.lck` automatically.
- Mutating pacman/paru calls inherit the terminal (no output capture, no prefixing, no
  tee) so prompts and progress bars reach the user. Icons/colours apply to novarch's
  own lines only (`docs/design.md` 5.1). `--noconfirm` only when the user passes `--yes`.
- While a child runs, SIGINT gets a no-op handler, never `SIG_IGN` (it would survive
  `exec` and make pacman unkillable).
- Update `managed.json` only after the pacman/paru call succeeded; write it atomically.
  A `managed.json` that fails to parse stops the run; never overwrite it as empty.
- Refuse to run with a missing/empty packages folder or zero declared packages.
  Any file that fails to parse stops the command.
- Mutating commands refuse when stdin is not a terminal.
- Protected packages (`docs/design.md` 4.6) are never removed. Removals are confirmed
  first; `--yes` removing >10 needs `--allow-mass-remove`.
- `--no-upgrade` installs only when `pacman -Qu` is empty (no partial upgrade).
- `add` installs first, then writes the TOML file.

## Testing

- Check before every commit: `cargo fmt --check && cargo clippy -- -D warnings && cargo test`.
- `plan.rs` is pure; its tests are a `#[cfg(test)]` module in the same file (`cargo test`).
  `lists.rs` has the same kind of module for parsing/validation.
- Try commands without touching your real config: `XDG_CONFIG_HOME=/tmp/x/cfg
  XDG_STATE_HOME=/tmp/x/state cargo run -- diff` (also set `HOME` when testing the
  first-run prompt, whose default folder is under `~`). Drive the prompt in a pty with
  `script -qec`.
- Test commands that change the system in a container (`docker run -it --rm archlinux:latest`)
  or a VM, never first on the dev machine. Bootstrap edits `/etc/pacman.conf`.

## Keeping this file current

Update this file in the same change whenever development changes something listed here:
status/next milestone, stack, a decision, an invariant, or how to run/test. Keep it to
what an agent cannot learn quickly from reading the code — no file-by-file tours, no
changelog, no restating `docs/design.md`. Delete lines that stop being true. If a change
alters a flow in `docs/design.md`, update that section too.
