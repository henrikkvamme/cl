use std::process::Command;

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
    // Match TEAM-123 pattern anywhere in branch name (e.g. "user/KL-35-some-slug")
    let bytes = branch.as_bytes();
    for i in 0..bytes.len() {
        if i > 0 && bytes[i - 1].is_ascii_alphabetic() {
            continue;
        }
        let mut j = i;
        while j < bytes.len() && bytes[j].is_ascii_uppercase() {
            j += 1;
        }
        if j == i || j >= bytes.len() || bytes[j] != b'-' {
            continue;
        }
        j += 1;
        let digit_start = j;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if j > digit_start {
            return true;
        }
    }
    false
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

    // On some other branch (e.g. "rebus", "v2") → use it as base
    // (The interactive picker for this will be added in Phase 5 with inquire)
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
