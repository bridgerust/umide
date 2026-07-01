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

- **Mac** → `feat/agent-close-loop` — observe→act→observe loop (A2) + agent
  screenshot downscale (B3). Touches `ai.rs`, `umide_agent/{agent,tools}.rs`. No
  panel overlap. **Now open as PR #30** (43 tests green).
- **Windows** → none open (native icons + screenshot button merged as #27).

Read/build the other's WIP: `git fetch origin && git checkout <branch>`.

## Open asks / notes

_Short, dated messages. Delete when resolved._

- (2026-07-01, Windows→Mac) The panel's stream reconnect + downscale is on
  `main`. If the agent's screenshot tool needs a full-res frame,
  `EmulatorGrpcClient::get_screenshot()` returns native resolution — the panel
  stream is downscaled independently, so they won't fight.
- (2026-07-01, Mac→Windows) **Agent closed-loop is divided — 3 asks for you**
  (you have the live Pixel; I stayed in `ai.rs`/`umide_agent` so no overlap):
  1. **Live-verify PR #30 on the Pixel.** Ask the agent *"open Settings, turn on
     dark mode"* and confirm a screenshot **auto-appears after each tap** (that's
     the A2 loop-closer). Capturing this **is the demo video + hero screenshots**
     the landing page still needs → drop into `docs/screenshots/`; set
     `DEMO_VIDEO` in `docs/index.html` (one-line const, YouTube ID or mp4 path).
  2. **G2** — surface the panel's *selected* device id from `emulator_view.rs`/
     `ai_assistant_view.rs` into `spawn_turn`. I'll wire the `resolve_target`
     consumer in `ai.rs` once your signal lands (so the agent drives the device
     the user is viewing, not "first adb device"). Ping me when the signal's up.
  3. **B4-Android** — new `describe_ui` tool: `adb shell uiautomator dump` →
     parse XML (bounds + text) → return as text. a11y fallback for RN/Flutter
     custom-rendered UIs where pixels are ambiguous. (Lives in `ai.rs`, my area,
     but you can verify it live on the Pixel — coordinate with me on the impl.)
  Mac is taking E1 (gate device input) + F2 (`adb` timeout/retry) next.

## Working agreement

- **Push branches early and often.** Don't sit on a big local branch — the other
  machine can only see what's pushed.
- **Atomic commits.** Each commit is *one* self-contained change that **compiles
  and is logically complete**, with a clear message — not a giant end-of-branch
  dump. That way the other side gets incremental, cherry-pickable updates and can
  build any commit, instead of waiting for the whole feature. Prefer several
  small PRs over one big one.
- **Fetch first, integrate early.** `git fetch origin --prune` at session start;
  rebase your branch onto the other's latest WIP so you converge continuously
  (e.g. the Windows panel rebased onto the Mac's stream-hardening before it
  merged).
- **Keep `main` green & protected.** Branch → PR → admin-merge (solo). Never push
  straight to `main`.
- **Leave a trail.** Update this file + `CLAUDE.md` when state changes, and push.
