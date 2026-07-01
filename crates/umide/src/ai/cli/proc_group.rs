//! Cross-platform process-group control for agent-CLI children.
//!
//! Agent CLIs spawn sub-shells and child tools, so killing only the direct
//! child (`Child::kill`) leaks the rest of the tree. We instead start each child
//! as the root of its own process group and, on cancel/timeout, signal the whole
//! group:
//!   * **unix** — `process_group(0)` makes the child a group leader (pgid == pid);
//!     `kill(-pgid, …)` then signals every process in the group.
//!   * **windows** — `CREATE_NEW_PROCESS_GROUP` plus `taskkill /T /F` to terminate
//!     the child and all of its descendants. (Avoids a heavyweight Job Object dep;
//!     `taskkill` ships with Windows.)

use std::process::Command as StdCommand;

/// Configure a freshly-built `std::process::Command` so its child starts in its
/// own process group. Call this before converting to a `tokio::process::Command`.
pub fn lead_new_group(cmd: &mut StdCommand) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // 0 → the child becomes a new process-group leader (pgid == its pid).
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }
}

/// Terminate the process group rooted at `pid`. On unix, `hard` selects
/// `SIGKILL` (true) vs `SIGTERM` (false) so the caller can try a graceful stop
/// first; on Windows `taskkill /F` is always forceful.
pub fn kill_group(pid: u32, hard: bool) {
    #[cfg(unix)]
    {
        let sig = if hard { libc::SIGKILL } else { libc::SIGTERM };
        // Negative pid → deliver the signal to the entire process group.
        unsafe {
            libc::kill(-(pid as i32), sig);
        }
    }
    #[cfg(windows)]
    {
        let _ = hard;
        let _ = StdCommand::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}
