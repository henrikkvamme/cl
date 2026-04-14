mod claude;
mod config;
mod git;
mod linear;
mod picker;
mod prompt;

use std::collections::HashMap;
use std::process;

use clap::{Parser, Subcommand};
use inquire::Select;

#[derive(Parser)]
#[command(name = "cl", about = "Bridge Linear issues to Claude Code")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Linear issue: URL, identifier (KL-35), or number (35)
    issue: Option<String>,

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
    /// Show active sessions with Linear status
    Status,
    /// Remove worktrees for completed/cancelled issues
    Clean,
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
        Some(Commands::Status) => {
            run_status();
        }
        Some(Commands::Clean) => {
            run_clean();
        }
        None => {
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

            match cli.issue {
                Some(issue) => {
                    run_start(&issue, branch, worktree, chrome, &base_branch, &cfg);
                }
                None => {
                    run_picker(branch, worktree, chrome, &base_branch, &cfg);
                }
            }
        }
    }
}

/// Resolve team from config, or show a picker if not configured.
fn resolve_team(cfg: &config::ClConfig, api_key: &str) -> String {
    if let Some(team) = cfg.team() {
        return team.to_string();
    }

    eprintln!("No team configured. Fetching teams from Linear...");
    let teams = linear::fetch_teams(api_key).unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        process::exit(1);
    });

    if teams.is_empty() {
        eprintln!("Error: No teams found in your Linear workspace.");
        process::exit(1);
    }

    let key = if teams.len() == 1 {
        let team = &teams[0];
        eprintln!("Using team: {} ({})", team.key, team.name);
        team.key.clone()
    } else {
        let selected = Select::new("Select team:", teams)
            .prompt()
            .unwrap_or_else(|e| {
                eprintln!("Error: {e}");
                process::exit(1);
            });
        selected.key
    };

    config::save_workspace_team(&key);
    key
}

fn run_picker(
    branch: bool,
    worktree: bool,
    chrome: bool,
    base_branch: &str,
    cfg: &config::ClConfig,
) {
    let api_key = config::load_token();
    let team = resolve_team(cfg, &api_key);

    // Fetch worktrees and issues in sequence (both need the API key context)
    let worktrees = git::list_worktrees().unwrap_or_else(|e| {
        eprintln!("Warning: Could not list worktrees: {e}");
        Vec::new()
    });

    eprintln!("Fetching issues for team {team}...");
    let (issues, viewer_id) = linear::fetch_team_issues(&api_key, &team).unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        process::exit(1);
    });

    // Fetch titles for active sessions from Linear
    let session_ids: Vec<&str> = worktrees
        .iter()
        .filter_map(|w| w.identifier.as_deref())
        .collect();
    let session_titles: HashMap<String, String> = if !session_ids.is_empty() {
        linear::fetch_issue_statuses(&api_key, &session_ids)
            .unwrap_or_default()
            .into_iter()
            .map(|s| (s.identifier, s.title))
            .collect()
    } else {
        HashMap::new()
    };

    let result =
        picker::show_unified_picker(worktrees, issues, &viewer_id, &session_titles)
            .unwrap_or_else(|e| {
                eprintln!("Error: {e}");
                process::exit(1);
            });

    match result {
        picker::PickerResult::ResumeSession(wt) => {
            let name = wt.identifier.as_deref().unwrap_or(&wt.branch);
            eprintln!("Resuming session in {}...", wt.path.display());
            claude::exec_claude_resume(name, &wt.path, cfg);
        }
        picker::PickerResult::StartIssue(identifier) => {
            run_start(&identifier, branch, worktree, chrome, base_branch, cfg);
        }
    }
}

fn run_start(
    issue_input: &str,
    branch: bool,
    worktree: bool,
    chrome: bool,
    base_branch: &str,
    cfg: &config::ClConfig,
) {
    let api_key = config::load_token();

    // Resolve team if needed (bare number input)
    let team = if issue_input.chars().all(|c| c.is_ascii_digit()) && cfg.team().is_none() {
        Some(resolve_team(cfg, &api_key))
    } else {
        cfg.team().map(String::from)
    };

    let identifier =
        linear::resolve_identifier(issue_input, team.as_deref()).unwrap_or_else(|e| {
            eprintln!("Error: {e}");
            process::exit(1);
        });

    eprintln!("Fetching issue {identifier}...");
    let issue = linear::fetch_issue(&api_key, &identifier).unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        process::exit(1);
    });
    eprintln!("Found: {} — {}", issue.identifier, issue.title);
    eprintln!("Branch: {}", issue.branch_name);

    linear::update_issue(&api_key, &issue);

    let worktree_path = if worktree {
        let path = git::create_worktree(&issue.branch_name).unwrap_or_else(|e| {
            eprintln!("Error: {e}");
            process::exit(1);
        });
        eprintln!("Worktree: {}", path.display());
        Some(path)
    } else if branch {
        git::ensure_clean_tree().unwrap_or_else(|e| {
            eprintln!("Error: {e}");
            process::exit(1);
        });
        git::create_branch(&issue.branch_name).unwrap_or_else(|e| {
            eprintln!("Error: {e}");
            process::exit(1);
        });
        eprintln!("Created branch: {}", issue.branch_name);
        None
    } else {
        None
    };

    if chrome {
        claude::ensure_chrome_running();
    }

    let prompt = prompt::build_prompt(&issue, worktree, chrome, base_branch);
    claude::exec_claude(
        &issue,
        &prompt,
        worktree_path.as_deref(),
        chrome,
        cfg,
    );
}

fn run_status() {
    let worktrees = git::list_worktrees().unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        process::exit(1);
    });

    if worktrees.is_empty() {
        println!("No active sessions.");
        return;
    }

    // Fetch issue statuses from Linear
    let api_key = config::load_token();
    let ids: Vec<&str> = worktrees
        .iter()
        .filter_map(|w| w.identifier.as_deref())
        .collect();

    let statuses: HashMap<String, linear::IssueSummary> =
        if !ids.is_empty() {
            linear::fetch_issue_statuses(&api_key, &ids)
                .unwrap_or_default()
                .into_iter()
                .map(|s| (s.identifier.clone(), s))
                .collect()
        } else {
            HashMap::new()
        };

    // Check for PRs via gh
    let pr_urls: HashMap<String, String> = worktrees
        .iter()
        .filter_map(|w| {
            let url = std::process::Command::new("gh")
                .args(["pr", "view", &w.branch, "--json", "url", "-q", ".url"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty())?;
            Some((w.branch.clone(), url))
        })
        .collect();

    println!("Active Sessions");
    println!("{}", "─".repeat(60));
    for wt in &worktrees {
        let id = wt.identifier.as_deref().unwrap_or(&wt.branch);
        let ago = git::time_ago(wt.last_activity);

        let (status, title) = statuses
            .get(id)
            .map(|s| (s.state_name.as_str(), s.title.as_str()))
            .unwrap_or(("", ""));

        let pr = pr_urls
            .get(&wt.branch)
            .map(|u| format!("  {u}"))
            .unwrap_or_default();

        println!("{id}  {status}  {title}  ({ago}){pr}");
    }
}

fn run_clean() {
    let worktrees = git::list_worktrees().unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        process::exit(1);
    });

    if worktrees.is_empty() {
        println!("No worktrees to clean.");
        return;
    }

    let api_key = config::load_token();
    let ids: Vec<&str> = worktrees
        .iter()
        .filter_map(|w| w.identifier.as_deref())
        .collect();

    // Fetch statuses to find completed/cancelled issues
    let statuses: HashMap<String, String> = if !ids.is_empty() {
        linear::fetch_issue_statuses(&api_key, &ids)
            .unwrap_or_default()
            .into_iter()
            .map(|s| (s.identifier.clone(), s.state_name.clone()))
            .collect()
    } else {
        HashMap::new()
    };

    // Also check which branches have merged PRs
    let merged_branches: Vec<String> = worktrees
        .iter()
        .filter(|w| {
            std::process::Command::new("gh")
                .args(["pr", "view", &w.branch, "--json", "state", "-q", ".state"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .is_some_and(|s| s == "MERGED")
        })
        .map(|w| w.branch.clone())
        .collect();

    let stale: Vec<&git::WorktreeInfo> = worktrees
        .iter()
        .filter(|w| {
            let is_done = w
                .identifier
                .as_ref()
                .and_then(|id| statuses.get(id))
                .is_some_and(|state| {
                    let s = state.to_lowercase();
                    s.contains("done") || s.contains("complete") || s.contains("cancel")
                });
            let is_merged = merged_branches.contains(&w.branch);
            is_done || is_merged
        })
        .collect();

    if stale.is_empty() {
        println!("No stale worktrees to clean.");
        return;
    }

    println!("Stale worktrees:");
    for wt in &stale {
        let id = wt.identifier.as_deref().unwrap_or(&wt.branch);
        let status = wt
            .identifier
            .as_ref()
            .and_then(|i| statuses.get(i))
            .map(|s| s.as_str())
            .unwrap_or("");
        let merged = if merged_branches.contains(&wt.branch) {
            " (PR merged)"
        } else {
            ""
        };
        println!("  {id}  {status}{merged}  {}", wt.path.display());
    }

    let confirm = inquire::Confirm::new("Remove these worktrees?")
        .with_default(false)
        .prompt()
        .unwrap_or(false);

    if !confirm {
        println!("Aborted.");
        return;
    }

    for wt in &stale {
        let id = wt.identifier.as_deref().unwrap_or(&wt.branch);
        match git::remove_worktree(&wt.path) {
            Ok(()) => {
                eprintln!("Removed worktree: {id}");
                if let Err(e) = git::delete_branch(&wt.branch) {
                    eprintln!("Warning: {e}");
                }
            }
            Err(e) => eprintln!("Warning: Failed to remove {id}: {e}"),
        }
    }
    println!("Done.");
}
