use std::fs;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{self, Command};

use clap::{Parser, Subcommand};
use serde_json::{Value, json};

// --- CLI ---

#[derive(Parser)]
#[command(name = "cl", about = "Bridge Linear issues to Claude Code")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Linear issue URL (e.g. https://linear.app/org/issue/KL-35/slug)
    issue_link: Option<String>,

    /// Create a new git branch instead of a worktree
    #[arg(short = 'b')]
    branch: bool,

    /// No worktree, no branch — run claude in the current working tree
    #[arg(long)]
    raw: bool,

    /// Enable Chrome integration
    #[arg(short = 'c')]
    chrome: bool,

    /// Base branch for rebasing (default: auto-detect)
    #[arg(long)]
    base: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Store your Linear personal access token
    Login {
        /// Linear API key
        api_key: String,
    },
}

// --- Token Storage ---

fn config_dir() -> PathBuf {
    let dir = home_dir().join(".config").join("cl");
    fs::create_dir_all(&dir).ok();
    dir
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .expect("HOME not set")
}

fn save_token(key: &str) {
    let path = config_dir().join("linear_token");
    fs::write(&path, key.trim()).unwrap_or_else(|e| {
        eprintln!("Error writing token to {}: {e}", path.display());
        process::exit(1);
    });
    println!("Token saved to {}", path.display());
}

fn load_token() -> String {
    if let Ok(token) = fs::read_to_string(config_dir().join("linear_token")) {
        let token = token.trim().to_string();
        if !token.is_empty() {
            return token;
        }
    }
    if let Ok(token) = std::env::var("LINEAR_API_KEY") {
        return token;
    }
    eprintln!("Error: No Linear API key found. Run `cl login <key>` or set LINEAR_API_KEY.");
    process::exit(1);
}

// --- Linear API ---

fn graphql(api_key: &str, query: &str, variables: Value) -> Result<Value, String> {
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

struct IssueData {
    id: String,
    identifier: String,
    title: String,
    branch_name: String,
    assignee_id: Option<String>,
    state_type: String,
    started_state_id: Option<String>,
    review_state_id: Option<String>,
    viewer_id: String,
    /// Rich prompt with all issue context (XML-structured)
    prompt: String,
}

fn fetch_issue(api_key: &str, identifier: &str) -> Result<IssueData, String> {
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

fn update_issue(api_key: &str, issue: &IssueData) {
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

// --- URL Parsing ---

fn parse_team_and_number(identifier: &str) -> Result<(String, f64), String> {
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

fn parse_identifier(link: &str) -> Result<String, String> {
    let parts: Vec<&str> = link.trim_end_matches('/').split('/').collect();
    let idx = parts
        .iter()
        .position(|&p| p == "issue")
        .ok_or("URL does not contain 'issue' segment")?;
    let ident = parts
        .get(idx + 1)
        .ok_or("No identifier found after 'issue' in URL")?;
    Ok(ident.to_uppercase())
}

// --- Git ---

fn current_branch() -> Result<String, String> {
    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .map_err(|e| format!("Failed to run git: {e}"))?;
    if !output.status.success() {
        return Err("Failed to get current branch".into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn default_branch() -> String {
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

fn is_issue_branch(branch: &str) -> bool {
    // Match TEAM-123 pattern anywhere in branch name (e.g. "user/KL-35-some-slug")
    let bytes = branch.as_bytes();
    for i in 0..bytes.len() {
        // Must start at beginning or after a non-letter char
        if i > 0 && bytes[i - 1].is_ascii_alphabetic() {
            continue;
        }
        // Find uppercase letters
        let mut j = i;
        while j < bytes.len() && bytes[j].is_ascii_uppercase() {
            j += 1;
        }
        if j == i || j >= bytes.len() || bytes[j] != b'-' {
            continue;
        }
        j += 1; // skip dash
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

fn resolve_base_branch(explicit: Option<&str>) -> Result<String, String> {
    if let Some(base) = explicit {
        return Ok(base.to_string());
    }

    let current = current_branch()?;
    let default = default_branch();

    // On default branch → use it
    if current == default {
        return Ok(default);
    }

    // On an issue branch → use default
    if is_issue_branch(&current) {
        return Ok(default);
    }

    // On some other branch (e.g. "rebus", "v2") → use it as base
    // (The interactive picker for this will be added in Phase 5 with inquire)
    eprintln!("Using current branch '{current}' as base (use --base to override)");
    Ok(current)
}

fn ensure_clean_tree() -> Result<(), String> {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .map_err(|e| format!("Failed to run git: {e}"))?;
    if !output.stdout.is_empty() {
        return Err("Working tree is not clean. Commit or stash changes first.".into());
    }
    Ok(())
}

fn create_branch(name: &str) -> Result<(), String> {
    let status = Command::new("git")
        .args(["checkout", "-b", name])
        .status()
        .map_err(|e| format!("Failed to run git checkout: {e}"))?;
    if !status.success() {
        return Err(format!("git checkout -b {name} failed"));
    }
    Ok(())
}

// --- Chrome ---

fn ensure_chrome_running() {
    let check = Command::new("pgrep").args(["-x", "Google Chrome"]).output();
    let running = check.is_ok_and(|o| o.status.success());
    if !running {
        eprintln!("Launching Google Chrome...");
        let _ = Command::new("open").args(["-a", "Google Chrome"]).status();
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}

// --- Prompt ---

fn build_prompt(issue: &IssueData, worktree: bool, chrome: bool, base_branch: &str) -> String {
    let mut prompt = issue.prompt.clone();

    prompt.push_str("\n\n---\n\n");

    if worktree || !issue.branch_name.is_empty() {
        prompt.push_str(&format!(
            "You are on working branch `{}`.\n\n",
            issue.branch_name
        ));
    }

    prompt.push_str(&format!(
        "Follow this workflow:\n\
         1. Read the issue description and plan your implementation approach.\n\
         2. Implement the changes.\n\
         3. Run any relevant linting and formatting tools if necessary.\n\
         4. Run `git fetch origin {base_branch} && git rebase origin/{base_branch}` to make sure we are up-to-date before pushing.\n\
         5. Create a pull request and assign it to me.\n",
    ));

    if let Some(ref review_id) = issue.review_state_id {
        prompt.push_str(&format!(
            "6. After the PR is created, use the Linear MCP server to update the issue status \
             to \"In Review\" (state ID: {review_id}).\n",
        ));
    }

    prompt.push_str(
        "\nFor any Linear operations (updating issue status, etc.), use the Linear MCP server.\n",
    );

    if chrome {
        prompt.push_str(
            "\nFor checking: if there are any visual changes, run the application in \
             debug/dev mode and verify that the feature works correctly in the browser.\n\
             When creating the PR, if there are visual changes, take a screenrecording using the \
             Chrome gif_creator tool and include it in the PR as a comment. Upload using the git_shot skill.\n\
             IMPORTANT: If the project uses bun, use `bunx gitshot ...`, otherwise it will not work.
             \n",
        );
    }

    prompt
}

// --- Exec Claude ---

fn exec_claude(issue: &IssueData, prompt: &str, worktree: bool, chrome: bool) -> ! {
    let mut cmd = Command::new("claude");

    cmd.arg("--allow-dangerously-skip-permissions");
    cmd.args(["--name", &issue.identifier]);
    cmd.args(["--permission-mode", "plan"]);

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

// --- Main ---

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Login { api_key }) => {
            save_token(&api_key);
        }
        None => {
            let issue_link = cli.issue_link.unwrap_or_else(|| {
                eprintln!("Usage: cl <issue-link> [-b -c] [--raw]");
                eprintln!("       cl login <api-key>");
                process::exit(1);
            });
            // Default: worktree. -b: branch (no worktree). --raw: neither.
            let worktree = !cli.branch && !cli.raw;
            let base_branch = resolve_base_branch(cli.base.as_deref()).unwrap_or_else(|e| {
                eprintln!("Error: {e}");
                process::exit(1);
            });
            run_start(&issue_link, cli.branch, worktree, cli.chrome, &base_branch);
        }
    }
}

fn run_start(issue_link: &str, branch: bool, worktree: bool, chrome: bool, base_branch: &str) {
    let identifier = parse_identifier(issue_link).unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        process::exit(1);
    });

    let api_key = load_token();

    eprintln!("Fetching issue {identifier}...");
    let issue = fetch_issue(&api_key, &identifier).unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        process::exit(1);
    });
    eprintln!("Found: {} — {}", issue.identifier, issue.title);
    eprintln!("Branch: {}", issue.branch_name);

    update_issue(&api_key, &issue);

    if branch {
        ensure_clean_tree().unwrap_or_else(|e| {
            eprintln!("Error: {e}");
            process::exit(1);
        });
        create_branch(&issue.branch_name).unwrap_or_else(|e| {
            eprintln!("Error: {e}");
            process::exit(1);
        });
        eprintln!("Created branch: {}", issue.branch_name);
    }

    if chrome {
        ensure_chrome_running();
    }

    let prompt = build_prompt(&issue, worktree, chrome, base_branch);
    exec_claude(&issue, &prompt, worktree, chrome);
}
