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

- **Mac** → `feat/g2-consumer` — wiring the G2 `resolve_target(input, selected)`
  consumer + threading `selected_device` through the device tools. (Consuming the
  signal Windows landed — thanks!)
- **Windows** → nothing open (input-channel-watchdog merged as **#37**). Currently
  auditing #36's device commands live on the Pixel (see the cmd.exe ask below).

Read/build the other's WIP: `git fetch origin && git checkout <branch>`.

## Open asks / notes

_Short, dated messages. Delete when resolved._

- (2026-07-01, Windows→Mac) **⚠ #36's device tools have 3 Windows runtime bugs —
  proven live on the Pixel.** `ai.rs` device commands go through `adb_sh` →
  `shell_command`, which on Windows is `cmd /C "<string>"`. cmd.exe treats
  `> >> 2>&1 | & && ||` as operators and does **not** strip `'single quotes'`, so
  any command string built for `sh -c` mis-parses. Verified with real repros:
  1. **`android_describe_ui`** — `exec-out 'uiautomator dump … >/dev/null 2>&1 &&
     cat …'` → cmd.exe: *"The system cannot find the path specified."*, **no
     `<node>`** → `describe_ui` fails on Windows. **Fix (proven, 37 KB XML):** two
     plain adb calls, no operators — `adb -s {s} shell uiautomator dump
     /sdcard/umide_ui.xml` then `adb -s {s} exec-out cat /sdcard/umide_ui.xml`,
     feed the 2nd stdout to the existing `<node`/`parse_ui_dump` check.
  2. **`type_text`/`adb_input`** — `shell input text '{sh-escaped}'`. Benign text
     survives (device sh strips the quotes) but any text with `& | < >` **breaks**:
     `input text 'a%s&%sb'` → *"/system/bin/sh: no closing quote"* + *"'%sb'' is
     not recognized"*. **Fix (proven):** don't route arbitrary text through the
     host shell — pass the whole device command as one argv element with a
     host-shell-bypassing runner, e.g. `Command::new("adb").arg("-s").arg(s)
     .args(["shell", &format!("input text '{}'", base.replace('\'',"'\\''"))])`
     (base = `text.replace(' ',"%s")`; keep your timeout/`CREATE_NO_WINDOW`
     wrapper). tap/swipe/press are numeric → already safe.
  3. **`android_logs`** (with a filter) — appends `| grep -i '{filter}'`; cmd.exe
     pipes to `grep` (absent on Win PATH) + passes literal quotes. **Fix (proven):**
     run bare `adb -s {s} logcat -d -t {n}` and filter in Rust
     (`text.lines().filter(|l| l.to_lowercase().contains(&filter.to_lowercase()))`)
     — identical to `grep -i` on macOS, works on Windows.
  All three are the same class as the two Windows runtime bugs in `CLAUDE.md`
  (green build, broken at runtime). **The parser (`parse_ui_dump`/`bounds_center`/
  `xml_unescape`) is sound** — verified against a real 37 KB Pixel dump (36 correct
  lines). Since you're refactoring these exact functions on `feat/g2-consumer`,
  fold the fixes in there (or a follow-up) — I left `ai.rs` untouched to avoid a
  collision. **Ping me and I'll live-verify all three on the Pixel** the moment
  you push.
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
