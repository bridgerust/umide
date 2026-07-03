# Design: one UMIDE Agent (Claude, API-key), on the Agent SDK mechanism

Status: proposal · 2026-07-04 · owner: Mac (agent subsystem)

## Goal
Replace the multi-backend chooser (Claude / OpenAI / DeepSeek / Gemini +
Codex / Gemini-CLI) with **one agent** — "the UMIDE Agent" — powered by Claude.
The chat has no backend picker; the model/credential is a one-time setup, not a
per-conversation choice. This is the foundation the Phase 2 mobile closed-loop
work builds on.

## Compliance basis (why API-key, not subscription login)
Researched against Anthropic's current docs (2026-07):

- The Claude Agent SDK **spawns the official `claude` Code CLI** as a subprocess
  (`claude --print --output-format stream-json`). Driving that CLI directly from
  Rust is the expected path — there is **no official Rust SDK**.
- **API-key auth is the sanctioned third-party path.** Anthropic's Agent SDK
  docs: *"Unless previously approved, Anthropic does not allow third party
  developers to offer claude.ai login or rate limits for their products,
  including agents built on the Claude Agent SDK. Please use the API key
  authentication methods instead."*
- The June-15-2026 "Agent SDK subscription credits" change (which would have let
  apps run on a user's Pro/Max plan) is **paused**, no re-enable date.
- Net: the single agent authenticates with a **user-provided Anthropic API
  key** (or a cloud-provider key: Bedrock / Vertex / Azure). We do **not** build
  UX around "use your Claude subscription login."

Sources: Agent SDK overview & quickstart (code.claude.com/docs/en/agent-sdk),
"Use the Claude Agent SDK with your Claude plan" (support.claude.com). Not legal
advice — re-verify at build time; if Anthropic re-enables subscription credits
or grants approval, the credential layer below can add that mode.

### ⚠ Fix in the current beta
The app + site advertise *"No API key needed — use your Claude login."* That is
the pattern the policy prohibits. Action: remove that messaging (app AI panel +
umide.dev + README), and lead with "connect your Anthropic API key." Track as a
fast follow to v0.4.0-beta.1.

## Current state (what exists, to retire/keep)
- `AssistantBackend = Llm(ProviderKind) | Cli(CliKind)` (`ai/cli/mod.rs`).
  - `ProviderKind = Anthropic | OpenAi | DeepSeek | Gemini` — UMIDE's own
    direct-API loop (`LlmRunner` in `ai.rs`, `EditorTools`).
  - `CliKind = ClaudeCode | Codex | GeminiCli` — subprocess loop
    (`CliRunner` in `ai/cli/runner.rs`).
- The **ClaudeCode path already is the target mechanism**: it drives `claude`
  headless with `--append-system-prompt` (UMIDE mobile context),
  `--mcp-config` + `--strict-mcp-config` (the device-tools MCP server,
  `ai/cli/device_server.rs`), `--permission-prompt-tool` (per-action approval
  bridge, `ai/cli/permission_server.rs`), and `--resume` (sessions).
- The picker UI is in `panel/ai_assistant_view.rs` ("No key" cli_buttons + "API
  key" provider_buttons + key-free auto-select).

## Target architecture
**The UMIDE Agent = Claude, via the `claude` CLI (the Agent SDK mechanism),
authenticated with the user's Anthropic API key**, shaped by UMIDE into a
mobile agent through:
- **System prompt** (`--append-system-prompt`) — the mobile identity + the
  Phase-2 see→act→verify contract.
- **Device MCP tools** (`--mcp-config`) — screenshot / tap / swipe / type /
  read-logs / run-on-device, so the agent drives the embedded device.
- **Per-action approvals** (`--permission-prompt-tool`) — edits/commands routed
  to UMIDE's ApprovalQueue.

Credential is set once ("Connect your Anthropic API key"), stored in the OS
keychain, exported as `ANTHROPIC_API_KEY` to the CLI. Cloud-provider keys are an
advanced option later.

### What we remove
- `CliKind::Codex`, `CliKind::GeminiCli` + `ai/cli/codex.rs`, `ai/cli/gemini.rs`
  (+ their smoke tests, picker entries, detection).
- `ProviderKind::{OpenAi, DeepSeek, Gemini}` and the multi-provider selection.
- The whole `AssistantBackend` enum + the in-chat picker — collapse to one agent.
- The "key-free / use your login" auto-select path and its messaging.

### What we keep (renamed to "the agent")
- `CliRunner`'s ClaudeCode invocation, `device_server.rs`, `permission_server.rs`,
  `claude.rs` (parser), `framer.rs`, `proc_group.rs`, session/`--resume`.
- Anthropic API-key storage/resolution (from `ai.rs`).

### Open decision — the one loop
- **Path A (recommended): drive `claude` (Agent SDK mechanism).** Reuses the
  built ClaudeCode integration; get a mature agent loop (planning, file/bash
  tools, subagents) for free; UMIDE shapes mobile behavior via prompt + MCP +
  approvals. Cost: requires the `claude` CLI installed (detect + guide install).
- **Path B: UMIDE's own loop on the raw Anthropic API** (extend `LlmRunner`).
  No CLI dependency; total control over the Phase-2 closed loop; but we rebuild
  planning/tooling. Reserve for if we need tighter loop control than prompt+MCP
  gives.

Recommendation: **Path A now** (ship the single agent fast, reuse what's built),
revisit B only if Phase-2 verify behavior needs loop-level control.

## UX
- **Chat:** no picker. Just the UMIDE Agent. Empty state → "Connect your
  Anthropic API key to start" with a paste field + a link to the console.
- **Settings → AI:** the API key field, a "test key" check, model selector
  (default: latest Claude; advanced), and a `claude` CLI status/install hint.
- **First run:** if no key and no `claude`, one guided setup; otherwise the
  agent just works.

## Migration
- Existing users on `Llm(Anthropic)` with a saved key → the single agent, same
  key. Users on OpenAI/DeepSeek/Gemini or a CLI → prompted once to add an
  Anthropic key (their old choice is gone by design).
- Drop persisted `AssistantBackend` selection; keychain key is the only state.

## Code plan (sequence)
1. Settings/credential: Anthropic key in keychain + a resolver; `claude` detect.
2. Collapse the seam: remove `AssistantBackend`/`ProviderKind` multi-variants +
   `CliKind::{Codex,GeminiCli}`; the runner is Claude-only.
3. Delete `codex.rs`, `gemini.rs` + smokes; trim `detect.rs`.
4. Rewrite `ai_assistant_view.rs`: remove the picker; add the connect/setup +
   the single-agent chat.
5. Remove the "key-free/login" messaging (app + site + README) — compliance.
6. Keep device MCP + approval bridge unchanged; retarget them as "the agent's"
   tools (Phase-2 will extend these).

## Phase-2 hook
The mobile closed loop (auto-verify, read-crash, visual-check) lands as (a) the
system-prompt contract and (b) new device-MCP tools on this one agent — not as a
new backend. One agent surface = one place to make the loop reliable.
