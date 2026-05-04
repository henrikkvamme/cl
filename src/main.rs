mod claude;
mod config;
mod dev;
mod git;
mod init;
mod linear;
mod picker;
mod prompt;

use std::collections::HashMap;
use std::process;

use clap::{Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
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

#[derive(Clone, ValueEnum)]
enum CompleteKind {
    /// Worktree identifiers, bare numbers, and branch names
    Worktrees,
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
    /// Print the worktree path for an issue (use with `cd (cl cd 113)`)
    Cd {
        /// Issue identifier (e.g. WS-112), bare number (113), or branch name
        issue: String,
    },
    /// Print shell integration script (source via e.g. `cl init fish | source`)
    Init {
        /// Target shell
        #[arg(value_enum)]
        shell: Shell,
    },
    /// Internal: emit completion data for shell scripts (used by `cl init`)
    #[command(name = "_complete", hide = true)]
    Complete {
        #[arg(value_enum)]
        kind: CompleteKind,
    },
    /// Manage dev servers in worktrees (one per named command, runs in parallel)
    ///
    /// `cl dev <issue> [name]` starts (or restarts) the named slot in the issue's
    /// worktree. Different slots (e.g. `rebus`, `hasura`) run in parallel; the
    /// same slot started twice is restarted in the new worktree.
    /// `cl dev` with no args prints status for every running slot.
    /// `cl dev --stop` stops everything; `cl dev --stop <name>` stops just one.
    Dev {
        /// Issue identifier (e.g. WS-112) or branch name. Omit to print status.
        issue: Option<String>,
        /// Named command from `[dev.commands]` config. Falls back to the configured default.
        name: Option<String>,
        /// Stop dev servers. With no value, stops all; with a name, stops just that slot.
        #[arg(long, num_args = 0..=1, default_missing_value = "", value_name = "SLOT")]
        stop: Option<String>,
        /// Run a custom shell command instead of any configured one
        #[arg(long)]
        cmd: Option<String>,
    },
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
        Some(Commands::Cd { issue }) => {
            run_cd(&issue);
        }
        Some(Commands::Init { shell }) => {
            run_init(shell);
        }
        Some(Commands::Complete { kind }) => {
            run_complete(kind);
        }
        Some(Commands::Dev { issue, name, stop, cmd }) => {
            run_dev(issue, name, stop, cmd, &cfg);
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

    eprintln!("Fetching issues for {team}...");
    let (issues, viewer_id) = linear::fetch_team_issues(&api_key, &team).unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        process::exit(1);
    });

    // Build session titles from already-fetched issues (avoids extra API call)
    let session_titles: HashMap<String, String> = issues
        .iter()
        .map(|i| (i.identifier.clone(), i.title.clone()))
        .collect();

    let result =
        picker::show_unified_picker(worktrees, issues, &viewer_id, &session_titles)
            .unwrap_or_else(|e| {
                eprintln!("Error: {e}");
                process::exit(1);
            });

    match result {
        picker::PickerResult::ResumeSession(wt) => {
            let name = wt.display_name();
            sync_worktree(&wt.path, cfg);
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

    eprintln!("Fetching {identifier}...");
    let issue = linear::fetch_issue(&api_key, &identifier).unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        process::exit(1);
    });

    eprintln!();
    eprintln!("  {} — {}", issue.identifier, issue.title);
    eprintln!("  Branch: {}", issue.branch_name);

    // Check for existing worktrees for this issue
    if worktree {
        let all_worktrees = git::list_worktrees().unwrap_or_default();
        let existing: Vec<git::WorktreeInfo> = all_worktrees
            .into_iter()
            .filter(|w| {
                w.identifier.as_deref() == Some(&identifier)
                    || w.branch == issue.branch_name
                    || w.branch.starts_with(&format!("{}-", issue.branch_name))
            })
            .collect();

        if !existing.is_empty() {
            let count = existing.len();
            eprintln!(
                "  {} existing session{}",
                count,
                if count > 1 { "s" } else { "" }
            );
            eprintln!();

            // Build picker items: sessions + New + Fork
            let mut options: Vec<String> = existing
                .iter()
                .map(|wt| format!("▶ {}  ({})", wt.branch, git::time_ago(wt.last_activity)))
                .collect();
            options.push("New session".to_string());
            options.push("Fork session".to_string());

            let choice = inquire::Select::new("Session action:", options)
                .prompt()
                .unwrap_or_else(|e| {
                    eprintln!("Error: {e}");
                    process::exit(1);
                });

            if choice == "Fork session" {
                let mut n = 2;
                loop {
                    let fork_branch = format!("{}-{n}", issue.branch_name);
                    let taken = existing.iter().any(|w| w.branch == fork_branch);
                    if !taken {
                        linear::update_issue(&api_key, &issue);
                        let path = git::create_worktree(&fork_branch, cfg.worktree_symlinks())
                            .unwrap_or_else(|e| {
                                eprintln!("Error: {e}");
                                process::exit(1);
                            });
                        eprintln!("  Forked: {}", path.display());
                        eprintln!();
                        if chrome {
                            claude::ensure_chrome_running();
                        }
                        let prompt = prompt::build_prompt(&issue, chrome, base_branch);
                        claude::exec_claude(&issue, &prompt, Some(&path), chrome, cfg);
                    }
                    n += 1;
                }
            } else if choice == "New session" {
                // Fresh prompt in the first existing worktree
                let wt = &existing[0];
                sync_worktree(&wt.path, cfg);
                linear::update_issue(&api_key, &issue);
                eprintln!();
                if chrome {
                    claude::ensure_chrome_running();
                }
                let prompt = prompt::build_prompt(&issue, chrome, base_branch);
                claude::exec_claude(&issue, &prompt, Some(&wt.path), chrome, cfg);
            } else {
                // Selected a session — find which one
                let idx = existing
                    .iter()
                    .position(|wt| {
                        choice.contains(&wt.branch)
                    })
                    .unwrap_or(0);
                let wt = &existing[idx];
                sync_worktree(&wt.path, cfg);
                claude::exec_claude_resume(wt.display_name(), &wt.path, cfg);
            }
        }
    }

    linear::update_issue(&api_key, &issue);

    let worktree_path = if worktree {
        let path = git::create_worktree(&issue.branch_name, cfg.worktree_symlinks())
            .unwrap_or_else(|e| {
                eprintln!("Error: {e}");
                process::exit(1);
            });
        eprintln!("  Worktree: {}", path.display());
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
        None
    } else {
        None
    };
    eprintln!();

    if chrome {
        claude::ensure_chrome_running();
    }

    let prompt = prompt::build_prompt(&issue, chrome, base_branch);
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
            let url = git::gh_pr_field(&w.branch, "url")?;
            Some((w.branch.clone(), url))
        })
        .collect();

    println!("Active Sessions");
    println!("{}", "─".repeat(60));
    for wt in &worktrees {
        let id = wt.display_name();
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
    let statuses: HashMap<String, linear::IssueSummary> = if !ids.is_empty() {
        linear::fetch_issue_statuses(&api_key, &ids)
            .unwrap_or_default()
            .into_iter()
            .map(|s| (s.identifier.clone(), s))
            .collect()
    } else {
        HashMap::new()
    };

    // Check which branches have merged PRs
    let merged_branches: Vec<String> = worktrees
        .iter()
        .filter(|w| {
            git::gh_pr_field(&w.branch, "state")
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
                .is_some_and(|s| {
                    matches!(s.state_type.as_str(), "completed" | "cancelled")
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
        let id = wt.display_name();
        let status = wt
            .identifier
            .as_ref()
            .and_then(|i| statuses.get(i))
            .map(|s| s.state_name.as_str())
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
        let id = wt.display_name();
        match git::remove_worktree(&wt.path) {
            Ok(()) => {
                eprintln!("Removed worktree: {id}");
                if let Err(e) = git::delete_branch(&wt.branch) {
                    eprintln!("Warning: {e}");
                }
            }
            Err(_) => {
                let dirty = git::worktree_dirty_files(&wt.path);
                if dirty.is_empty() {
                    eprintln!("Warning: Failed to remove {id}");
                    continue;
                }

                eprintln!();
                eprintln!("⚠ {id} has uncommitted changes:");
                for line in dirty.iter().take(10) {
                    eprintln!("    {line}");
                }
                if dirty.len() > 10 {
                    eprintln!("    … and {} more", dirty.len() - 10);
                }

                let prompt =
                    format!("Force-remove {id} (will DISCARD these changes)?");
                let force = inquire::Confirm::new(&prompt)
                    .with_default(false)
                    .prompt()
                    .unwrap_or(false);

                if !force {
                    eprintln!("Skipped {id}");
                    continue;
                }

                match git::force_remove_worktree(&wt.path) {
                    Ok(()) => {
                        eprintln!("Force-removed worktree: {id}");
                        if let Err(e) = git::delete_branch(&wt.branch) {
                            eprintln!("Warning: {e}");
                        }
                    }
                    Err(e) => eprintln!("Error: Failed to force-remove {id}: {e}"),
                }
            }
        }
    }
    println!("Done.");
}

/// Run sync_worktree_symlinks against a worktree using the merged config.
/// Silent on success — only the env-files / user-paths code prints on failure.
fn sync_worktree(worktree_path: &std::path::Path, cfg: &config::ClConfig) {
    let Ok(root) = git::main_repo_root() else { return };
    git::sync_worktree_symlinks(worktree_path, &root, cfg.worktree_symlinks());
}

fn run_init(shell: Shell) {
    print!("{}", init::render(shell));
}

fn run_complete(kind: CompleteKind) {
    match kind {
        CompleteKind::Worktrees => {
            let worktrees = git::list_worktrees().unwrap_or_default();
            // Emit identifier, bare number, and branch name as separate
            // candidates so prefix completion finds the worktree regardless of
            // which form the user typed.
            for w in worktrees {
                if let Some(id) = &w.identifier {
                    println!("{id}");
                    if let Some((_, num)) = id.rsplit_once('-') {
                        println!("{num}");
                    }
                }
                println!("{}", w.branch);
            }
        }
    }
}

fn run_cd(issue: &str) {
    let worktrees = git::list_worktrees().unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        process::exit(1);
    });

    match worktrees.into_iter().find(|w| w.matches(issue)) {
        Some(w) => println!("{}", w.path.display()),
        None => {
            eprintln!("Error: No worktree found for '{issue}'.");
            process::exit(1);
        }
    }
}

fn run_dev(
    issue: Option<String>,
    name: Option<String>,
    stop: Option<String>,
    cmd: Option<String>,
    cfg: &config::ClConfig,
) {
    if let Some(target) = stop {
        if target.is_empty() {
            let n = dev::kill_all();
            if n == 0 {
                println!("No dev servers were running.");
            } else {
                println!("Stopped {n} dev server{}.", if n == 1 { "" } else { "s" });
            }
        } else if dev::kill_slot(&target) {
            println!("Stopped '{target}'.");
        } else {
            println!("No dev server named '{target}' was running.");
        }
        return;
    }

    let Some(issue) = issue else {
        dev::print_status();
        return;
    };

    // Resolve the worktree for this issue/branch
    let worktrees = git::list_worktrees().unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        process::exit(1);
    });

    let wt = match worktrees.into_iter().find(|w| w.matches(&issue)) {
        Some(w) => w,
        None => {
            eprintln!("Error: No worktree found for '{issue}'.");
            eprintln!("Run `cl status` to see active worktrees, or `cl <issue>` to create one.");
            process::exit(1);
        }
    };

    // Resolve the slot name and command. The slot identifies which "service"
    // this is (rebus, hasura, retail) — different slots run in parallel; the
    // same slot started again is restarted in the new worktree.
    let (slot, command) = if let Some(c) = cmd {
        let slot = name.unwrap_or_else(|| "_adhoc".to_string());
        (slot, c)
    } else {
        let resolved = name.or_else(|| cfg.dev_default_name().map(String::from));
        let Some(slot) = resolved else {
            eprintln!("Error: No dev command name given and no default configured.");
            let names = cfg.dev_command_names();
            if names.is_empty() {
                eprintln!();
                eprintln!("Add commands to ~/.config/cl/projects/<repo>/config.toml:");
                eprintln!();
                eprintln!("  [dev]");
                eprintln!("  default = \"app\"");
                eprintln!();
                eprintln!("  [dev.commands]");
                eprintln!("  app = \"yarn dev\"");
            } else {
                eprintln!("  Available commands: {}", names.join(", "));
                eprintln!("  Try: cl dev {issue} <name>");
            }
            process::exit(1);
        };
        match cfg.dev_command(Some(&slot)) {
            Some(c) => (slot, c),
            None => {
                let names = cfg.dev_command_names();
                eprintln!("Error: No command named '{slot}'.");
                if !names.is_empty() {
                    eprintln!("  Available: {}", names.join(", "));
                }
                process::exit(1);
            }
        }
    };

    // Restart only this slot — leave other running slots (e.g. hasura) alone.
    dev::kill_slot(&slot);

    if let Err(e) = dev::spawn_foreground(&slot, &wt.path, &command) {
        eprintln!("Error: {e}");
        process::exit(1);
    }
}
