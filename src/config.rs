use std::fs;
use std::path::PathBuf;
use std::process;

use serde::{Deserialize, Serialize};

// --- Config Types ---

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct ClConfig {
    pub workspace: Option<WorkspaceConfig>,
    pub claude: Option<ClaudeConfig>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct WorkspaceConfig {
    pub team: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct ClaudeConfig {
    /// Whether to pass --allow-dangerously-skip-permissions (default: false)
    pub skip_permissions: Option<bool>,
    /// Claude permission mode (default: "plan")
    pub permission_mode: Option<String>,
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
                permission_mode: b.permission_mode.or(a.permission_mode),
                chrome: b.chrome.or(a.chrome),
                mode: b.mode.or(a.mode),
            }),
            (a, b) => b.or(a),
        };
        ClConfig { workspace, claude }
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

    pub fn permission_mode(&self) -> &str {
        self.claude
            .as_ref()
            .and_then(|c| c.permission_mode.as_deref())
            .unwrap_or("plan")
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
}

// --- Config Loading ---

pub fn load_config() -> ClConfig {
    let global = load_global_config();
    let workspace = load_workspace_config();
    global.merge(workspace)
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

fn load_config_file(path: &PathBuf) -> ClConfig {
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

/// Save team to workspace config (.cl/config.toml in repo root).
/// Creates the file and directory if they don't exist.
/// Merges with existing config if present.
pub fn save_workspace_team(team: &str) {
    let dir = std::env::current_dir().expect("Could not get current directory");

    // Find git root to place .cl/ there
    let root = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim().to_string()))
        .unwrap_or(dir);

    let cl_dir = root.join(".cl");
    fs::create_dir_all(&cl_dir).ok();
    let config_path = cl_dir.join("config.toml");

    // Load existing config and merge
    let mut config = load_config_file(&config_path);
    let workspace = config.workspace.get_or_insert_with(WorkspaceConfig::default);
    workspace.team = Some(team.to_uppercase());

    let content = toml::to_string_pretty(&config).unwrap_or_default();
    fs::write(&config_path, content).unwrap_or_else(|e| {
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
