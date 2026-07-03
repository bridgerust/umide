# Design: one UMIDE Agent — multi-provider, BYO API key, switchable

Status: proposal · 2026-07-04 · owner: Mac (agent subsystem)

## Goal
Replace the 6-way backend chooser (Claude / OpenAI / DeepSeek / Gemini +
Codex / Gemini-CLI) with **one agent interface** — "the UMIDE Agent." The chat
has no per-message backend menu. The user brings their own API key for whatever
provider they have, and can **switch the active provider any day** via a small
control — it isn't a permanent lock, just not a loud menu in every conversation.
This is the foundation the Phase-2 mobile closed-loop work builds on.

## Principle: one interface, provider is a switchable setting
- **Chat surface:** always "UMIDE Agent." No picker cluttering it.
- **Provider:** chosen at first setup, **changeable anytime** through a compact
  switch in the panel header (and in Settings). Shows the active provider; click
  to switch among configured ones or add a key.
- **Switching applies to the next turn.** The transcript is provider-agnostic
  text, so a conversation can continue on a newly-selected provider; each
  provider has a sensible default model (advanced override available).

## Why multi-provider (don't force one)
A single provider would force, e.g., a Claude API key on a user who only has an
OpenAI or Gemini key — unacceptable. So the agent stays **provider-agnostic**;
the user powers it with whatever key they hold.

## Compliance basis (why BYO API key for every provider)
Researched 2026-07 against Anthropic and OpenAI docs:
- **Subscription login in a distributed third-party app is not a clean path for
  anyone.** Anthropic **explicitly prohibits** third parties offering claude.ai
  login (the June-15 Agent-SDK subscription-credit change is paused). OpenAI
  **doesn't endorse** it either — its docs recommend API keys for programmatic/
  distributed use and point commercial products to legal counsel.
- → **BYO API key per provider** is the only unambiguously-sanctioned credential
  across the board. No "use your Claude/ChatGPT login" UX.
- If a provider later sanctions third-party subscription auth, the credential
  layer below can add it as a mode — no rework.

Sources: Anthropic Agent SDK docs + "Use the Agent SDK with your Claude plan";
OpenAI Codex Authentication docs + discussion #8338. Not legal advice.

### ⚠ Fast-follow on the shipped beta
The app + umide.dev + README advertise *"No API key needed — use your Claude
login."* That's the pattern the policy prohibits. Remove that messaging; lead
with "connect a provider API key." Track as a fast follow to v0.4.0-beta.1.

## Architecture: UMIDE's own loop (provider-agnostic)
The agent is **UMIDE's own agent loop** (`LlmRunner` + `EditorTools` + device
tools + the ApprovalQueue) — which is already provider-agnostic via
`ProviderKind`. We do NOT drive a vendor CLI, because:
1. **Multi-provider requires it** — the vendor CLIs (Codex, Gemini-CLI, Claude
   Code) are each single-vendor and can't be one switchable agent.
2. **Phase-2 needs loop control** — the see→act→verify contract and the
   read-crash / visual-check tools live in our loop, consistent across providers.
3. **No external CLI install dependency.**

Providers (v1 candidates): **Anthropic, OpenAI, Gemini, DeepSeek** — each via its
own API with the user's key. (Final set: decision below.)

## What we remove
- The three vendor CLIs: `CliKind::{ClaudeCode, Codex, GeminiCli}` and
  `ai/cli/{codex.rs, gemini.rs, claude.rs, runner.rs, framer.rs}`, the CLI
  MCP device-server + permission-server (**CLI-specific**), detection, smokes.
- The `AssistantBackend = Llm | Cli` enum — the agent is always the LlmRunner.
- The per-chat picker and the key-free/login auto-select + messaging.

## What we keep
- `LlmRunner`, `EditorTools`/`ToolExecutor` (the loop + its tools), the device
  control tools (screenshot/tap/swipe/type/keys), `auto_observe`, and the
  ApprovalQueue — these are already the LLM path's native tools, so dropping the
  CLI loses nothing here (the CLI just re-exposed them over MCP).
- Per-provider key storage/resolution (keychain), extended to hold multiple keys
  + the active-provider selection.

## UX
- **Chat empty state:** "Connect your AI — pick a provider and paste a key."
- **Provider switch:** compact control in the AI panel header showing the active
  provider; click → switch among configured providers, add a key, or change the
  model. Flippable anytime; next turn uses the new one.
- **Settings → AI:** manage keys per provider, default model per provider, and
  the active provider.

## Migration
- Existing saved provider + key → that becomes the active provider, unchanged.
- Users who were on a CLI backend (Codex/Gemini-CLI/Claude Code) or a provider
  we drop → prompted once to add an API key; the old key-free path is gone by
  design.
- Persisted `AssistantBackend`/`CliKind` selection is dropped; the keychain +
  active-provider setting is the only state.

## Code plan (sequence)
1. Credential store: multiple provider keys in the keychain + an
   `active_provider` setting + resolver.
2. Collapse the seam: remove `AssistantBackend`/`CliKind`; the runner is always
   `LlmRunner(active_provider)`.
3. Delete `ai/cli/` CLI machinery (codex/gemini/claude parsers, runner, framer,
   device-server, permission-server) — the LlmRunner keeps device tools +
   approvals natively.
4. Rewrite `panel/ai_assistant_view.rs`: remove the picker; add the connect
   flow + the header provider-switch.
5. Remove the "no key needed / use your login" messaging (app + umide.dev +
   README) — compliance.
6. Phase-2 hooks (auto-verify contract + read-crash / visual-check tools) land
   on the LlmRunner tool set — one agent, one place to make the loop reliable.

## Open (small) decisions
1. **Provider set for v1** — all four (Claude/OpenAI/Gemini/DeepSeek), or a
   tighter start? More providers = more per-provider tool-calling quirks to test.
2. **Compliance patch timing** — remove the "use your login" messaging now as a
   small patch on the beta, or bundle it into this change?
