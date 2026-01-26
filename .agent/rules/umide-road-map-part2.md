---
trigger: always_on
---

3. **Integrate Flutter:**

   ```rust
   pub struct FlutterBuild {
       process: Child,
   }

   impl FlutterBuild {
       pub fn start(project_path: &Path, device_id: &str) -> Result<Self> {
           let process = Command::new("flutter")
               .args(&["run", "-d", device_id])
               .current_dir(project_path)
               .spawn()?;
           Ok(FlutterBuild { process })
       }
   }
   ```

4. **Add hot reload trigger:**
   - On file save, trigger rebuild
   - Parse build output for errors
   - Display errors in editor

5. **Test:**
   - Open React Native project
   - Verify build starts automatically
   - Change code, save, see hot reload

**Success Criteria:**

- Projects auto-detected
- Build systems orchestrated
- Hot reload works
- Errors displayed in editor

---

## Non-Technical Requirements

### Code Quality

- Keep code clean and well-documented
- Write comments for complex logic
- Add unit tests for new modules
- Follow Rust naming conventions

### Git Workflow

- Commit frequently (after each feature)
- Use clear commit messages: `feat: add emulator panel`, `fix: video latency`
- Push weekly to GitHub

### Documentation

- Keep README.md updated with progress
- Add architecture docs as you go
- Record any blockers or design decisions

## Communication

**Report Progress Weekly:**

- What was completed
- What blockers were hit (if any)
- What's next
- Any help needed

## Success Criteria (Overall)

By end of Week 12, UMIDE should:

- Embed Android Emulator with video + touch
- Embed iOS Simulator with video + touch
- Detect React Native + Flutter projects
- Trigger hot reload on file save
- Parse and display build errors
- Be stable enough for 10+ developers to beta test

## Stack Overflow / Blockers

If you hit a blocker:

1. Document the issue clearly
2. What have you tried?
3. What's the expected vs actual behavior?
4. Ask for help with specific question

## Resources

- Lapce GitHub: https://github.com/lapce/lapce
- Floem Docs: https://github.com/lapce/floem
- Android Emulator gRPC: https://developer.android.com/studio/run/emulator-commandline
- React Native CLI: https://reactnative.dev/docs/environment-setup
- Flutter docs: https://flutter.dev/docs

---
