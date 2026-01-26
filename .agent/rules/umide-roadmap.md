---
trigger: always_on
---

# UMIDE Agent Prompt: Mobile IDE Development

You are an expert Rust developer building UMIDE, a unified IDE for cross-platform mobile development (React Native + Flutter). You are working from a Lapce fork that has already been cloned in the `umide` directory.

## Project Context

**Project Name:** UMIDE (Unified Mobile IDE)  
**Base:** Lapce (Rust + Floem GUI framework)  
**Goal:** Create an IDE that embeds Android Emulator and iOS Simulator directly as panels, eliminating context-switching for mobile developers.

**Tech Stack:**

- Language: Rust
- UI Framework: Floem
- Emulator Integration: gRPC (Android), simctl (iOS)
- Video Streaming: H.264 decode + GPU rendering

## Current Status

- Lapce has been cloned to `/umide` directory
- Lapce builds and runs successfully
- Ready to rename and begin mobile feature development

## Your Tasks (In Order)

### Phase 1: Rename Lapce to UMIDE (Week 1)

**Objective:** Rename the project from "Lapce" to "UMIDE" without breaking functionality.

**Steps:**

1. **Update root Cargo.toml:**
   - Change `[package] name = "lapce"` → `name = "umide"`
   - Change `[[bin]] name = "lapce"` → `name = "umide"`
   - Update `default-members = ["crates/lapce"]` → `["crates/umide"]`

2. **Rename core crate folder:**

   ```bash
   cd crates
   mv lapce umide
   ```

3. **Update crates/umide/Cargo.toml:**
   - Change `name = "lapce"` → `name = "umide"`

4. **Update platform metadata:**
   - **macOS:** `crates/umide/macos/Info.plist`
     - `<string>Lapce</string>` → `<string>UMIDE</string>`
     - `<string>com.lapce.app</string>` → `<string>com.umide.app</string>`
   - **Linux:** `crates/umide/linux/lapce.desktop` → `umide.desktop`
     - `Name=Lapce` → `Name=UMIDE`
   - **Windows:** `crates/umide/windows/installer.nsi`
     - `Name "Lapce"` → `Name "UMIDE"`

5. **Update README.md:**
   - Replace title from "Lapce" to "UMIDE"
   - Add tagline: "The Unified IDE for Cross-Platform Mobile Development"
   - Replace description (use the clean README provided)

6. **Update LICENSE:**
   - Add copyright line: `Copyright 2024 UMIDE contributors`
   - Keep Lapce attribution: `Portions (original editor) Copyright 2023 Lapce contributors`

7. **Search & replace in code:**

   ```bash
   # In UI strings, about dialogs, window titles
   grep -r "Lapce" src/ --include="*.rs" | head -20
   # Replace "Lapce" → "UMIDE" in about dialogs and window titles ONLY
   # Do NOT rename internal crate names (lapce_*)
   ```

8. **Build and verify:**

   ```bash
   cargo build --release
   ./target/release/umide
   # Verify: Window title shows "UMIDE", about dialog says "UMIDE"
   ```

9. **Commit:**
   ```bash
   git add -A
   git commit -m "refactor: rename Lapce to UMIDE (mobile IDE fork)"
   ```

**Success Criteria:**

- Binary is named `umide` (not `lapce`)
- App window title displays "UMIDE"
- All internal crate names remain unchanged (lapce\_\*)
- Builds without errors
- Runs without crashes

---

### Phase 2: Understand Floem & Lapce Architecture (Week 2-3)

**Objective:** Understand the codebase so you can add mobile features.

**Required Reading:**

1. `crates/umide/src/main.rs` — App entry point, window setup
2. `crates/umide/src/panel.rs` — How panels are created and managed
3. `crates/umide/src/editor.rs` — Editor UI and state management
4. `crates/lapce-ui/src/lib.rs` — UI components (buttons, inputs, layouts)

**Questions to Answer:**

- How does Lapce create and manage side panels?
- Where would you add a new panel for the emulator?
- How does Floem handle widget state and rendering?
- What's the data flow from user input to screen?

**Deliverable:** Write a document (`docs/architecture.md`) explaining:

- How panels work in Lapce/Floem
- Where you'll add the "Emulator Panel"
- Data flow for emulator video rendering

---

### Phase 3: Create Emulator Integration Module (Week 4-5)

**Objective:** Create a new Rust crate for emulator integration.

**Steps:**

1. **Create new crate structure:**

   ```bash
   mkdir -p crates/umide_emulator/{src,examples}
   cd crates/umide_emulator
   ```

2. **Create `Cargo.toml`:**

   ```toml
   [package]
   name = "umide_emulator"
   version = "0.1.0"
   edition = "2021"

   [dependencies]
   tokio = { version = "1", features = ["full"] }
   tonic = "0.12"
   prost = "0.13"
   anyhow = "1.0"
   serde = { version = "1.0", features = ["derive"] }
   serde_json = "1.0"
   ```

3. **Create module structure:**

   ```rust
   // crates/umide_emulator/src/lib.rs
   pub mod android;
   pub mod ios;
   pub mod common;

   pub use android::AndroidEmulator;
   pub use ios::iOSSimulator;
   ```

4. **Implement Android module (`src/android.rs`):**
   - Struct: `AndroidEmulator { device_id, grpc_address }`
   - Method: `connect()` → connects to emulator gRPC
   - Method: `get_screenshot()` → returns screenshot bytes
   - Method: `send_touch(x, y)` → sends touch event
   - Method: `stream_video()` → opens video stream (stub for now)

5. **Implement iOS module (`src/ios.rs`):**
   - Struct: `iOSSimulator { udid }`
   - Method: `detect_simulator()` → finds running iOS Simulator
   - Method: `get_screenshot()` → captures via simctl
   - Method: `send_touch(x, y)` → sends touch via simctl

6. **Add to root workspace:**
   - Add to root `Cargo.toml` members: `"crates/umide_emulator"`
   - Add dependency: `umide_emulator = { path = "crates/umide_emulator" }`

7. **Create tests:**
   ```bash
   cargo test -p umide_emulator
   ```

**Success Criteria:**

- New crate compiles without errors
- Can detect running emulators (print to console)
- Can capture screenshots
- Tests pass

---

### Phase 4: Add Emulator Panel to UI (Week 5-6)

**Objective:** Add a blank emulator panel to the right side of the editor.

**Steps:**

1. **Create emulator panel widget:**

   ```rust
   // crates/umide/src/emulator_panel.rs
   use floem::prelude::*;

   pub struct EmulatorPanel {
       device_type: String,  // "Android" or "iOS"
   }

   impl EmulatorPanel {
       pub fn new(device_type: String) -> Self {
           Self { device_type }
       }

       pub fn view(self) -> impl IntoView {
           container(
               text(format!("Emulator: {}", self.device_type))
           )
           .style(|s| s.width_full().height_full())
       }
   }
   ```

2. **Integrate into workspace:**
   - Find where panels are created in `src/main.rs`
   - Add emulator panel alongside editor panel
   - Layout: Left = file tree, Center = editor, Right = emulator

3. **Add to panel registry:**
   - Register emulator panel so it can be toggled on/off
   - Add menu item: "View" → "Show Emulator Panel"

4. **Build and test:**
   ```bash
   cargo build --release
   ./target/release/umide
   # Verify: Emulator panel appears on the right
   ```

**Success Criteria:**

- Emulator panel displays on right side
- Shows placeholder text (e.g., "Android Emulator" or "iOS Simulator")
- Panel can be toggled on/off
- No crashes

---

### Phase 5: Integrate Video Streaming (Week 6-8)

**Objective:** Display actual emulator video in the panel.

**Steps:**

1. **Add video decode dependencies:**

   ```toml
   # In crates/umide_emulator/Cargo.toml
   ffmpeg-sys-next = "4.4"  # Or use pure Rust decoder
   image = "0.25"
   ```

2. **Implement H.264 decoder:**

   ```rust
   // crates/umide_emulator/src/video.rs
   pub struct VideoDecoder {
       width: u32,
       height: u32,
   }

   impl VideoDecoder {
       pub fn decode_h264(&mut self, data: &[u8]) -> Result<Vec<u8>> {
           // Decode H.264 → RGBA pixels
           Ok(vec![])  // Stub
       }
   }
   ```

3. **Connect Android gRPC video stream:**

   ```rust
   // In crates/umide_emulator/src/android.rs
   pub async fn stream_video(&mut self) -> Result<impl Stream<Item = Vec<u8>>> {
       // Connect to emulator gRPC on port 5556
       // Request video stream
       // Decode frames in real-time
   }
   ```

4. **Create GPU texture from decoded video:**

   ```rust
   // Use Floem's rendering context to create textures
   // Render decoded frames to GPU texture
   ```

5. **Display in panel:**
   - Update `EmulatorPanel` to render video texture
   - Update at ~30fps

6. **Build and test:**

   ```bash
   # Launch Android Emulator with gRPC enabled
   ~/Android/Sdk/emulator/emulator -avd <avd_name> -grpc 5556

   cargo build --release
   ./target/release/umide
   # Verify: Video from emulator displays in right panel
   ```

**Success Criteria:**

- Android Emulator video displays in panel
- Latency < 200ms
- Runs at ~30fps
- No crashes on disconnect

---

### Phase 6: iOS Simulator Support (Week 8-9)

**Objective:** Add iOS Simulator video streaming.

**Steps:**

1. **Implement simctl integration:**

   ```rust
   // crates/umide_emulator/src/ios.rs
   pub async fn stream_video_loop(&self) -> Result<()> {
       loop {
           let screenshot = self.capture_screenshot()?;
           // Decode PNG
           // Send to panel
           tokio::time::sleep(Duration::from_millis(100)).await;
       }
   }
   ```

2. **Update panel to detect + display both:**
   - Check: Is Android Emulator running? Show Android stream
   - Check: Is iOS Simulator running? Show iOS stream
   - Allow switching between them

3. **Test:**

   ```bash
   # Launch iOS Simulator
   open /Applications/Xcode.app/Contents/Developer/Applications/Simulator.app

   cargo build --release
   ./target/release/umide
   # Verify: iOS Simulator video displays
   ```

**Success Criteria:**

- iOS Simulator video displays
- Can switch between Android and iOS
- Auto-detects running devices

---

### Phase 7: Touch Input Integration (Week 9-10)

**Objective:** Send touch events from panel to emulator.

**Steps:**

1. **Add mouse event handling to panel:**

   ```rust
   // In EmulatorPanel
   pub fn on_mouse_down(&mut self, x: f64, y: f64) {
       // Convert panel coordinates to emulator coordinates
       let emu_x = (x * device_width) as i32;
       let emu_y = (y * device_height) as i32;

       // Send to emulator
       emulator.send_touch(emu_x, emu_y)?;
   }
   ```

2. **Implement send_touch in Android:**

   ```rust
   pub async fn send_touch(&self, x: i32, y: i32) -> Result<()> {
       // Send via gRPC
   }
   ```

3. **Implement send_touch in iOS:**

   ```rust
   pub fn send_touch(&self, x: i32, y: i32) -> Result<()> {
       // Call simctl
       Command::new("xcrun")
           .args(&["simctl", "ui", &self.udid, "tap", &x.to_string(), &y.to_string()])
           .output()?;
       Ok(())
   }
   ```

4. **Test:**
   - Click in emulator panel
   - Verify touch events appear on device

**Success Criteria:**

- Can tap/click in emulator video
- App responds to touches
- Latency < 100ms

---

### Phase 8: Project Detection + Build System (Week 10-12)

**Objective:** Detect React Native/Flutter projects and integrate build systems.

**Steps:**

1. **Create project detection module:**

   ```rust
   // crates/umide/src/project_detector.rs
   pub enum ProjectType {
       ReactNative,
       Flutter,
       Unknown,
   }

   pub fn detect_project(path: &Path) -> ProjectType {
       if path.join("package.json").exists()
           && path.join("node_modules/react-native").exists() {
           return ProjectType::ReactNative;
       }
       if path.join("pubspec.yaml").exists() {
           return ProjectType::Flutter;
       }
       ProjectType::Unknown
   }
   ```

2. **Integrate Metro (React Native):**

   ```rust
   pub struct MetroBuild {
       process: Child,
   }

   impl MetroBuild {
       pub fn start(project_path: &Path) -> Result<Self> {
           let process = Command::new("npx")
               .args(&["react-native", "start"])
               .current_dir(project_path)
               .spawn()?;
           Ok(MetroBuild { process })
       }
   }
   ```
