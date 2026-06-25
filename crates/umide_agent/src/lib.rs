//! # umide_agent
//!
//! The AI engine behind UMIDE's assistant panel. It is a self-contained,
//! UI-agnostic library: it speaks the Anthropic Messages API (streaming SSE,
//! tool use, vision, prompt caching) and runs an agentic loop, but it knows
//! nothing about Floem, the editor, the proxy, or the emulator.
//!
//! The editor wires it in by:
//!   1. building a [`provider::ProviderConfig`] (BYO Anthropic key),
//!   2. implementing [`tools::ToolExecutor`] over its real capabilities
//!      (read/edit files, run commands, screenshot + drive the emulator),
//!   3. constructing an [`agent::Agent`] and pumping [`event::AgentEvent`]s into
//!      the AI panel's signals.
//!
//! This separation is intentional: the assistant is **additive**. Nothing here
//! can seize the editor or the emulator — it only acts through tools the editor
//! chooses to expose, and hard-to-reverse actions are approval-gated at the
//! `ToolExecutor` boundary. Developers keep coding and using the embedded
//! emulators exactly as before; the agent is just another optional panel.

pub mod agent;
pub mod client;
pub mod error;
pub mod event;
pub mod provider;
pub mod tools;
pub mod types;

pub use agent::Agent;
pub use error::AgentError;
pub use event::AgentEvent;
pub use provider::ProviderConfig;
pub use tools::{ToolExecutor, ToolInvocation, ToolOutput};
pub use types::{ContentBlock, Message, Role, ToolDef, ToolResultContent, Usage};
