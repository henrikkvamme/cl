use crate::linear::IssueData;

pub fn build_prompt(issue: &IssueData, worktree: bool, chrome: bool, base_branch: &str) -> String {
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
