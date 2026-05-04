use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

use serde::{Deserialize, Serialize};

// --- Config Types ---

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct ClConfig {
    pub workspace: Option<WorkspaceConfig>,
    pub claude: Option<ClaudeConfig>,
    pub dev: Option<DevConfig>,
    pub worktree: Option<WorktreeConfig>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct DevConfig {
    /// Name of the default command to run when none is specified
    pub default: Option<String>,
    /// Named dev commands keyed by short name (e.g. "rebus" → "yarn turbo run dev --filter=...")
    pub commands: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct WorktreeConfig {
    /// Paths (relative to main repo root) to symlink into every worktree.
    /// Use for personal, globally-gitignored scratch dirs like `.notes` that
    /// should be shared across worktrees rather than duplicated.
    pub symlinks: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct WorkspaceConfig {
    pub team: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct ClaudeConfig {
    /// Whether to pass --allow-dangerously-skip-permissions (default: false)
    pub skip_permissions: Option<bool>,
    /// Enable Chrome integration by default
    pub chrome: Option<bool>,
    /// Default mode: "worktree", "branch", or "raw"
    pub mode: Option<String>,
}

impl ClConfig {
    /// Merge two configs: `other` overrides `self` field-by-field
    pub fn merge(self, other: ClConfig) -> ClConfig {
        let workspace = match (self.workspace, other.workspace) {
            (Some(a), Some(b)) => Some(WorkspaceConfig {
                team: b.team.or(a.team),
            }),
            (a, b) => b.or(a),
        };
        let claude = match (self.claude, other.claude) {
            (Some(a), Some(b)) => Some(ClaudeConfig {
                skip_permissions: b.skip_permissions.or(a.skip_permissions),
                chrome: b.chrome.or(a.chrome),
                mode: b.mode.or(a.mode),
            }),
            (a, b) => b.or(a),
        };
        let dev = match (self.dev, other.dev) {
            (Some(a), Some(b)) => {
                let commands = match (a.commands, b.commands) {
                    (Some(mut a_cmds), Some(b_cmds)) => {
                        a_cmds.extend(b_cmds);
                        Some(a_cmds)
                    }
                    (a, b) => b.or(a),
                };
                Some(DevConfig {
                    default: b.default.or(a.default),
                    commands,
                })
            }
            (a, b) => b.or(a),
        };
        // Symlink lists union across layers — a project can extend the global
        // list rather than replacing it.
        let worktree = match (self.worktree, other.worktree) {
            (Some(a), Some(b)) => {
                let symlinks = match (a.symlinks, b.symlinks) {
                    (Some(mut a_links), Some(b_links)) => {
                        for link in b_links {
                            if !a_links.contains(&link) {
                                a_links.push(link);
                            }
                        }
                        Some(a_links)
                    }
                    (a, b) => b.or(a),
                };
                Some(WorktreeConfig { symlinks })
            }
            (a, b) => b.or(a),
        };
        ClConfig { workspace, claude, dev, worktree }
    }

    pub fn dev_command(&self, name: Option<&str>) -> Option<String> {
        let dev = self.dev.as_ref()?;
        let commands = dev.commands.as_ref()?;
        let key = name.or(dev.default.as_deref())?;
        commands.get(key).cloned()
    }

    pub fn dev_default_name(&self) -> Option<&str> {
        self.dev.as_ref()?.default.as_deref()
    }

    pub fn dev_command_names(&self) -> Vec<String> {
        self.dev
            .as_ref()
            .and_then(|d| d.commands.as_ref())
            .map(|c| c.keys().cloned().collect())
            .unwrap_or_default()
    }

    pub fn team(&self) -> Option<&str> {
        self.workspace.as_ref().and_then(|w| w.team.as_deref())
    }

    pub fn skip_permissions(&self) -> bool {
        self.claude
            .as_ref()
            .and_then(|c| c.skip_permissions)
            .unwrap_or(false)
    }

    pub fn chrome(&self) -> bool {
        self.claude
            .as_ref()
            .and_then(|c| c.chrome)
            .unwrap_or(false)
    }

    pub fn mode(&self) -> &str {
        self.claude
            .as_ref()
            .and_then(|c| c.mode.as_deref())
            .unwrap_or("worktree")
    }

    pub fn worktree_symlinks(&self) -> &[String] {
        self.worktree
            .as_ref()
            .and_then(|w| w.symlinks.as_deref())
            .unwrap_or(&[])
    }
}

// --- Config Loading ---

pub fn load_config() -> ClConfig {
    let global = load_global_config();
    let workspace = load_workspace_config();
    let local = load_local_project_config();
    global.merge(workspace).merge(local)
}

fn load_global_config() -> ClConfig {
    let path = config_dir().join("config.toml");
    load_config_file(&path)
}

fn load_workspace_config() -> ClConfig {
    match find_workspace_config() {
        Some(path) => load_config_file(&path),
        None => ClConfig::default(),
    }
}

fn load_local_project_config() -> ClConfig {
    load_config_file(&local_project_config_path())
}

/// User-local project config path: ~/.config/cl/projects/<path-hash>/config.toml
/// This avoids polluting the repo with config files.
///
/// Uses the **main worktree** root so the same config is shared across all
/// linked worktrees of the same repository (resolved via `git rev-parse
/// --git-common-dir`, falling back to `--show-toplevel` outside a repo).
fn local_project_config_path() -> PathBuf {
    let root = main_repo_root().unwrap_or_else(|| {
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string()
    });

    // Use the path itself as directory name (replace / with -)
    let safe_name = root.replace('/', "-");
    config_dir()
        .join("projects")
        .join(safe_name)
        .join("config.toml")
}

/// Resolve the main worktree's repo root. From a linked worktree this returns
/// the main worktree path, not the linked one — so per-project config is shared.
pub fn main_repo_root() -> Option<String> {
    let common_dir = std::process::Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())?;

    let common_path = PathBuf::from(&common_dir);
    let abs = common_path.canonicalize().ok()?;
    // .git → repo root is parent
    let root = abs.parent()?.to_path_buf();
    Some(root.to_string_lossy().to_string())
}

fn load_config_file(path: &Path) -> ClConfig {
    match fs::read_to_string(path) {
        Ok(content) => toml::from_str(&content).unwrap_or_else(|e| {
            eprintln!("Warning: Failed to parse {}: {e}", path.display());
            ClConfig::default()
        }),
        Err(_) => ClConfig::default(),
    }
}

fn find_workspace_config() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let candidate = dir.join(".cl").join("config.toml");
        if candidate.exists() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Save team to user-local project config (~/.config/cl/projects/<path>/config.toml).
pub fn save_workspace_team(team: &str) {
    let config_path = local_project_config_path();
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent).ok();
    }

    let mut config = load_config_file(&config_path);
    let workspace = config.workspace.get_or_insert_with(WorkspaceConfig::default);
    workspace.team = Some(team.to_uppercase());

    let content = toml::to_string_pretty(&config).unwrap_or_default();
    fs::write(&config_path, &content).unwrap_or_else(|e| {
        eprintln!("Warning: Could not save config to {}: {e}", config_path.display());
    });
    eprintln!("Saved team '{}' to {}", team.to_uppercase(), config_path.display());
}

pub fn print_config(config: &ClConfig) {
    println!(
        "{}",
        toml::to_string_pretty(config).unwrap_or_else(|_| "Failed to serialize config".into())
    );
}

// --- Token Storage ---

pub fn config_dir() -> PathBuf {
    let dir = home_dir().join(".config").join("cl");
    fs::create_dir_all(&dir).ok();
    dir
}

pub fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .expect("HOME not set")
}

pub fn save_token(key: &str) {
    let path = config_dir().join("linear_token");
    fs::write(&path, key.trim()).unwrap_or_else(|e| {
        eprintln!("Error writing token to {}: {e}", path.display());
        process::exit(1);
    });
    println!("Token saved to {}", path.display());
}

pub fn load_token() -> String {
    if let Ok(token) = fs::read_to_string(config_dir().join("linear_token")) {
        let token = token.trim().to_string();
        if !token.is_empty() {
            return token;
        }
    }
    if let Ok(token) = std::env::var("LINEAR_API_KEY") {
        return token;
    }
    eprintln!("Error: No Linear API key found. Run `cl login <key>` or set LINEAR_API_KEY.");
    process::exit(1);
}
