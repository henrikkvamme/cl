use std::fmt;

use inquire::Select;

use crate::git::{self, WorktreeInfo};
use crate::linear::IssueSummary;

/// A single item in the unified picker
pub enum PickerItem {
    Session {
        worktree: WorktreeInfo,
        title: String,
    },
    Issue(IssueSummary),
    Separator(String),
}

/// What the user selected
pub enum PickerResult {
    ResumeSession(WorktreeInfo),
    StartIssue(String), // identifier
}

impl fmt::Display for PickerItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PickerItem::Session { worktree, title } => {
                let id = worktree
                    .identifier
                    .as_deref()
                    .unwrap_or(&worktree.branch);
                let ago = git::time_ago(worktree.last_activity);
                write!(f, "{id}  {title}  ({ago})")
            }
            PickerItem::Issue(issue) => write!(f, "{issue}"),
            PickerItem::Separator(label) => write!(f, "── {label} ──"),
        }
    }
}

impl PickerItem {
    fn searchable_text(&self) -> String {
        match self {
            PickerItem::Session { worktree, title } => {
                format!(
                    "{} {}",
                    worktree.identifier.as_deref().unwrap_or(&worktree.branch),
                    title,
                )
            }
            PickerItem::Issue(issue) => {
                format!("{} {} {}", issue.identifier, issue.title, issue.priority_label)
            }
            PickerItem::Separator(_) => String::new(),
        }
    }
}

/// Build and show the unified picker. Returns what the user selected.
pub fn show_unified_picker(
    worktrees: Vec<WorktreeInfo>,
    issues: Vec<IssueSummary>,
    viewer_id: &str,
    session_titles: &std::collections::HashMap<String, String>,
) -> Result<PickerResult, String> {
    let mut items: Vec<PickerItem> = Vec::new();

    // Collect identifiers of active sessions to dedup from issue list
    let active_ids: Vec<String> = worktrees
        .iter()
        .filter_map(|w| w.identifier.clone())
        .collect();

    // Section 1: Active Sessions
    if !worktrees.is_empty() {
        items.push(PickerItem::Separator("Active Sessions".to_string()));
        for wt in worktrees {
            let title = wt
                .identifier
                .as_ref()
                .and_then(|id| session_titles.get(id))
                .cloned()
                .unwrap_or_default();
            items.push(PickerItem::Session {
                worktree: wt,
                title,
            });
        }
    }

    // Split issues: mine vs team, excluding active sessions
    let (my_issues, team_issues): (Vec<_>, Vec<_>) = issues
        .into_iter()
        .filter(|i| !active_ids.contains(&i.identifier))
        .partition(|i| i.assignee_id.as_deref() == Some(viewer_id));

    // Section 2: My Issues
    if !my_issues.is_empty() {
        items.push(PickerItem::Separator("My Issues".to_string()));
        for issue in my_issues {
            items.push(PickerItem::Issue(issue));
        }
    }

    // Section 3: Team Issues
    if !team_issues.is_empty() {
        items.push(PickerItem::Separator("Team Issues".to_string()));
        for issue in team_issues {
            items.push(PickerItem::Issue(issue));
        }
    }

    let selectable_count = items
        .iter()
        .filter(|i| !matches!(i, PickerItem::Separator(_)))
        .count();
    if selectable_count == 0 {
        return Err("No issues or sessions found.".into());
    }

    let scorer = |input: &str, item: &PickerItem, _display: &str, _idx: usize| -> Option<i64> {
        if input.is_empty() {
            // Show everything when no filter, but hide nothing
            return match item {
                PickerItem::Separator(_) => Some(0),
                _ => Some(1),
            };
        }
        // Hide separators when filtering
        if matches!(item, PickerItem::Separator(_)) {
            return None;
        }
        let haystack = item.searchable_text().to_lowercase();
        let needle = input.to_lowercase();
        // All terms must match
        if needle.split_whitespace().all(|term| haystack.contains(term)) {
            Some(1)
        } else {
            None
        }
    };

    let selection = Select::new("Select issue or session:", items)
        .with_page_size(20)
        .with_scorer(&scorer)
        .prompt()
        .map_err(|e| format!("{e}"))?;

    match selection {
        PickerItem::Session { worktree, .. } => Ok(PickerResult::ResumeSession(worktree)),
        PickerItem::Issue(issue) => Ok(PickerResult::StartIssue(issue.identifier)),
        PickerItem::Separator(_) => Err("Invalid selection".into()),
    }
}
