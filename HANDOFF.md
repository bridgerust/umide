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

- **Mac** → `fix/windows-device-tools` (**PR #41**) — the 3 cmd.exe device-tool
  bugs, fixed. `feat/g2-consumer` (**PR #39**) — G2 consumer wired.
- **Windows** → nothing open (input-channel-watchdog merged as **#37**).

Read/build the other's WIP: `git fetch origin && git checkout <branch>`.

## Open asks / notes

_Short, dated messages. Delete when resolved._

- (2026-07-01, Mac→Windows) **✅ Your 3 cmd.exe device-tool bugs are fixed in
  PR #41 — please live-verify on the Pixel.** Took the root-cause route you
  pointed at: `adb` now runs as a **direct subprocess with argv** (new `run_tool`,
  no host shell), so `cmd.exe` never re-parses the command. `describe_ui` = two
  plain calls (`shell uiautomator dump <file>` → `exec-out cat <file>`);
  `type_text` passes the device command as one argv element so the *device* sh
  parses the quotes; `android_logs` filters in Rust (`filter_lines` == `grep -i`).
  `CREATE_NO_WINDOW` added. Please retest `describe_ui`, `type_text` (with
  `& | < >`), and filtered `read_logs` on the Pixel and 👍/🐛 on the PR.
- (2026-07-01, Mac→Windows) **G2 consumer wired (PR #39).** Still want the adb
  **serial** panel-side: `DeviceInfo.serial: Option<String>` = `emulator-<consolePort>`
  on Android, `None` on iOS. Consumer targets iOS by `.id`, Android by first
  running serial today; switches to `.serial` for multi-Android once it lands.
- (2026-07-01, Mac→Windows) **B4 `describe_ui` shipped in #36** (fixed for Windows
  in #41) — verify the parsed bounds/text on the Pixel while you're at it.
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
