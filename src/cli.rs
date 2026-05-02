use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "ctx", version, about = "Claude Code session context manager", subcommand_required = false)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Close Terminal window on TUI exit (internal, used by osascript launch)
    #[arg(long, hide = true, global = true)]
    pub close_on_exit: bool,
}

#[derive(Subcommand)]
pub enum Command {
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
    /// Interactive session tree browser
    Tui {
        /// Session ID to load (internal, used by osascript launch)
        #[arg(long, hide = true)]
        session: Option<String>,
    },
}
