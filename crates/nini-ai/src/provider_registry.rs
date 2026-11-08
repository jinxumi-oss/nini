//! Pi 0.85.1 的 provider 目录 → nini 的**薄壳表**（R5）。
//!
//! # 为什么有这张表
//!
//! Pi 0.85.1 的 `builtinProviders()` 返回 **40** 个 provider，但**它们的协议方言
//! 高度集中**：39 个有静态目录的 provider 里，
//!
//! * **9** 个 nini 已有原生实现（anthropic / openai / openai-responses / openai-compat / google / deepseek / groq / mistral / cohere）；
//! * **5** 个需要新协议或 OAuth，**本轮明确不做**：amazon-bedrock（AWS SigV4）、
//!   google-vertex（Google OAuth + 项目 ID）、openai-codex（OAuth）、
//!   azure-openai-responses（Azure 方言）、github-copilot（OAuth）；
//! * 剩下 **28** 个用的都是 nini **已经实现**的方言 —— 它们缺的只是
//!   「名字 → base URL + 密钥环境变量」这条映射。
//!
//! 换句话说：**「补齐 provider」的成本不在协议，在配置。**
//!
//! # 数据来源（可复算）
//!
//! * **方言与 base URL**：Pi 的模型目录 `pi-ai/dist/models.generated.js` 的 `MODELS`
//!   ——每个 provider 的每个模型都带 `api` 与 `baseUrl`；这里取**该 provider 模型数最多的
//!   那个方言**作为默认方言，取该方言下出现最多的 baseUrl。
//! * **密钥环境变量**：Pi 的 `pi-ai/dist/env-api-keys.js` 的 `getApiKeyEnvVars()`。
//!   **逐条抄，不自己发明**（例如 `huggingface` → `HF_TOKEN` 而不是 `HUGGINGFACE_API_KEY`；
//!   `google` → `GEMINI_API_KEY`；`vercel-ai-gateway` → `AI_GATEWAY_API_KEY`）。
//!
//! # 两处需要用户自己给值
//!
//! * `needs_custom_base_url: true` —— Pi 的 baseUrl 里带占位符
//!   （`{CLOUDFLARE_ACCOUNT_ID}` 等），静态表填不了；这类 provider 必须由用户通过
//!   `models.json` 或 `{PROVIDER}_BASE_URL` 给出 base URL，否则**显式拒绝并说明**
//!   （不接受「静默失败」）。
//! * 密钥一律从环境变量读；**nini 不自建凭据存储**（R4 的结论）。
//!
//! # 边界
//!
//! 这张表只回答「**这个 provider 叫什么、用什么方言、base URL 是什么、密钥放哪**」。
//! 它**不**回答「这个 provider 怎么在 wire 上说话」——那是
//! `openai_compat::with_compat` / `anthropic` / `openai_responses` 各自的事。

/// Pi 的 OpenAI 兼容方言（`/v1/chat/completions`）。
pub const API_OPENAI_COMPLETIONS: &str = "openai-completions";
/// Pi 的 Anthropic 兼容方言（`/v1/messages`）。
pub const API_ANTHROPIC_MESSAGES: &str = "anthropic-messages";
/// Pi 的 OpenAI Responses 方言（`/v1/responses`）。
pub const API_OPENAI_RESPONSES: &str = "openai-responses";

/// 一家 provider 的「名字 → 方言 + base URL + 密钥环境变量」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderEntry {
    /// `--provider` 用的名字（与 Pi 的 provider id 逐字相同）。
    pub id: &'static str,
    /// 该 provider 用的协议方言，见本文件的 `API_*` 常量。
    pub api: &'static str,
    /// 默认 base URL。`None` 表示必须由用户自己给（见 `needs_custom_base_url`）。
    pub base_url: Option<&'static str>,
    /// 密钥放哪个环境变量（与 Pi 的 `getApiKeyEnvVars()` 逐字相同）。
    pub api_key_env: &'static str,
    /// `true` 表示 Pi 的 baseUrl 带占位符，静态表填不了。
    pub needs_custom_base_url: bool,
}

/// 28 个薄壳：方言 nini 已实现，缺的只是配置。
pub static THIN_SHELLS: &[ProviderEntry] = &[
    ProviderEntry {
        id: "ant-ling",
        api: API_OPENAI_COMPLETIONS,
        base_url: Some("https://api.ant-ling.com/v1"),
        api_key_env: "ANT_LING_API_KEY",
        needs_custom_base_url: false,
    },
    ProviderEntry {
        id: "baseten",
        api: API_OPENAI_COMPLETIONS,
        base_url: Some("https://inference.baseten.co/v1"),
        api_key_env: "BASETEN_API_KEY",
        needs_custom_base_url: false,
    },
    ProviderEntry {
        id: "cerebras",
        api: API_OPENAI_COMPLETIONS,
        base_url: Some("https://api.cerebras.ai/v1"),
        api_key_env: "CEREBRAS_API_KEY",
        needs_custom_base_url: false,
    },
    ProviderEntry {
        id: "cloudflare-ai-gateway",
        api: API_OPENAI_RESPONSES,
        base_url: None,
        api_key_env: "CLOUDFLARE_API_KEY",
        needs_custom_base_url: true,
    },
    ProviderEntry {
        id: "cloudflare-workers-ai",
        api: API_OPENAI_COMPLETIONS,
        base_url: None,
        api_key_env: "CLOUDFLARE_API_KEY",
        needs_custom_base_url: true,
    },
    ProviderEntry {
        id: "fireworks",
        api: API_ANTHROPIC_MESSAGES,
        base_url: Some("https://api.fireworks.ai/inference"),
        api_key_env: "FIREWORKS_API_KEY",
        needs_custom_base_url: false,
    },
    ProviderEntry {
        id: "huggingface",
        api: API_OPENAI_COMPLETIONS,
        base_url: Some("https://router.huggingface.co/v1"),
        api_key_env: "HF_TOKEN",
        needs_custom_base_url: false,
    },
    ProviderEntry {
        id: "kimi-coding",
        api: API_ANTHROPIC_MESSAGES,
        base_url: Some("https://api.kimi.com/coding"),
        api_key_env: "KIMI_API_KEY",
        needs_custom_base_url: false,
    },
    ProviderEntry {
        id: "minimax",
        api: API_ANTHROPIC_MESSAGES,
        base_url: Some("https://api.minimax.io/anthropic"),
        api_key_env: "MINIMAX_API_KEY",
        needs_custom_base_url: false,
    },
    ProviderEntry {
        id: "minimax-cn",
        api: API_ANTHROPIC_MESSAGES,
        base_url: Some("https://api.minimaxi.com/anthropic"),
        api_key_env: "MINIMAX_CN_API_KEY",
        needs_custom_base_url: false,
    },
    ProviderEntry {
        id: "moonshotai",
        api: API_OPENAI_COMPLETIONS,
        base_url: Some("https://api.moonshot.ai/v1"),
        api_key_env: "MOONSHOT_API_KEY",
        needs_custom_base_url: false,
    },
    ProviderEntry {
        id: "moonshotai-cn",
        api: API_OPENAI_COMPLETIONS,
        base_url: Some("https://api.moonshot.cn/v1"),
        api_key_env: "MOONSHOT_API_KEY",
        needs_custom_base_url: false,
    },
    ProviderEntry {
        id: "nvidia",
        api: API_OPENAI_COMPLETIONS,
        base_url: Some("https://integrate.api.nvidia.com/v1"),
        api_key_env: "NVIDIA_API_KEY",
        needs_custom_base_url: false,
    },
    ProviderEntry {
        id: "opencode",
        api: API_OPENAI_RESPONSES,
        base_url: Some("https://opencode.ai/zen/v1"),
        api_key_env: "OPENCODE_API_KEY",
        needs_custom_base_url: false,
    },
    ProviderEntry {
        id: "opencode-go",
        api: API_OPENAI_COMPLETIONS,
        base_url: Some("https://opencode.ai/zen/go/v1"),
        api_key_env: "OPENCODE_API_KEY",
        needs_custom_base_url: false,
    },
    ProviderEntry {
        id: "openrouter",
        api: API_OPENAI_COMPLETIONS,
        base_url: Some("https://openrouter.ai/api/v1"),
        api_key_env: "OPENROUTER_API_KEY",
        needs_custom_base_url: false,
    },
    ProviderEntry {
        id: "qwen-token-plan",
        api: API_OPENAI_COMPLETIONS,
        base_url: Some("https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1"),
        api_key_env: "QWEN_TOKEN_PLAN_API_KEY",
        needs_custom_base_url: false,
    },
    ProviderEntry {
        id: "qwen-token-plan-cn",
        api: API_OPENAI_COMPLETIONS,
        base_url: Some("https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1"),
        api_key_env: "QWEN_TOKEN_PLAN_CN_API_KEY",
        needs_custom_base_url: false,
    },
    ProviderEntry {
        id: "qwen-token-plan-individual",
        api: API_OPENAI_COMPLETIONS,
        base_url: Some("https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1"),
        api_key_env: "QWEN_TOKEN_PLAN_API_KEY",
        needs_custom_base_url: false,
    },
    ProviderEntry {
        id: "together",
        api: API_OPENAI_COMPLETIONS,
        base_url: Some("https://api.together.ai/v1"),
        api_key_env: "TOGETHER_API_KEY",
        needs_custom_base_url: false,
    },
    ProviderEntry {
        id: "vercel-ai-gateway",
        api: API_ANTHROPIC_MESSAGES,
        base_url: Some("https://ai-gateway.vercel.sh"),
        api_key_env: "AI_GATEWAY_API_KEY",
        needs_custom_base_url: false,
    },
    ProviderEntry {
        id: "xai",
        api: API_OPENAI_RESPONSES,
        base_url: Some("https://api.x.ai/v1"),
        api_key_env: "XAI_API_KEY",
        needs_custom_base_url: false,
    },
    ProviderEntry {
        id: "xiaomi",
        api: API_OPENAI_COMPLETIONS,
        base_url: Some("https://api.xiaomimimo.com/v1"),
        api_key_env: "XIAOMI_API_KEY",
        needs_custom_base_url: false,
    },
    ProviderEntry {
        id: "xiaomi-token-plan-ams",
        api: API_OPENAI_COMPLETIONS,
        base_url: Some("https://token-plan-ams.xiaomimimo.com/v1"),
        api_key_env: "XIAOMI_TOKEN_PLAN_AMS_API_KEY",
        needs_custom_base_url: false,
    },
    ProviderEntry {
        id: "xiaomi-token-plan-cn",
        api: API_OPENAI_COMPLETIONS,
        base_url: Some("https://token-plan-cn.xiaomimimo.com/v1"),
        api_key_env: "XIAOMI_TOKEN_PLAN_CN_API_KEY",
        needs_custom_base_url: false,
    },
    ProviderEntry {
        id: "xiaomi-token-plan-sgp",
        api: API_OPENAI_COMPLETIONS,
        base_url: Some("https://token-plan-sgp.xiaomimimo.com/v1"),
        api_key_env: "XIAOMI_TOKEN_PLAN_SGP_API_KEY",
        needs_custom_base_url: false,
    },
    ProviderEntry {
        id: "zai",
        api: API_OPENAI_COMPLETIONS,
        base_url: Some("https://api.z.ai/api/coding/paas/v4"),
        api_key_env: "ZAI_API_KEY",
        needs_custom_base_url: false,
    },
    ProviderEntry {
        id: "zai-coding-cn",
        api: API_OPENAI_COMPLETIONS,
        base_url: Some("https://open.bigmodel.cn/api/coding/paas/v4"),
        api_key_env: "ZAI_CODING_CN_API_KEY",
        needs_custom_base_url: false,
    },
];

/// nini 已有原生实现的 provider（不是 Pi 的 provider id，是 nini 的 API 选择器）。
pub static NATIVES: &[ProviderEntry] = &[
    ProviderEntry {
        id: "anthropic",
        api: API_ANTHROPIC_MESSAGES,
        base_url: Some("https://api.anthropic.com"),
        api_key_env: "ANTHROPIC_API_KEY",
        needs_custom_base_url: false,
    },
    ProviderEntry {
        id: "openai",
        api: API_OPENAI_RESPONSES,
        base_url: Some("https://api.openai.com/v1"),
        api_key_env: "OPENAI_API_KEY",
        needs_custom_base_url: false,
    },
    ProviderEntry {
        id: "openai-responses",
        api: API_OPENAI_RESPONSES,
        base_url: Some("https://api.openai.com/v1"),
        api_key_env: "OPENAI_API_KEY",
        needs_custom_base_url: false,
    },
    ProviderEntry {
        id: "openai-compat",
        api: API_OPENAI_COMPLETIONS,
        base_url: None,
        api_key_env: "OPENAI_API_KEY",
        needs_custom_base_url: true,
    },
    ProviderEntry {
        id: "google",
        api: "google-generative-ai",
        base_url: Some("https://generativelanguage.googleapis.com/v1beta"),
        api_key_env: "GEMINI_API_KEY",
        needs_custom_base_url: false,
    },
    ProviderEntry {
        id: "deepseek",
        api: API_OPENAI_COMPLETIONS,
        base_url: Some("https://api.deepseek.com"),
        api_key_env: "DEEPSEEK_API_KEY",
        needs_custom_base_url: false,
    },
    ProviderEntry {
        id: "groq",
        api: API_OPENAI_COMPLETIONS,
        base_url: Some("https://api.groq.com/openai/v1"),
        api_key_env: "GROQ_API_KEY",
        needs_custom_base_url: false,
    },
    ProviderEntry {
        id: "mistral",
        api: "mistral-conversations",
        base_url: Some("https://api.mistral.ai"),
        api_key_env: "MISTRAL_API_KEY",
        needs_custom_base_url: false,
    },
    ProviderEntry {
        id: "cohere",
        api: API_OPENAI_COMPLETIONS,
        base_url: Some("https://api.cohere.com/v2"),
        api_key_env: "COHERE_API_KEY",
        needs_custom_base_url: false,
    },
];

/// 按名字找一家 provider（先查薄壳，再查原生实现）。
pub fn find(id: &str) -> Option<&'static ProviderEntry> {
    THIN_SHELLS
        .iter()
        .chain(NATIVES.iter())
        .find(|e| e.id == id)
}

/// 这个名字是不是一个薄壳。
pub fn is_thin_shell(id: &str) -> bool {
    THIN_SHELLS.iter().any(|e| e.id == id)
}

/// `/login` 要列的全部 provider：原生实现 + 薄壳。
pub fn all() -> impl Iterator<Item = &'static ProviderEntry> {
    NATIVES.iter().chain(THIN_SHELLS.iter())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_thin_shell_count_is_28() {
        assert_eq!(
            THIN_SHELLS.len(),
            28,
            "Pi 的 39 个有目录的 provider 里，9 个已原生实现、5 个明确不做，剩下的应当是 28 个薄壳"
        );
    }

    #[test]
    fn every_id_is_unique() {
        let mut ids: Vec<&str> = all().map(|e| e.id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "provider id 重复了");
    }

    #[test]
    fn every_shell_names_a_real_api_and_env_var() {
        for e in THIN_SHELLS {
            assert!(
                matches!(
                    e.api,
                    API_OPENAI_COMPLETIONS | API_ANTHROPIC_MESSAGES | API_OPENAI_RESPONSES
                ),
                "{}: 未知方言 {}",
                e.id,
                e.api
            );
            assert!(
                e.api_key_env.ends_with("_API_KEY") || e.api_key_env == "HF_TOKEN",
                "{}: 密钥环境变量不像 Pi 的约定: {}",
                e.id,
                e.api_key_env
            );
        }
    }

    #[test]
    fn a_placeholder_base_url_is_never_baked_in() {
        for e in THIN_SHELLS {
            if e.needs_custom_base_url {
                assert!(
                    e.base_url.is_none(),
                    "{}: 带占位符的 base URL 不能进静态表",
                    e.id
                );
            }
        }
    }
}
