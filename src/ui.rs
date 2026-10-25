//! novarch's own output: icons, colours, questions and the log file (design section 5).
//! pacman/paru output never passes through here; they write to the terminal directly.

use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, IsTerminal, Write};
use std::sync::OnceLock;

use crate::config;

struct Style {
    color: bool,
    unicode: bool,
}

static STYLE: OnceLock<Style> = OnceLock::new();

const LOG_MAX_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Copy)]
enum Kind {
    Phase,
    Ok,
    Warn,
    Error,
    Ask,
}

impl Kind {
    /// (unicode icon, ASCII fallback, ANSI colour code)
    fn look(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Kind::Phase => ("::", "::", "1;34"),
            Kind::Ok => ("✓", "ok", "32"),
            Kind::Warn => ("⚠", "!", "33"),
            Kind::Error => ("✗", "x", "1;31"),
            Kind::Ask => ("?", "?", "1;36"),
        }
    }
}

/// `color` is the value of `--color`: "auto", "always" or "never".
pub fn init(color: &str) {
    let color = std::env::var_os("NO_COLOR").is_none()
        && match color {
            "always" => true,
            "never" => false,
            _ => io::stderr().is_terminal(),
        };
    let _ = STYLE.set(Style {
        color,
        unicode: unicode_ok(),
    });
}

/// The Linux console of a fresh install cannot draw ✓ or ➜; neither can a non-UTF-8 locale.
fn unicode_ok() -> bool {
    if std::env::var("TERM").is_ok_and(|t| t == "linux") {
        return false;
    }
    let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty()))
        .unwrap_or_default()
        .to_ascii_lowercase();
    locale.contains("utf-8") || locale.contains("utf8")
}

fn style() -> &'static Style {
    STYLE.get_or_init(|| Style {
        color: false,
        unicode: false,
    })
}

fn paint(code: &str, text: &str) -> String {
    if style().color {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

fn icon(kind: Kind) -> String {
    let (uni, ascii, code) = kind.look();
    paint(code, if style().unicode { uni } else { ascii })
}

fn emit(kind: Kind, msg: &str) {
    let text = match kind {
        Kind::Phase => paint("1", msg),
        _ => msg.to_string(),
    };
    eprintln!("{} {text}", icon(kind));
    log(msg);
}

pub fn phase(msg: &str) {
    emit(Kind::Phase, msg);
}

pub fn ok(msg: &str) {
    emit(Kind::Ok, msg);
}

pub fn warn(msg: &str) {
    emit(Kind::Warn, msg);
}

pub fn error(msg: &str) {
    emit(Kind::Error, msg);
}

/// A question line without an answer prompt (the caller reads the answer itself).
pub fn ask(msg: &str) {
    emit(Kind::Ask, msg);
}

/// Plain indented detail line (package lists etc.), not logged.
pub fn detail(msg: &str) {
    eprintln!("  {msg}");
}

pub fn dim(text: &str) -> String {
    paint("2", text)
}

/// Ask `[Y/n]` / `[y/N]` on stderr and read the answer from stdin.
pub fn confirm(question: &str, default_yes: bool) -> Result<bool, String> {
    let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
    let stdin = io::stdin();
    loop {
        eprint!("{} {question} {hint} ", icon(Kind::Ask));
        let _ = io::stderr().flush();
        let mut line = String::new();
        if stdin
            .lock()
            .read_line(&mut line)
            .map_err(|e| e.to_string())?
            == 0
        {
            return Err("no answer: input closed".into());
        }
        let answer = match line.trim().to_ascii_lowercase().as_str() {
            "" => default_yes,
            "y" | "yes" => true,
            "n" | "no" => false,
            _ => continue,
        };
        log(&format!(
            "? {question} -> {}",
            if answer { "yes" } else { "no" }
        ));
        return Ok(answer);
    }
}

/// Append one timestamped line to novarch.log. Logging problems never stop a run.
pub fn log(msg: &str) {
    let Ok(dir) = config::state_dir() else { return };
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join("novarch.log");
    if fs::metadata(&path).is_ok_and(|m| m.len() > LOG_MAX_BYTES) {
        let _ = fs::rename(&path, dir.join("novarch.log.1"));
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{} [{}] {msg}", now(), std::process::id());
    }
}

fn now() -> String {
    // SAFETY: time(NULL) and localtime_r with a valid out-pointer have no other requirements.
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        )
    }
}
