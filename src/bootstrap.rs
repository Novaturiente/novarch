//! Chaotic-AUR and paru setup (design 4.2, 4.3).

use std::fs;

use crate::{config, system, ui};

const PACMAN_CONF: &str = "/etc/pacman.conf";
const CHAOTIC_KEY: &str = "3056513887B78AEB";
const CHAOTIC_PACKAGES: [&str; 2] = [
    "https://cdn-mirror.chaotic.cx/chaotic-aur/chaotic-keyring.pkg.tar.zst",
    "https://cdn-mirror.chaotic.cx/chaotic-aur/chaotic-mirrorlist.pkg.tar.zst",
];

pub fn has_chaotic() -> bool {
    fs::read_to_string(PACMAN_CONF).is_ok_and(|t| t.lines().any(|l| l.trim() == "[chaotic-aur]"))
}

/// Ask once (answer saved in config.toml), then add the repo if accepted. Must run before
/// the keyring refresh, whose `-Sy` then picks the new repo up.
pub fn chaotic(choice: Option<bool>, yes: bool) -> Result<(), String> {
    if has_chaotic() {
        return Ok(());
    }
    let want = match choice {
        Some(c) => c,
        None => {
            ui::ask(
                "Chaotic-AUR is a third-party repo of prebuilt AUR packages. Adding it trusts its signing key for root installs; AUR packages then install in seconds instead of compiling.",
            );
            let answer = yes || ui::confirm("Add Chaotic-AUR to /etc/pacman.conf?", true)?;
            config::set_chaotic(answer)?;
            answer
        }
    };
    if !want {
        return Ok(());
    }
    ui::phase("Adding Chaotic-AUR");
    system::run(
        "sudo",
        &[
            "pacman-key",
            "--recv-key",
            CHAOTIC_KEY,
            "--keyserver",
            "keyserver.ubuntu.com",
        ],
    )?;
    system::run("sudo", &["pacman-key", "--lsign-key", CHAOTIC_KEY]).map_err(|e| {
        format!("{e}\nIf it says there is no secret key, initialise pacman's keyring once: sudo pacman-key --init")
    })?;
    let mut args = vec!["-U", "--needed"];
    args.extend(CHAOTIC_PACKAGES);
    system::pacman(&args, yes)?;
    system::run("sudo", &["cp", PACMAN_CONF, "/etc/pacman.conf.novarch.bak"])?;
    // At the end: the first repo listed wins on name clashes, so official repos stay first.
    system::exec(
        "sudo",
        &["tee".into(), "-a".into(), PACMAN_CONF.into()],
        None,
        Some("\n[chaotic-aur]\nInclude = /etc/pacman.d/chaotic-mirrorlist\n"),
    )?;
    ui::ok("Chaotic-AUR added (backup: /etc/pacman.conf.novarch.bak)");
    Ok(())
}

/// Install paru if missing. Call right after the keyring refresh: its `-Sy` must be
/// followed by `-Su`, so paru comes in a full-upgrade transaction (no partial upgrade).
pub fn paru(yes: bool) -> Result<(), String> {
    if system::in_path("paru") {
        return Ok(());
    }
    ui::phase("Installing paru (AUR helper)");
    if has_chaotic() {
        return system::pacman(&["-Su", "--needed", "paru"], yes);
    }
    // base-devel does not include git.
    system::pacman(&["-Su", "--needed", "base-devel", "git"], yes)?;
    // paru-bin skips a long Rust compile, but its binary can lag behind pacman's libalpm
    // soname (seen: built for libalpm.so.15 on pacman 7.1 / .so.16). Verify it starts.
    build_from_aur("paru-bin", yes)?;
    if paru_works() {
        return Ok(());
    }
    ui::warn(
        "paru-bin does not start on this system (built against another pacman); building paru from source instead, this takes several minutes",
    );
    let installed = system::installed()?;
    let mut remove = vec!["-R", "paru-bin"];
    if installed.contains("paru-bin-debug") {
        remove.push("paru-bin-debug");
    }
    // Installed seconds ago and nothing depends on it: a plain -R is safe.
    system::pacman(&remove, yes)?;
    build_from_aur("paru", yes)?;
    if paru_works() {
        Ok(())
    } else {
        Err("paru was built but does not start; run paru --version to see why".into())
    }
}

fn paru_works() -> bool {
    std::process::Command::new("paru")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Clone an AUR package into a temp folder and `makepkg -sir` it (installs, then removes
/// its build-only dependencies).
fn build_from_aur(name: &str, yes: bool) -> Result<(), String> {
    let dir = std::env::temp_dir().join(format!("novarch-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let dir_s = dir.display().to_string();
    let url = format!("https://aur.archlinux.org/{name}.git");
    let result = system::run("git", &["clone", "--depth=1", &url, &dir_s]).and_then(|()| {
        let mut args = vec!["-sir".to_string(), "--needed".to_string()];
        if yes {
            args.push("--noconfirm".into());
        }
        system::exec("makepkg", &args, Some(&dir), None)
    });
    let _ = fs::remove_dir_all(&dir);
    result
}
