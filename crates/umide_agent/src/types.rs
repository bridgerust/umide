//! Wire types for the Anthropic Messages API (`POST /v1/messages`).
//!
//! Rust has no official Anthropic SDK, so these mirror the documented JSON
//! shapes directly. Field names and the `anthropic-version` value are pinned to
//! the current API; see <https://docs.claude.com> for the schema.

use serde::{Deserialize, Serialize};

/// Pinned API version header value.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Default model for the main agent loop (vision + tool use + 1M context).
pub const DEFAULT_MODEL: &str = "claude-opus-4-8";
/// Cheaper/faster model for quick side checks (e.g. "is this screenshot a crash?").
pub const FAST_MODEL: &str = "claude-haiku-4-5";

// ---------------------------------------------------------------------------
// Request
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct MessagesRequest {
    pub model: String,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub system: Vec<SystemBlock>,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Thinking>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_config: Option<OutputConfig>,
    pub stream: bool,
}

/// A `system` prompt block. The last block carries a cache breakpoint so the
/// (large, stable) tools + system prefix is served from cache on every turn.
#[derive(Debug, Clone, Serialize)]
pub struct SystemBlock {
    #[serde(rename = "type")]
    pub kind: &'static str, // always "text"
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

impl SystemBlock {
    pub fn cached(text: impl Into<String>) -> Self {
        Self {
            kind: "text",
            text: text.into(),
            cache_control: Some(CacheControl::ephemeral()),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CacheControl {
    #[serde(rename = "type")]
    pub kind: &'static str, // "ephemeral"
}

impl CacheControl {
    pub fn ephemeral() -> Self {
        Self { kind: "ephemeral" }
    }
}

/// Adaptive thinking — the only supported on-mode for Opus 4.7/4.8.
#[derive(Debug, Clone, Serialize)]
pub struct Thinking {
    #[serde(rename = "type")]
    pub kind: &'static str, // "adaptive"
}

impl Thinking {
    pub fn adaptive() -> Self {
        Self { kind: "adaptive" }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct OutputConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>, // "low" | "medium" | "high" | "xhigh" | "max"
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

// ---------------------------------------------------------------------------
// Messages & content blocks (used both request- and response-side)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

impl Message {
    pub fn user(content: Vec<ContentBlock>) -> Self {
        Self {
            role: Role::User,
            content,
        }
    }
    pub fn assistant(content: Vec<ContentBlock>) -> Self {
        Self {
            role: Role::Assistant,
            content,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    /// Returned by Opus 4.8 with adaptive thinking. Must be echoed back to the
    /// same model unchanged (including the `signature`) on the next turn.
    Thinking {
        thinking: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    RedactedThinking {
        data: String,
    },
    Image {
        source: ImageSource,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: Vec<ToolResultContent>,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
}

impl ContentBlock {
    pub fn text(text: impl Into<String>) -> Self {
        ContentBlock::Text { text: text.into() }
    }

    /// Build an image block from raw PNG bytes (e.g. an emulator screenshot via
    /// `DecodedFrame::to_png()`), so the agent can *see* the running app.
    pub fn image_png(png_bytes: &[u8]) -> Self {
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(png_bytes);
        ContentBlock::Image {
            source: ImageSource::Base64 {
                media_type: "image/png".to_string(),
                data,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageSource {
    Base64 { media_type: String, data: String },
    Url { url: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolResultContent {
    Text { text: String },
    Image { source: ImageSource },
}

impl ToolResultContent {
    pub fn text(text: impl Into<String>) -> Self {
        ToolResultContent::Text { text: text.into() }
    }
    pub fn image_png(png_bytes: &[u8]) -> Self {
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(png_bytes);
        ToolResultContent::Image {
            source: ImageSource::Base64 {
                media_type: "image/png".to_string(),
                data,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Streaming (SSE) events
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    MessageStart {
        message: StreamMessage,
    },
    ContentBlockStart {
        index: usize,
        content_block: ContentBlock,
    },
    ContentBlockDelta {
        index: usize,
        delta: ContentDelta,
    },
    ContentBlockStop {
        index: usize,
    },
    MessageDelta {
        delta: MessageDeltaBody,
        #[serde(default)]
        usage: Option<Usage>,
    },
    MessageStop,
    Ping,
    Error {
        error: ApiErrorBody,
    },
}

#[derive(Debug, Deserialize)]
pub struct StreamMessage {
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentDelta {
    TextDelta { text: String },
    ThinkingDelta { thinking: String },
    SignatureDelta { signature: String },
    InputJsonDelta { partial_json: String },
}

#[derive(Debug, Deserialize)]
pub struct MessageDeltaBody {
    #[serde(default)]
    pub stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ApiErrorBody {
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub message: String,
}

/// Token accounting. `cache_read_input_tokens > 0` confirms the prompt cache is
/// working — surface this in the panel so users see what their key spends.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
    #[serde(default)]
    pub cache_creation_input_tokens: u32,
    #[serde(default)]
    pub cache_read_input_tokens: u32,
}

impl Usage {
    pub fn merge(&mut self, other: &Usage) {
        // message_delta carries cumulative output tokens; the rest come once on
        // message_start. Take the max so repeated merges stay monotonic.
        self.input_tokens = self.input_tokens.max(other.input_tokens);
        self.output_tokens = self.output_tokens.max(other.output_tokens);
        self.cache_creation_input_tokens = self
            .cache_creation_input_tokens
            .max(other.cache_creation_input_tokens);
        self.cache_read_input_tokens = self
            .cache_read_input_tokens
            .max(other.cache_read_input_tokens);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_block_roundtrips() {
        let v = serde_json::to_value(ContentBlock::text("hi")).unwrap();
        assert_eq!(v["type"], "text");
        assert_eq!(v["text"], "hi");
        let back: ContentBlock = serde_json::from_value(v).unwrap();
        assert!(matches!(back, ContentBlock::Text { .. }));
    }

    #[test]
    fn image_block_is_base64_png() {
        let bytes = b"\x89PNG\r\n\x1a\n test-bytes";
        let v = serde_json::to_value(ContentBlock::image_png(bytes)).unwrap();
        assert_eq!(v["type"], "image");
        assert_eq!(v["source"]["type"], "base64");
        assert_eq!(v["source"]["media_type"], "image/png");
        use base64::Engine;
        let data = v["source"]["data"].as_str().unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(data)
            .unwrap();
        assert_eq!(decoded, bytes);
    }

    #[test]
    fn tool_result_omits_is_error_when_none() {
        let v = serde_json::to_value(ContentBlock::ToolResult {
            tool_use_id: "t1".into(),
            content: vec![ToolResultContent::text("ok")],
            is_error: None,
        })
        .unwrap();
        assert_eq!(v["type"], "tool_result");
        assert_eq!(v["tool_use_id"], "t1");
        assert!(v.get("is_error").is_none());
    }

    #[test]
    fn parses_streamed_text_delta() {
        let json = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#;
        match serde_json::from_str::<StreamEvent>(json).unwrap() {
            StreamEvent::ContentBlockDelta { index, delta } => {
                assert_eq!(index, 0);
                assert!(
                    matches!(delta, ContentDelta::TextDelta { text } if text == "Hi")
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn parses_tool_use_start() {
        let json = r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"tu1","name":"grep","input":{}}}"#;
        match serde_json::from_str::<StreamEvent>(json).unwrap() {
            StreamEvent::ContentBlockStart { content_block, .. } => {
                assert!(
                    matches!(content_block, ContentBlock::ToolUse { name, .. } if name == "grep")
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn usage_merge_takes_max() {
        let mut a = Usage {
            input_tokens: 10,
            output_tokens: 5,
            ..Default::default()
        };
        a.merge(&Usage {
            output_tokens: 12,
            cache_read_input_tokens: 7,
            ..Default::default()
        });
        assert_eq!(a.input_tokens, 10);
        assert_eq!(a.output_tokens, 12);
        assert_eq!(a.cache_read_input_tokens, 7);
    }

    #[test]
    fn request_has_cache_breakpoint_and_adaptive_thinking() {
        let req = MessagesRequest {
            model: "claude-opus-4-8".into(),
            max_tokens: 100,
            system: vec![SystemBlock::cached("sys")],
            messages: vec![Message::user(vec![ContentBlock::text("hi")])],
            tools: vec![],
            thinking: Some(Thinking::adaptive()),
            output_config: Some(OutputConfig {
                effort: Some("xhigh".into()),
            }),
            stream: true,
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["model"], "claude-opus-4-8");
        assert_eq!(v["stream"], true);
        assert_eq!(v["thinking"]["type"], "adaptive");
        assert_eq!(v["output_config"]["effort"], "xhigh");
        assert_eq!(v["system"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(v["messages"][0]["role"], "user");
        // empty tools array is skipped entirely
        assert!(v.get("tools").is_none());
    }
}
