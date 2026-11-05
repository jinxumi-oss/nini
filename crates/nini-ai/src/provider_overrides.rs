//! model 级覆盖。
//!
//! 这一模块承载 **provider 级 / model 级请求头与认证头** 的配置形态，
//! 以及三个 provider（OpenAI Chat Completions、Anthropic Messages、
//! OpenAI Responses）共用的合并规则。
//!
//! 为什么载体在 provider 上而不在 `nini_core::provider::Request` 上：
//! `Request` 有 30 个构造点，没有一个用 `..Default::default()`，其中
//! 20 个在 oracle / 测试文件里。给 `Request` 加字段会打破这 20 个
//! 构造点，而项目的纪律是「oracle 只能引用修复前后都存在的 API」。
//! 所以这些配置挂在 provider 构造上，请求时再按 `req.model` 查表。

use reqwest::RequestBuilder;

/// model 级覆盖。
/// 一个模型可以有自己的请求头和 base URL。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelOverride {
    /// 这个模型专属的请求头。
    pub headers: Vec<(String, String)>,
    /// 这个模型专属的 base URL（有它就不用 provider 自己的）。
    pub base_url: Option<String>,
}

/// provider 级与 model 级的请求头配置。
///
/// 三个 provider 共用同一份配置与同一套合并规则，因此语义天然一致。
#[derive(Debug, Clone, Default)]
pub struct HeaderOverrides {
    /// provider 级自定义请求头：附加到**每个**请求上。
    pub headers: Vec<(String, String)>,
    /// 认证头的名字。`None` 表示用各 provider 的默认写法
    /// （OpenAI 系是 `Authorization: Bearer <key>`，Anthropic 是
    /// `x-api-key: <key>`）；`Some(name)` 时用这个名字发**裸值**的 key
    /// （不带 `Bearer` 前缀）。
    pub auth_header: Option<&'static str>,
    /// model 级覆盖表，键是模型名（`req.model` 的原样值）。
    pub model_overrides: std::collections::BTreeMap<String, ModelOverride>,
}

impl HeaderOverrides {
    /// 该模型专属的 base URL（`None` 表示用 provider 自己的）。
    pub fn base_url_for(&self, model: &str) -> Option<&str> {
        self.model_overrides
            .get(model)
            .and_then(|o| o.base_url.as_deref())
    }

    /// 该模型专属的请求头。
    pub fn model_headers_for(&self, model: &str) -> &[(String, String)] {
        self.model_overrides
            .get(model)
            .map(|o| o.headers.as_slice())
            .unwrap_or(&[])
    }

    /// 合并出最终要发的请求头。
    ///
    /// 顺序即优先级：**provider 默认头 → provider 级自定义头 →
    /// model 级自定义头**，同名（大小写不敏感）后者覆盖前者。
    pub fn resolve(&self, model: &str, defaults: &[(String, String)]) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for (name, value) in defaults
            .iter()
            .chain(self.headers.iter())
            .chain(self.model_headers_for(model).iter())
        {
            let lower = name.to_ascii_lowercase();
            match out
                .iter_mut()
                .find(|(existing, _)| existing.to_ascii_lowercase() == lower)
            {
                Some(slot) => slot.1 = value.clone(),
                None => out.push((name.clone(), value.clone())),
            }
        }
        out
    }
}

/// 把合并好的请求头写进请求。
pub fn apply_headers(builder: RequestBuilder, headers: &[(String, String)]) -> RequestBuilder {
    headers
        .iter()
        .fold(builder, |acc, (name, value)| acc.header(name, value))
}
