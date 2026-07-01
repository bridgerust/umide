//! External agent-CLI backends for the AI assistant panel.
//!
//! Alongside the built-in [`crate::ai`] LLM path (a UMIDE-owned agentic loop over
//! the Anthropic / OpenAI-compatible APIs, with every mutating action gated by
//! the [`crate::ai::ApprovalQueue`]), the panel can optionally be powered by an
//! *external* coding agent the user already has installed — Claude Code, Codex,
//! or Gemini CLI. Those run their **own** agentic loop and edit files / run
//! commands themselves, so they sit at a different altitude than [`LlmBackend`]:
//! instead of one model turn, a whole conversation. We drive them as child
//! processes in the project directory, parse their streaming-JSON event feed
//! with [`framer::CliFramer`], and translate each event into the *same*
//! [`umide_agent::AgentEvent`] stream the panel already renders.
//!
//! This module (P0) defines the selection data model; the runner, per-CLI
//! parsers, detection, and process-group control land in the CLI backend phases.
//!
//! [`LlmBackend`]: umide_agent::backend::LlmBackend

pub mod claude;
pub mod codex;
pub mod detect;
pub mod framer;
pub mod gemini;
pub mod permission_server;
pub mod proc_group;
pub mod runner;

use umide_agent::ProviderKind;

/// An external coding-agent CLI that can back the assistant panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CliKind {
    /// Anthropic's `claude` CLI (Claude Code).
    ClaudeCode,
    /// OpenAI's `codex` CLI.
    Codex,
    /// Google's `gemini` CLI.
    GeminiCli,
}

impl CliKind {
    /// The executable name to look up on `PATH`.
    pub fn binary_name(self) -> &'static str {
        match self {
            CliKind::ClaudeCode => "claude",
            CliKind::Codex => "codex",
            CliKind::GeminiCli => "gemini",
        }
    }

    /// Human-facing label for the picker. Kept deliberately distinct from the
    /// raw-API provider labels (e.g. "Claude Code" vs the API "Claude") so an
    /// autonomous agent is never mistaken for an approval-gated API model.
    pub fn label(self) -> &'static str {
        match self {
            CliKind::ClaudeCode => "Claude Code",
            CliKind::Codex => "Codex",
            CliKind::GeminiCli => "Gemini CLI",
        }
    }

    /// Shown when the CLI is not installed, so the user knows how to get it.
    pub fn install_hint(self) -> &'static str {
        match self {
            CliKind::ClaudeCode => {
                "Claude Code not found — install: npm i -g @anthropic-ai/claude-code"
            }
            CliKind::Codex => {
                "Codex not found — install: npm i -g @openai/codex (or brew install codex)"
            }
            CliKind::GeminiCli => {
                "Gemini CLI not found — install: npm i -g @google/gemini-cli"
            }
        }
    }

    pub fn all() -> [CliKind; 3] {
        [CliKind::ClaudeCode, CliKind::Codex, CliKind::GeminiCli]
    }
}

/// What the assistant panel is currently pointed at: either a raw-API LLM
/// provider (UMIDE's own approval-gated loop, the safe default) or an external
/// autonomous agent CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssistantBackend {
    /// A bring-your-own-key API provider, run through UMIDE's own agent loop.
    Llm(ProviderKind),
    /// An external agent CLI that runs its own loop with direct file/command
    /// access.
    Cli(CliKind),
}

impl AssistantBackend {
    /// True for the external-agent path (direct file/command access).
    pub fn is_cli(self) -> bool {
        matches!(self, AssistantBackend::Cli(_))
    }

    pub fn label(self) -> &'static str {
        match self {
            AssistantBackend::Llm(p) => p.label(),
            AssistantBackend::Cli(c) => c.label(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_kind_basics() {
        assert_eq!(CliKind::ClaudeCode.binary_name(), "claude");
        assert_eq!(CliKind::Codex.binary_name(), "codex");
        assert_eq!(CliKind::GeminiCli.binary_name(), "gemini");
        assert_eq!(CliKind::all().len(), 3);
    }

    #[test]
    fn backend_is_cli() {
        assert!(AssistantBackend::Cli(CliKind::ClaudeCode).is_cli());
        assert!(!AssistantBackend::Llm(ProviderKind::Anthropic).is_cli());
        assert_eq!(AssistantBackend::Cli(CliKind::Codex).label(), "Codex");
    }
}
