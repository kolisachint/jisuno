//! `shell.run {cmd, cwd?}`: `/bin/zsh -lc <cmd>` in `cwd` (`~` expanded).
//!
//! Non-interactive: stdin is null, the shell is never a login prompt, and
//! the command runs behind the step runner's `confirm` gate (validation
//! rejects `shell.run` without `"confirm": true`). A 60 s timeout kills the
//! child; the failure reason stays the spec string (`shell exited <code>`,
//! `124` on timeout). Stdout is discarded; stderr is drained (so a chatty
//! child can never block on a full pipe) and only its tail is kept — the
//! reason text itself never carries output.

use std::time::Duration;

/// Kill-after timeout for the child.
pub(crate) const SHELL_TIMEOUT: Duration = Duration::from_secs(60);
/// How much stderr tail is kept (for diagnostics, not for the reason text).
pub(crate) const STDERR_TAIL_BYTES: usize = 4096;

/// Last `max` bytes of `bytes` as lossy text. Pure: unit-tested on all OSes.
pub(crate) fn stderr_tail(bytes: &[u8], max: usize) -> String {
    let start = bytes.len().saturating_sub(max);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

/// Run `cmd` via `/bin/zsh -lc` in `cwd` (or the process cwd when `None`).
pub(crate) fn run_command(cmd: &str, cwd: Option<&str>) -> Result<(), String> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (cmd, cwd);
        return Err("jisuno-platform: macOS only".to_string());
    }
    #[cfg(target_os = "macos")]
    {
        let dir = cwd
            .filter(|cwd| !cwd.is_empty())
            .map(super::expand_tilde_live)
            .unwrap_or_default();
        run(cmd, if dir.is_empty() { None } else { Some(&dir) })
    }
}

#[cfg(target_os = "macos")]
fn run(cmd: &str, dir: Option<&str>) -> Result<(), String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;

    let mut command = Command::new("/bin/zsh");
    command.arg("-lc").arg(cmd);
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("shell exited 1 ({error:#})"))?;

    // Drain both pipes on threads: a child that fills the pipe buffer must
    // never wedge the 60 s wait. Stdout is discarded; the stderr tail is
    // kept for diagnostics.
    let (tail_tx, tail_rx) = mpsc::channel();
    if let Some(stderr) = child.stderr.take() {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let mut stderr = stderr;
            let _ = stderr.read_to_end(&mut bytes);
            let _ = tail_tx.send(bytes);
        });
    }
    if let Some(mut stdout) = child.stdout.take() {
        std::thread::spawn(move || {
            let mut sink = Vec::new();
            let _ = stdout.read_to_end(&mut sink);
        });
    }

    let deadline = std::time::Instant::now() + SHELL_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("shell exited 124".to_string());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(format!("shell exited 1 ({error:#})")),
        }
    };
    let _tail = tail_rx
        .recv_timeout(Duration::from_secs(2))
        .map(|bytes| stderr_tail(&bytes, STDERR_TAIL_BYTES))
        .unwrap_or_default();
    if status.success() {
        Ok(())
    } else {
        Err(format!("shell exited {}", status.code().unwrap_or(1)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_keeps_the_end() {
        assert_eq!(stderr_tail(b"abc", 10), "abc");
        assert_eq!(stderr_tail(b"abcdef", 3), "def");
        assert_eq!(stderr_tail(b"", 8), "");
        // Lossy on invalid UTF-8, still bounded.
        let mut bytes = vec![b'x'; 100];
        bytes.extend_from_slice(&[0xFF, 0xFE]);
        let tail = stderr_tail(&bytes, 4);
        assert!(tail.len() <= 8, "bounded tail, got {tail:?}");
        assert!(tail.ends_with('\u{FFFD}'), "lossy tail, got {tail:?}");
    }

    #[test]
    fn timeout_and_tail_sizes_are_sane() {
        assert_eq!(SHELL_TIMEOUT, Duration::from_secs(60));
        assert_eq!(STDERR_TAIL_BYTES, 4096);
    }
}
