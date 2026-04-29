use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "ctx", version, about = "Claude Code session context manager", subcommand_required = false)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// List contents of current node
    Ls,
    /// Change current node (e.g. cd 2, cd 2/1, cd .., cd /)
    Cd {
        path: Vec<String>,
    },
    /// Show current path
    Pwd,
    /// Show conversation tree centered on current position
    List {
        /// Max depth per root (default: 2)
        #[arg(short = 'd', long, default_value = "2")]
        depth: usize,
        /// Upstream parent chain levels (default: 3)
        #[arg(short = 'u', long, default_value = "3")]
        upstream: usize,
        /// Max message length before truncation (default: 80)
        #[arg(short = 'l', long, default_value = "80")]
        max_len: usize,
    },
    /// Compact session summary
    Summary,
    /// Show session stats
    Info,
    /// Follow live session (like tail -f)
    Tail,
    /// Insert a context note (Claude must be stopped)
    Insert {
        #[arg(long)]
        under: Option<String>,
        #[arg(required = true, num_args = 1..)]
        text: Vec<String>,
    },
    /// Remove message + all descendants (Claude must be stopped)
    Rm {
        uuid: String,
    },
    /// Export session as JSON
    Export {
        file: Option<String>,
    },
}
