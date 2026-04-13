use std::os::unix::process::CommandExt;
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

pub fn exec_claude(
    issue: &IssueData,
    prompt: &str,
    worktree: bool,
    chrome: bool,
    cfg: &ClConfig,
) -> ! {
    let mut cmd = Command::new("claude");

    if cfg.skip_permissions() {
        cmd.arg("--allow-dangerously-skip-permissions");
    }
    cmd.args(["--name", &issue.identifier]);
    cmd.args(["--permission-mode", cfg.permission_mode()]);

    if chrome {
        cmd.arg("--chrome");
    }

    if worktree {
        cmd.args(["--worktree", &issue.branch_name]);
    }

    cmd.arg(prompt);

    let err = cmd.exec();
    eprintln!("Failed to exec claude: {err}");
    process::exit(1);
}
