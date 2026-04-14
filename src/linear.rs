use std::fmt;

use serde_json::{Value, json};

pub struct IssueSummary {
    pub identifier: String,
    pub title: String,
    pub priority: i64,
    pub priority_label: String,
    pub state_name: String,
    pub assignee_id: Option<String>,
    pub assignee_name: Option<String>,
}

impl fmt::Display for IssueSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let pri = if self.priority_label.is_empty() {
            String::new()
        } else {
            format!("  [{}]", self.priority_label)
        };
        write!(f, "{}{}  {}", self.identifier, pri, self.title)
    }
}

pub struct IssueData {
    pub id: String,
    pub identifier: String,
    pub title: String,
    pub branch_name: String,
    pub assignee_id: Option<String>,
    pub state_type: String,
    pub started_state_id: Option<String>,
    pub review_state_id: Option<String>,
    pub viewer_id: String,
    pub prompt: String,
}

pub fn graphql(api_key: &str, query: &str, variables: Value) -> Result<Value, String> {
    let client = reqwest::blocking::Client::new();
    let resp = client
        .post("https://api.linear.app/graphql")
        .header("Authorization", api_key)
        .json(&json!({ "query": query, "variables": variables }))
        .send()
        .map_err(|e| format!("HTTP error: {e}"))?
        .error_for_status()
        .map_err(|e| format!("API error: {e}"))?;
    let body: Value = resp.json().map_err(|e| format!("JSON parse error: {e}"))?;
    if let Some(errors) = body.get("errors") {
        return Err(format!("GraphQL errors: {errors}"));
    }
    body.get("data")
        .cloned()
        .ok_or_else(|| "No data in response".to_string())
}

pub fn fetch_issue(api_key: &str, identifier: &str) -> Result<IssueData, String> {
    let (team_key, number) = parse_team_and_number(identifier)?;

    let query = r#"
        query($teamKey: String!, $number: Float!) {
            issues(filter: { team: { key: { eq: $teamKey } }, number: { eq: $number } }) {
                nodes {
                    id
                    identifier
                    title
                    description
                    branchName
                    priority
                    priorityLabel
                    estimate
                    dueDate
                    assignee { id name }
                    state { id name type }
                    team {
                        id
                        name
                        states { nodes { id name type } }
                    }
                    labels { nodes { name } }
                    parent {
                        identifier
                        title
                    }
                    children { nodes { identifier title state { name } } }
                    relations {
                        nodes {
                            type
                            relatedIssue { identifier title }
                        }
                    }
                    comments {
                        nodes {
                            body
                            user { name }
                            createdAt
                        }
                    }
                    attachments {
                        nodes {
                            title
                            url
                            metadata
                        }
                    }
                }
            }
            viewer { id }
        }
    "#;
    let data = graphql(
        api_key,
        query,
        json!({ "teamKey": team_key, "number": number }),
    )?;

    let issue = data["issues"]["nodes"]
        .as_array()
        .and_then(|a| a.first())
        .ok_or_else(|| format!("Issue {identifier} not found"))?;

    let viewer_id = data["viewer"]["id"]
        .as_str()
        .ok_or("Could not get viewer ID")?
        .to_string();

    let states = issue["team"]["states"]["nodes"]
        .as_array()
        .ok_or("Could not get team states")?;
    let find_state = |substring: &str| {
        states
            .iter()
            .find(|s| {
                s["name"]
                    .as_str()
                    .is_some_and(|n| n.to_lowercase().contains(substring))
            })
            .and_then(|s| s["id"].as_str())
            .map(String::from)
    };
    let started_state_id = find_state("progress");
    let review_state_id = find_state("review");

    let ident = str_field(issue, "identifier")?;
    let title = str_field(issue, "title")?;
    let branch_name = str_field(issue, "branchName")?;
    let prompt = build_issue_prompt(issue, &ident, &branch_name);

    Ok(IssueData {
        id: str_field(issue, "id")?,
        identifier: ident,
        title,
        branch_name,
        assignee_id: issue["assignee"]["id"].as_str().map(String::from),
        state_type: issue["state"]["type"].as_str().unwrap_or("").to_string(),
        started_state_id,
        review_state_id,
        viewer_id,
        prompt,
    })
}

fn build_issue_prompt(issue: &Value, identifier: &str, branch_name: &str) -> String {
    let mut p = format!("# Linear Issue {identifier}\n\n");

    let title = issue["title"].as_str().unwrap_or("");
    p.push_str(&format!("**Title:** {title}\n\n"));

    p.push_str(&format!("**Branch:** `{branch_name}`\n\n"));

    // Metadata
    let mut meta = Vec::new();
    if let Some(team) = issue["team"]["name"].as_str() {
        meta.push(format!("**Team:** {team}"));
    }
    if let Some(state) = issue["state"]["name"].as_str() {
        meta.push(format!("**Status:** {state}"));
    }
    if let Some(pri) = issue["priorityLabel"].as_str() {
        meta.push(format!("**Priority:** {pri}"));
    }
    if let Some(due) = issue["dueDate"].as_str() {
        meta.push(format!("**Due:** {due}"));
    }
    if let Some(est) = issue["estimate"].as_f64() {
        meta.push(format!("**Estimate:** {est}"));
    }
    if let Some(name) = issue["assignee"]["name"].as_str() {
        meta.push(format!("**Assignee:** {name}"));
    }
    if let Some(labels) = issue["labels"]["nodes"].as_array() {
        let names: Vec<&str> = labels.iter().filter_map(|l| l["name"].as_str()).collect();
        if !names.is_empty() {
            meta.push(format!("**Labels:** {}", names.join(", ")));
        }
    }
    if !meta.is_empty() {
        p.push_str(&meta.join(" | "));
        p.push_str("\n\n");
    }

    // Description
    let desc = issue["description"].as_str().unwrap_or("");
    if !desc.is_empty() {
        p.push_str(&format!("## Description\n\n{desc}\n\n"));
    }

    // Parent issue
    if let (Some(pid), Some(ptitle)) = (
        issue["parent"]["identifier"].as_str(),
        issue["parent"]["title"].as_str(),
    ) {
        p.push_str(&format!("## Parent Issue\n\n- **{pid}**: {ptitle}\n\n"));
    }

    // Sub-issues
    if let Some(children) = issue["children"]["nodes"]
        .as_array()
        .filter(|a| !a.is_empty())
    {
        p.push_str("## Sub-issues\n\n");
        for child in children {
            let ci = child["identifier"].as_str().unwrap_or("?");
            let ct = child["title"].as_str().unwrap_or("");
            let cs = child["state"]["name"].as_str().unwrap_or("");
            p.push_str(&format!("- **{ci}** [{cs}]: {ct}\n"));
        }
        p.push('\n');
    }

    // Relations
    if let Some(relations) = issue["relations"]["nodes"]
        .as_array()
        .filter(|a| !a.is_empty())
    {
        p.push_str("## Related Issues\n\n");
        for rel in relations {
            let rtype = rel["type"].as_str().unwrap_or("related");
            let ri = rel["relatedIssue"]["identifier"].as_str().unwrap_or("?");
            let rt = rel["relatedIssue"]["title"].as_str().unwrap_or("");
            p.push_str(&format!("- **{ri}** ({rtype}): {rt}\n"));
        }
        p.push('\n');
    }

    // Attachments
    if let Some(attachments) = issue["attachments"]["nodes"]
        .as_array()
        .filter(|a| !a.is_empty())
    {
        p.push_str("## Attachments\n\n");
        for att in attachments {
            let at = att["title"].as_str().unwrap_or("Untitled");
            let au = att["url"].as_str().unwrap_or("");
            p.push_str(&format!("- [{at}]({au})\n"));
        }
        p.push('\n');
    }

    // Comments
    if let Some(comments) = issue["comments"]["nodes"]
        .as_array()
        .filter(|a| !a.is_empty())
    {
        p.push_str("## Comments\n\n");
        for comment in comments {
            let user = comment["user"]["name"].as_str().unwrap_or("Unknown");
            let date = comment["createdAt"].as_str().unwrap_or("");
            let body = comment["body"].as_str().unwrap_or("");
            p.push_str(&format!("**{user}** ({date}):\n{body}\n\n"));
        }
    }

    p
}

fn str_field(obj: &Value, field: &str) -> Result<String, String> {
    obj[field]
        .as_str()
        .map(String::from)
        .ok_or_else(|| format!("Missing field: {field}"))
}

pub fn update_issue(api_key: &str, issue: &IssueData) {
    let mut input = json!({});

    let needs_assign = issue
        .assignee_id
        .as_ref()
        .is_none_or(|id| id != &issue.viewer_id);
    if needs_assign {
        input["assigneeId"] = json!(issue.viewer_id);
    }

    if issue.state_type != "started" && issue.started_state_id.is_some() {
        input["stateId"] = json!(issue.started_state_id);
    }

    if input.as_object().is_none_or(|o| o.is_empty()) {
        return;
    }

    let query = r#"
        mutation($issueId: String!, $input: IssueUpdateInput!) {
            issueUpdate(id: $issueId, input: $input) {
                success
            }
        }
    "#;

    match graphql(
        api_key,
        query,
        json!({ "issueId": issue.id, "input": input }),
    ) {
        Ok(_) => {
            if needs_assign {
                eprintln!("Assigned issue to you.");
            }
            if issue.state_type != "started" && issue.started_state_id.is_some() {
                eprintln!("Moved issue to In Progress.");
            }
        }
        Err(e) => eprintln!("Warning: Failed to update issue: {e}"),
    }
}

/// Fetch open issues for a team. Returns (issues, viewer_id).
pub fn fetch_team_issues(
    api_key: &str,
    team_key: &str,
) -> Result<(Vec<IssueSummary>, String), String> {
    let query = r#"
        query($teamKey: String!) {
            issues(
                filter: {
                    team: { key: { eq: $teamKey } }
                    state: { type: { nin: ["completed", "cancelled"] } }
                }
                orderBy: updatedAt
                first: 100
            ) {
                nodes {
                    identifier
                    title
                    priority
                    priorityLabel
                    state { name }
                    assignee { id name }
                }
            }
            viewer { id }
        }
    "#;

    let data = graphql(api_key, query, json!({ "teamKey": team_key }))?;

    let viewer_id = data["viewer"]["id"]
        .as_str()
        .ok_or("Could not get viewer ID")?
        .to_string();

    let nodes = data["issues"]["nodes"]
        .as_array()
        .ok_or("No issues in response")?;

    let issues = nodes
        .iter()
        .map(|n| IssueSummary {
            identifier: n["identifier"].as_str().unwrap_or("").to_string(),
            title: n["title"].as_str().unwrap_or("").to_string(),
            priority: n["priority"].as_i64().unwrap_or(4),
            priority_label: n["priorityLabel"].as_str().unwrap_or("").to_string(),
            state_name: n["state"]["name"].as_str().unwrap_or("").to_string(),
            assignee_id: n["assignee"]["id"].as_str().map(String::from),
            assignee_name: n["assignee"]["name"].as_str().map(String::from),
        })
        .collect();

    Ok((issues, viewer_id))
}

/// Batch-fetch issue statuses by identifiers.
pub fn fetch_issue_statuses(
    api_key: &str,
    identifiers: &[&str],
) -> Result<Vec<IssueSummary>, String> {
    if identifiers.is_empty() {
        return Ok(Vec::new());
    }

    let filter_values: Vec<Value> = identifiers.iter().map(|id| json!(id)).collect();
    let query = r#"
        query($identifiers: [String!]!) {
            issues(filter: { identifier: { in: $identifiers } }) {
                nodes {
                    identifier
                    title
                    priority
                    priorityLabel
                    state { name type }
                    assignee { id name }
                }
            }
        }
    "#;

    let data = graphql(api_key, query, json!({ "identifiers": filter_values }))?;

    let nodes = data["issues"]["nodes"]
        .as_array()
        .ok_or("No issues in response")?;

    Ok(nodes
        .iter()
        .map(|n| IssueSummary {
            identifier: n["identifier"].as_str().unwrap_or("").to_string(),
            title: n["title"].as_str().unwrap_or("").to_string(),
            priority: n["priority"].as_i64().unwrap_or(4),
            priority_label: n["priorityLabel"].as_str().unwrap_or("").to_string(),
            state_name: n["state"]["name"].as_str().unwrap_or("").to_string(),
            assignee_id: n["assignee"]["id"].as_str().map(String::from),
            assignee_name: n["assignee"]["name"].as_str().map(String::from),
        })
        .collect())
}

pub fn parse_team_and_number(identifier: &str) -> Result<(String, f64), String> {
    let parts: Vec<&str> = identifier.splitn(2, '-').collect();
    if parts.len() != 2 {
        return Err(format!(
            "Invalid identifier format: {identifier} (expected TEAM-123)"
        ));
    }
    let team_key = parts[0].to_string();
    let number: f64 = parts[1]
        .parse()
        .map_err(|_| format!("Invalid issue number in: {identifier}"))?;
    Ok((team_key, number))
}

/// Resolve an issue input to a TEAM-123 identifier.
/// Accepts: full Linear URL, "KL-35", or bare "35" (requires team from config).
pub fn resolve_identifier(input: &str, team: Option<&str>) -> Result<String, String> {
    let input = input.trim();

    // Case 1: bare number → prepend team
    if input.chars().all(|c| c.is_ascii_digit()) {
        let team = team.ok_or(
            "Bare issue number requires a team. Set [workspace] team in .cl/config.toml",
        )?;
        return Ok(format!("{}-{}", team.to_uppercase(), input));
    }

    // Case 2: TEAM-123 format
    let upper = input.to_uppercase();
    if parse_team_and_number(&upper).is_ok() {
        return Ok(upper);
    }

    // Case 3: full URL
    parse_identifier_from_url(input)
}

fn parse_identifier_from_url(link: &str) -> Result<String, String> {
    let parts: Vec<&str> = link.trim_end_matches('/').split('/').collect();
    let idx = parts
        .iter()
        .position(|&p| p == "issue")
        .ok_or("Invalid input: expected a Linear URL, issue identifier (KL-35), or number (35)")?;
    let ident = parts
        .get(idx + 1)
        .ok_or("No identifier found after 'issue' in URL")?;
    Ok(ident.to_uppercase())
}
