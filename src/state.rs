//! managed.json (what novarch owns), its backups, and the single-run lock (design 1, 4.10).

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::path::PathBuf;

use crate::plan::Set;
use crate::{config, ui};

const BACKUPS: usize = 5;

fn path() -> Result<PathBuf, String> {
    Ok(config::state_dir()?.join("managed.json"))
}

fn backup(n: usize) -> Result<PathBuf, String> {
    Ok(config::state_dir()?.join(format!("managed.json.{n}")))
}

/// Missing file = nothing managed yet. A file that does not parse stops the run: treating
/// it as empty and saving would silently forget what novarch owns.
pub fn load() -> Result<Set, String> {
    let p = path()?;
    match fs::read_to_string(&p) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| {
            format!(
                "{} is corrupt ({e}). Fix it, or copy back a backup (managed.json.1 to .{BACKUPS}); novarch will not overwrite it",
                p.display()
            )
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Set::new()),
        Err(e) => Err(format!("{}: {e}", p.display())),
    }
}

/// Rotate backups, then write atomically. Call only after the pacman/paru step succeeded.
pub fn save(managed: &Set) -> Result<(), String> {
    let p = path()?;
    if p.exists() {
        for n in (1..BACKUPS).rev() {
            let _ = fs::rename(backup(n)?, backup(n + 1)?);
        }
        fs::copy(&p, backup(1)?).map_err(|e| format!("backing up {}: {e}", p.display()))?;
    }
    let text = serde_json::to_string_pretty(managed).map_err(|e| e.to_string())? + "\n";
    config::write_atomic(&p, &text)?;
    ui::log(&format!("saved managed.json: {managed:?}"));
    Ok(())
}

/// Held for the whole run; the kernel releases it when novarch exits, so it never goes stale.
pub struct Lock {
    _file: File,
}

pub fn lock() -> Result<Lock, String> {
    let dir = config::state_dir()?;
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let p = dir.join("lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&p)
        .map_err(|e| format!("{}: {e}", p.display()))?;
    // SAFETY: flock on a file descriptor owned by `file`, which outlives the call.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(format!(
            "another novarch is already running (lock {})",
            p.display()
        ));
    }
    Ok(Lock { _file: file })
}
