//! iOS Simulator window detection using CoreGraphics
//!
//! This module provides utilities to find iOS Simulator windows by their
//! device UDID so we can capture their screen content.

use std::process::Command;
use tracing::info;

/// Find the CGWindowID for an iOS Simulator by its device UDID
/// 
/// This uses AppleScript to find the Simulator window.
pub fn find_simulator_window(device_name: &str) -> Option<u32> {
    // First, try to find using the window list approach via osascript
    let output = Command::new("osascript")
        .args([
            "-e",
            &format!(
                r#"tell application "System Events"
                    set simApp to application process "Simulator"
                    set windowList to every window of simApp
                    repeat with w in windowList
                        set windowTitle to name of w
                        if windowTitle contains "{}" then
                            return id of w
                        end if
                    end repeat
                    return -1
                end tell"#,
                device_name
            ),
        ])
        .output()
        .ok()?;
    
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    info!("AppleScript result: {}", stdout);
    
    // Try to parse as window ID
    stdout.parse::<i64>().ok().and_then(|id| {
        if id > 0 { Some(id as u32) } else { None }
    })
}

/// List all Simulator windows
pub fn list_simulator_windows() -> Vec<(u32, String)> {
    // Use CoreGraphics window list via swift/objc would be better,
    // but for now we can use the screencapture tool approach
    let output = Command::new("osascript")
        .args([
            "-e",
            r#"tell application "System Events"
                set simApp to application process "Simulator"
                set windowList to every window of simApp
                set result to ""
                repeat with w in windowList
                    set result to result & (id of w) & ":" & (name of w) & "\n"
                end repeat
                return result
            end tell"#,
        ])
        .output()
        .ok();
    
    let mut windows = Vec::new();
    if let Some(output) = output {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if let Some((id_str, name)) = line.split_once(':') {
                if let Ok(id) = id_str.parse::<u32>() {
                    windows.push((id, name.to_string()));
                }
            }
        }
    }
    windows
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_list_simulator_windows() {
        let windows = list_simulator_windows();
        println!("Found {} simulator windows", windows.len());
        for (id, name) in &windows {
            println!("  Window {}: {}", id, name);
        }
    }
}
