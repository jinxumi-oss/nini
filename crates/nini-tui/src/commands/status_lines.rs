//! v0.8.3: Build the multi-line `/status` output for the AppState.
//!
//! Pure read function — does not mutate state.

use crate::state::AppState;

/// Render the current session status as a `Vec<String>` (one
/// line per row). Used by `/status` and the `nini info` debug path.
pub fn build_status_lines(state: &AppState) -> Vec<String> {
    let mut out = Vec::new();
    let cwd = state.session_state.cwd
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "<unset>".to_string());
    let branch = state.session_state.git_branch
        .as_deref()
        .unwrap_or("<not a git repo>");
    let sid = state.session_state.session_id
        .as_deref()
        .unwrap_or("<no session>");
    let transcript_lines = state.transcript_state.lines.len();
    let (in_tok, out_tok) = (state.run_state.tokens.input, state.run_state.tokens.output);
    out.push(format!("session    : {sid}"));
    out.push(format!("cwd        : {cwd}"));
    out.push(format!("git branch : {branch}"));
    out.push(format!("transcript : {transcript_lines} line(s)"));
    out.push(format!("tokens     : in={in_tok} out={out_tok}"));
    out.push(format!("cost       : ${:.4}", state.run_state.cost_usd));
    if state.run_state.context_window > 0 {
        let pct = (state.run_state.context_used as f64 / state.run_state.context_window as f64) * 100.0;
        out.push(format!(
            "context    : {:.0}% of {} (used {})",
            pct, state.run_state.context_window, state.run_state.context_used
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;

    #[test]
    fn default_state_produces_6_lines() {
        let state = AppState::default();
        let lines = build_status_lines(&state);
        // 6 lines for the basic stats (no context line because window=0)
        assert_eq!(lines.len(), 6, "expected 6 status lines");
        assert!(lines[0].starts_with("session    : "));
        assert!(lines.iter().any(|l| l.starts_with("tokens     : ")));
    }

    #[test]
    fn cwd_and_branch_placeholders() {
        let state = AppState::default();
        let lines = build_status_lines(&state);
        assert!(lines.iter().any(|l| l.contains("<unset>")));
        assert!(lines.iter().any(|l| l.contains("<not a git repo>")));
        assert!(lines.iter().any(|l| l.contains("<no session>")));
    }

    #[test]
    fn context_line_appears_when_window_set() {
        let mut state = AppState::default();
        state.run_state.context_window = 1000;
        state.run_state.context_used = 250;
        let lines = build_status_lines(&state);
        assert_eq!(lines.len(), 7, "context line should be present");
        assert!(lines.iter().any(|l| l.contains("25%")));
    }
}