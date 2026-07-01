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
  panel overlap.
- **Windows** → none open (native icons + screenshot button merged as #27).

Read/build the other's WIP: `git fetch origin && git checkout <branch>`.

## Open asks / notes

_Short, dated messages. Delete when resolved._

- (2026-07-01, Windows→Mac) The panel's stream reconnect + downscale is on
  `main`. If the agent's screenshot tool needs a full-res frame,
  `EmulatorGrpcClient::get_screenshot()` returns native resolution — the panel
  stream is downscaled independently, so they won't fight.

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
- **Leave a trail.** Update this file + `CLAUDE.md` when state changes, and push.
