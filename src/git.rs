use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct WorktreeInfo {
    pub path: PathBuf,
    pub branch: String,
    pub identifier: Option<String>,
    pub last_activity: i64,
}

impl WorktreeInfo {
    pub fn display_name(&self) -> &str {
        self.identifier.as_deref().unwrap_or(&self.branch)
    }

    /// Match this worktree against a user-supplied query: identifier (KL-35),
    /// bare number (35), or exact branch name. Identifiers match case-insensitively.
    pub fn matches(&self, query: &str) -> bool {
        if query.is_empty() {
            return false;
        }
        let upper = query.to_uppercase();
        if self.identifier.as_deref().is_some_and(|s| s.eq_ignore_ascii_case(&upper)) {
            return true;
        }
        let is_bare_number = query.chars().all(|c| c.is_ascii_digit());
        if is_bare_number
            && self
                .identifier
                .as_deref()
                .and_then(|s| s.rsplit_once('-'))
                .is_some_and(|(_, n)| n == query)
        {
            return true;
        }
        self.branch == query
    }
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

/// Extract TEAM-123 identifier from a branch name. Matches case-insensitively
/// and normalizes the team prefix to uppercase, so both
/// "user/KL-35-some-slug" and "feature/ws-113-some-slug" → "KL-35" / "WS-113".
pub fn extract_identifier(branch: &str) -> Option<String> {
    let bytes = branch.as_bytes();
    for i in 0..bytes.len() {
        if i > 0 && bytes[i - 1].is_ascii_alphabetic() {
            continue;
        }
        let start = i;
        let mut j = i;
        while j < bytes.len() && bytes[j].is_ascii_alphabetic() {
            j += 1;
        }
        if j == start || j >= bytes.len() || bytes[j] != b'-' {
            continue;
        }
        let prefix_end = j;
        j += 1;
        let digit_start = j;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if j > digit_start {
            let prefix = branch[start..prefix_end].to_ascii_uppercase();
            let number = &branch[digit_start..j];
            return Some(format!("{prefix}-{number}"));
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

    eprintln!("  Base: {current} (override with --base)");
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

pub fn create_worktree(
    branch: &str,
    user_symlinks: &[String],
    setup: Option<&str>,
) -> Result<PathBuf, String> {
    let root = main_repo_root()?;
    let worktree_dir = root.parent().unwrap_or(&root).join(".cl-worktrees");
    std::fs::create_dir_all(&worktree_dir)
        .map_err(|e| format!("Failed to create worktree dir: {e}"))?;

    let worktree_path = worktree_dir.join(branch);
    let path_str = worktree_path
        .to_str()
        .ok_or("Worktree path is not valid UTF-8")?;

    if worktree_path.exists() {
        // Idempotent — keep existing worktree but still sync symlinks in case
        // the user added new entries to [worktree] symlinks since it was made.
        sync_worktree_symlinks(&worktree_path, &root, user_symlinks);
        return Ok(worktree_path);
    }

    // Try creating worktree with a new branch first, fall back to existing branch
    let output = Command::new("git")
        .args(["worktree", "add", path_str, "-b", branch])
        .output()
        .map_err(|e| format!("Failed to create worktree: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);

        if stderr.contains("already exists") {
            // Branch exists — use it directly instead of creating
            let retry = Command::new("git")
                .args(["worktree", "add", path_str, branch])
                .output()
                .map_err(|e| format!("Failed to create worktree: {e}"))?;

            if !retry.status.success() {
                let retry_err = String::from_utf8_lossy(&retry.stderr);
                try_no_checkout(path_str, branch, &retry_err)?;
            }
        } else if stderr.contains("smudge filter") || stderr.contains("git-crypt") {
            try_no_checkout(path_str, branch, &stderr)?;
        } else {
            return Err(format!(
                "git worktree add failed for branch {branch}: {stderr}"
            ));
        }
    }

    sync_worktree_symlinks(&worktree_path, &root, user_symlinks);

    if let Some(cmd) = setup {
        run_setup(&worktree_path, cmd)?;
    }

    Ok(worktree_path)
}

/// Run a one-time setup command (e.g. `yarn install`) inside a freshly created
/// worktree. Streams stdout/stderr so the user sees progress; fails the whole
/// `create_worktree` call if the command exits non-zero, since downstream tools
/// (dev servers, claude) will likely break without dependencies installed.
fn run_setup(worktree_path: &Path, cmd: &str) -> Result<(), String> {
    eprintln!("  Setup: {cmd}");
    let status = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(worktree_path)
        .status()
        .map_err(|e| format!("Failed to run setup command: {e}"))?;
    if !status.success() {
        return Err(format!(
            "Worktree setup command failed (exit {}): {cmd}",
            status.code().map(|c| c.to_string()).unwrap_or_else(|| "?".into())
        ));
    }
    Ok(())
}

/// Resolve the main worktree's repo root, even when called from a linked worktree.
/// Falls back to the current worktree's root if `--git-common-dir` isn't usable.
pub fn main_repo_root() -> Result<PathBuf, String> {
    let output = Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .output()
        .map_err(|e| format!("Failed to run git: {e}"))?;
    if !output.status.success() {
        return repo_root();
    }
    let common = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    let abs = common
        .canonicalize()
        .map_err(|e| format!("Failed to canonicalize git-common-dir: {e}"))?;
    // git-common-dir points at .git → repo root is its parent
    abs.parent()
        .map(PathBuf::from)
        .ok_or_else(|| "git-common-dir has no parent".into())
}

fn try_no_checkout(path_str: &str, branch: &str, stderr: &str) -> Result<(), String> {
    if !stderr.contains("smudge filter") && !stderr.contains("git-crypt") {
        return Err(format!("git worktree add failed for branch {branch}: {stderr}"));
    }

    let devnull = std::process::Stdio::null();
    let _ = Command::new("git")
        .args(["worktree", "remove", "--force", path_str])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    let status = Command::new("git")
        .args(["worktree", "add", "--no-checkout", path_str, branch])
        .stderr(devnull)
        .status()
        .map_err(|e| format!("Failed to create worktree: {e}"))?;

    if !status.success() {
        return Err(format!("git worktree add --no-checkout failed for branch {branch}"));
    }

    // git-crypt looks for keys at <git-dir>/git-crypt/keys/ but in a linked
    // worktree git-dir is .git/worktrees/<name>/, where no key dir exists.
    // Symlinking ../../git-crypt resolves to the common-dir's git-crypt/, where
    // the key actually lives — so smudge filters work normally during checkout.
    if let Err(e) = link_git_crypt_keys(path_str) {
        eprintln!("Warning: Could not link git-crypt keys for worktree: {e}");
    }

    let status = Command::new("git")
        .args(["checkout", "HEAD", "--", "."])
        .current_dir(path_str)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("Failed to checkout in worktree: {e}"))?;
    if !status.success() {
        return Err(format!("git checkout failed in worktree {branch}"));
    }

    Ok(())
}

/// Symlink the worktree's per-worktree git-dir/git-crypt entry to the common-dir's
/// git-crypt directory, so git-crypt finds the decryption key when smudge runs.
fn link_git_crypt_keys(worktree_path: &str) -> Result<(), String> {
    let git_dir_out = Command::new("git")
        .args(["-C", worktree_path, "rev-parse", "--git-dir"])
        .output()
        .map_err(|e| format!("git rev-parse: {e}"))?;
    if !git_dir_out.status.success() {
        return Err("could not resolve worktree git-dir".into());
    }
    let git_dir = PathBuf::from(String::from_utf8_lossy(&git_dir_out.stdout).trim());

    let target = git_dir.join("git-crypt");
    if target.exists() || target.is_symlink() {
        return Ok(());
    }

    // Verify the common-dir git-crypt/ exists before linking — no point creating
    // a dangling symlink if the repo isn't actually using git-crypt.
    let common_dir_out = Command::new("git")
        .args(["-C", worktree_path, "rev-parse", "--git-common-dir"])
        .output()
        .map_err(|e| format!("git rev-parse common-dir: {e}"))?;
    if !common_dir_out.status.success() {
        return Err("could not resolve common-dir".into());
    }
    let common_dir = PathBuf::from(String::from_utf8_lossy(&common_dir_out.stdout).trim());
    let common_abs = common_dir.canonicalize().map_err(|e| format!("canonicalize: {e}"))?;
    if !common_abs.join("git-crypt").is_dir() {
        return Err("no git-crypt key directory in common dir (repo not git-crypted?)".into());
    }

    // ../../git-crypt resolves from .git/worktrees/<name>/ → .git/git-crypt
    std::os::unix::fs::symlink("../../git-crypt", &target)
        .map_err(|e| format!("symlink failed: {e}"))?;
    Ok(())
}

/// Sync all auto-symlinks (env files + user-configured paths) into a worktree.
/// Idempotent — safe to call repeatedly. Existing files/symlinks are left alone.
pub fn sync_worktree_symlinks(worktree_path: &Path, repo_root: &Path, user_paths: &[String]) {
    symlink_env_files(worktree_path, repo_root);
    symlink_user_paths(worktree_path, repo_root, user_paths);
}

/// Symlink user-configured paths from main repo into the worktree.
/// Skips paths that don't exist in main, or already exist (file or symlink) in the worktree.
fn symlink_user_paths(worktree_path: &Path, repo_root: &Path, paths: &[String]) {
    for rel in paths {
        let rel = rel.trim_start_matches('/').trim();
        if rel.is_empty() || rel.contains("..") {
            continue;
        }
        let src = repo_root.join(rel);
        let dst = worktree_path.join(rel);
        if !src.exists() {
            continue;
        }
        // is_symlink catches dangling links that exists() reports as false
        if dst.exists() || dst.is_symlink() {
            continue;
        }
        if let Some(parent) = dst.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::os::unix::fs::symlink(&src, &dst) {
            eprintln!("Warning: Failed to symlink {rel}: {e}");
        }
    }
}

fn symlink_env_files(worktree_path: &Path, repo_root: &Path) {
    // Find every .env.local in the main repo (commonly gitignored personal secrets
    // that wouldn't otherwise propagate to a fresh worktree). git-crypted .env
    // files are handled separately by linking the git-crypt key directory.
    let Ok(output) = Command::new("git")
        .args([
            "-C",
            repo_root.to_str().unwrap_or("."),
            "ls-files",
            "--others",
            "--exclude-standard",
            "--cached",
            "*.env.local",
            ".env.local",
        ])
        .output()
    else {
        return;
    };
    if !output.status.success() {
        return;
    }

    let mut paths: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|s| s.to_string())
        .collect();
    // Always include the top-level .env.local even if not picked up
    if !paths.iter().any(|p| p == ".env.local") {
        paths.push(".env.local".to_string());
    }

    for rel in paths {
        let src = repo_root.join(&rel);
        let dst = worktree_path.join(&rel);
        if src.exists() && !dst.exists() {
            if let Some(parent) = dst.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::os::unix::fs::symlink(&src, &dst) {
                eprintln!("Warning: Failed to symlink {rel}: {e}");
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
    // Filter against the actual main worktree, not the cwd's worktree —
    // otherwise calling from a linked worktree drops *that* worktree from the
    // list and includes the main repo as a fake entry.
    let main_root = main_repo_root()?;
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

pub fn force_remove_worktree(path: &Path) -> Result<(), String> {
    let status = Command::new("git")
        .args(["worktree", "remove", "--force", path.to_str().unwrap_or("")])
        .status()
        .map_err(|e| format!("Failed to force-remove worktree: {e}"))?;
    if !status.success() {
        return Err(format!(
            "git worktree remove --force failed for {}",
            path.display()
        ));
    }
    Ok(())
}

/// Lines from `git -C <path> status --porcelain`. Empty if clean or unreadable.
pub fn worktree_dirty_files(path: &Path) -> Vec<String> {
    let Ok(output) = Command::new("git")
        .args(["-C", path.to_str().unwrap_or("."), "status", "--porcelain"])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|s| s.to_string())
        .collect()
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

pub fn gh_pr_field(branch: &str, field: &str) -> Option<String> {
    let query = format!(".{field}");
    Command::new("gh")
        .args(["pr", "view", branch, "--json", field, "-q", &query])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
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
