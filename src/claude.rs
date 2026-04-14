use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{self, Command};

use crate::config::ClConfig;
use crate::linear::IssueData;

pub fn ensure_chrome_running() {
    let check = Command::new("pgrep").args(["-x", "Google Chrome"]).output();
    let running = check.is_ok_and(|o| o.status.success());
    if !running {
        eprintln!("Launching Google Chrome...");
        let _ = Command::new("open").args(["-a", "Google Chrome"]).status();
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}

fn base_cmd(name: &str, cfg: &ClConfig) -> Command {
    let mut cmd = Command::new("claude");
    if cfg.skip_permissions() {
        cmd.arg("--allow-dangerously-skip-permissions");
    }
    cmd.args(["--name", name]);
    cmd
}

pub fn exec_claude(
    issue: &IssueData,
    prompt: &str,
    worktree_path: Option<&Path>,
    chrome: bool,
    cfg: &ClConfig,
) -> ! {
    let mut cmd = base_cmd(&issue.identifier, cfg);

    if chrome {
        cmd.arg("--chrome");
    }
    if let Some(path) = worktree_path {
        cmd.current_dir(path);
    }
    cmd.arg(prompt);

    let err = cmd.exec();
    eprintln!("Failed to exec claude: {err}");
    process::exit(1);
}

pub fn exec_claude_resume(name: &str, worktree_path: &Path, cfg: &ClConfig) -> ! {
    let mut cmd = base_cmd(name, cfg);
    cmd.arg("--continue");
    cmd.current_dir(worktree_path);

    let err = cmd.exec();
    eprintln!("Failed to exec claude: {err}");
    process::exit(1);
}
