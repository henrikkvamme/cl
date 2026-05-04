# cl

Rust CLI bridging Linear issues to Claude Code worktrees.

## Shell integration

`cl init <shell>` (fish/bash/zsh/elvish/powershell) emits a self-contained
script users source from their shell config:

    cl init fish | source

It bundles three layers, all defined in `src/init.rs`:

1. **Static completions** — generated from the clap definition via
   `clap_complete`. Subcommand and flag changes flow through automatically.
2. **`cd` wrapper** — a shell function that intercepts `cl cd <issue>` and
   runs the parent shell's `cd` against the path the binary prints. Required
   because subprocesses can't change the parent shell's CWD.
3. **Dynamic completions** — call back into the hidden `cl _complete <kind>`
   subcommand for runtime data (live worktree identifiers). Wired up in fish
   only today; bash/zsh get static completions and the cd wrapper.

## Rule: when CLI surface changes, refresh shell integration

After editing subcommands, flags, or `src/init.rs`:

1. Reinstall: `cargo install --path .`
2. Tell the user to reload their shell (or open a new terminal) — the user's
   currently-open shell is running stale completions until `cl init` re-runs.
   New shells pick up changes automatically since the source line evaluates
   on every shell startup.
3. **If a new subcommand takes an issue/worktree argument**, append it to
   `WORKTREE_SUBCOMMANDS` in `src/init.rs` so dynamic completion fires there.
4. **If renaming or removing the `cd` subcommand**, update the wrapper
   functions in `src/init.rs` (`fish_extras` / `posix_wrapper`) to match.
   The wrapper hardcodes `"cd"` as its trigger.

## Conventions

- The `_complete` subcommand is an internal contract for shell scripts; its
  output format is consumed by `cl init` output. Treat it as a stable
  interface — don't change without updating `src/init.rs`.
- Worktree resolution from a user query uses `WorktreeInfo::matches` in
  `src/git.rs`. Reuse it for any new subcommand that takes an issue argument.
