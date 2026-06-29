# UMIDE Roadmap & Fix List

Status as of the `feat/ai-assistant` branch. UMIDE = a Rust + Floem IDE (Lapce
fork) that embeds the Android emulator and iOS simulator, now with an **AI
coding assistant** (`umide_agent` engine + AI Assistant panel) that can see and
drive the embedded devices.

Legend: **P0** ship-blocker · **P1** core · **P2** depth · **P3** vision ·
🔴 known bug/gap · 🟡 unverified · 🟢 nice-to-have.

---

## 0. Status snapshot

- AI assistant builds, the binary launches (Metal renderer confirmed), and
  **18 unit tests pass**. The **live end-to-end loop has never been run** —
  needs a real key + GUI + booted device.
- Providers: Claude (Anthropic, native) is the only path with unit-tested wire
  format. OpenAI / DeepSeek / Gemini compile and follow the documented
  OpenAI-compatible shape but are **unverified against the live APIs**.
- Everything AI is **additive** and approval-gated; the base editor + emulator
  panels are unchanged.

---

## P0 — Harden before shipping the AI feature

- [ ] 🟡 **Live smoke test, all providers.** Run the loop against real keys:
      read-only Q&A → an `edit_file` diff → a `run_command` → a device
      screenshot + tap. Fix whatever breaks. This is the single most important
      item — nothing below matters until this passes.
- [ ] 🟡 **Verify OpenAI/DeepSeek/Gemini wire paths.** Confirm streaming
      tool-call accumulation, vision (`image_url`), and tool-result messages
      against each live endpoint; fix translation edge cases in
      `crates/umide_agent/src/openai.rs`.
- [ ] 🔴 **Refresh model defaults + add a model-override field.** Defaults are
      `gpt-4o` / `deepseek-chat` / `gemini-2.0-flash` / `claude-opus-4-8`; the
      UI has no way to pick a different model. Add a model text field per
      provider (the config already supports `with_model`).
- [ ] 🔴 **Request/command timeouts.** LLM requests and `run_command` have no
      timeout — a hung server or command blocks the agent worker. Add a
      per-request timeout and a per-command wall-clock cap.
- [ ] 🔴 **Rate-limit / transient-error handling.** No retry/backoff on 429/5xx.
      Surface a clear message and retry with backoff.
- [ ] 🟡 **`run_command` safety review.** It's approval-gated but runs arbitrary
      shell. Decide on an allowlist or at least a clearer "this will run X"
      confirmation; never auto-approve.

---

## P1 — Core UX & capability

### Chat panel
- [ ] 🔴 **Auto-scroll** the transcript as tokens stream (long answers currently
      scroll out of view).
- [ ] 🔴 **Enter-to-send** in the input (Shift+Enter for newline).
- [ ] 🟡 **Markdown rendering** of responses (code blocks, lists) — currently
      plain text. `pulldown-cmark` is already a dependency.
- [ ] 🟢 **New chat / clear conversation** button + **conversation persistence**
      across restarts (per workspace).
- [ ] 🟢 **Mask the API-key input**; add a "change/clear key" affordance and an
      at-a-glance indicator of which providers are configured.
- [ ] 🟢 **Running cost/token meter** (accumulate across the session, not just
      the last turn).
- [ ] 🟢 **"Ask AI" command + keybinding** (workbench command to toggle/focus
      the panel; send selection as context).

### Editing
- [ ] 🔴 **Multi-hunk / multi-file edits** in one approval card (today:
      single unique-snippet replace per call).
- [ ] 🟡 **`create_file` tool** (new files, not just edits).
- [ ] 🟢 **Stronger staleness handling** when applying to an open, dirty buffer
      (today it reads disk; a dirty buffer can mismatch).

### Context
- [ ] 🔴 **`@file` / `@selection` mentions** to pin context into a prompt.
- [ ] 🟡 **Automatic context**: current file, selection, and **LSP diagnostics**
      fed to the agent so it can fix errors it can see.
- [ ] 🟢 **Repo map** (symbol/file overview) for cheaper, better grounding.

### Device loop
- [ ] 🟡 **`idb` presence check / install hint** for iOS input; **app
      install + launch** tool (`adb install` / `simctl install+launch`).
- [ ] 🟢 **Dedicated `hot_reload`** tool for RN (Metro) and Flutter, instead of
      composing it from menu+tap.
- [ ] 🟢 **Per-device targeting** when multiple emulators/simulators run.

---

## P2 — Depth

- [ ] **LSP-as-tools**: expose go-to-definition, find-references, and rename to
      the agent (the proxy already speaks LSP).
- [ ] **Inline / ghost-text completions** (Copilot-style) using the selected
      provider — a second, latency-optimized surface beyond chat.
- [ ] **Background tasks / agents**: "make the failing tests pass", "wire this
      screen", running off the main chat with progress in the panel.
- [ ] **Context editing / compaction** for long sessions (Anthropic supports it;
      add provider-neutral history trimming).
- [ ] **Cheap-model triage**: use Haiku / `gpt-4o-mini` / `gemini-flash` for
      quick yes/no vision checks to cut cost on the device loop.
- [ ] **Usage dashboard**: per-provider spend, request log, error history.

---

## P3 — The "next level" vision

- [ ] **Design-to-running-app loop as a first-class flow**: "build this screen
      from a mockup" → agent codes → hot-reloads → screenshots → **vision-verifies
      against the target** → iterates. This is UMIDE's unique differentiator —
      no other IDE owns both the editor and the live device surface.
- [ ] **Multi-agent / parallel subtasks** (fan-out across files/screens).
- [ ] **RN + Flutter project scaffolding** and run/debug from one workspace.
- [ ] **Plugin-registered AI tools**: let WASM plugins expose tools to the agent.
- [ ] **Team / cloud tier**: optional hosted inference, shared sessions, cloud
      build/run so heavy emulator work isn't local-only.

---

## Tech debt & cleanup (parallel track)

- [ ] 🔴 **App-identifier inconsistency.** Logs exist under *both*
      `dev.umide.Umide-Debug` and `dev.lapce.Umide-Debug`. Pick one identifier
      (`directories`/app id) and migrate; stale data under the other is a sign
      the rebrand is half-done.
- [ ] 🟡 **Finish the lapce → umide rebrand.** Entry points and internal names
      still reference `lapce` in places (e.g. the architecture docs cite
      `lapce.rs`); module path is `umide_app`.
- [ ] 🟡 **19 pre-existing compiler warnings** in `umide-app` (deprecations in
      `keypress/keymap.rs`, unused items). Clean with `cargo fix` + review.
- [ ] 🟡 **CI for the AI crates**: ensure `umide_agent` + `umide-app` build and
      `cargo test` runs in CI; add the new crate to release builds.
- [ ] 🟢 **Linux keychain** support (`keyring` is configured for
      `apple-native` + `windows-native` only).
- [ ] 🟢 **Agent worker runtime**: a fresh current-thread tokio runtime is built
      per turn — fine for now; consider a pooled runtime if turn rate grows.
- [ ] 🟢 **Dedicated icons/assets**: the AI panel now has a sparkle glyph; audit
      for any other reused/placeholder icons.

---

## Suggested order

1. **P0 smoke test + provider verification + timeouts** (make it real and safe).
2. **P1 chat UX** (auto-scroll, Enter-to-send, markdown) — biggest daily-use win.
3. **P1 context (`@file`, diagnostics) + multi-file edits** — biggest capability win.
4. **P1 device tooling (install/launch, hot-reload)** — completes the loop.
5. Then P2 depth and the P3 design-loop vision.

Tech-debt items run alongside whenever touching the relevant area.
