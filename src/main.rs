mod config;
mod lists;
mod ui;

use clap::{Parser, Subcommand};

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
    /// Print a shell completion script (bash, zsh or fish).
    Completions { shell: String },
}

fn main() {
    let cli = Cli::parse();
    ui::init(&cli.color);
    let result = match cli.command {
        Command::Diff => diff(),
        other => not_implemented(&other),
    };
    if let Err(e) = result {
        for line in e.lines() {
            ui::error(line);
        }
        std::process::exit(1);
    }
}

/// Step 2: prints the declared set. Steps 3-4 add to_install, to_adopt and to_remove.
fn diff() -> Result<(), String> {
    let cfg = config::load()?;
    ui::phase(&format!(
        "Reading package lists from {} (host {})",
        cfg.packages_dir.display(),
        cfg.host
    ));
    let declared = lists::load(&cfg.packages_dir, &cfg.host)?;
    let width = declared.keys().map(String::len).max().unwrap_or(0);
    for (name, files) in &declared {
        let files: Vec<String> = files.iter().map(|f| f.display().to_string()).collect();
        ui::detail(&format!("{name:<width$}  {}", ui::dim(&files.join(", "))));
    }
    let file_count = declared
        .values()
        .flatten()
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let plural = |n: usize, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
    ui::ok(&format!(
        "{} declared in {}",
        plural(declared.len(), "package"),
        plural(file_count, "file")
    ));
    Ok(())
}

fn not_implemented(command: &Command) -> Result<(), String> {
    let name = match command {
        Command::Sync { .. } => "sync",
        Command::Diff => "diff",
        Command::Upgrade => "upgrade",
        Command::Add { .. } => "add",
        Command::Remove { .. } => "remove",
        Command::Status => "status",
        Command::List { .. } => "list",
        Command::Completions { .. } => "completions",
    };
    Err(format!("{name}: not implemented yet"))
}
