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

- **Windows** → `feat/device-logs-panel` (**PR #67**) — the Device Logs panel UI
  (both platforms) + the critical non-UTF-8 stream fix. CI green; rebased onto
  post-#68 main. (Everything earlier — #59/#60/#61/#62/#64/#65/#66/#68 — is
  MERGED; see notes below.)

Read/build the other's WIP: `git fetch origin && git checkout <branch>`.

## Open asks / notes

_Short, dated messages. Delete when resolved._

- (2026-07-02, Mac→Windows) **Run on Device is up (#68)** — the status-bar badge
  is clickable (▶) and a `Run on Device` palette command runs the detected app
  in a live terminal via RunAndDebug: flutter `-d <serial|udid>`, bare RN
  `run-android`/`run-ios --udid`, Expo `npx expo run:*` (`is_expo` probe).
  Platform follows `panel.active_device`. Live-verified on macOS (RN scratch →
  iOS sim path). **Please verify the Android path on the Pixel** — a real RN
  app + `npx react-native run-android` from the badge — and sanity-check the
  run terminal on Windows (program spawn goes through the existing RunAndDebug
  terminal, so npm shims should be fine, but eyes-on is worth it).

- (2026-07-02, Windows→Mac) **Device Logs panel UI is BUILT — branch
  `feat/device-logs-panel`, rebased onto main (post-#66 squash), PR up.** New
  `PanelKind::DeviceLogs` in the bottom dock beside the terminal; streams the
  `active_device`'s native logs live (Android `start_logcat_stream`, iOS
  `start_ios_log_stream` — same contract, so both platforms share one view).
  Severity-coloured monospace, live line count + Clear, capped at 1000 lines,
  tails to bottom. **Live-verified on Windows/Pixel_9a.** iOS path shares the
  contract — **please live-verify on a simulator (macOS)** after it merges.
  - **⚠ Critical shared-backend fix rides along (`device_logs_stream.rs`) —
    #66 as merged still has it:** the reader used `BufRead::lines()`, which
    errors on the first **non-UTF-8** line; logcat / `simctl log` emit those
    routinely, so the stream **died right after the initial dump** (looked like
    "shows a dump then freezes" — found via tracing at ~5k lines on the Pixel).
    Now reads bytes via `read_until` + `from_utf8_lossy`; verified 70k+ lines
    and still following. Without this the panel never follows on ANY platform.
  - `LogcatHandle` name kept as-is (UI now built) — rename later if you like.

- (2026-07-02, Windows→Mac) **#59 dock: fixed the Windows right-dock collapse —
  on `feat/right-dock-layout` @ `6ef2cbee`.** The wide AI panel overflowed the
  centre column and shoved the fixed-width right dock off the window edge (state
  said shown, paint was off-screen). Fix: side docks pin their width
  (`min_width(size)` + `flex_shrink(0)`), the centre column gets `min_width(0)`
  so it shrinks instead of overflowing. Verified live on the Pixel; Mac
  re-verified the whole layout after pulling. **Also took the transcript-wrap
  polish in `ai_assistant_view.rs`** (@ `f12613f0`) — Mac reviewed: exactly
  right, keep. Left for Mac: provider-row wrap at very narrow widths (needs a
  taffy-level tweak; floem has no `flex_wrap`).
- (2026-07-02, Windows→Mac) **Mobile-first split (user direction):** Windows —
  detection (#60) + logcat backend (#61), both MERGED; Device Logs panel UI next
  (on the #59 layout once merged). Mac — the iOS half of Device Logs
  (`xcrun simctl spawn <udid> log stream` into the same panel) and **AI
  project-context injection** (`CommonData.project_kind` is live — feed it into
  SYSTEM_PROMPT/WRITE_NOTE so the agent stops re-discovering the stack).
- (2026-07-01) ✅ Device-MCP wiring + mobile context shipped in Mac's #53; core
  #46. Claude Code drives the device key-free. Demo assets captured on Windows
  (v0.3.0 hero screenshots + emulator GIF) — publishing PR pending; demo video
  re-take parked.
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
