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
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Upgrade, install listed packages, remove unlisted managed packages.
    Sync,
    /// Show what sync would install, adopt and remove. Changes nothing.
    Diff,
    /// Refresh keyrings and run a full system upgrade.
    Upgrade,
    /// Install packages, then add them to a package file.
    Add {
        #[arg(required = true)]
        packages: Vec<String>,
        /// Package file to append to (default: added.toml).
        #[arg(long)]
        to: Option<String>,
    },
    /// Remove managed packages from the package files and the system.
    Remove {
        #[arg(required = true)]
        packages: Vec<String>,
    },
}

fn main() {
    let cli = Cli::parse();
    let name = match cli.command {
        Command::Sync => "sync",
        Command::Diff => "diff",
        Command::Upgrade => "upgrade",
        Command::Add { .. } => "add",
        Command::Remove { .. } => "remove",
    };
    eprintln!(":: novarch: {name}: not implemented yet");
    std::process::exit(1);
}
