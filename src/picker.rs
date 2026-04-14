use std::fmt;

use inquire::Select;

use crate::git::{self, WorktreeInfo};
use crate::linear::IssueSummary;

enum ItemKind {
    Session,
    MyIssue,
    TeamIssue,
}

pub enum PickerItem {
    Session {
        worktree: WorktreeInfo,
        title: String,
    },
    Issue {
        issue: IssueSummary,
        mine: bool,
    },
}

pub enum PickerResult {
    ResumeSession(WorktreeInfo),
    StartIssue(String),
}

impl PickerItem {
    fn kind(&self) -> ItemKind {
        match self {
            PickerItem::Session { .. } => ItemKind::Session,
            PickerItem::Issue { mine: true, .. } => ItemKind::MyIssue,
            PickerItem::Issue { mine: false, .. } => ItemKind::TeamIssue,
        }
    }

    fn marker(&self) -> &str {
        match self.kind() {
            ItemKind::Session => "▶",
            ItemKind::MyIssue => "●",
            ItemKind::TeamIssue => " ",
        }
    }

    fn searchable_text(&self) -> String {
        match self {
            PickerItem::Session { worktree, title, .. } => {
                format!("{} {}", worktree.display_name(), title)
            }
            PickerItem::Issue { issue, .. } => {
                format!("{} {} {}", issue.identifier, issue.title, issue.priority_label)
            }
        }
    }
}

impl fmt::Display for PickerItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let marker = self.marker();
        match self {
            PickerItem::Session { worktree, title, .. } => {
                let id = worktree.display_name();
                let ago = git::time_ago(worktree.last_activity);
                write!(f, "{marker} {id}  {title}  ({ago})")
            }
            PickerItem::Issue { issue, .. } => {
                write!(f, "{marker} {issue}")
            }
        }
    }
}

pub fn show_unified_picker(
    worktrees: Vec<WorktreeInfo>,
    issues: Vec<IssueSummary>,
    viewer_id: &str,
    session_titles: &std::collections::HashMap<String, String>,
) -> Result<PickerResult, String> {
    let mut items: Vec<PickerItem> = Vec::new();

    let active_ids: Vec<String> = worktrees
        .iter()
        .filter_map(|w| w.identifier.clone())
        .collect();

    for wt in worktrees {
        let title = wt
            .identifier
            .as_ref()
            .and_then(|id| session_titles.get(id))
            .cloned()
            .unwrap_or_default();
        items.push(PickerItem::Session { worktree: wt, title });
    }

    let (my_issues, team_issues): (Vec<_>, Vec<_>) = issues
        .into_iter()
        .filter(|i| !active_ids.contains(&i.identifier))
        .partition(|i| i.assignee_id.as_deref() == Some(viewer_id));

    for issue in my_issues {
        items.push(PickerItem::Issue { issue, mine: true });
    }

    for issue in team_issues {
        items.push(PickerItem::Issue { issue, mine: false });
    }

    if items.is_empty() {
        return Err("No issues or sessions found.".into());
    }

    let item_count = items.len() as i64;
    let scorer = |input: &str, item: &PickerItem, _display: &str, idx: usize| -> Option<i64> {
        let position_score = item_count - idx as i64;
        if input.is_empty() {
            return Some(position_score);
        }
        let haystack = item.searchable_text().to_lowercase();
        let needle = input.to_lowercase();
        if needle.split_whitespace().all(|term| haystack.contains(term)) {
            Some(position_score)
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
        PickerItem::Issue { issue, .. } => Ok(PickerResult::StartIssue(issue.identifier)),
    }
}
