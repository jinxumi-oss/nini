//! provider 级 / model 级请求头与认证头。
//!
//! Pi 的 provider / model 配置里有 `headers` 与 `authHeader` 两个字段，
//! 但 nini 侧原先没有承载点：声明了就被 serde 丢掉，请求上什么也看不到。
//! 这一组测试用 wiremock 真起一个 HTTP 服务器，**看服务器收到的请求**，
//! 而不是看中间变量 —— 请求头这种东西，只有落到线上才算数。
//!
//! 三个 provider（OpenAI Chat Completions、Anthropic Messages、OpenAI
//! Responses）共用同一套配置与合并规则，所以这里每个用例都对三者各跑
//! 一遍：语义必须完全一致，差一个就是 bug。

use futures_core::Stream;
use futures_util::StreamExt;
use nini_ai::anthropic::AnthropicProvider;
use nini_ai::openai::OpenAiProvider;
use nini_ai::openai_responses::OpenAiResponsesProvider;
use nini_ai::provider_overrides::ModelOverride;
use nini_core::provider::{
    ContentBlock, Message, Provider, ProviderError, Request, Role, StreamEvent,
};
use std::collections::BTreeMap;
use std::pin::Pin;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// 测试用的 key。
const KEY: &str = "test-key";

/// 一个待测的 provider：名字、期望的请求路径、实例本身。
struct Case {
    name: &'static str,
    path: &'static str,
    provider: Box<dyn Provider>,
}

/// 用同一套配置构造三个 provider。
///
/// 参数对三者完全一样，这是「语义一致」的测试前提：任何差异都只可能
/// 来自 provider 自己，而不是来自配置。
fn cases(
    base: &str,
    headers: Vec<(String, String)>,
    auth_header: Option<&'static str>,
    overrides: BTreeMap<String, ModelOverride>,
) -> Vec<Case> {
    let mut openai = OpenAiProvider::new(KEY)
        .with_base_url(base)
        .with_headers(headers.clone())
        .with_model_overrides(overrides.clone());
    if let Some(name) = auth_header {
        openai = openai.with_auth_header(name);
    }

    let mut anthropic = AnthropicProvider::new(KEY)
        .with_base_url(base)
        .with_headers(headers.clone())
        .with_model_overrides(overrides.clone());
    if let Some(name) = auth_header {
        anthropic = anthropic.with_auth_header(name);
    }

    let mut responses = OpenAiResponsesProvider::new(KEY)
        .with_base_url(base)
        .with_headers(headers.clone())
        .with_model_overrides(overrides.clone());
    if let Some(name) = auth_header {
        responses = responses.with_auth_header(name);
    }

    vec![
        Case {
            name: "openai",
            path: "/v1/chat/completions",
            provider: Box::new(openai),
        },
        Case {
            name: "anthropic",
            path: "/v1/messages",
            provider: Box::new(anthropic),
        },
        Case {
            name: "openai-responses",
            path: "/v1/responses",
            provider: Box::new(responses),
        },
    ]
}

/// 一个平凡但合法的请求 —— 关注的是请求头，不是 body。
fn request(model: &str) -> Request {
    Request {
        model: model.to_string(),
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "hello".to_string(),
            }],
            timestamp: 0,
        }],
        tools: vec![],
        max_tokens: Some(256),
        temperature: None,
        system: None,
        reasoning: None,
    }
}

/// 一个接受任何路径的 POST mock。
async fn serve() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string(""))
        .mount(&server)
        .await;
    server
}

/// 跑完一个请求，把流里的事件丢掉 —— 只关心请求本身发出去了没有。
async fn drain(provider: &dyn Provider, req: Request) {
    let mut stream: Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send>> =
        provider.stream(req);
    while let Some(_event) = stream.next().await {}
}

/// 取出服务器收到的请求头（大小写不敏感，HTTP 头名本来就如此）。
fn header(req: &wiremock::Request, name: &str) -> Option<String> {
    req.headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string())
}

/// 服务器收到的所有请求。
async fn requests(server: &MockServer) -> Vec<wiremock::Request> {
    server
        .received_requests()
        .await
        .expect("wiremock did not record the requests")
}

/// 服务器收到的最后一个请求；一个都没有就说明请求根本没发出去。
async fn last_request(server: &MockServer) -> wiremock::Request {
    requests(server)
        .await
        .last()
        .cloned()
        .expect("the provider never sent a request")
}

// ---------------------------------------------------------------------------
// 1. provider 级自定义请求头真的上线
// ---------------------------------------------------------------------------

#[tokio::test]
async fn provider_level_headers_reach_the_request() {
    let server = serve().await;
    let uri = server.uri();

    for case in cases(
        &uri,
        vec![("x-custom".to_string(), "v1".to_string())],
        None,
        BTreeMap::new(),
    ) {
        drain(case.provider.as_ref(), request("test-model")).await;

        let received = last_request(&server).await;
        assert_eq!(
            header(&received, "x-custom").as_deref(),
            Some("v1"),
            "{}: the provider-level header never reached the server",
            case.name
        );
        assert_eq!(
            received.url.path(),
            case.path,
            "{}: the request went to the wrong endpoint",
            case.name
        );
    }
}

// ---------------------------------------------------------------------------
// 2. 自定义认证头：裸值，且不再有 Authorization
// ---------------------------------------------------------------------------

/// Azure 的 `api-key`、Google 的 `x-goog-api-key` 都是**裸值**，
/// 不是 `Bearer <key>`。所以设了认证头名字之后，既要发对名字，
/// 也不能再留着 `Authorization`。
#[tokio::test]
async fn a_custom_auth_header_carries_the_bare_key() {
    let server = serve().await;
    let uri = server.uri();

    for case in cases(&uri, vec![], Some("api-key"), BTreeMap::new()) {
        drain(case.provider.as_ref(), request("test-model")).await;

        let received = last_request(&server).await;
        assert_eq!(
            header(&received, "api-key").as_deref(),
            Some(KEY),
            "{}: the custom auth header did not carry the key",
            case.name
        );
        assert!(
            header(&received, "authorization").is_none(),
            "{}: the default Authorization header must be gone, not merged",
            case.name
        );
    }
}

// ---------------------------------------------------------------------------
// 3. 默认认证头：OpenAI 系 Bearer，Anthropic 裸 x-api-key
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_default_auth_header_is_the_dialects_own() {
    let server = serve().await;
    let uri = server.uri();

    for case in cases(&uri, vec![], None, BTreeMap::new()) {
        drain(case.provider.as_ref(), request("test-model")).await;

        let received = last_request(&server).await;
        match case.name {
            "anthropic" => {
                assert_eq!(
                    header(&received, "x-api-key").as_deref(),
                    Some(KEY),
                    "anthropic: the default auth header is x-api-key"
                );
                assert!(
                    header(&received, "authorization").is_none(),
                    "anthropic: there must be no Authorization header"
                );
            }
            _ => {
                assert_eq!(
                    header(&received, "authorization").as_deref(),
                    Some(format!("Bearer {KEY}").as_str()),
                    "{}: the default auth header is Authorization: Bearer <key>",
                    case.name
                );
                assert!(
                    header(&received, "x-api-key").is_none(),
                    "{}: there must be no x-api-key header",
                    case.name
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 4. model 级头覆盖 provider 级同名头
// ---------------------------------------------------------------------------

#[tokio::test]
async fn model_level_headers_win_over_provider_level_headers() {
    let server = serve().await;
    let uri = server.uri();

    let mut overrides = BTreeMap::new();
    overrides.insert(
        "special-model".to_string(),
        ModelOverride {
            headers: vec![("x-shared".to_string(), "from-model".to_string())],
            base_url: None,
            ..Default::default()
        },
    );

    for case in cases(
        &uri,
        vec![("x-shared".to_string(), "from-provider".to_string())],
        None,
        overrides.clone(),
    ) {
        // 覆盖表里的模型：model 级赢。
        drain(case.provider.as_ref(), request("special-model")).await;
        let received = last_request(&server).await;
        assert_eq!(
            header(&received, "x-shared").as_deref(),
            Some("from-model"),
            "{}: the model-level header must win over the provider-level one",
            case.name
        );

        // 覆盖表里没有的模型：provider 级照旧。
        drain(case.provider.as_ref(), request("other-model")).await;
        let received = last_request(&server).await;
        assert_eq!(
            header(&received, "x-shared").as_deref(),
            Some("from-provider"),
            "{}: a model outside the table must keep the provider-level header",
            case.name
        );
    }
}

// ---------------------------------------------------------------------------
// 5. model 级 base URL 生效
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_model_level_base_url_takes_over() {
    let provider_server = serve().await;
    let model_server = serve().await;

    let mut overrides = BTreeMap::new();
    overrides.insert(
        "special-model".to_string(),
        ModelOverride {
            headers: vec![],
            base_url: Some(model_server.uri()),
            ..Default::default()
        },
    );

    for case in cases(&provider_server.uri(), vec![], None, overrides.clone()) {
        drain(case.provider.as_ref(), request("special-model")).await;
    }
    assert_eq!(
        requests(&model_server).await.len(),
        3,
        "every provider must send the covered model to the model-level base URL"
    );
    assert!(
        requests(&provider_server).await.is_empty(),
        "the request must not go to the provider-level base URL"
    );

    // 覆盖表里没有的模型：走 provider 自己的 base URL。
    for case in cases(&provider_server.uri(), vec![], None, overrides.clone()) {
        drain(case.provider.as_ref(), request("other-model")).await;
    }
    assert_eq!(
        requests(&provider_server).await.len(),
        3,
        "a model outside the table must use the provider's base URL"
    );
    assert_eq!(
        requests(&model_server).await.len(),
        3,
        "a model outside the table must not go to the model-level base URL"
    );
}

// ---------------------------------------------------------------------------
// 6. model 级 base URL 依然走 dialect 的 URL 规则
// ---------------------------------------------------------------------------

/// model 级 base URL 不是「原样替换整条 URL」：它仍然是 base，
/// 后面照样要接 dialect 的路径（`/v1/chat/completions` 等）。
#[tokio::test]
async fn a_model_level_base_url_still_gets_the_dialect_path() {
    let provider_server = serve().await;
    let model_server = serve().await;

    let mut overrides = BTreeMap::new();
    overrides.insert(
        "special-model".to_string(),
        ModelOverride {
            headers: vec![],
            base_url: Some(model_server.uri()),
            ..Default::default()
        },
    );

    for case in cases(&provider_server.uri(), vec![], None, overrides.clone()) {
        drain(case.provider.as_ref(), request("special-model")).await;

        let received = requests(&model_server)
            .await
            .last()
            .cloned()
            .expect("the request must have reached the model-level base URL");
        assert_eq!(
            received.url.path(),
            case.path,
            "{}: the model-level base URL must still get the dialect path",
            case.name
        );
    }
}
