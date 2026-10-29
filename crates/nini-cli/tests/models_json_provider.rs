//! Oracle: a provider the user's `models.json` declares is a provider
//! nini can actually talk to.
//!
//! # The gap this file exists for
//!
//! nini read `models.json` and showed the models in it. It never
//! **used** it: `build_provider` knew eleven provider names and
//! answered `unknown provider: <id>` for everything else — including
//! the gateway a user had configured, with the base URL and the
//! credential, in their own file.
//!
//! So the catalogue said a model was reachable and the program could
//! not reach it.
//!
//! # What is asserted here
//!
//! 1. the request goes to the **file's** base URL, not to a host nini
//!    invented and not to one an environment variable named;
//! 2. it carries the **file's** API key, as a bearer token;
//! 3. a `models.json` in the *working directory* is not adopted: a
//!    directory a user merely cloned must not be able to hand nini a
//!    base URL and a credential.
//!
//! The first two are checked against a real mock server over a real
//! socket, by the real binary — not by calling the factory from a test.

use std::process::Command;

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A gateway provider, in Pi's spelling.
fn models_json(base_url: &str, api_key: &str) -> String {
    format!(
        r#"{{
  "providers": {{
    "my-gw": {{
      "baseUrl": "{base_url}",
      "api": "openai-completions",
      "apiKey": "{api_key}",
      "models": [
        {{"id": "gw-model", "reasoning": false, "input": ["text"],
          "contextWindow": 100000, "maxTokens": 8000}}
      ]
    }}
  }}
}}"#
    )
}

/// An OpenAI-style SSE answer.
fn sse_body() -> String {
    let chunks = [
        r#"{"choices":[{"delta":{"role":"assistant"},"finish_reason":"","index":0}],"usage":null}"#,
        r#"{"choices":[{"delta":{"content":"Hello from the mock"},"finish_reason":"","index":0}]}"#,
        r#"{"choices":[{"delta":{},"finish_reason":"stop","index":0}],"usage":{"prompt_tokens":9,"completion_tokens":3}}"#,
    ];
    let mut body = String::new();
    for chunk in chunks {
        body.push_str("data: ");
        body.push_str(chunk);
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
}

/// A mock provider that answers `/v1/chat/completions`.
async fn mock_provider() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body()),
        )
        .mount(&server)
        .await;
    server
}

/// The credential the mock saw, if any.
fn authorization(requests: &[wiremock::Request]) -> Option<String> {
    requests
        .first()?
        .headers
        .get("authorization")
        .map(|value| value.to_str().unwrap_or_default().to_string())
}

/// The headline: the request goes where the file says, with the file's
/// key.
#[tokio::test]
async fn the_request_goes_to_the_files_base_url_with_the_files_key() {
    let server = mock_provider().await;

    let dir = tempfile::tempdir().expect("temp dir");
    // The user-level `models.json`: the agent directory is the only
    // place a provider may be configured.
    std::fs::write(
        dir.path().join("models.json"),
        models_json(&server.uri(), "secret-from-models-json"),
    )
    .expect("write models.json");

    let out = Command::new(env!("CARGO_BIN_EXE_nini"))
        .env("NINI_AGENT_DIR", dir.path())
        .env("HOME", dir.path())
        .current_dir(dir.path())
        .args(["--provider", "my-gw", "--model", "gw-model", "-p", "hi"])
        .output()
        .expect("nini runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "nini did not get an answer.\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("Hello from the mock"),
        "the answer never reached the user.\nstdout: {stdout}\nstderr: {stderr}"
    );

    let requests = server.received_requests().await.expect("requests");
    assert_eq!(
        requests.len(),
        1,
        "the provider in the user's models.json must be used exactly once"
    );
    assert_eq!(
        requests[0].url.path(),
        "/v1/chat/completions",
        "the request must go to the endpoint of the file's base URL"
    );
    assert_eq!(
        authorization(&requests).as_deref(),
        Some("Bearer secret-from-models-json"),
        "the request must carry the API key the file declares"
    );
}

/// The other half: a `models.json` in the working directory is **not**
/// a configuration.
///
/// Pi 0.85.1 has no project-level `models.json` at all. nini has one,
/// and if it were read as a configuration then a repository a user
/// merely cloned could point nini at a host of its choosing and hand it
/// a credential — or a `!command` to run.
#[tokio::test]
async fn a_project_level_models_json_is_not_adopted() {
    let server = mock_provider().await;

    let dir = tempfile::tempdir().expect("temp dir");
    let project = dir.path().join("project");
    std::fs::create_dir_all(project.join(".nini")).expect("project dir");
    std::fs::write(
        project.join(".nini").join("models.json"),
        models_json(&server.uri(), "secret-from-models-json"),
    )
    .expect("write models.json");
    // The user-level agent directory is empty: the project-level file
    // is the only one there is.
    let agent = dir.path().join("agent");
    std::fs::create_dir_all(&agent).expect("agent dir");

    let out = Command::new(env!("CARGO_BIN_EXE_nini"))
        .env("NINI_AGENT_DIR", &agent)
        .env("HOME", dir.path())
        .current_dir(&project)
        .args(["--provider", "my-gw", "--model", "gw-model", "-p", "hi"])
        .output()
        .expect("nini runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown provider"),
        "the provider must not be adopted from the working directory.\nstderr: {stderr}"
    );

    let requests = server.received_requests().await.expect("requests");
    assert!(
        requests.is_empty(),
        "a project-level models.json must not send a request anywhere: {requests:?}"
    );
}
