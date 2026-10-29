//! config.toml, the first-run packages-folder prompt, and the host name (design 2, 4.0).

use std::fs;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};

use rustyline::completion::{Completer, FilenameCompleter, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::history::DefaultHistory;
use rustyline::validate::Validator;
use rustyline::{Context, Editor, Helper};
use toml_edit::{DocumentMut, value};

use crate::ui;

const DEFAULT_PACKAGES_DIR: &str = "~/.config/novarch/packages";

pub struct Config {
    pub packages_dir: PathBuf,
    pub host: String,
    /// Answer to the Chaotic-AUR question; `None` = not asked yet.
    pub chaotic_aur: Option<bool>,
}

fn config_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("config.toml"))
}

fn read_doc(path: &Path) -> Result<DocumentMut, String> {
    match fs::read_to_string(path) {
        Ok(text) => text
            .parse::<DocumentMut>()
            .map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(DocumentMut::new()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Save the Chaotic-AUR answer, keeping the rest of config.toml (and its comments).
pub fn set_chaotic(answer: bool) -> Result<(), String> {
    let path = config_path()?;
    let mut doc = read_doc(&path)?;
    doc["chaotic_aur"] = value(answer);
    write_atomic(&path, &doc.to_string())
}

fn home() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is not set".to_string())
}

/// `$VAR` if it is an absolute path, else `$HOME/<fallback>` (XDG base directory rules).
fn xdg(var: &str, fallback: &str) -> Result<PathBuf, String> {
    match std::env::var_os(var).map(PathBuf::from) {
        Some(p) if p.is_absolute() => Ok(p),
        _ => Ok(home()?.join(fallback)),
    }
}

pub fn config_dir() -> Result<PathBuf, String> {
    Ok(xdg("XDG_CONFIG_HOME", ".config")?.join("novarch"))
}

pub fn state_dir() -> Result<PathBuf, String> {
    Ok(xdg("XDG_STATE_HOME", ".local/state")?.join("novarch"))
}

/// Package names from the last `search`, one per line (tab completion for `add`).
pub fn names_cache() -> Result<PathBuf, String> {
    Ok(xdg("XDG_CACHE_HOME", ".cache")?.join("novarch/packages.txt"))
}

/// Load config.toml. If the packages folder is unset or unusable, ask for one (terminal
/// only) until it is usable, then save it.
pub fn load() -> Result<Config, String> {
    let path = config_path()?;
    let mut doc = read_doc(&path)?;
    for (key, item) in doc.iter() {
        let ok = match key {
            "packages_dir" | "host" => item.is_str(),
            "chaotic_aur" => item.as_bool().is_some(),
            _ => return Err(format!("{}: unknown key `{key}`", path.display())),
        };
        if !ok {
            return Err(format!("{}: `{key}` has the wrong type", path.display()));
        }
    }

    let saved = doc
        .get("packages_dir")
        .and_then(|i| i.as_str())
        .map(PathBuf::from);
    let mut dir = saved.clone();
    loop {
        match &dir {
            None => ui::phase("First run: choose the folder that holds your package lists"),
            Some(d) => match check_packages_dir(d) {
                Ok(()) => break,
                Err(why) => {
                    ui::warn(&why);
                    // An existing folder without package files: on an existing system the
                    // natural first step is importing what is installed (design 4.9).
                    if d.is_dir() && io::stdin().is_terminal() && offer_import(d)? {
                        continue;
                    }
                }
            },
        }
        if !io::stdin().is_terminal() {
            return Err(format!(
                "packages folder is not usable; run novarch in a terminal to choose one (config: {})",
                path.display()
            ));
        }
        let initial = dir
            .as_deref()
            .map(|d| d.display().to_string())
            .unwrap_or_else(|| DEFAULT_PACKAGES_DIR.to_string());
        dir = Some(prompt_dir(&initial)?);
    }
    let dir = dir.expect("loop only breaks with a folder");

    if saved.as_ref() != Some(&dir) {
        doc["packages_dir"] = value(dir.display().to_string());
        write_atomic(&path, &doc.to_string())?;
        ui::ok(&format!("Saved packages folder to {}", path.display()));
    }

    let host = match doc.get("host").and_then(|i| i.as_str()) {
        Some(h) => h.to_string(),
        None => hostname()?,
    };
    if host.is_empty() || host.contains('/') || host == "." || host == ".." {
        return Err(format!("invalid host name `{host}`"));
    }
    Ok(Config {
        packages_dir: dir,
        host,
        chaotic_aur: doc.get("chaotic_aur").and_then(|i| i.as_bool()),
    })
}

fn offer_import(dir: &Path) -> Result<bool, String> {
    let explicit = crate::system::explicit()?;
    if explicit.is_empty() {
        return Ok(false);
    }
    let question = format!(
        "Import the {} packages you installed explicitly into preinstalled.toml?",
        explicit.len()
    );
    if !ui::confirm(&question, true)? {
        return Ok(false);
    }
    let file = dir.join("preinstalled.toml");
    crate::lists::write_import(&file, &explicit, &crate::system::foreign()?)?;
    ui::ok(&format!(
        "Wrote {}; review it, then run novarch diff",
        file.display()
    ));
    Ok(true)
}

/// The folder must exist and hold at least one top-level `*.toml` (design 4.0 step 2).
fn check_packages_dir(dir: &Path) -> Result<(), String> {
    let shown = dir.display();
    if !dir.exists() {
        return Err(format!("packages folder {shown} does not exist"));
    }
    if !dir.is_dir() {
        return Err(format!("{shown} is not a folder"));
    }
    let entries = fs::read_dir(dir).map_err(|e| format!("{shown}: {e}"))?;
    let has_toml = entries
        .flatten()
        .any(|e| e.path().extension().is_some_and(|x| x == "toml") && e.path().is_file());
    if !has_toml {
        return Err(format!(
            "packages folder {shown} has no *.toml files; add one, e.g. base.toml with packages = [\"git\"]"
        ));
    }
    Ok(())
}

fn prompt_dir(initial: &str) -> Result<PathBuf, String> {
    ui::ask("Packages folder (Tab completes, Ctrl-C quits):");
    let mut editor: Editor<DirHelper, DefaultHistory> = Editor::new().map_err(|e| e.to_string())?;
    editor.set_helper(Some(DirHelper(FilenameCompleter::new())));
    let line = match editor.readline_with_initial("> ", (initial, "")) {
        Ok(l) => l,
        Err(ReadlineError::Interrupted | ReadlineError::Eof) => return Err("cancelled".into()),
        Err(e) => return Err(e.to_string()),
    };
    let dir = absolute(line.trim())?;
    ui::log(&format!("packages folder answer: {}", dir.display()));
    if !dir.exists() && ui::confirm(&format!("Create {}?", dir.display()), true)? {
        fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        ui::ok(&format!("Created {}", dir.display()));
    }
    Ok(dir)
}

/// Expand a leading `~` and make the path absolute.
fn absolute(input: &str) -> Result<PathBuf, String> {
    let path = if input == "~" {
        home()?
    } else if let Some(rest) = input.strip_prefix("~/") {
        home()?.join(rest)
    } else {
        PathBuf::from(input)
    };
    if path.as_os_str().is_empty() {
        return Err("no folder given".into());
    }
    std::path::absolute(&path).map_err(|e| e.to_string())
}

fn hostname() -> Result<String, String> {
    fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|h| h.trim().to_string())
        .map_err(|e| format!("cannot read host name: {e}"))
}

/// Write via a temp file in the same folder, then rename, so a crash never leaves half a file.
pub fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let dir = path.parent().ok_or("path has no parent folder")?;
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Tab completion that offers folders only, like `cd` in a shell.
struct DirHelper(FilenameCompleter);

impl Completer for DirHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let (start, mut pairs) = self.0.complete_path(line, pos)?;
        pairs.retain(|p| p.replacement.ends_with('/'));
        Ok((start, pairs))
    }
}

impl Hinter for DirHelper {
    type Hint = String;
}
impl Highlighter for DirHelper {}
impl Validator for DirHelper {}
impl Helper for DirHelper {}
