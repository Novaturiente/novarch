# AGENTS.md

novarch: a Rust CLI that manages Arch Linux packages declaratively. Package names live
in TOML files under `~/.config/novarch/packages/`; `novarch sync` installs them via
pacman/paru and removes packages that novarch manages but that are no longer listed.
Full design, flows and pitfalls: `docs/design.md`. Read it before changing behavior.

## Status

Step 1 (skeleton) done: `novarch --help` works, every command prints "not implemented".
Next milestone: step 2 of "Build order" in `docs/design.md`.

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
- **Chaotic-AUR is set up automatically** if missing; paru is installed from it.

## Invariants (breaking these can damage a user's system)

- Never run as root. Use `sudo pacman` for repo work; paru calls sudo itself.
- Never `-Sy` without `-u` in the same run. Installs go in the same `paru -Su --needed`
  transaction as the upgrade.
- Never pass `-Rc`, `-Rdd`, or `--nodeps` to removals. Use `-Rs`, scoped to `to_remove` only.
  Never run a system-wide orphan cleanup.
- Never delete `/var/lib/pacman/db.lck` automatically.
- Mutating pacman/paru calls inherit the terminal (no output capture) so prompts and
  progress bars reach the user. `--noconfirm` only when the user passes `--yes`.
- Update `managed.json` only after the pacman/paru call succeeded; write it atomically.
- `add` installs first, then writes the TOML file.

## Testing

- Check before every commit: `cargo fmt --check && cargo clippy -- -D warnings && cargo test`.
- `plan.rs` is pure; its tests are a `#[cfg(test)]` module in the same file (`cargo test`).
- Test commands that change the system in a container (`docker run -it --rm archlinux:latest`)
  or a VM, never first on the dev machine. Bootstrap edits `/etc/pacman.conf`.

## Keeping this file current

Update this file in the same change whenever development changes something listed here:
status/next milestone, stack, a decision, an invariant, or how to run/test. Keep it to
what an agent cannot learn quickly from reading the code — no file-by-file tours, no
changelog, no restating `docs/design.md`. Delete lines that stop being true. If a change
alters a flow in `docs/design.md`, update that section too.
