use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config;

#[derive(Debug, Deserialize, Serialize)]
pub struct DevState {
    pub pid: u32,
    pub worktree: PathBuf,
    pub command: String,
    pub started_at: i64,
}

fn pid_dir() -> PathBuf {
    let dir = config::config_dir().join("dev");
    let _ = fs::create_dir_all(&dir);
    dir
}

fn pid_file(slot: &str) -> PathBuf {
    pid_dir().join(format!("{slot}.pid"))
}

pub fn read_state(slot: &str) -> Option<DevState> {
    let content = fs::read_to_string(pid_file(slot)).ok()?;
    toml::from_str(&content).ok()
}

fn write_state(slot: &str, state: &DevState) -> Result<(), String> {
    let path = pid_file(slot);
    let content = toml::to_string_pretty(state)
        .map_err(|e| format!("Failed to serialize dev state: {e}"))?;
    fs::write(&path, content).map_err(|e| format!("Failed to write {}: {e}", path.display()))
}

fn clear_state(slot: &str) {
    let _ = fs::remove_file(pid_file(slot));
}

/// All slots with on-disk state, alive or not.
fn list_slots() -> Vec<String> {
    let Ok(entries) = fs::read_dir(pid_dir()) else { return Vec::new() };
    let mut slots: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            let stem = path.file_stem()?.to_str()?.to_string();
            if path.extension().and_then(|s| s.to_str()) == Some("pid") {
                Some(stem)
            } else {
                None
            }
        })
        .collect();
    slots.sort();
    slots
}

fn is_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Kill an existing dev server in the given slot (if any). Returns true if one was killed.
pub fn kill_slot(slot: &str) -> bool {
    let Some(state) = read_state(slot) else { return false };

    if !is_alive(state.pid) {
        clear_state(slot);
        return false;
    }

    eprintln!(
        "Stopping previous '{slot}' dev server (pid {}, {})…",
        state.pid,
        state.worktree.display()
    );

    let _ = Command::new("kill")
        .args(["-TERM", &format!("-{}", state.pid)])
        .status();

    for _ in 0..30 {
        if !is_alive(state.pid) {
            clear_state(slot);
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let _ = Command::new("kill")
        .args(["-KILL", &format!("-{}", state.pid)])
        .status();
    let _ = Command::new("kill")
        .args(["-KILL", &state.pid.to_string()])
        .status();

    clear_state(slot);
    true
}

/// Kill every running dev slot. Returns the number actually killed.
pub fn kill_all() -> usize {
    let mut killed = 0;
    for slot in list_slots() {
        if kill_slot(&slot) {
            killed += 1;
        }
    }
    killed
}

/// Spawn the dev server in the foreground, recording its PID under the given slot.
/// Blocks until exit.
pub fn spawn_foreground(slot: &str, worktree: &Path, command: &str) -> Result<(), String> {
    use std::os::unix::process::CommandExt;

    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(worktree)
        // Own process group → kill the whole tree (next dev + workers) cleanly
        .process_group(0)
        .spawn()
        .map_err(|e| format!("Failed to spawn dev server: {e}"))?;

    let pid = child.id();
    let started_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    if let Err(e) = write_state(
        slot,
        &DevState {
            pid,
            worktree: worktree.to_path_buf(),
            command: command.to_string(),
            started_at,
        },
    ) {
        eprintln!("Warning: {e}");
    }

    eprintln!("▶ Dev server '{slot}' (pid {pid})");
    eprintln!("  cwd:     {}", worktree.display());
    eprintln!("  command: {command}");
    eprintln!("  stop:    cl dev --stop {slot}  (or Ctrl-C here)");
    eprintln!();

    // Forward Ctrl-C from the controlling terminal to the child group.
    // sh's process group differs from cl's, so the kernel won't auto-deliver SIGINT.
    let child_pid = pid;
    ctrlc_forward(child_pid);

    let status = child.wait().map_err(|e| format!("Wait failed: {e}"))?;
    clear_state(slot);

    if !status.success() {
        if let Some(code) = status.code() {
            // Suppress noisy "exit 130" (SIGINT, expected on Ctrl-C)
            if code != 130 {
                eprintln!("Dev server '{slot}' exited with code {code}");
            }
        }
    }
    Ok(())
}

/// Install a SIGINT/SIGTERM handler that forwards to the child process group.
/// Uses libc directly to avoid pulling in extra deps.
fn ctrlc_forward(child_pid: u32) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static CHILD_PID: AtomicU32 = AtomicU32::new(0);
    CHILD_PID.store(child_pid, Ordering::SeqCst);

    extern "C" fn handler(_: i32) {
        let pid = CHILD_PID.load(Ordering::SeqCst);
        if pid > 0 {
            unsafe {
                // Send SIGTERM to the negative pid = process group
                libc_kill(-(pid as i32), 15);
            }
        }
    }

    unsafe {
        libc_signal(2, handler as *const () as usize); // SIGINT
        libc_signal(15, handler as *const () as usize); // SIGTERM
    }
}

// Minimal libc bindings to avoid adding the `libc` crate.
unsafe extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
    #[link_name = "signal"]
    fn libc_signal(signum: i32, handler: usize) -> usize;
}

/// Print every running dev slot's state.
pub fn print_status() {
    let slots = list_slots();
    let mut any_running = false;
    for slot in &slots {
        let Some(state) = read_state(slot) else { continue };
        if !is_alive(state.pid) {
            clear_state(slot);
            continue;
        }
        any_running = true;
        let ago = crate::git::time_ago(state.started_at);
        println!("▶ {slot}  (pid {}, {ago})", state.pid);
        println!("  cwd:     {}", state.worktree.display());
        println!("  command: {}", state.command);
    }
    if !any_running {
        println!("No dev servers running.");
    }
}
