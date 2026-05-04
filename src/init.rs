//! Shell integration: `cl init <shell>` emits a self-contained script users
//! source from their shell config (e.g. `cl init fish | source`).
//!
//! Three layers per shell:
//! 1. **Static completions** — generated from clap, so subcommand/flag changes
//!    flow through automatically on rebuild + shell reload.
//! 2. **`cd` wrapper** — a shell function that intercepts `cl cd <issue>` and
//!    runs the parent shell's `cd` against the path the binary prints, since a
//!    subprocess can't change its parent's CWD.
//! 3. **Dynamic completions** — call back into `cl _complete <kind>` for
//!    runtime data (e.g. live worktree identifiers). Wired up per-shell.
//!
//! When adding a subcommand that takes an issue/worktree argument, append it
//! to `WORKTREE_SUBCOMMANDS` so the dynamic completer offers identifiers there.

use clap::CommandFactory;
use clap_complete::{Shell, generate};

use crate::Cli;

/// Subcommands whose first positional argument is an issue/worktree query.
/// The dynamic completer offers live worktree identifiers in this position.
const WORKTREE_SUBCOMMANDS: &[&str] = &["cd", "dev", "sync"];

pub fn render(shell: Shell) -> String {
    let mut out = String::new();
    out.push_str(prelude(shell));
    out.push_str(&clap_static(shell));
    out.push('\n');
    out.push_str(&extras(shell));
    out
}

/// Reset any prior completions so re-sourcing the script doesn't stack
/// duplicate registrations.
fn prelude(shell: Shell) -> &'static str {
    match shell {
        Shell::Fish => "complete -c cl -e\n",
        _ => "",
    }
}

fn clap_static(shell: Shell) -> String {
    let mut buf = Vec::new();
    generate(shell, &mut Cli::command(), "cl", &mut buf);
    String::from_utf8(buf).unwrap_or_default()
}

fn extras(shell: Shell) -> String {
    match shell {
        Shell::Fish => fish_extras(),
        Shell::Bash | Shell::Zsh => posix_wrapper().to_string(),
        // Elvish / PowerShell get static completions only; no cd wrapper.
        _ => String::new(),
    }
}

fn fish_extras() -> String {
    let predicate = format!(
        "__fish_seen_subcommand_from {}",
        WORKTREE_SUBCOMMANDS.join(" ")
    );
    // Two completion rules:
    //   1. An empty `-f` rule that declares "no files" for the worktree-arg
    //      position. Needed because fish's `--no-files` on a dynamic-source
    //      rule only suppresses file fallback when the dynamic returns at
    //      least one candidate — empty output (e.g. running outside a repo)
    //      otherwise lets the file fallback show through.
    //   2. The dynamic candidate source itself.
    format!(
        r#"# cl shell integration: cd wrapper + dynamic worktree completions
function cl
    if test (count $argv) -ge 1; and test "$argv[1]" = cd
        set -l target (command cl cd $argv[2..-1])
        or return $status
        cd $target
    else
        command cl $argv
    end
end

complete -c cl -f -n '{predicate}'
complete -c cl -f -n '{predicate}' -a '(command cl _complete worktrees)'
"#
    )
}

fn posix_wrapper() -> &'static str {
    // Bash and zsh share an identical wrapper. Static completions come from
    // clap; dynamic per-arg completion isn't wired here (would require
    // overriding clap's generated completer per shell — fish handles this
    // cleanly because its completion model is additive).
    r#"# cl shell integration: cd wrapper
cl() {
    if [ "${1:-}" = "cd" ]; then
        shift
        local target
        target="$(command cl cd "$@")" || return $?
        cd "$target"
    else
        command cl "$@"
    fi
}
"#
}
