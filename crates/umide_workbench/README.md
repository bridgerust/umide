# Umide Workbench

Umide Workbench is the Rust and Floem desktop rewrite. Open an app folder, choose Codex or Claude Code, and run the app alongside the conversation. The workbench stores chat history and provider session IDs locally for each project.

## Run from source

Use stable Rust and install the Protocol Buffers compiler (`protoc`). On macOS, install the Xcode command line tools. From the repository root:

```sh
cargo run --locked -p umide-workbench
```

You can also pass an app folder after `--`. To build a local macOS development bundle, run `./crates/umide_workbench/package-macos.sh` and open `target/debug/Umide Workbench.app`.

## Agents

- **Codex** uses the local Codex CLI and its configured authentication. Use a version that supports `codex app-server --stdio`.
- **Claude Code** uses the local Claude CLI in bare mode. Set `ANTHROPIC_API_KEY` or configure a supported cloud provider in the environment that starts the app. Launch from a configured terminal if the credentials are set in your shell.
- Configure `UMIDE_CODEX_PATH` or `UMIDE_CLAUDE_PATH` with an absolute executable path if automatic detection fails.
- Select a running device and enable **Attach screen** to send its current screenshot with the next message. Command and edit requests requiring approval appear in the conversation. Use **Stop** to interrupt the active turn.
- **Changes** reads the staged and unstaged Git diff and lists untracked files. Click the tab again to refresh.

## Devices and builds

Install the Android SDK platform tools and emulator, then create an Android virtual device. For iOS, install Xcode and an available Simulator runtime on macOS. Install `idb_companion` for touch input on iOS. Xcode alone can supply the screen preview and logs.

Select a device to start it or open its preview. Click or drag on its screen to tap or swipe. Use **Run app** to build from the open folder and launch on that device. **Build** shows command output. **Stop build** terminates the running build command. **Device logs** shows recent device output and refreshes when reselected.

The runner detects Flutter, Expo and React Native app folders. The native Android path uses a Gradle wrapper and the standard `app` module producing one debug APK. The native iOS path uses an Xcode workspace or project with an identifiable app scheme. Install the chosen project's dependencies before running. Workbench currently shows one selected device and runs one agent turn at a time.

Run the workbench tests with `cargo test --locked -p umide-workbench -p umide_workbench_agents`. Each provider can be tested with a local model endpoint. `UMIDE_WORKBENCH_DATA_DIR` can point to an absolute temporary directory for isolated chat history.
