//! LLM provider configuration (bring-your-own key, multi-provider).

use crate::error::AgentError;

/// The supported providers. OpenAI, DeepSeek, and Gemini all use the
/// OpenAI-compatible backend; only base URL, model, and key differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Anthropic,
    OpenAi,
    DeepSeek,
    Gemini,
}

impl ProviderKind {
    pub fn label(self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "Claude",
            ProviderKind::OpenAi => "OpenAI",
            ProviderKind::DeepSeek => "DeepSeek",
            ProviderKind::Gemini => "Gemini",
        }
    }

    pub fn env_var(self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "ANTHROPIC_API_KEY",
            ProviderKind::OpenAi => "OPENAI_API_KEY",
            ProviderKind::DeepSeek => "DEEPSEEK_API_KEY",
            ProviderKind::Gemini => "GEMINI_API_KEY",
        }
    }

    pub fn default_base_url(self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "https://api.anthropic.com",
            ProviderKind::OpenAi => "https://api.openai.com/v1",
            ProviderKind::DeepSeek => "https://api.deepseek.com/v1",
            // Gemini's OpenAI-compatible surface.
            ProviderKind::Gemini => {
                "https://generativelanguage.googleapis.com/v1beta/openai"
            }
        }
    }

    pub fn default_model(self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "claude-opus-4-8",
            ProviderKind::OpenAi => "gpt-4o",
            ProviderKind::DeepSeek => "deepseek-chat",
            ProviderKind::Gemini => "gemini-2.0-flash",
        }
    }

    pub fn all() -> [ProviderKind; 4] {
        [
            ProviderKind::Anthropic,
            ProviderKind::OpenAi,
            ProviderKind::DeepSeek,
            ProviderKind::Gemini,
        ]
    }
}

#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub max_tokens: u32,
    /// `output_config.effort` — Anthropic only.
    pub effort: Option<String>,
    /// Request adaptive thinking — Anthropic only.
    pub thinking: bool,
}

impl ProviderConfig {
    pub fn new(kind: ProviderKind, api_key: impl Into<String>) -> Self {
        let anthropic = kind == ProviderKind::Anthropic;
        Self {
            kind,
            api_key: api_key.into(),
            base_url: kind.default_base_url().to_string(),
            model: kind.default_model().to_string(),
            // Anthropic streams up to 128k; others are smaller. 16k/8k is ample
            // headroom for a single assistant turn.
            max_tokens: if anthropic { 16_000 } else { 8_192 },
            effort: anthropic.then(|| "xhigh".to_string()),
            thinking: anthropic,
        }
    }

    /// Resolve a key for `kind`: an explicit settings/keychain key first, then
    /// the provider's environment variable. `MissingApiKey` if neither is set.
    pub fn resolve(
        kind: ProviderKind,
        configured_key: Option<String>,
    ) -> Result<Self, AgentError> {
        if let Some(key) = configured_key.filter(|k| !k.trim().is_empty()) {
            return Ok(Self::new(kind, key));
        }
        let env = std::env::var(kind.env_var())
            .ok()
            .filter(|k| !k.trim().is_empty())
            .ok_or(AgentError::MissingApiKey)?;
        Ok(Self::new(kind, env))
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_defaults() {
        let p = ProviderConfig::new(ProviderKind::Anthropic, "k");
        assert_eq!(p.model, "claude-opus-4-8");
        assert_eq!(p.base_url, "https://api.anthropic.com");
        assert_eq!(p.effort.as_deref(), Some("xhigh"));
        assert!(p.thinking);
    }

    #[test]
    fn openai_family_defaults() {
        let o = ProviderConfig::new(ProviderKind::OpenAi, "k");
        assert_eq!(o.model, "gpt-4o");
        assert!(o.base_url.contains("openai.com"));
        assert!(o.effort.is_none());
        assert!(!o.thinking);

        let d = ProviderConfig::new(ProviderKind::DeepSeek, "k");
        assert_eq!(d.model, "deepseek-chat");
        assert!(d.base_url.contains("deepseek.com"));

        let g = ProviderConfig::new(ProviderKind::Gemini, "k");
        assert_eq!(g.model, "gemini-2.0-flash");
        assert!(g.base_url.contains("generativelanguage"));
    }

    #[test]
    fn resolve_prefers_configured_key() {
        let p =
            ProviderConfig::resolve(ProviderKind::OpenAi, Some("explicit".into()))
                .unwrap();
        assert_eq!(p.api_key, "explicit");
        assert_eq!(p.kind, ProviderKind::OpenAi);
    }

    #[test]
    fn resolve_rejects_blank_key_when_no_env() {
        if std::env::var("DEEPSEEK_API_KEY").is_err() {
            assert!(matches!(
                ProviderConfig::resolve(ProviderKind::DeepSeek, Some("  ".into())),
                Err(AgentError::MissingApiKey)
            ));
        }
    }
}
