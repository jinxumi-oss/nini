//! D5：`models.json` 的 provider / model 级配置**真正生效**。
//!
//! # 这份文件防的是什么
//!
//! `models.json` 的 provider 条目以前只有三个字段真的被用上：`api`、
//! `baseUrl`、`apiKey`。其余的（`headers`、`authHeader`、`compat`、
//! model 级的 `headers` / `baseUrl` / `name`）**被 serde 丢掉**——文件读
//! 进来，字段没了，用户以为配了，其实没配。
//!
//! 而且还有两个更硬的问题：
//!
//!   * **缺省 api**：Pi 命中内置 provider 时 api **有缺省值**，nini 却要求
//!     用户必须写 `"api"`，否则整条 provider 建不起来；
//!   * **类型不符就丢整个文件**：`"authHeader": true`（Pi 自己的写法）在
//!     一个只认字符串的 `serde` 面前会让**整个 `models.json`** 变成一份
//!     空配置。
//!
//! # 断言的是什么
//!
//! 1. 缺省 api：内置的名字（含薄壳）能建，自定义的名字**显式拒绝**并点名
//!    `api`；
//! 2. provider 级 `headers` 真的在请求上；
//! 3. `authHeader` 真的换了认证头（带那个头、不带默认那个）；
//! 4. model 级 `headers` 生效，**且覆盖** provider 级的同名头；
//! 5. model 级 `baseUrl` 生效（请求去了另一个地址）；
//! 6. `compat` 生效（请求体的形状变了）；
//! 7. 未知字段仍然被忽略（宽松模式没被破坏）。

use std::sync::Arc;

use futures_util::StreamExt;
use nini_cli::provider_factory::build_provider;
use nini_core::provider::{ContentBlock, Message, Provider, Request, Role};
use nini_core::settings::ModelsJson;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// 把一段 `models.json` 的文本读成 nini 的结构。
///
/// 走的是**真的** `serde` 路径：字段读不进来，这里就会 panic——所以这份
/// 文件同时也是「字段真的被读进来了」的证据。
fn models_json(text: &str) -> ModelsJson {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("the models.json parses: {e}"))
}

/// 一个最小可用的请求。
fn a_request(model: &str) -> Request {
    Request {
        model: model.to_string(),
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "hi".to_string(),
            }],
            timestamp: 0,
        }],
        tools: Vec::new(),
        max_tokens: Some(64),
        temperature: None,
        system: None,
        reasoning: None,
    }
}

/// 一个 OpenAI 风格的 SSE 回答。
fn sse_body() -> String {
    let chunks = [
        r#"{"choices":[{"delta":{"role":"assistant"},"finish_reason":"","index":0}],"usage":null}"#,
        r#"{"choices":[{"delta":{"content":"hello"},"finish_reason":"","index":0}]}"#,
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

/// 起一个 mock server，对任何 POST 回一个 SSE 回答。
///
/// 我们要看的是**请求发到了哪里、带了什么头、body 是什么形状**，不是回答。
async fn mock_server() -> MockServer {
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

/// 把一个请求真的发出去。
async fn send_one(provider: &Arc<dyn Provider>, model: &str) {
    let mut stream = provider.stream(a_request(model));
    while let Some(_event) = stream.next().await {}
}

/// 从一个请求里取一个头（大小写不敏感）。
fn header<'a>(request: &'a wiremock::Request, name: &str) -> Option<&'a str> {
    request
        .headers
        .get(name)
        .map(|value| value.to_str().unwrap_or_default())
}

// ---
// 1. 缺省 api
// ---

/// 内置 provider（这里是薄壳 `together`）**不写** `api` 也能建。
///
/// Pi 的口径：命中内置 provider 时 api 有缺省值（来自内置目录），
/// 只有自定义 provider 才必须写（`provider-composer.js:52` 的
/// `defaults?.api`）。
#[tokio::test]
async fn a_known_provider_without_an_api_still_builds() {
    let server = mock_server().await;
    let models = models_json(&format!(
        r#"{{
  "providers": {{
    "together": {{
      "baseUrl": "{base}",
      "apiKey": "the-key",
      "models": [{{"id": "some-model"}}]
    }}
  }}
}}"#,
        base = server.uri()
    ));

    let provider = build_provider("together", Vec::new(), &[], &[], None, Some(&models))
        .expect("a known provider without an `api` must still build");

    // 缺省 api 是 `openai-completions`：请求打到了 chat/completions 上。
    send_one(&provider, "some-model").await;
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests.len(),
        1,
        "the request must reach the mock server (the default api is openai-completions)"
    );
    assert_eq!(requests[0].url.path(), "/v1/chat/completions");
}

/// 自定义 provider **不写** `api` → **显式拒绝**，消息里点名 `api`。
///
/// 不猜一个 api，也不静默失败：猜出来的 api 会把请求发到用户没有选择的
/// 地方。
#[tokio::test]
async fn a_custom_provider_without_an_api_is_refused_by_name() {
    let models = models_json(
        r#"{
  "providers": {
    "my-gw": {
      "baseUrl": "http://gw.invalid/v1",
      "apiKey": "the-key",
      "models": [{"id": "some-model"}]
    }
  }
}"#,
    );

    let err = match build_provider("my-gw", Vec::new(), &[], &[], None, Some(&models)) {
        Ok(_) => panic!("a custom provider without an `api` must not build"),
        Err(e) => e,
    };
    let message = err.to_string();
    assert!(
        message.contains("api"),
        "the refusal must name the missing item: {message}"
    );
    assert!(
        message.contains("must be set"),
        "the refusal must say the `api` has to be written: {message}"
    );
    assert!(
        message.contains("my-gw"),
        "the refusal must name the provider: {message}"
    );
}

/// 薄壳在 `models.json` 里没写 `baseUrl` 时，用 `{ID}_BASE_URL` 环境变量。
///
/// 口径：**models.json > 环境变量 > 注册表默认**（文件最具体，环境变量次之，
/// 注册表默认是最后的选择）。这一条断言的是中间那一层。
#[tokio::test]
async fn a_thin_shell_without_a_base_url_uses_the_environment_variable() {
    let server = mock_server().await;
    let models = models_json(
        r#"{
  "providers": {
    "together": {
      "apiKey": "the-key",
      "models": [{"id": "some-model"}]
    }
  }
}"#,
    );

    // 环境变量是进程全局的，测试又是并行的：改它必须上锁。
    // 用 `tokio` 的锁：请求要在这个锁的保护下真的发出去，而
    // `std::sync::Mutex` 的 guard 不能跨 `.await`。
    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _guard = ENV_LOCK.lock().await;
    let previous = std::env::var("TOGETHER_BASE_URL").ok();
    // SAFETY: 上面的锁保证没有别的测试同时动这个变量。
    unsafe { std::env::set_var("TOGETHER_BASE_URL", server.uri()) };

    let built = build_provider("together", Vec::new(), &[], &[], None, Some(&models));
    let provider = built.expect("a thin shell with an environment base URL must build");
    send_one(&provider, "some-model").await;

    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests.len(),
        1,
        "the request must go to the base URL the environment variable names"
    );

    match previous {
        Some(value) => unsafe { std::env::set_var("TOGETHER_BASE_URL", value) },
        None => unsafe { std::env::remove_var("TOGETHER_BASE_URL") },
    }
}

// ---
// 2. provider 级 headers
// ---

/// provider 级的 `headers` 真的在请求上。
#[tokio::test]
async fn provider_headers_reach_the_request() {
    let server = mock_server().await;
    let models = models_json(&format!(
        r#"{{
  "providers": {{
    "my-gw": {{
      "api": "openai-completions",
      "baseUrl": "{base}",
      "apiKey": "the-key",
      "headers": {{"X-Gateway": "gw-1", "HTTP-Referer": "nini"}},
      "models": [{{"id": "some-model"}}]
    }}
  }}
}}"#,
        base = server.uri()
    ));

    let provider = build_provider("my-gw", Vec::new(), &[], &[], None, Some(&models))
        .expect("the provider builds");
    send_one(&provider, "some-model").await;

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        header(&requests[0], "x-gateway"),
        Some("gw-1"),
        "the provider-level header must be on the wire"
    );
    assert_eq!(header(&requests[0], "http-referer"), Some("nini"));
}

// ---
// 3. authHeader
// ---

/// `authHeader` 换了认证头：带那个头，**不带**默认那个。
///
/// 默认是 `Authorization: Bearer <key>`；写了 `authHeader` 之后，key 走
/// 那个头（裸值，没有 `Bearer ` 前缀——Azure 的 `api-key`、Google 的
/// `x-goog-api-key` 都是裸值）。
#[tokio::test]
async fn the_auth_header_replaces_the_default_one() {
    let server = mock_server().await;
    let models = models_json(&format!(
        r#"{{
  "providers": {{
    "my-gw": {{
      "api": "openai-completions",
      "baseUrl": "{base}",
      "apiKey": "the-key",
      "authHeader": "X-Api-Key",
      "models": [{{"id": "some-model"}}]
    }}
  }}
}}"#,
        base = server.uri()
    ));

    let provider = build_provider("my-gw", Vec::new(), &[], &[], None, Some(&models))
        .expect("the provider builds");
    send_one(&provider, "some-model").await;

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        header(&requests[0], "x-api-key"),
        Some("the-key"),
        "the key must travel in the header the file named"
    );
    assert_eq!(
        header(&requests[0], "authorization"),
        None,
        "the default Authorization header must not be sent as well"
    );
}

/// Pi 自己的写法是**布尔**（`"authHeader": true`）。这样的文件必须照样
/// 读得进来——一个类型不符就让**整个 `models.json`** 变成空配置，是比
/// 「这个开关没生效」严重得多的故障。
#[tokio::test]
async fn pis_boolean_auth_header_does_not_break_the_file() {
    let server = mock_server().await;
    let models = models_json(&format!(
        r#"{{
  "providers": {{
    "my-gw": {{
      "api": "openai-completions",
      "baseUrl": "{base}",
      "apiKey": "the-key",
      "authHeader": true,
      "models": [{{"id": "some-model"}}]
    }}
  }}
}}"#,
        base = server.uri()
    ));

    let provider = build_provider("my-gw", Vec::new(), &[], &[], None, Some(&models))
        .expect("a file with Pi's boolean authHeader must still build");
    send_one(&provider, "some-model").await;

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "the provider must still be usable");
    assert_eq!(
        header(&requests[0], "authorization"),
        Some("Bearer the-key"),
        "nini's OpenAI dialect already puts the key in `Authorization: Bearer`, \
         which is what Pi's `true` means"
    );
}

// ---
// 4. model 级 headers
// ---

/// model 级的 `headers` 生效，**且覆盖** provider 级的同名头。
///
/// Pi 的合并顺序是 provider 级在前、model 级在后（`rawModelHeaders`，
/// `provider-composer.js:319-327`），所以 model 级赢。
#[tokio::test]
async fn model_headers_override_the_provider_level_ones() {
    let server = mock_server().await;
    let models = models_json(&format!(
        r#"{{
  "providers": {{
    "my-gw": {{
      "api": "openai-completions",
      "baseUrl": "{base}",
      "apiKey": "the-key",
      "headers": {{"X-Scope": "provider", "X-Only-Provider": "p"}},
      "models": [
        {{"id": "some-model", "headers": {{"X-Scope": "model"}}}},
        {{"id": "other-model"}}
      ]
    }}
  }}
}}"#,
        base = server.uri()
    ));

    let provider = build_provider("my-gw", Vec::new(), &[], &[], None, Some(&models))
        .expect("the provider builds");
    send_one(&provider, "some-model").await;

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        header(&requests[0], "x-scope"),
        Some("model"),
        "the model-level header must win over the provider-level one"
    );
    assert_eq!(
        header(&requests[0], "x-only-provider"),
        Some("p"),
        "the provider-level header must still be there"
    );
}

/// 没有 model 级头的那个模型，拿到的还是 provider 级的头。
#[tokio::test]
async fn a_model_without_its_own_headers_keeps_the_provider_level_ones() {
    let server = mock_server().await;
    let models = models_json(&format!(
        r#"{{
  "providers": {{
    "my-gw": {{
      "api": "openai-completions",
      "baseUrl": "{base}",
      "apiKey": "the-key",
      "headers": {{"X-Scope": "provider"}},
      "models": [
        {{"id": "some-model", "headers": {{"X-Scope": "model"}}}},
        {{"id": "other-model"}}
      ]
    }}
  }}
}}"#,
        base = server.uri()
    ));

    let provider = build_provider("my-gw", Vec::new(), &[], &[], None, Some(&models))
        .expect("the provider builds");
    send_one(&provider, "other-model").await;

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        header(&requests[0], "x-scope"),
        Some("provider"),
        "a model without its own header must keep the provider-level one"
    );
}

// ---
// 5. model 级 baseUrl
// ---

/// model 级的 `baseUrl` 生效：请求去了那个地址，**不是** provider 自己的。
#[tokio::test]
async fn a_model_level_base_url_receives_the_request() {
    let provider_server = mock_server().await;
    let model_server = mock_server().await;
    let models = models_json(&format!(
        r#"{{
  "providers": {{
    "my-gw": {{
      "api": "openai-completions",
      "baseUrl": "{provider_base}",
      "apiKey": "the-key",
      "models": [
        {{"id": "some-model", "baseUrl": "{model_base}"}},
        {{"id": "other-model"}}
      ]
    }}
  }}
}}"#,
        provider_base = provider_server.uri(),
        model_base = model_server.uri()
    ));

    let provider = build_provider("my-gw", Vec::new(), &[], &[], None, Some(&models))
        .expect("the provider builds");
    send_one(&provider, "some-model").await;

    let model_requests = model_server.received_requests().await.unwrap();
    assert_eq!(
        model_requests.len(),
        1,
        "the request must go to the model-level base URL"
    );
    let provider_requests = provider_server.received_requests().await.unwrap();
    assert_eq!(
        provider_requests.len(),
        0,
        "the provider-level base URL must not receive the model-level request"
    );
}

/// 另一个模型没有自己的 base URL 时，还是走 provider 自己的地址。
#[tokio::test]
async fn a_model_without_its_own_base_url_uses_the_provider_level_one() {
    let provider_server = mock_server().await;
    let model_server = mock_server().await;
    let models = models_json(&format!(
        r#"{{
  "providers": {{
    "my-gw": {{
      "api": "openai-completions",
      "baseUrl": "{provider_base}",
      "apiKey": "the-key",
      "models": [
        {{"id": "some-model", "baseUrl": "{model_base}"}},
        {{"id": "other-model"}}
      ]
    }}
  }}
}}"#,
        provider_base = provider_server.uri(),
        model_base = model_server.uri()
    ));

    let provider = build_provider("my-gw", Vec::new(), &[], &[], None, Some(&models))
        .expect("the provider builds");
    send_one(&provider, "other-model").await;

    let provider_requests = provider_server.received_requests().await.unwrap();
    assert_eq!(
        provider_requests.len(),
        1,
        "a model without its own base URL must use the provider-level one"
    );
    let model_requests = model_server.received_requests().await.unwrap();
    assert_eq!(model_requests.len(), 0);
}

// ---
// 6. compat
// ---

/// `compat` 生效：请求体的形状变了。
///
/// 挑的是 nini 建模了的两个开关：
///
///   * `maxTokensField: "max_tokens"` → body 里是 `max_tokens`，没有
///     `max_completion_tokens`；
///   * `supportsStore: false` → body 里没有 `store`。
///
/// （Pi 的 `compat` 是三个方言的并集，三十多个字段；nini 只认这两个。）
#[tokio::test]
async fn compat_changes_the_shape_of_the_request_body() {
    let server = mock_server().await;
    let models = models_json(&format!(
        r#"{{
  "providers": {{
    "my-gw": {{
      "api": "openai-completions",
      "baseUrl": "{base}",
      "apiKey": "the-key",
      "compat": {{"maxTokensField": "max_tokens", "supportsStore": false}},
      "models": [{{"id": "some-model"}}]
    }}
  }}
}}"#,
        base = server.uri()
    ));

    let provider = build_provider("my-gw", Vec::new(), &[], &[], None, Some(&models))
        .expect("the provider builds");
    send_one(&provider, "some-model").await;

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body: serde_json::Value =
        serde_json::from_slice(&requests[0].body).expect("the body is JSON");
    assert_eq!(
        body.get("max_tokens").and_then(|v| v.as_u64()),
        Some(64),
        "the compat switch must change the field the budget travels in: {body}"
    );
    assert!(
        body.get("max_completion_tokens").is_none(),
        "the other budget field must not be sent as well: {body}"
    );
    assert!(
        body.get("store").is_none(),
        "`supportsStore: false` must keep the `store` field out: {body}"
    );
}

/// 没有 `compat` 时，body 保持 OpenAI 自己的形状（`max_completion_tokens`
/// 在、`store` 在）。
#[tokio::test]
async fn without_compat_the_body_keeps_openais_own_shape() {
    let server = mock_server().await;
    let models = models_json(&format!(
        r#"{{
  "providers": {{
    "my-gw": {{
      "api": "openai-completions",
      "baseUrl": "{base}",
      "apiKey": "the-key",
      "models": [{{"id": "some-model"}}]
    }}
  }}
}}"#,
        base = server.uri()
    ));

    let provider = build_provider("my-gw", Vec::new(), &[], &[], None, Some(&models))
        .expect("the provider builds");
    send_one(&provider, "some-model").await;

    let requests = server.received_requests().await.unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(&requests[0].body).expect("the body is JSON");
    assert_eq!(
        body.get("max_completion_tokens").and_then(|v| v.as_u64()),
        Some(64),
        "without a compat the body keeps OpenAI's own field: {body}"
    );
    assert_eq!(
        body.get("store").and_then(|v| v.as_bool()),
        Some(false),
        "without a compat the body keeps `store: false`: {body}"
    );
}

// ---
// 7. 宽松模式没被破坏
// ---

/// 未知的字段仍然被忽略——包括 Pi 有、nini 本轮**不做**的那几个
/// （`oauth` / `modelOverrides` / `cost` / `samplingParams`）。
///
/// 一个不认识的字段不该把整个文件变成空配置。
#[tokio::test]
async fn unknown_fields_are_still_ignored() {
    let server = mock_server().await;
    let models = models_json(&format!(
        r#"{{
  "providers": {{
    "my-gw": {{
      "api": "openai-completions",
      "baseUrl": "{base}",
      "apiKey": "the-key",
      "oauth": "radius",
      "modelOverrides": {{"some-model": {{"maxTokens": 1}}}},
      "futureField": {{"nested": true}},
      "models": [
        {{"id": "some-model", "cost": {{"input": 1}}, "samplingParams": {{"top_p": 0.9}}}}
      ]
    }}
  }}
}}"#,
        base = server.uri()
    ));

    let provider = build_provider("my-gw", Vec::new(), &[], &[], None, Some(&models))
        .expect("unknown fields must not break the file");
    send_one(&provider, "some-model").await;

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "the provider must still be usable");
}
