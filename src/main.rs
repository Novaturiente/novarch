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
    let name = match cli.command {
        Command::Sync { .. } => "sync",
        Command::Diff => "diff",
        Command::Upgrade => "upgrade",
        Command::Add { .. } => "add",
        Command::Remove { .. } => "remove",
        Command::Status => "status",
        Command::List { .. } => "list",
        Command::Completions { .. } => "completions",
    };
    eprintln!(":: novarch: {name}: not implemented yet");
    std::process::exit(1);
}
