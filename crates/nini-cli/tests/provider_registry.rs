//! R5：provider 薄壳 —— **清单与实现必须一致**。
//!
//! # 这份文件防的是什么
//!
//! `/login` 列出来的 provider 和 `--provider` 能用的 provider 是两份名单。
//! 它们一旦漂移，用户看到的清单就在骗人：列了一个 provider，选它却得到
//! 「unknown provider」。
//!
//! 权威表在 `nini_ai::provider_registry`（只读），这份文件断言的是**实现**
//! 跟得上那张表：
//!
//! 1. 表里 28 个薄壳，每一个都能被 `build_provider` 构造出来；
//! 2. `provider_registry::all()`（= 原生实现 + 薄壳）里的每一个 id，都能
//!    被 `build_provider` 认识；
//! 3. 需要用户自己给 base URL 的 provider，在没有 base URL 时**显式拒绝**，
//!    而且消息里点名是哪个 provider；
//! 4. `{ID}_BASE_URL` 的覆盖真的生效（请求确实打到了那个地址）；
//! 5. Pi 的 `detectCompat` 移植与 Pi 逐条一致。
//!
//! # 为什么测的是 lib 目标里的那份代码
//!
//! nini-cli 的 bin 目标不链接 lib（`main.rs` 自己有一份 `provider_factory`），
//! 但两者是**同一份源码**，测的就是要发布的那份代码，不是副本。

use std::sync::Mutex;

use nini_ai::openai::MaxTokensField;
use nini_ai::openai_compat::detect_compat;
use nini_ai::provider_registry::{self, ProviderEntry};
use nini_cli::provider_factory;
use nini_core::provider::{ContentBlock, Message, Request, Role};
use nini_core::settings::{ModelsJson, ProviderConfig};

/// 改环境变量、跑一段代码、然后恢复。
///
/// 环境变量是**进程全局**的，而测试是并行的：两个测试同时改同一个变量，
/// 结论就会取决于谁先跑。所以这里既上锁，也把改动限制在一个闭包里。
fn with_env<T>(vars: &[(&str, Option<&str>)], f: impl FnOnce() -> T) -> T {
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let saved: Vec<(String, Option<String>)> = vars
        .iter()
        .map(|(name, _)| (name.to_string(), std::env::var(name).ok()))
        .collect();
    for (name, value) in vars {
        match value {
            Some(value) => std::env::set_var(name, value),
            None => std::env::remove_var(name),
        }
    }
    let out = f();
    for (name, value) in saved {
        match value {
            Some(value) => std::env::set_var(&name, value),
            None => std::env::remove_var(&name),
        }
    }
    out
}

/// 薄壳的 base URL 覆盖变量名：`{ID}_BASE_URL`（`-` 换成 `_`，大写）。
///
/// 这条规则写在这里（而不是调用实现里的函数）是有意的：测试要独立地说出
/// 规则，实现改了名字，这里就会红。
fn base_url_var(id: &str) -> String {
    format!("{}_BASE_URL", id.replace('-', "_").to_uppercase())
}

/// 每个薄壳需要的环境变量：
///
///   * 密钥变量 —— 给一个假值，让构造能走完（不发请求）；
///   * base URL 覆盖变量 —— 只有 `needs_custom_base_url` 的那几个需要，
///     其余的先清掉，免得开发者的 shell 决定结论。
fn shell_env(entries: &[&'static ProviderEntry]) -> Vec<(String, Option<String>)> {
    let mut vars: Vec<(String, Option<String>)> = Vec::new();
    for entry in entries {
        vars.push((entry.api_key_env.to_string(), Some("test-key".to_string())));
        vars.push((
            base_url_var(entry.id),
            if entry.needs_custom_base_url {
                Some("http://base.invalid/v1".to_string())
            } else {
                None
            },
        ));
    }
    vars
}

/// 清单里**每一个** provider 需要的环境变量（含原生实现）。
///
/// `openai-compat` 是唯一没有默认 base URL 的原生实现，它读的是
/// `OPENAI_BASE_URL`（不是 `{ID}_BASE_URL`），所以单独给。
fn registry_env() -> Vec<(String, Option<String>)> {
    let entries: Vec<&'static ProviderEntry> = provider_registry::all().collect();
    let mut vars = shell_env(&entries);
    vars.push((
        "OPENAI_BASE_URL".to_string(),
        Some("http://openai-compat.invalid/v1".to_string()),
    ));
    vars
}

/// 一个最小可用的请求。
fn a_request() -> Request {
    Request {
        model: "test-model".to_string(),
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "hi".to_string(),
            }],
            timestamp: 0,
        }],
        tools: Vec::new(),
        max_tokens: None,
        temperature: None,
        system: None,
        reasoning: None,
    }
}

/// 起一个 mock server，并让它对任何 POST 回 500。
///
/// 500 是故意的：我们要看的是**请求发到了哪里、带了什么头**，不是回答。
fn a_mock_server(runtime: &tokio::runtime::Runtime) -> wiremock::MockServer {
    let server = runtime.block_on(wiremock::MockServer::start());
    runtime.block_on(async {
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(500))
            .mount(&server)
            .await;
    });
    server
}

/// 把一个请求真的发出去，把响应丢掉。
fn send_one(
    runtime: &tokio::runtime::Runtime,
    provider: &std::sync::Arc<dyn nini_core::provider::Provider>,
) {
    runtime.block_on(async {
        use futures_util::StreamExt;
        let mut stream = provider.stream(a_request());
        while let Some(_event) = stream.next().await {}
    });
}

// ---------------------------------------------------------------------------
// 1. 28 个薄壳，每一个都能被构造出来
// ---------------------------------------------------------------------------

#[test]
fn every_thin_shell_builds_a_provider() {
    let entries: Vec<&'static ProviderEntry> = provider_registry::THIN_SHELLS.iter().collect();
    assert_eq!(entries.len(), 28, "Pi 的薄壳表是 28 个");

    let vars = shell_env(&entries);
    let vars: Vec<(&str, Option<&str>)> = vars
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_deref()))
        .collect();

    with_env(&vars, || {
        for entry in entries {
            let provider =
                provider_factory::build_provider(entry.id, Vec::new(), &[], &[], None, None)
                    .unwrap_or_else(|e| panic!("薄壳 {} 构造不出来: {e}", entry.id));
            assert!(
                !provider.name().is_empty(),
                "{} 构造出来了却没有名字",
                entry.id
            );
        }
    });
}

// ---------------------------------------------------------------------------
// 2. 清单与实现一致：`provider_registry::all()` 里的 id 都要被认识
// ---------------------------------------------------------------------------

#[test]
fn every_provider_in_the_registry_is_known_to_the_factory() {
    let vars = registry_env();
    let vars: Vec<(&str, Option<&str>)> = vars
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_deref()))
        .collect();

    with_env(&vars, || {
        for entry in provider_registry::all() {
            let provider = provider_factory::build_provider(
                entry.id,
                Vec::new(),
                &[],
                &[],
                Some("test-key"),
                None,
            )
            .unwrap_or_else(|e| panic!("清单里的 {} 构造不出来: {e}", entry.id));
            assert!(
                !provider.name().is_empty(),
                "{} 在清单里，却构造不出 provider",
                entry.id
            );
        }
    });
}

// ---------------------------------------------------------------------------
// 3. 需要自己给 base URL 的 provider：没有就显式拒绝，并点名
// ---------------------------------------------------------------------------

#[test]
fn a_thin_shell_without_a_base_url_is_refused_and_says_which_one() {
    let entries: Vec<&'static ProviderEntry> = provider_registry::THIN_SHELLS
        .iter()
        .filter(|e| e.needs_custom_base_url)
        .collect();
    assert!(
        !entries.is_empty(),
        "Pi 那几个 baseUrl 带占位符的 provider 必须在这张表里"
    );

    for entry in &entries {
        let var = base_url_var(entry.id);
        with_env(&[(&var, None)], || {
            let err = provider_factory::resolve_shell_base_url(entry)
                .expect_err("没有 base URL 时必须拒绝，而不是猜一个 URL");
            let message = err.to_string();
            assert!(
                message.contains(entry.id),
                "拒绝的消息要点名是哪个 provider: {message}"
            );
            assert!(
                message.contains(&var),
                "拒绝的消息要说怎么给（哪个环境变量）: {message}"
            );
        });
    }
}

#[test]
fn a_thin_shell_with_a_default_base_url_needs_nothing() {
    let entry = provider_registry::find("together").expect("together 在表里");
    let var = base_url_var("together");
    with_env(&[(&var, None)], || {
        let base = provider_factory::resolve_shell_base_url(entry)
            .expect("表里有默认 base URL 的薄壳不需要用户给");
        assert_eq!(base, "https://api.together.ai/v1");
    });
}

// ---------------------------------------------------------------------------
// 4. base URL 覆盖生效：请求确实打到了那个地址
// ---------------------------------------------------------------------------

#[test]
fn the_base_url_override_is_the_url_the_provider_talks_to() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let server = a_mock_server(&runtime);

    let base = server.uri();
    let vars = [
        ("TOGETHER_BASE_URL", Some(base.as_str())),
        ("TOGETHER_API_KEY", Some("key-from-the-environment")),
    ];
    with_env(&vars, || {
        let provider =
            provider_factory::build_provider("together", Vec::new(), &[], &[], None, None)
                .expect("薄壳能构造出 provider");
        send_one(&runtime, &provider);
    });

    let requests = runtime
        .block_on(server.received_requests())
        .expect("recorded requests");
    assert_eq!(requests.len(), 1, "请求应该打到覆盖后的 base URL");
    assert_eq!(
        requests[0].url.path(),
        "/v1/chat/completions",
        "覆盖后的 base URL 决定请求的路径"
    );
    assert_eq!(
        requests[0]
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok()),
        Some("Bearer key-from-the-environment"),
        "密钥来自 provider 自己的环境变量"
    );
}

#[test]
fn a_thin_shell_declared_in_models_json_falls_back_to_the_provider_env_var() {
    // 用户在 `models.json` 里声明了一个薄壳，却没写 `apiKey`：Pi 仍会去找
    // 那个 provider 的环境变量，nini 也要找得到。
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let server = a_mock_server(&runtime);

    let base = server.uri();
    let mut config = ProviderConfig::default();
    config.base_url = Some(base.clone());
    config.api = Some("openai-completions".to_string());
    let mut models = ModelsJson::default();
    models.providers.insert("together".to_string(), config);

    with_env(
        &[("TOGETHER_API_KEY", Some("key-from-the-environment"))],
        || {
            let provider = provider_factory::build_provider(
                "together",
                Vec::new(),
                &[],
                &[],
                None,
                Some(&models),
            )
            .expect("models.json 里的薄壳能构造出 provider");
            send_one(&runtime, &provider);
        },
    );

    let requests = runtime
        .block_on(server.received_requests())
        .expect("recorded requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0]
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok()),
        Some("Bearer key-from-the-environment"),
        "文件里没写 apiKey 时，密钥来自 provider 的环境变量"
    );
}

// ---------------------------------------------------------------------------
// 5. `detectCompat` 的移植：与 Pi 逐条一致
// ---------------------------------------------------------------------------
//
// Pi 的源：`references/pi-0.85.1/src/api/openai-completions.ts:1577-1670`
// （`detectCompat`）。
//
// 判定只有两个字段进了 nini 的 `OpenAiCompat`：
// `supportsStore: !isNonStandard` 与 `maxTokensField`。

/// DeepSeek / Together / Z.ai / Moonshot 收 `max_tokens`，且拒绝 `store`。
#[test]
fn detect_compat_matches_pi_for_the_non_standard_providers() {
    for (provider, base) in [
        ("together", "https://api.together.ai/v1"),
        ("deepseek", "https://api.deepseek.com"),
        ("zai", "https://api.z.ai/api/coding/paas/v4"),
        ("moonshotai", "https://api.moonshot.ai/v1"),
    ] {
        let compat = detect_compat(provider, base);
        assert_eq!(
            compat.max_tokens_field,
            MaxTokensField::MaxTokens,
            "{provider}: Pi 把这些 provider 放在 `max_tokens` 上"
        );
        assert!(
            !compat.supports_store,
            "{provider}: Pi 的 `isNonStandard` 把这些 provider 排除在 `store` 之外"
        );
    }
}

/// OpenRouter 不在 Pi 的 `isNonStandard` 名单里，也不吃 `max_tokens`。
#[test]
fn detect_compat_matches_pi_for_openrouter() {
    let compat = detect_compat("openrouter", "https://openrouter.ai/api/v1");
    assert_eq!(
        compat.max_tokens_field,
        MaxTokensField::MaxCompletionTokens,
        "Pi 的 `useMaxTokens` 名单里没有 openrouter"
    );
    assert!(
        compat.supports_store,
        "Pi 的 `isNonStandard` 名单里没有 openrouter"
    );
}

/// 判定看的是 **provider 名 + base URL**：换个名字指向同一个地址，判定不变。
#[test]
fn detect_compat_also_looks_at_the_base_url() {
    // Pi: `baseUrl.includes("api.together.ai")` —— provider 名不认识也照样命中。
    let compat = detect_compat("my-gateway", "https://api.together.ai/v1");
    assert_eq!(compat.max_tokens_field, MaxTokensField::MaxTokens);
    assert!(!compat.supports_store);

    // Pi: `baseUrl.toLowerCase().includes("deepseek.com")` —— 只有这一条转小写。
    let compat = detect_compat("my-gateway", "https://API.DEEPSEEK.COM/v1");
    assert_eq!(compat.max_tokens_field, MaxTokensField::MaxTokens);
    assert!(!compat.supports_store);
}

/// 一个既不认识、地址也不沾边的 provider：就是 OpenAI 自己。
#[test]
fn detect_compat_leaves_an_unknown_endpoint_alone() {
    let compat = detect_compat("my-gateway", "http://localhost:1234/v1");
    assert_eq!(compat.max_tokens_field, MaxTokensField::MaxCompletionTokens);
    assert!(compat.supports_store);
}

/// Pi 的 `isNonStandard` 里还有几个 nini 表上的 provider，一并锁住。
#[test]
fn detect_compat_matches_pi_for_the_other_non_standard_providers() {
    for (provider, base) in [
        ("nvidia", "https://integrate.api.nvidia.com/v1"),
        ("cerebras", "https://api.cerebras.ai/v1"),
        ("xai", "https://api.x.ai/v1"),
        ("ant-ling", "https://api.ant-ling.com/v1"),
        ("opencode", "https://opencode.ai/zen/v1"),
        ("moonshotai-cn", "https://api.moonshot.cn/v1"),
        (
            "zai-coding-cn",
            "https://open.bigmodel.cn/api/coding/paas/v4",
        ),
    ] {
        let compat = detect_compat(provider, base);
        assert!(
            !compat.supports_store,
            "{provider}: Pi 的 `isNonStandard` 里有它"
        );
    }
    // 这几个在 `useMaxTokens` 名单里，所以收 `max_tokens`。
    for (provider, base) in [
        ("nvidia", "https://integrate.api.nvidia.com/v1"),
        ("ant-ling", "https://api.ant-ling.com/v1"),
        ("moonshotai-cn", "https://api.moonshot.cn/v1"),
        (
            "zai-coding-cn",
            "https://open.bigmodel.cn/api/coding/paas/v4",
        ),
    ] {
        let compat = detect_compat(provider, base);
        assert_eq!(
            compat.max_tokens_field,
            MaxTokensField::MaxTokens,
            "{provider}: Pi 的 `useMaxTokens` 名单里有它"
        );
    }
    // 而 cerebras / xai / opencode 不在 `useMaxTokens` 名单里。
    for (provider, base) in [
        ("cerebras", "https://api.cerebras.ai/v1"),
        ("xai", "https://api.x.ai/v1"),
        ("opencode", "https://opencode.ai/zen/v1"),
    ] {
        let compat = detect_compat(provider, base);
        assert_eq!(
            compat.max_tokens_field,
            MaxTokensField::MaxCompletionTokens,
            "{provider}: Pi 的 `useMaxTokens` 名单里没有它"
        );
    }
}
