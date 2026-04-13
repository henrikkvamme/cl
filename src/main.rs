mod claude;
mod config;
mod git;
mod linear;
mod prompt;

use std::process;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "cl", about = "Bridge Linear issues to Claude Code")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Linear issue URL (e.g. https://linear.app/org/issue/KL-35/slug)
    issue_link: Option<String>,

    /// Create a new git branch instead of a worktree
    #[arg(short = 'b')]
    branch: bool,

    /// No worktree, no branch — run claude in the current working tree
    #[arg(long)]
    raw: bool,

    /// Enable Chrome integration
    #[arg(short = 'c')]
    chrome: bool,

    /// Base branch for rebasing (default: auto-detect)
    #[arg(long)]
    base: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Store your Linear personal access token
    Login {
        /// Linear API key
        api_key: String,
    },
    /// Print current merged configuration
    Config,
}

fn main() {
    let cli = Cli::parse();
    let cfg = config::load_config();

    match cli.command {
        Some(Commands::Login { api_key }) => {
            config::save_token(&api_key);
        }
        Some(Commands::Config) => {
            config::print_config(&cfg);
        }
        None => {
            let issue_link = cli.issue_link.unwrap_or_else(|| {
                eprintln!("Usage: cl <issue-link> [-b -c] [--raw]");
                eprintln!("       cl login <api-key>");
                process::exit(1);
            });

            // CLI flags override config
            let mode = if cli.branch {
                "branch"
            } else if cli.raw {
                "raw"
            } else {
                cfg.mode()
            };
            let worktree = mode == "worktree";
            let branch = mode == "branch";
            let chrome = cli.chrome || cfg.chrome();

            let base_branch =
                git::resolve_base_branch(cli.base.as_deref()).unwrap_or_else(|e| {
                    eprintln!("Error: {e}");
                    process::exit(1);
                });
            run_start(&issue_link, branch, worktree, chrome, &base_branch, &cfg);
        }
    }
}

fn run_start(
    issue_link: &str,
    branch: bool,
    worktree: bool,
    chrome: bool,
    base_branch: &str,
    cfg: &config::ClConfig,
) {
    let identifier = linear::parse_identifier(issue_link).unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        process::exit(1);
    });

    let api_key = config::load_token();

    eprintln!("Fetching issue {identifier}...");
    let issue = linear::fetch_issue(&api_key, &identifier).unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        process::exit(1);
    });
    eprintln!("Found: {} — {}", issue.identifier, issue.title);
    eprintln!("Branch: {}", issue.branch_name);

    linear::update_issue(&api_key, &issue);

    if branch {
        git::ensure_clean_tree().unwrap_or_else(|e| {
            eprintln!("Error: {e}");
            process::exit(1);
        });
        git::create_branch(&issue.branch_name).unwrap_or_else(|e| {
            eprintln!("Error: {e}");
            process::exit(1);
        });
        eprintln!("Created branch: {}", issue.branch_name);
    }

    if chrome {
        claude::ensure_chrome_running();
    }

    let prompt = prompt::build_prompt(&issue, worktree, chrome, base_branch);
    claude::exec_claude(&issue, &prompt, worktree, chrome, cfg);
}
