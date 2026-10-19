//! Oracle: `find` must be distinguishable from `bash` when the request is
//! "list the files in a directory".
//! invariant: 545fd548

use nini_core::tool::Tool;
use nini_tools::{BashTool, FindTool};

fn norm(s: &str) -> String {
    s.to_lowercase()
}

/// The system prompt contribution, flattened: snippet + every guideline.
fn contribution_text(tool: &dyn Tool) -> String {
    let c = tool
        .system_prompt_contribution()
        .expect("tool must contribute a system prompt section");
    format!("{}\n{}", c.snippet, c.guidelines.join("\n")).to_lowercase()
}

#[test]
fn find_description_points_listing_at_bash() {
    let description = norm(&FindTool::new().spec().description);

    // The pre-fix description was "Find files by glob pattern ..." — the exact
    // wording that made the model pick find for "list files in <dir>".
    assert!(
        !description.contains("find files by glob pattern"),
        "the description is back to the wording that collides with `list files`"
    );

    // The fix's actual content: a negative direction naming the right tool.
    assert!(
        description.contains("bash"),
        "find's description must name bash as the alternative: {description}"
    );
    assert!(
        description.contains("ls"),
        "find's description must name `ls`, the command the model should reach for: {description}"
    );
    assert!(
        description.contains("not for listing"),
        "find's description must explicitly exclude listing a directory's contents: {description}"
    );
}

#[test]
fn find_guideline_states_the_negative_direction() {
    let text = contribution_text(&FindTool::new());
    assert!(
        text.contains("bash") && text.contains("ls"),
        "the guidelines must say when NOT to use find: {text}"
    );
    // Pre-fix guideline was only mechanics ("Anchor glob patterns with `**/`").
    assert!(
        !text.contains("anchor glob patterns"),
        "the guidelines reverted to mechanics-only, losing the routing signal"
    );
}

#[test]
fn find_and_bash_are_not_interchangeable_on_a_listing_request() {
    // The end-to-end symptom: "List files in crates/nini-tui/src/, just first 5"
    // chose find (with arguments: null) instead of bash + ls. What the model
    // reads is each tool's full guidance — description plus system-prompt
    // section — so the assertion is made on exactly that text.
    let find_text = format!(
        "{}\n{}",
        norm(&FindTool::new().spec().description),
        contribution_text(&FindTool::new())
    );
    let bash_text = format!(
        "{}\n{}",
        norm(&BashTool::new().spec().description),
        contribution_text(&BashTool::new())
    );

    assert_ne!(
        find_text, bash_text,
        "find and bash present the same guidance to the model"
    );
    // bash advertises `ls` as a first-class capability ...
    assert!(
        bash_text.contains("ls"),
        "bash's guidance must advertise `ls`: {bash_text}"
    );
    // ... and find must hand the listing case over to it. Without this the
    // model has no signal and picks find with `arguments: null`.
    assert!(
        find_text.contains("not for listing") && find_text.contains("bash"),
        "find does not defer the listing case to bash: {find_text}"
    );
    // find must not also claim the capability it handed over.
    assert!(
        !find_text.contains("list the files") && !find_text.contains("lists the files"),
        "find must not claim the listing capability it handed to bash: {find_text}"
    );
}
