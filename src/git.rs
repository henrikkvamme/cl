use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct WorktreeInfo {
    pub path: PathBuf,
    pub branch: String,
    pub identifier: Option<String>,
    pub last_activity: i64,
}

pub fn current_branch() -> Result<String, String> {
    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .map_err(|e| format!("Failed to run git: {e}"))?;
    if !output.status.success() {
        return Err("Failed to get current branch".into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn default_branch() -> String {
    Command::new("git")
        .args(["config", "init.defaultBranch"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if s.is_empty() { None } else { Some(s) }
        })
        .unwrap_or_else(|| "main".to_string())
}

pub fn is_issue_branch(branch: &str) -> bool {
    extract_identifier(branch).is_some()
}

/// Extract TEAM-123 identifier from a branch name (e.g. "user/KL-35-some-slug" → "KL-35")
pub fn extract_identifier(branch: &str) -> Option<String> {
    let bytes = branch.as_bytes();
    for i in 0..bytes.len() {
        if i > 0 && bytes[i - 1].is_ascii_alphabetic() {
            continue;
        }
        let start = i;
        let mut j = i;
        while j < bytes.len() && bytes[j].is_ascii_uppercase() {
            j += 1;
        }
        if j == start || j >= bytes.len() || bytes[j] != b'-' {
            continue;
        }
        j += 1;
        let digit_start = j;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if j > digit_start {
            return Some(branch[start..j].to_string());
        }
    }
    None
}

pub fn resolve_base_branch(explicit: Option<&str>) -> Result<String, String> {
    if let Some(base) = explicit {
        return Ok(base.to_string());
    }

    let current = current_branch()?;
    let default = default_branch();

    if current == default {
        return Ok(default);
    }

    if is_issue_branch(&current) {
        return Ok(default);
    }

    eprintln!("Using current branch '{current}' as base (use --base to override)");
    Ok(current)
}

pub fn ensure_clean_tree() -> Result<(), String> {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .map_err(|e| format!("Failed to run git: {e}"))?;
    if !output.stdout.is_empty() {
        return Err("Working tree is not clean. Commit or stash changes first.".into());
    }
    Ok(())
}

pub fn create_branch(name: &str) -> Result<(), String> {
    let status = Command::new("git")
        .args(["checkout", "-b", name])
        .status()
        .map_err(|e| format!("Failed to run git checkout: {e}"))?;
    if !status.success() {
        return Err(format!("git checkout -b {name} failed"));
    }
    Ok(())
}

pub fn repo_root() -> Result<PathBuf, String> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|e| format!("Failed to run git: {e}"))?;
    if !output.status.success() {
        return Err("Not in a git repository".into());
    }
    Ok(PathBuf::from(
        String::from_utf8_lossy(&output.stdout).trim(),
    ))
}

pub fn create_worktree(branch: &str) -> Result<PathBuf, String> {
    let root = repo_root()?;
    let worktree_dir = root.parent().unwrap_or(&root).join(".cl-worktrees");
    std::fs::create_dir_all(&worktree_dir).map_err(|e| format!("Failed to create worktree dir: {e}"))?;

    let worktree_path = worktree_dir.join(branch);

    // Check if worktree already exists
    if worktree_path.exists() {
        return Ok(worktree_path);
    }

    let status = Command::new("git")
        .args([
            "worktree",
            "add",
            worktree_path.to_str().unwrap(),
            "-b",
            branch,
        ])
        .status()
        .map_err(|e| format!("Failed to create worktree: {e}"))?;
    if !status.success() {
        return Err(format!("git worktree add failed for branch {branch}"));
    }

    // Symlink .env files from repo root
    symlink_env_files(&worktree_path, &root);

    Ok(worktree_path)
}

fn symlink_env_files(worktree_path: &Path, repo_root: &Path) {
    for name in [".env", ".env.local"] {
        let src = repo_root.join(name);
        let dst = worktree_path.join(name);
        if src.exists() && !dst.exists() {
            if let Err(e) = std::os::unix::fs::symlink(&src, &dst) {
                eprintln!("Warning: Failed to symlink {name}: {e}");
            }
        }
    }
}

pub fn list_worktrees() -> Result<Vec<WorktreeInfo>, String> {
    let output = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .output()
        .map_err(|e| format!("Failed to run git worktree list: {e}"))?;
    if !output.status.success() {
        return Err("git worktree list failed".into());
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let main_root = repo_root()?;
    let mut worktrees = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_branch: Option<String> = None;

    for line in text.lines() {
        if let Some(path_str) = line.strip_prefix("worktree ") {
            current_path = Some(PathBuf::from(path_str));
            current_branch = None;
        } else if let Some(branch_ref) = line.strip_prefix("branch ") {
            // branch refs/heads/feature-branch → feature-branch
            current_branch = Some(
                branch_ref
                    .strip_prefix("refs/heads/")
                    .unwrap_or(branch_ref)
                    .to_string(),
            );
        } else if line.is_empty() {
            if let (Some(path), Some(branch)) = (current_path.take(), current_branch.take()) {
                // Skip the main worktree
                if path == main_root {
                    continue;
                }
                let identifier = extract_identifier(&branch);
                let last_activity = get_last_activity(&path);
                worktrees.push(WorktreeInfo {
                    path,
                    branch,
                    identifier,
                    last_activity,
                });
            }
        }
    }

    // Sort by most recently active first
    worktrees.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));
    Ok(worktrees)
}

fn get_last_activity(worktree_path: &Path) -> i64 {
    // Try git log first
    if let Ok(output) = Command::new("git")
        .args(["-C", worktree_path.to_str().unwrap_or("."), "log", "-1", "--format=%ct"])
        .output()
    {
        if output.status.success() {
            let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if let Ok(ts) = s.parse::<i64>() {
                return ts;
            }
        }
    }
    // Fallback: directory mtime
    std::fs::metadata(worktree_path)
        .and_then(|m| m.modified())
        .map(|t| t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64)
        .unwrap_or(0)
}

pub fn remove_worktree(path: &Path) -> Result<(), String> {
    let status = Command::new("git")
        .args(["worktree", "remove", path.to_str().unwrap_or("")])
        .status()
        .map_err(|e| format!("Failed to remove worktree: {e}"))?;
    if !status.success() {
        return Err(format!("git worktree remove failed for {}", path.display()));
    }
    Ok(())
}

pub fn delete_branch(name: &str) -> Result<(), String> {
    let status = Command::new("git")
        .args(["branch", "-d", name])
        .status()
        .map_err(|e| format!("Failed to delete branch: {e}"))?;
    if !status.success() {
        return Err(format!("git branch -d {name} failed"));
    }
    Ok(())
}

pub fn time_ago(timestamp: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let diff = now - timestamp;
    if diff < 60 {
        "just now".to_string()
    } else if diff < 3600 {
        let mins = diff / 60;
        format!("{mins}m ago")
    } else if diff < 86400 {
        let hours = diff / 3600;
        format!("{hours}h ago")
    } else {
        let days = diff / 86400;
        format!("{days}d ago")
    }
}
