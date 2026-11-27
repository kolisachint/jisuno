//! `shell.run {cmd, cwd?}`: `/bin/zsh -lc <cmd>` in `cwd` (`~` expanded).
//!
//! Non-interactive: stdin is null, the shell is never a login prompt, and
//! the command runs behind the step runner's `confirm` gate (validation
//! rejects `shell.run` without `"confirm": true`). A 60 s timeout kills the
//! child's process group (the command runs in its own group); the failure reason stays the spec string (`shell exited <code>`,
//! `shell timed out after 60 s`). Stdout is discarded; stderr is drained (so a chatty
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
        run(
            cmd,
            if dir.is_empty() { None } else { Some(&dir) },
            SHELL_TIMEOUT,
        )
    }
}

#[cfg(target_os = "macos")]
fn run(cmd: &str, dir: Option<&str>, timeout: Duration) -> Result<(), String> {
    use std::io::Read;
    use std::os::unix::process::CommandExt as _;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;

    let mut command = Command::new("/bin/zsh");
    command.arg("-lc").arg(cmd);
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    // Own process group: a timeout kills the shell and everything it started.
    command.process_group(0);
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

    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    // Kill the whole group: grandchildren holding the pipes
                    // would otherwise keep the reader threads alive.
                    // SAFETY: the group id is the child's pid (process_group(0)).
                    unsafe {
                        libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
                    }
                    let _ = child.wait();
                    return Err("shell timed out after 60 s".to_string());
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

    #[cfg(target_os = "macos")]
    #[test]
    fn live_echo_succeeds_and_exit_code_surfaces() {
        assert!(run_command("echo jisuno-probe", None).is_ok());
        assert_eq!(
            run_command("exit 3", None),
            Err("shell exited 3".to_string())
        );
        assert!(run_command("echo hi", Some("~")).is_ok());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn timeout_kills_the_process_group() {
        let pid_file =
            std::env::temp_dir().join(format!("jisuno-shell-test-{}", std::process::id()));
        let cmd = format!("sleep 30 & echo $! > '{}'; sleep 30", pid_file.display());
        let result = run(&cmd, None, Duration::from_millis(500));
        assert_eq!(result, Err("shell timed out after 60 s".to_string()));
        // The background child must be gone too, not just the shell.
        let pid: libc::pid_t = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let _ = std::fs::remove_file(&pid_file);
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while unsafe { libc::kill(pid, 0) } == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_ne!(
            unsafe { libc::kill(pid, 0) },
            0,
            "grandchild {pid} survived"
        );
    }

    #[test]
    fn timeout_and_tail_sizes_are_sane() {
        assert_eq!(SHELL_TIMEOUT, Duration::from_secs(60));
        assert_eq!(STDERR_TAIL_BYTES, 4096);
    }
}
