//! LLM provider configuration.
//!
//! Deliberately abstract: today it resolves a BYO (bring-your-own) Anthropic key
//! and points at the public API, but a future hosted "UMIDE Pro" tier or a
//! self-hosted gateway is just a different `base_url`/key here — the agent loop,
//! tools, and UI never change.

use crate::error::AgentError;
use crate::types::{DEFAULT_MODEL, FAST_MODEL};

#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    /// Cheaper model for quick auxiliary calls (vision triage, classification).
    pub fast_model: String,
    pub max_tokens: u32,
    /// `output_config.effort`: "low".."xhigh".."max". `xhigh` suits coding/agentic.
    pub effort: Option<String>,
    /// Whether to request adaptive thinking on the main loop.
    pub thinking: bool,
}

impl ProviderConfig {
    /// Resolve a key from `ANTHROPIC_API_KEY`. Callers that have a key from
    /// settings/keychain should use [`ProviderConfig::with_key`] instead.
    pub fn from_env() -> Result<Self, AgentError> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .ok()
            .filter(|k| !k.trim().is_empty())
            .ok_or(AgentError::MissingApiKey)?;
        Ok(Self::with_key(api_key))
    }

    /// Resolution order the editor should use: explicit settings/keychain key,
    /// then `ANTHROPIC_API_KEY`. Returns `MissingApiKey` if neither is present
    /// so the panel can show the "add your key" empty state.
    pub fn resolve(configured_key: Option<String>) -> Result<Self, AgentError> {
        if let Some(key) = configured_key.filter(|k| !k.trim().is_empty()) {
            return Ok(Self::with_key(key));
        }
        Self::from_env()
    }

    pub fn with_key(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: "https://api.anthropic.com".to_string(),
            model: DEFAULT_MODEL.to_string(),
            fast_model: FAST_MODEL.to_string(),
            // Opus 4.8 supports 128k output with streaming; 64k is ample headroom.
            max_tokens: 64_000,
            effort: Some("xhigh".to_string()),
            thinking: true,
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_key_uses_expected_defaults() {
        let p = ProviderConfig::with_key("k");
        assert_eq!(p.model, "claude-opus-4-8");
        assert_eq!(p.fast_model, "claude-haiku-4-5");
        assert_eq!(p.effort.as_deref(), Some("xhigh"));
        assert_eq!(p.max_tokens, 64_000);
        assert!(p.thinking);
        assert_eq!(p.base_url, "https://api.anthropic.com");
    }

    #[test]
    fn resolve_prefers_configured_key() {
        let p = ProviderConfig::resolve(Some("explicit".into())).unwrap();
        assert_eq!(p.api_key, "explicit");
    }

    #[test]
    fn resolve_rejects_blank_configured_key_when_no_env() {
        // Only assert the blank path when the env var isn't providing a fallback.
        if std::env::var("ANTHROPIC_API_KEY").is_err() {
            assert!(matches!(
                ProviderConfig::resolve(Some("   ".into())),
                Err(AgentError::MissingApiKey)
            ));
        }
    }
}
