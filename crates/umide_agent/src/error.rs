use thiserror::Error;

/// Errors surfaced by the agent engine. These are returned to the UI so the
/// assistant panel can show an actionable message instead of crashing — the
/// rest of the IDE (editor, emulator panels) is never affected by an agent error.
#[derive(Debug, Error)]
pub enum AgentError {
    #[error("no Anthropic API key: set ANTHROPIC_API_KEY or add a key in settings")]
    MissingApiKey,

    #[error("network error talking to Anthropic: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Anthropic API error (status {status}): {body}")]
    Api { status: u16, body: String },

    #[error("failed to (de)serialize Anthropic payload: {0}")]
    Json(#[from] serde_json::Error),

    #[error("agent stopped after exceeding the {0}-step safety limit")]
    MaxIterations(u32),
}
