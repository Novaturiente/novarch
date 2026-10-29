//! Every external command (design 4, 5). Read-only queries capture output; mutating
//! calls inherit the terminal so pacman/paru prompts and progress bars reach the user.

use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::plan::Set;
use crate::ui;

static INTERRUPTED: AtomicBool = AtomicBool::new(false);
static SUDO_HINT_SHOWN: AtomicBool = AtomicBool::new(false);

pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

struct Captured {
    code: i32,
    stdout: String,
    stderr: String,
}

fn capture(program: &str, args: &[&str]) -> Result<Captured, String> {
    if ui::verbose() {
        ui::cmd(&format!("{program} {}", args.join(" ")));
    }
    let out = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run {program}: {e}"))?;
    Ok(Captured {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

fn lines(text: &str) -> Set {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

fn failed(program: &str, args: &[&str], c: &Captured) -> String {
    let why = c.stderr.lines().next().unwrap_or("").trim();
    format!(
        "{program} {} failed (exit {}): {why}",
        args.join(" "),
        c.code
    )
}

/// A pacman query. Exit 1 with no output at all means "no matches", not an error.
fn pacman_list(args: &[&str]) -> Result<Set, String> {
    let c = capture("pacman", args)?;
    match c.code {
        0 => Ok(lines(&c.stdout)),
        1 if c.stdout.trim().is_empty() && c.stderr.trim().is_empty() => Ok(Set::new()),
        _ => Err(failed("pacman", args, &c)),
    }
}

pub fn installed() -> Result<Set, String> {
    pacman_list(&["-Qq"])
}

pub fn explicit() -> Result<Set, String> {
    pacman_list(&["-Qqe"])
}

/// Installed packages not found in any sync repo (AUR and other foreign packages).
pub fn foreign() -> Result<Set, String> {
    pacman_list(&["-Qqm"])
}

pub fn installed_as_deps() -> Result<Set, String> {
    pacman_list(&["-Qqd"])
}

/// Installed packages no other package hard-requires (`-tt`: optional deps do not count).
pub fn unrequired() -> Result<Set, String> {
    pacman_list(&["-Qqtt"])
}

/// Installed as a dependency and needed by nothing, not even optionally (single `-t`).
pub fn orphans() -> Result<Set, String> {
    pacman_list(&["-Qqdt"])
}

/// Packages the local sync database has newer versions of.
pub fn pending_upgrades() -> Result<Set, String> {
    pacman_list(&["-Qqu"])
}

pub fn sync_packages() -> Result<Set, String> {
    pacman_list(&["-Slq"])
}

/// Keyring packages (archlinux-keyring, chaotic-keyring, ...) without hardcoding names.
pub fn keyrings() -> Result<Set, String> {
    let dir = "/usr/share/pacman/keyrings";
    if Path::new(dir).is_dir() {
        pacman_list(&["-Qqo", dir])
    } else {
        Ok(Set::new())
    }
}

/// `pacman -T`: the names that are not satisfied. Exit 127 is data, not an error.
pub fn unsatisfied(names: &Set) -> Result<Set, String> {
    if names.is_empty() {
        return Ok(Set::new());
    }
    let mut args = vec!["-T"];
    args.extend(names.iter().map(String::as_str));
    let c = capture("pacman", &args)?;
    match c.code {
        0 => Ok(Set::new()),
        127 => Ok(lines(&c.stdout)),
        _ => Err(failed("pacman", &["-T", "..."], &c)),
    }
}

/// Satisfied declared name -> installed package that satisfies it (`pacman -Qq sh` = bash).
pub fn real_names(names: &Set, installed: &Set) -> Result<BTreeMap<String, String>, String> {
    let mut real = BTreeMap::new();
    for name in names {
        if installed.contains(name) {
            real.insert(name.clone(), name.clone());
            continue;
        }
        let c = capture("pacman", &["-Qq", name])?;
        match c.stdout.lines().next() {
            Some(r) if c.code == 0 => {
                real.insert(name.clone(), r.trim().to_string());
            }
            _ => ui::warn(&format!(
                "{name} is satisfied but pacman cannot name the package; not managing it"
            )),
        }
    }
    Ok(real)
}

/// Members of every name that is a sync-repo group (`pacman -Sg a b c`, one call).
pub fn groups(names: &Set) -> Result<BTreeMap<String, Vec<String>>, String> {
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if names.is_empty() {
        return Ok(groups);
    }
    let mut args = vec!["-Sg"];
    args.extend(names.iter().map(String::as_str));
    // Exit status is 0 even when some names are not groups; those only print to stderr.
    let c = capture("pacman", &args)?;
    for line in c.stdout.lines() {
        if let Some((group, member)) = line.split_once(' ') {
            groups
                .entry(group.to_string())
                .or_default()
                .push(member.trim().to_string());
        }
    }
    Ok(groups)
}

/// `Required By` of an installed package.
pub fn required_by(name: &str) -> Result<Vec<String>, String> {
    let c = capture("pacman", &["-Qi", name])?;
    let line = c
        .stdout
        .lines()
        .find(|l| l.starts_with("Required By"))
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| v.trim().to_string())
        .unwrap_or_default();
    Ok(if line == "None" || line.is_empty() {
        Vec::new()
    } else {
        line.split_whitespace().map(String::from).collect()
    })
}

/// One search row: name, repo, version, installed mark, description.
type Row = [String; 5];

/// `pacman -Ss` with no term (every sync package, description on the next line):
/// `repo/name version (groups) [installed]` / `    description`.
fn parse_ss(text: &str) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    for line in text.lines() {
        if line.starts_with(char::is_whitespace) {
            if let Some(row) = rows.last_mut() {
                row[4] = line.trim().to_string();
            }
            continue;
        }
        let mut f = line.split_whitespace();
        let (Some(id), Some(version)) = (f.next(), f.next()) else {
            continue;
        };
        let Some((repo, name)) = id.split_once('/') else {
            continue;
        };
        let rest: Vec<&str> = f.collect();
        let mark = rest
            .iter()
            .position(|t| t.starts_with('['))
            .map(|i| rest[i..].join(" "))
            .unwrap_or_default();
        rows.push([name, repo, version, &mark, ""].map(String::from));
    }
    rows
}

/// `paru -Sl aur`: `aur name unknown-version [installed]`, no descriptions.
fn parse_sl(text: &str) -> Vec<Row> {
    text.lines()
        .filter_map(|line| {
            let mut f = line.split_whitespace();
            let (repo, name) = (f.next()?, f.next()?);
            let version = f.next().filter(|v| *v != "unknown-version").unwrap_or("");
            let mark = f.collect::<Vec<_>>().join(" ");
            Some([name, repo, version, &mark, ""].map(String::from))
        })
        .collect()
}

/// One fzf line per sync-repo package (with description) and AUR package (name only),
/// tab-separated: name, repo, version + mark, description. The caller searches fields 1
/// and 4 only (`--nth 1,4`); `{1}` is the name, `{2}` the repo.
pub fn package_index() -> Result<String, String> {
    let repo = capture("pacman", &["-Ss"])?;
    if repo.code != 0 {
        return Err(failed("pacman", &["-Ss"], &repo));
    }
    let mut rows = parse_ss(&repo.stdout);
    if in_path("paru") {
        let aur = capture("paru", &["-Sl", "aur"])?;
        if aur.code == 0 {
            rows.extend(parse_sl(&aur.stdout));
        } else {
            ui::warn(&failed("paru", &["-Sl", "aur"], &aur));
            ui::warn("showing repo packages only");
        }
    } else {
        ui::warn("paru is not installed: showing repo packages only");
    }
    let mut out = String::new();
    for [name, repo, version, mark, desc] in rows {
        let colour = if repo == "aur" { "35" } else { "34" };
        out.push_str(&format!(
            "{name:<36}\t{}\t{} {}\t{}\n",
            ui::paint(colour, &format!("{repo:<16}")),
            ui::dim(&format!("{version:<14}")),
            ui::paint("32", &mark),
            ui::dim(&desc)
        ));
    }
    Ok(out)
}

/// `paru -Ss` (repo and AUR, grouped with descriptions and installed marks), or
/// `pacman -Ss` without paru. Read-only, so it prints straight to the terminal.
/// Returns false when nothing matched.
pub fn search(words: &[String]) -> Result<bool, String> {
    let program = if in_path("paru") { "paru" } else { "pacman" };
    let status = Command::new(program)
        .args(["--color", ui::color_arg(), "-Ss"])
        .args(words)
        .stdin(Stdio::null())
        .status()
        .map_err(|e| format!("cannot run {program}: {e}"))?;
    match status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(format!("{program} -Ss failed ({status})")),
    }
}

/// Unread Arch news (`paru -Pw`: newer than the installed repo packages), which can
/// announce manual steps an upgrade needs. None when there is none or paru is missing.
pub fn news() -> Result<Option<String>, String> {
    if !in_path("paru") {
        return Ok(None);
    }
    let c = capture("paru", &["-Pw"])?;
    if c.code == 0 && !c.stdout.trim().is_empty() {
        return Ok(Some(c.stdout));
    }
    // Exit 1 with "no new news" on stderr is the normal case.
    if c.stderr.trim_start().starts_with("error") {
        ui::warn(&format!(
            "could not check Arch news: {}",
            c.stderr.lines().next().unwrap_or("").trim()
        ));
    }
    Ok(None)
}

/// fzf multi-select over `lines` (package name first on each line); `opts` adds the
/// prompt, header, preview, query. Returns the chosen names; empty when cancelled.
pub fn fzf_pick(lines: &str, opts: &[&str]) -> Result<Vec<String>, String> {
    let mut child = Command::new("fzf")
        .args([
            "--multi",
            "--ansi",
            "--layout",
            "reverse",
            "--preview-window",
            "right,55%,wrap",
        ])
        .args(opts)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run fzf: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        // A broken pipe only means the user quit before the list was fully read.
        let _ = stdin.write_all(lines.as_bytes());
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("fzf failed: {e}"))?;
    match out.status.code() {
        Some(0) => Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| l.split_whitespace().next())
            .map(String::from)
            .collect()),
        // 1: no match, 130: Esc or Ctrl-C.
        Some(1 | 130) => Ok(Vec::new()),
        _ => Err(format!("fzf failed ({})", out.status)),
    }
}

pub fn pacman_running() -> bool {
    Command::new("pgrep")
        .args(["-x", "pacman"])
        .stdout(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

pub fn in_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(program).is_file()))
}

extern "C" fn on_sigint(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

/// Run a mutating command with the terminal inherited. `input` (if any) is fed to stdin
/// and stdout is discarded (used for `sudo tee`).
pub fn exec(
    program: &str,
    args: &[String],
    cwd: Option<&Path>,
    input: Option<&str>,
) -> Result<(), String> {
    ui::cmd(&format!("{program} {}", args.join(" ")));
    if matches!(program, "sudo" | "paru" | "makepkg")
        && !SUDO_HINT_SHOWN.swap(true, Ordering::SeqCst)
    {
        ui::detail(&ui::dim("sudo may ask for your password"));
    }
    let mut cmd = Command::new(program);
    cmd.args(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    if input.is_some() {
        cmd.stdin(Stdio::piped()).stdout(Stdio::null());
    }
    ui::log(&format!("exec: {program} {}", args.join(" ")));
    let start = Instant::now();
    // A no-op handler, not SIG_IGN: ignored signals stay ignored across exec, which would
    // make the child unkillable with Ctrl-C. Handlers are reset to default in the child.
    // SAFETY: the handler only stores to an atomic, which is async-signal-safe.
    unsafe {
        libc::signal(
            libc::SIGINT,
            on_sigint as extern "C" fn(libc::c_int) as libc::sighandler_t,
        );
    }
    let result = (|| {
        let mut child = cmd.spawn()?;
        if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
            stdin.write_all(text.as_bytes())?;
        }
        child.wait()
    })();
    // SAFETY: restoring the default disposition.
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_DFL);
    }
    let status = result.map_err(|e| format!("cannot run {program}: {e}"))?;
    let took = ui::duration(start.elapsed());
    ui::log(&format!("exit: {status} after {took}"));
    if interrupted() || status.signal() == Some(libc::SIGINT) || status.code() == Some(130) {
        INTERRUPTED.store(true, Ordering::SeqCst);
        return Err("interrupted".into());
    }
    if status.success() {
        ui::ok(&format!("done in {took}"));
        Ok(())
    } else {
        let code = status.code().unwrap_or(-1);
        Err(if code == 1 {
            format!("{program} exited with code 1 (also happens when you answer \"n\")")
        } else {
            format!("{program} exited with code {code}")
        })
    }
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

pub fn run(program: &str, args: &[&str]) -> Result<(), String> {
    exec(program, &strings(args), None, None)
}

/// `sudo pacman --color <c> <args> [--noconfirm]`.
pub fn pacman(args: &[&str], noconfirm: bool) -> Result<(), String> {
    let mut a = strings(&["pacman", "--color", ui::color_arg()]);
    a.extend(strings(args));
    if noconfirm {
        a.push("--noconfirm".into());
    }
    exec("sudo", &a, None, None)
}

/// `paru --color <c> --sudoloop <args> [--noconfirm]`. paru calls sudo itself.
pub fn paru(args: &[&str], noconfirm: bool) -> Result<(), String> {
    let mut a = strings(&["--color", ui::color_arg(), "--sudoloop"]);
    a.extend(strings(args));
    if noconfirm {
        a.push("--noconfirm".into());
    }
    exec("paru", &a, None, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pacman_ss() {
        let text = "core/bash 5.3-1 [installed]\n    The GNU shell\ncore/base-devel 1-2 (base-devel) [installed: 1-1]\n    Basic tools\nextra/zsh 5.9-5\n    A shell\n";
        let rows = parse_ss(text);
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows[0],
            ["bash", "core", "5.3-1", "[installed]", "The GNU shell"]
        );
        assert_eq!(rows[1][3], "[installed: 1-1]");
        assert_eq!(rows[2], ["zsh", "extra", "5.9-5", "", "A shell"]);
    }

    #[test]
    fn parses_paru_sl_aur() {
        let rows = parse_sl("aur foo unknown-version\naur bar unknown-version [installed]\n");
        assert_eq!(rows[0], ["foo", "aur", "", "", ""]);
        assert_eq!(rows[1][3], "[installed]");
    }
}
