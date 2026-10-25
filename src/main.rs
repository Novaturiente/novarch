mod bootstrap;
mod config;
mod lists;
mod plan;
mod state;
mod system;
mod ui;

use std::collections::BTreeMap;
use std::fs;
use std::io::IsTerminal;
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use clap::{CommandFactory, Parser, Subcommand};

use lists::Declared;
use plan::{Plan, Set};

const MASS_REMOVE_LIMIT: usize = 10;
const DB_LCK: &str = "/var/lib/pacman/db.lck";
const DEFAULT_ADD_FILE: &str = "manual-installs.toml";

/// Declarative package management for Arch Linux.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Pass --noconfirm to pacman/paru.
    #[arg(long, global = true)]
    yes: bool,
    /// Print more detail.
    #[arg(long, global = true)]
    verbose: bool,
    /// When to use colours (NO_COLOR forces never).
    #[arg(long, global = true, default_value = "auto", value_parser = ["auto", "always", "never"])]
    color: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Upgrade, install listed packages, remove unlisted managed packages.
    Sync {
        /// With --yes, allow removing more than 10 packages.
        #[arg(long)]
        allow_mass_remove: bool,
    },
    /// Show what sync would install, adopt and remove. Changes nothing.
    Diff,
    /// Refresh keyrings and run a full system upgrade.
    Upgrade,
    /// Install packages, then add them to a package file.
    Add {
        #[arg(required = true)]
        packages: Vec<String>,
        /// Package file to append to, relative to the packages folder
        /// (default: manual-installs.toml).
        #[arg(long)]
        to: Option<String>,
        /// Emergency install without a full upgrade (refused if one is pending).
        #[arg(long)]
        no_upgrade: bool,
    },
    /// Remove managed packages from the package files and the system.
    Remove {
        #[arg(required = true)]
        packages: Vec<String>,
        /// With --yes, allow removing more than 10 packages.
        #[arg(long)]
        allow_mass_remove: bool,
    },
    /// List installed packages no file declares; offer to write them to a new file.
    Status,
    /// Show declared packages, the file that lists each, and install state.
    List { packages: Vec<String> },
    /// Print a shell completion script.
    Completions { shell: clap_complete::Shell },
}

fn main() {
    let cli = Cli::parse();
    ui::init(&cli.color, cli.verbose);
    let yes = cli.yes;
    if let Command::Completions { shell } = cli.command {
        clap_complete::generate(
            shell,
            &mut Cli::command(),
            "novarch",
            &mut std::io::stdout(),
        );
        return;
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    ui::log(&format!("run: novarch {}", args.join(" ")));
    let result = match cli.command {
        Command::Sync { allow_mass_remove } => sync(yes, allow_mass_remove),
        Command::Diff => diff(),
        Command::Upgrade => upgrade(yes),
        Command::Add {
            packages,
            to,
            no_upgrade,
        } => add(packages, to, no_upgrade, yes),
        Command::Remove {
            packages,
            allow_mass_remove,
        } => remove(packages, yes, allow_mass_remove),
        Command::Status => status(),
        Command::List { packages } => list(packages),
        Command::Completions { .. } => unreachable!("handled above"),
    };
    if let Err(e) = result {
        if system::interrupted() {
            ui::error("interrupted, nothing saved for this step");
            std::process::exit(130);
        }
        for line in e.lines() {
            ui::error(line);
        }
        std::process::exit(1);
    }
}

/// Add a next-step hint to an error (novarch sees exit codes only, not pacman's output).
fn hint(e: String, next: &str) -> String {
    if system::interrupted() {
        e
    } else {
        format!("{e}\n{next}")
    }
}

/// Everything the plan is computed from.
struct World {
    declared: Declared,
    names: Set,
    installed: Set,
    unsatisfied: Set,
    real: BTreeMap<String, String>,
    managed: Set,
}

impl World {
    fn look(cfg: &config::Config) -> Result<World, String> {
        let declared = load_declared(cfg)?;
        let mut w = World {
            names: declared.keys().cloned().collect(),
            declared,
            installed: Set::new(),
            unsatisfied: Set::new(),
            real: BTreeMap::new(),
            managed: state::load()?,
        };
        w.refresh()?;
        Ok(w)
    }

    /// Re-read what is installed (after pacman/paru changed it).
    fn refresh(&mut self) -> Result<(), String> {
        self.installed = system::installed()?;
        self.unsatisfied = system::unsatisfied(&self.names)?;
        let satisfied: Set = self.names.difference(&self.unsatisfied).cloned().collect();
        self.real = system::real_names(&satisfied, &self.installed)?;
        Ok(())
    }

    fn plan(&self) -> Plan {
        plan::plan(
            &self.names,
            &self.unsatisfied,
            &self.real,
            &self.managed,
            &self.installed,
        )
    }

    fn wanted(&self) -> Set {
        plan::wanted(&self.names, &self.real)
    }

    fn files(&self, name: &str) -> String {
        self.declared
            .get(name)
            .map(|f| {
                f.iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default()
    }
}

/// Package files plus group expansion (a group name is not a package; design 6).
fn load_declared(cfg: &config::Config) -> Result<Declared, String> {
    ui::phase(&format!(
        "Reading package lists from {} (host {})",
        cfg.packages_dir.display(),
        cfg.host
    ));
    let mut declared = lists::load(&cfg.packages_dir, &cfg.host)?;
    let names: Set = declared.keys().cloned().collect();
    let groups = system::groups(&names)?;
    if !groups.is_empty() {
        let packages = system::sync_packages()?;
        for (group, members) in groups {
            if packages.contains(&group) {
                continue;
            }
            let files = declared.remove(&group).unwrap_or_default();
            ui::detail(&ui::dim(&format!(
                "group {group}: {} packages",
                members.len()
            )));
            for m in members {
                let entry = declared.entry(m).or_default();
                for f in &files {
                    if !entry.contains(f) {
                        entry.push(f.clone());
                    }
                }
            }
        }
    }
    Ok(declared)
}

fn show_plan(w: &World, p: &Plan) {
    ui::phase("Plan");
    let width = [&p.install, &p.adopt, &p.remove]
        .iter()
        .flat_map(|s| s.iter())
        .map(String::len)
        .max()
        .unwrap_or(0);
    for n in &p.install {
        ui::detail(&format!(
            "{} {n:<width$}  {}",
            ui::paint("32", "+"),
            ui::dim(&w.files(n))
        ));
    }
    for r in &p.adopt {
        let from: Vec<&String> = w
            .real
            .iter()
            .filter(|(_, v)| *v == r)
            .map(|(k, _)| k)
            .collect();
        let note = match from.as_slice() {
            [only] if *only == r => w.files(r),
            _ => format!(
                "declared as {} in {}",
                from.iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                from.iter()
                    .map(|k| w.files(k))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
        ui::detail(&format!(
            "{} {r:<width$}  {}",
            ui::paint("34", "~"),
            ui::dim(&format!("adopt: already installed; {note}"))
        ));
    }
    for n in &p.remove {
        ui::detail(&format!(
            "{} {n:<width$}  {}",
            ui::paint("31", "-"),
            ui::dim("managed, no longer listed in any file")
        ));
    }
    for n in &p.unmanage {
        if w.installed.contains(n) {
            ui::warn(&format!(
                "{n} is protected: novarch stops managing it but will not remove it"
            ));
        } else {
            ui::detail(&ui::dim(&format!(
                "{n}: no longer installed; novarch forgets it"
            )));
        }
    }
    ui::ok(&format!(
        "{} to install, {} to adopt, {} to remove",
        p.install.len(),
        p.adopt.len(),
        p.remove.len()
    ));
    ui::log(&format!("plan: {p:?}"));
}

fn preflight() -> Result<state::Lock, String> {
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        return Err("do not run novarch as root or with sudo: AUR builds refuse root.\nRun it as your normal user; it calls sudo itself.".into());
    }
    if !std::io::stdin().is_terminal() {
        return Err(
            "this command changes the system and needs a terminal (there is no unattended mode)"
                .into(),
        );
    }
    let lock = state::lock()?;
    if Path::new(DB_LCK).exists() {
        return Err(if system::pacman_running() {
            format!("pacman is running ({DB_LCK} exists); wait for it to finish")
        } else {
            format!(
                "{DB_LCK} exists but no pacman is running, so it is probably stale.\nIf no other package manager is running, remove it: sudo rm {DB_LCK}"
            )
        });
    }
    Ok(lock)
}

/// Chaotic-AUR (asked once), keyring refresh with `-Sy`, then paru if missing (its
/// install is the `-Su` that must follow). Counts as one numbered step.
fn prepare(cfg: &config::Config, yes: bool) -> Result<(), String> {
    bootstrap::chaotic(cfg.chaotic_aur, yes)?;
    ui::step("Refreshing keyrings and package databases");
    let keyrings: Vec<String> = system::keyrings()?.into_iter().collect();
    let mut args = vec!["-Sy", "--needed"];
    args.extend(keyrings.iter().map(String::as_str));
    system::pacman(&args, yes)
        .map_err(|e| hint(e, "check the network and mirrors, then run novarch upgrade"))?;
    bootstrap::paru(yes)
}

/// Warn before paru builds packages from source (they are in no sync repo). Returns extra
/// targets for the same transaction: makepkg needs base-devel, which paru does not pull in.
fn note_source_builds(names: &Set, installed: &Set) -> Result<Vec<String>, String> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let repo = system::sync_packages()?;
    let aur: Vec<&str> = names
        .iter()
        .filter(|n| !repo.contains(*n))
        .map(String::as_str)
        .collect();
    if !aur.is_empty() {
        ui::warn(&format!(
            "{} not in any repo: paru builds from source, which can take minutes: {}",
            aur.len(),
            aur.join(", ")
        ));
        if !installed.contains("base-devel") {
            return Ok(vec!["base-devel".into()]);
        }
    }
    Ok(Vec::new())
}

fn diff() -> Result<(), String> {
    let cfg = config::load()?;
    let w = World::look(&cfg)?;
    show_plan(&w, &w.plan());
    Ok(())
}

fn sync(yes: bool, allow_mass_remove: bool) -> Result<(), String> {
    let start = Instant::now();
    let _lock = preflight()?;
    let cfg = config::load()?;
    let mut w = World::look(&cfg)?;
    let p = w.plan();
    show_plan(&w, &p);
    check_mass(p.remove.len(), yes, allow_mass_remove)?;
    if !yes && !ui::confirm("Proceed?", true)? {
        ui::warn("Nothing changed");
        return Ok(());
    }

    ui::set_steps(5);
    prepare(&cfg, yes)?;

    ui::step(&if p.install.is_empty() {
        "Upgrading the system".to_string()
    } else {
        format!("Upgrading the system and installing {}", p.install.len())
    });
    let extra = note_source_builds(&p.install, &w.installed)?;
    let mut args = vec!["-Su", "--needed"];
    args.extend(extra.iter().map(String::as_str));
    args.extend(p.install.iter().map(String::as_str));
    system::paru(&args, yes).map_err(|e| {
        hint(
            e,
            "nothing was saved; scroll up for pacman's or paru's error",
        )
    })?;
    w.refresh()?;
    let missing: Vec<String> = p.install.intersection(&w.unsatisfied).cloned().collect();
    if !missing.is_empty() {
        ui::warn(&format!("still not installed: {}", missing.join(", ")));
    }
    adopt(&w)?;
    let mut managed: Set = w
        .managed
        .iter()
        .chain(w.real.values())
        .filter(|m| !p.unmanage.contains(*m))
        .cloned()
        .collect();
    state::save(&managed)?;

    ui::step("Removing packages that are no longer listed");
    let to_remove: Set = p.remove.intersection(&w.installed).cloned().collect();
    let removed = remove_packages(&to_remove, yes, true)?;
    if !removed.is_empty() {
        managed.retain(|m| !removed.contains(m));
        state::save(&managed)?;
    }

    ui::step("Checking for orphaned packages");
    w.refresh()?;
    let orphans = offer_orphans(&w.wanted())?;

    ui::step("Report");
    report();
    ui::ok(&format!(
        "sync finished in {}: {} installed, {} adopted, {} removed, {} orphans removed",
        ui::duration(start.elapsed()),
        p.install.len() - missing.len(),
        p.adopt.len(),
        removed.len(),
        orphans
    ));
    log_hint();
    Ok(())
}

fn check_mass(count: usize, yes: bool, allow: bool) -> Result<(), String> {
    if yes && count > MASS_REMOVE_LIMIT && !allow {
        return Err(format!(
            "{count} packages would be removed without questions (--yes). Check with novarch diff, then add --allow-mass-remove if this is intended."
        ));
    }
    Ok(())
}

/// Declared packages installed as dependencies become explicit, so they are not removed
/// as orphans when whatever pulled them in goes away (design 1, adoption).
fn adopt(w: &World) -> Result<(), String> {
    let deps = system::installed_as_deps()?;
    let mark: Vec<&str> = w
        .real
        .values()
        .filter(|r| deps.contains(*r))
        .map(String::as_str)
        .collect();
    if mark.is_empty() {
        return Ok(());
    }
    let mut args = vec!["-D", "--asexplicit"];
    args.extend(mark);
    system::pacman(&args, false)
}

/// Design 4.6. Returns the packages novarch no longer owns (removed, or handed to pacman
/// as dependencies). An empty set when the user declines.
fn remove_packages(set: &Set, yes: bool, ask: bool) -> Result<Set, String> {
    if set.is_empty() {
        ui::ok("nothing to remove");
        return Ok(Set::new());
    }
    for n in set {
        ui::detail(&format!("{} {n}", ui::paint("31", "-")));
    }
    if ask && !yes && !ui::confirm(&format!("Remove these {} packages?", set.len()), false)? {
        ui::warn("Skipped; they stay managed and are offered again next sync");
        return Ok(Set::new());
    }
    let (rs, asdeps) = plan::split_removal(set, &system::unrequired()?);
    if !asdeps.is_empty() {
        for n in &asdeps {
            ui::warn(&format!(
                "{n} is still required by another package: marking it as a dependency; pacman removes it with its last dependent"
            ));
        }
        let mut args = vec!["-D", "--asdeps"];
        args.extend(asdeps.iter().map(String::as_str));
        system::pacman(&args, false)?;
    }
    if !rs.is_empty() {
        let mut args = vec!["-Rs"];
        args.extend(rs.iter().map(String::as_str));
        system::pacman(&args, yes).map_err(|e| hint(e, "nothing was removed"))?;
    }
    Ok(set.clone())
}

/// Design 4.8: always asks, default no, never answered by --yes, never --noconfirm.
fn offer_orphans(wanted: &Set) -> Result<usize, String> {
    let orphans = plan::safe_orphans(&system::orphans()?, wanted);
    if orphans.is_empty() {
        ui::ok("no orphaned packages");
        return Ok(0);
    }
    ui::warn(&format!(
        "{} (installed as dependencies, needed by nothing, not even optionally):",
        match orphans.len() {
            1 => "1 orphaned package".to_string(),
            n => format!("{n} orphaned packages"),
        }
    ));
    for o in &orphans {
        ui::detail(o);
    }
    ui::detail(&ui::dim(
        "Usually safe to remove. Keep any you use directly by adding it to a package file.",
    ));
    if !ui::confirm("Remove these orphaned packages?", false)? {
        return Ok(0);
    }
    let mut args = vec!["-Rs"];
    args.extend(orphans.iter().map(String::as_str));
    system::pacman(&args, false)?;
    Ok(orphans.len())
}

fn report() {
    let mut pacnew = Vec::new();
    find_pacnew(Path::new("/etc"), &mut pacnew);
    if pacnew.is_empty() {
        ui::ok("no .pacnew files");
    } else {
        ui::warn(&format!(
            "{} .pacnew files (new default configs); merge with: sudo pacdiff (pacman-contrib)",
            pacnew.len()
        ));
        for f in pacnew.iter().take(10) {
            ui::detail(&f.display().to_string());
        }
    }
    if reboot_needed() {
        ui::warn("the running kernel was upgraded: reboot recommended");
    }
}

fn log_hint() {
    if let Ok(dir) = config::state_dir() {
        ui::detail(&ui::dim(&format!(
            "log: {} (pacman: /var/log/pacman.log)",
            dir.join("novarch.log").display()
        )));
    }
}

fn find_pacnew(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let Ok(t) = e.file_type() else { continue };
        let p = e.path();
        if t.is_dir() {
            find_pacnew(&p, out);
        } else if p.extension().is_some_and(|x| x == "pacnew") {
            out.push(p);
        }
    }
}

/// The running kernel's module folder disappears when its package is upgraded.
fn reboot_needed() -> bool {
    let modules = Path::new("/usr/lib/modules");
    let has_kernels = fs::read_dir(modules).is_ok_and(|mut d| d.next().is_some());
    let running = fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
    has_kernels && !modules.join(running.trim()).exists()
}

fn upgrade(yes: bool) -> Result<(), String> {
    let start = Instant::now();
    let _lock = preflight()?;
    let cfg = config::load()?;
    ui::set_steps(3);
    prepare(&cfg, yes)?;
    ui::step("Upgrading the system");
    system::paru(&["-Su"], yes).map_err(|e| hint(e, "scroll up for pacman's or paru's error"))?;
    ui::step("Report");
    report();
    ui::ok(&format!(
        "upgrade finished in {}",
        ui::duration(start.elapsed())
    ));
    log_hint();
    Ok(())
}

/// `--to` must be a `*.toml` path inside the packages folder.
fn target_file(to: Option<&str>) -> Result<PathBuf, String> {
    let rel = PathBuf::from(to.unwrap_or(DEFAULT_ADD_FILE));
    let inside = rel.components().all(|c| matches!(c, Component::Normal(_)));
    if !inside || rel.extension().is_none_or(|x| x != "toml") {
        return Err(format!(
            "{} must be a .toml file path inside the packages folder (e.g. hosts/laptop/dev.toml)",
            rel.display()
        ));
    }
    Ok(rel)
}

fn add(names: Vec<String>, to: Option<String>, no_upgrade: bool, yes: bool) -> Result<(), String> {
    if let Some(bad) = names.iter().find(|n| !lists::valid_name(n)) {
        return Err(format!("invalid package name \"{bad}\""));
    }
    let rel = target_file(to.as_deref())?;
    let _lock = preflight()?;
    let cfg = config::load()?;
    let w = World::look(&cfg)?;
    let mut new = Vec::new();
    for n in names {
        if w.declared.contains_key(&n) {
            ui::warn(&format!(
                "{n} is already listed in {}; run novarch sync to install it",
                w.files(&n)
            ));
        } else if !new.contains(&n) {
            new.push(n);
        }
    }
    if new.is_empty() {
        return Ok(());
    }
    ui::phase("Plan");
    for n in &new {
        ui::detail(&format!(
            "{} {n}  {}",
            ui::paint("32", "+"),
            ui::dim(&format!("-> {}", rel.display()))
        ));
    }
    if !yes && !ui::confirm("Proceed?", true)? {
        ui::warn("Nothing changed");
        return Ok(());
    }
    let new_set: Set = new.iter().cloned().collect();
    if no_upgrade {
        ui::set_steps(3);
        ui::step("Checking that no upgrade is pending");
        let pending = system::pending_upgrades()?;
        if !pending.is_empty() {
            return Err(format!(
                "{} packages have updates waiting in the local database (pacman -Sy ran without -u).\nInstalling now would be a partial upgrade. Run novarch add without --no-upgrade.",
                pending.len()
            ));
        }
        if !system::in_path("paru") {
            return Err("paru is not installed yet; run novarch add without --no-upgrade once".into());
        }
        ui::ok("no pending upgrades");
        ui::step("Installing without upgrading");
        let mut args = vec!["-S", "--needed"];
        args.extend(new.iter().map(String::as_str));
        system::paru(&args, yes)
    } else {
        ui::set_steps(3);
        prepare(&cfg, yes)?;
        ui::step(&format!("Upgrading the system and installing {}", new.len()));
        let extra = note_source_builds(&new_set, &w.installed)?;
        let mut args = vec!["-Su", "--needed"];
        args.extend(extra.iter().map(String::as_str));
        args.extend(new.iter().map(String::as_str));
        system::paru(&args, yes)
    }
    .map_err(|e| hint(e, "nothing was added to your package files"))?;

    ui::step("Saving");
    let missing = system::unsatisfied(&new_set)?;
    let done: Vec<String> = new.into_iter().filter(|n| !missing.contains(n)).collect();
    if !missing.is_empty() {
        ui::warn(&format!(
            "not installed, so not added: {}",
            missing.iter().cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    if done.is_empty() {
        return Ok(());
    }
    lists::append(&cfg.packages_dir.join(&rel), &done)?;
    ui::ok(&format!("added {} to {}", done.join(", "), rel.display()));
    let done_set: Set = done.into_iter().collect();
    let real = system::real_names(&done_set, &system::installed()?)?;
    let managed: Set = w.managed.iter().chain(real.values()).cloned().collect();
    state::save(&managed)?;
    Ok(())
}

fn remove(names: Vec<String>, yes: bool, allow_mass_remove: bool) -> Result<(), String> {
    let _lock = preflight()?;
    let cfg = config::load()?;
    let w = World::look(&cfg)?;
    let mut targets = Set::new();
    for n in &names {
        let real = if w.managed.contains(n) {
            n.clone()
        } else if let Some(r) = w.real.get(n).filter(|r| w.managed.contains(*r)) {
            r.clone()
        } else {
            return Err(format!(
                "{n} is not managed by novarch; remove it with: sudo pacman -Rs {n}"
            ));
        };
        targets.insert(real);
    }
    check_mass(targets.len(), yes, allow_mass_remove)?;
    let (protected, removable): (Set, Set) =
        targets.into_iter().partition(|t| plan::is_protected(t));
    ui::phase("Plan");
    for n in &names {
        let files = w.files(n);
        ui::detail(&format!(
            "{} {n}  {}",
            ui::paint("31", "-"),
            ui::dim(if files.is_empty() {
                "not in any file"
            } else {
                &files
            })
        ));
    }
    for p in &protected {
        ui::warn(&format!(
            "{p} is protected: novarch stops managing it but will not remove it"
        ));
    }
    if !yes && !ui::confirm("Proceed?", true)? {
        ui::warn("Nothing changed");
        return Ok(());
    }

    ui::set_steps(2);
    ui::step("Updating package files");
    for n in &names {
        for f in w.declared.get(n).into_iter().flatten() {
            if lists::remove_name(&cfg.packages_dir.join(f), n)? {
                ui::ok(&format!("removed {n} from {}", f.display()));
            } else {
                ui::warn(&format!(
                    "{n} comes from a group listed in {}; remove the group or it comes back",
                    f.display()
                ));
            }
        }
    }

    ui::step("Removing packages");
    let installed: Set = removable.intersection(&w.installed).cloned().collect();
    let removed = remove_packages(&installed, yes, false)?;
    let managed: Set = w
        .managed
        .iter()
        .filter(|m| !removed.contains(*m) && !protected.contains(*m))
        .filter(|m| !(removable.contains(*m) && !w.installed.contains(*m)))
        .cloned()
        .collect();
    state::save(&managed)?;
    Ok(())
}

fn status() -> Result<(), String> {
    let cfg = config::load()?;
    let w = World::look(&cfg)?;
    let foreign = system::foreign()?;
    let wanted = w.wanted();
    let unlisted: Set = system::explicit()?
        .into_iter()
        .filter(|p| !wanted.contains(p))
        .collect();
    if unlisted.is_empty() {
        ui::ok("every explicitly installed package is in a package file");
        return Ok(());
    }
    ui::phase(&format!(
        "{} explicitly installed packages are not in any package file",
        unlisted.len()
    ));
    for n in &unlisted {
        let src = if foreign.contains(n) {
            "AUR / foreign"
        } else {
            "repo"
        };
        ui::detail(&format!("{n}  {}", ui::dim(src)));
    }
    if !std::io::stdin().is_terminal()
        || !ui::confirm(
            "Write them to a new package file so novarch manages them?",
            true,
        )?
    {
        return Ok(());
    }
    let name = ui::input("File name inside the packages folder", "preinstalled.toml")?;
    let rel = target_file(Some(&name))?;
    lists::write_import(&cfg.packages_dir.join(&rel), &unlisted, &foreign)?;
    ui::ok(&format!(
        "wrote {}; run novarch sync to adopt them (nothing is downloaded)",
        rel.display()
    ));
    Ok(())
}

fn list(names: Vec<String>) -> Result<(), String> {
    let cfg = config::load()?;
    let w = World::look(&cfg)?;
    let rows: Vec<String> = if names.is_empty() {
        w.names.iter().cloned().collect()
    } else {
        names
    };
    let width = rows.iter().map(String::len).max().unwrap_or(0);
    for n in &rows {
        let line = if w.declared.contains_key(n) {
            let state = if w.unsatisfied.contains(n) {
                ui::paint("31", &format!("{:<10}", "missing"))
            } else {
                ui::paint("32", &format!("{:<10}", "installed"))
            };
            let real = w.real.get(n);
            let managed = match real {
                Some(r) if w.managed.contains(r) => "managed",
                Some(_) => "not managed yet",
                None => "",
            };
            let via = match real {
                Some(r) if r != n => format!(" (as {r})"),
                _ => String::new(),
            };
            format!(
                "{n:<width$}  {state}  {:<15}  {}",
                format!("{managed}{via}"),
                ui::dim(&w.files(n))
            )
        } else if w.installed.contains(n) {
            let req = system::required_by(n)?;
            let why = match req.len() {
                0 => "installed, not declared".to_string(),
                1..=5 => format!("installed, not declared; required by {}", req.join(", ")),
                more => format!(
                    "installed, not declared; required by {} and {} more",
                    req[..5].join(", "),
                    more - 5
                ),
            };
            format!("{n:<width$}  {}", ui::paint("33", &why))
        } else {
            format!("{n:<width$}  {}", ui::dim("not installed, not declared"))
        };
        ui::detail(&line);
    }
    if names_were_all(&rows, &w) {
        ui::ok(&format!("{} packages declared", rows.len()));
    }
    Ok(())
}

fn names_were_all(rows: &[String], w: &World) -> bool {
    rows.len() == w.names.len() && rows.iter().all(|r| w.names.contains(r))
}
