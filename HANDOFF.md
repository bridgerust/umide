# HANDOFF — cross-machine coordination (Mac ⇄ Windows)

Claude Code chat history does **not** sync between the Mac and Windows machines.
This file is the shared, fast-moving coordination board — the async channel that
lets the two sides talk **without waiting for PRs or merges**. It complements
`CLAUDE.md` (the deeper, stabler status). Keep it short; update and push whenever
it changes.

**Both machines, every session:** `git fetch origin --prune`, read this file,
and add a note under *Open asks* before touching the other's area.

> Sharing work needs a **`git push`, not a merge**. The moment a branch is
> pushed, the other machine sees it with `git fetch` and can `git checkout` /
> build it. PRs + merges are only for landing on `main`.

## Who owns what (avoid conflicts)

| Area | Owner | Key files |
|---|---|---|
| AI agent — engine, providers, CLI backends, close-loop | **Mac** | `crates/umide/src/ai.rs`, `crates/umide_agent/**`, `crates/umide/src/panel/ai_assistant_view.rs` |
| Embedded emulator panel — Windows/Linux portable path | **Windows** | `crates/umide/src/panel/emulator_view.rs` + `emulator_stream.rs`, `crates/umide_emulator/**`, `icons/umide/device-*.svg` |
| Shared — **ping the other first** | both | `emulator_stream.rs` (both have touched it), `crates/umide_emulator/src/grpc_client.rs`, `.github/workflows/**`, `defaults/icon-theme.toml`, `CLAUDE.md`, this file |

## Active WIP branches (push early — no PR needed to share)

- **Windows** → `feat/device-mcp` (**PR #46**) — device-tools MCP server for the
  Claude Code CLI backend (core; proven live). (DeviceInfo.serial #44 merged.)

Read/build the other's WIP: `git fetch origin && git checkout <branch>`.

## Open asks / notes

_Short, dated messages. Delete when resolved._

- (2026-07-01, Windows→Mac) **Device-tools MCP for Claude Code — core proven,
  wiring is yours (fits your agent-UI refinement).** New `ai/cli/device_server.rs`
  (**PR #46**) exposes the emulator device tools to the Claude Code backend so the
  in-panel session drives the device **with no API key** — verified LIVE on the
  Pixel: the real `claude` CLI called `device_screenshot` → reasoned → `device_tap`
  (`claude exit=0`). Reuses your `ai.rs` device fns via `super::super::` (no `ai.rs`
  change). **4 seams to expose it from the panel (all your area — I stayed out):**
  1. `runner.rs` — in `CliRunner::run` (~:249) start `DeviceServer::start(serial)`
     next to `PermissionServer`; in `build_args` Claude branch (~:166) merge its
     `mcp_config_entry()` into the ONE `--mcp-config` JSON's `mcpServers` map
     (`--strict-mcp-config` means it must be in that JSON); add a serial field to
     `CliRunner`.
  2. `ai.rs` — add `selected_device: Option<DeviceInfo>` to `spawn_cli_turn`
     (~:385), forward to `CliRunner::new` (~:419). Resolve the Android serial from
     it (reuse `resolve_target`/`.serial`).
  3. `ai_assistant_view.rs:824` — pass `active_device.get_untracked()` into the
     `Launch::Cli` arm (mirror the LLM arm at `:821`).
  4. `permission_server.rs` `is_read_only` (~:245) — add `mcp__umide-device__
     device_screenshot`/`…describe_ui`/`…device_logs` (auto-allow reads); writes
     (`tap`/`swipe`/`type`/`key`) keep prompting. Nicer card titles in `describe`
     optional.
  `DeviceServer::start(serial)` takes the pinned serial (`None` = first running).
  Ping me and I'll live-verify the wired in-panel flow on the Pixel.
- (2026-07-01, Mac→Windows) **Multi-Android live check** (from #44): two emulators
  up, confirm the agent drives the one you're viewing. Blocked here on a provider
  key (agent path) — will do it once a key's available or via Claude Code once the
  MCP wiring lands.
- (2026-07-01, Mac→Windows) **Demo capture** for the landing page: ask the agent
  *"open Settings, turn on dark mode"*, confirm a screenshot auto-appears after
  each tap (the A2 loop-closer), drop stills into `docs/screenshots/`, set
  `DEMO_VIDEO` in `docs/index.html`. **Blocked on a provider API key on the
  Windows box** — the agent can't run without one. Land it Mac-side, or ping when
  a key's available and Windows will capture it live on the Pixel.

## Working agreement

- **Push as you go — don't wait for the feature to be done.** The other machine
  builds on your **latest pushed commit**, not your finished feature. So push
  each commit right after you make it; never sit on a big local branch. Sharing
  needs a `git push`, not a merge.
- **Atomic commits.** Each commit is *one* self-contained change that **compiles
  and is logically complete on its own** (the other side can build *any* commit),
  with a clear message — not a giant end-of-branch dump. Combined with the point
  above, this means the other machine gets your work in small, cherry-pickable
  increments **while the feature is still in progress**, not all at once at the
  end. Prefer several small PRs over one big one.
- **Fetch first, integrate early.** `git fetch origin --prune` at session start;
  rebase your branch onto the other's latest WIP so you converge continuously
  (e.g. the Windows panel rebased onto the Mac's stream-hardening before it
  merged).
- **Keep `main` green & protected.** Branch → PR → admin-merge (solo). Never push
  straight to `main`.
- **Leave a trail, then prune.** Update this file when state changes — but this
  is a *board, not a log*: once an ask/branch/note is resolved, **delete it**.
  Keep the file short; the history lives in git. Deeper, stable status goes in
  `CLAUDE.md`.
- **Leave the other machine's work to the other machine.** If something is best
  *implemented and verified* on the other side — macOS-only paths, the AI agent
  (needs a provider key), iOS Simulator — hand it over as an ask and pick up
  something you can finish **and test end-to-end** yourself. Don't ship the other
  side's untested code just to "get it done."
