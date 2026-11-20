# Umide Workbench

Umide Workbench is the Rust and Floem desktop rewrite. Open a mobile or web project, choose Codex or Claude Code, and run the app alongside the conversation. The workbench stores chat history and provider session IDs locally for each project.

![Umide Workbench with the conversation, integrated terminal and web preview open together](../../screenshots/umide-workbench.png)

## First launch

When both standalone tools are missing, choose **Install both** to get them from OpenAI and Anthropic. Use **Install** on an individual provider to add just that tool. Existing installations appear with their version. **Sign in** opens the provider’s own authentication flow. Once it finishes, Umide checks the connection. Use **Continue to workspace** whenever you are ready. Reopen the assistant from **Agent setup** in the sidebar.

Umide follows the system appearance by default. Choose **Appearance → Light** or **Dark** in the sidebar to keep a fixed theme. Your choice applies to every project on this computer.

## Run from source

Use stable Rust and install the Protocol Buffers compiler (`protoc`). On macOS, install the Xcode command line tools. From the repository root:

```sh
cargo run --locked -p umide-workbench
```

You can also pass an app folder after `--`. To build a local macOS development bundle, run `./crates/umide_workbench/package-macos.sh` and open `target/debug/Umide Workbench.app`.

## Agents

- **Codex** uses the local Codex CLI and its configured authentication. Use a version that supports `codex app-server --stdio`.
- **Claude Code** uses the local Claude CLI and its own sign-in. Existing API key and cloud-provider setups can also use the CLI’s bare mode. Launch Umide from a configured environment when using shell-only credentials.
- Configure `UMIDE_CODEX_PATH` or `UMIDE_CLAUDE_PATH` with an absolute executable path if automatic detection fails.
- Select a running device and enable **Attach screen** to send its current screenshot with the next message. Command and edit requests requiring approval appear in the conversation. Use **Stop** to interrupt the active turn.
- Use **Project folder** for a conversation that edits the current checkout. For parallel work, choose **Isolated Git copy** before sending the first message. Each copy starts from the project's latest commit and excludes local uncommitted files. Commits, branches and edits made in a task checkout are accessible from **Changes → Open task folder**.
- You can move between active conversations. The sidebar indicates which agent is running or waiting for permission. Approving a tool or stopping a turn affects the selected task. Umide limits the project folder to one active agent turn at a time.
- **Changes** shows the selected task's edits. For an isolated task, it includes changes committed after the task started and lists untracked files. Its checkout stays on disk for review and later conversation turns. Open the task folder to commit or transfer the changes with Git.

## Terminal

The workspace opens on **Conversation**. Choose **Terminal** in the toolbar to open a shell below the conversation, in the current project or isolated Git checkout. Drag the divider to resize it. Hide the terminal to give the conversation more space and reopen it to resume the same session. Umide closes the sessions when you close its window.

## Project files

Choose **Files** next to **Terminal** to browse the checkout used by the current conversation. Expand a folder and select a file to edit it alongside the chat. Use **Browse files** to switch files. A wide window can show the tree and editor side by side.

Use **Save**, **⌘S** on macOS or **Ctrl+S** on Windows and Linux. Press **Esc** in the editor to return to the conversation input when it is available. Switching to another file, project or preview keeps your edits in the window. The editor opens UTF-8 text files up to 2 MiB.

If an agent or another program changes the file on disk, use **Copy edits** to keep your version before reloading. On a normal window close Umide stores unsaved drafts locally and restores them when you reopen the file. If the original file has moved or disappeared, open **Files** in the same checkout and choose **Copy edits** next to the original path.

![Editing a Swift file alongside the file tree, conversation and terminal](../../screenshots/umide-workbench-files.png)

## Web preview

Choose **Web → Start server** to run the selected conversation’s checkout. Umide uses the `dev` or `start` script from `package.json` and detects npm, pnpm, Yarn or Bun. Install the project's dependencies before starting it. Custom scripts should accept `PORT` and bind to the supplied `HOST`. For a plain HTML project, open the folder containing `index.html`. Static previews use Python 3.

On macOS the preview is interactive inside the workbench. Use **Reload preview** to refresh a static site after editing a file. **Open browser** opens the same local address in the system browser and is available on other platforms. **Show logs** displays server output and **Stop server** ends the process.

## Devices and builds

Install the Android SDK platform tools and emulator, then create an Android virtual device. For iOS, install Xcode and an available Simulator runtime on macOS. Install `idb_companion` for touch input on iOS. Xcode alone can supply the screen preview and logs.

Select a device to start it or open its preview. Click or drag on its screen to tap or swipe. Use **Run app** to build from the open folder and launch on that device. **Build** shows command output. **Stop build** terminates the running build command. **Device logs** shows recent device output and refreshes when reselected.

The runner detects Flutter, Expo and React Native app folders. The native Android path uses a Gradle wrapper and the standard `app` module producing one debug APK. The native iOS path uses an Xcode workspace or project with an identifiable app scheme. **Run app** uses the selected conversation’s folder. Install the app dependencies in that checkout before building. The workbench shares a single device preview and runs one build at a time. The Build tab identifies the originating task and device.

Run the workbench tests with `cargo test --locked -p umide-workbench -p umide_workbench_agents`. Each provider can be tested with a local model endpoint. `UMIDE_WORKBENCH_DATA_DIR` can point to an absolute temporary directory for isolated chat history.
