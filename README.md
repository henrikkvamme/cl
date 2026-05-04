# cl

Bridge Linear issues to Claude Code. Takes a Linear issue URL, fetches full context (description, comments, sub-issues, relations, attachments), and launches Claude Code with a structured prompt to implement the issue.

## Install

```bash
cargo install --path .
```

## Setup

Store your Linear personal access token:

```bash
cl login <linear-api-key>
```

Or set the `LINEAR_API_KEY` environment variable.

## Usage

```bash
cl <linear-issue-url>              # Run in a git worktree (default)
cl <linear-issue-url> -b           # Create a new branch instead
cl <linear-issue-url> --raw        # Run in the current working tree
cl <linear-issue-url> -c           # Enable Chrome integration for visual verification
```

### What it does

1. Parses the Linear issue identifier from the URL
2. Fetches the full issue context via the Linear GraphQL API
3. Assigns the issue to you and moves it to "In Progress"
4. Sets up the working environment (worktree, branch, or current tree)
5. Launches `claude` with a structured prompt containing all issue context
6. Claude implements the changes, creates a PR, and moves the issue to "In Review"

### Flags

| Flag | Description |
|------|-------------|
| `-b` | Create a new git branch instead of a worktree |
| `--raw` | No worktree or branch — use the current working tree |
| `-c` | Enable Chrome for visual change verification and screenshots |

### Worktree symlinks

Personal, globally-gitignored scratch directories (e.g. `.notes/`) can be
symlinked into every worktree so they're shared instead of duplicated.

In `~/.config/cl/config.toml`:

```toml
[worktree]
symlinks = [".notes"]
```

Paths are resolved against the **main worktree's** repo root. Symlinks are
created when `cl <issue>` makes a new worktree and refreshed every time you
resume into one — so adding a new entry takes effect the next time you open
each worktree, no separate sync step needed.

`.env.local` files are always symlinked automatically — no config needed.
