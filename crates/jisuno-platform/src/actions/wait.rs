//! `wait {window | ms}`: sleep, or poll for the window.
//!
//! `ms` sleeps that long (schema caps it at 10000). A `window` reference
//! (workspace id or app name) polls every 100 ms for up to 5 s and fails
//! with `timed out after 5 s` when nothing appears.

use std::time::Duration;

use super::ResolvedWindow;

/// Poll interval for window waits.
pub(crate) const WAIT_POLL: Duration = Duration::from_millis(100);
/// Window-wait budget: 50 polls x 100 ms.
pub(crate) const WAIT_POLLS: usize = 50;

/// Sleep `ms` milliseconds.
pub(crate) fn wait_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

/// Poll for the window up to 5 s.
pub(crate) fn wait_for_window(resolved: &ResolvedWindow) -> Result<(), String> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = resolved;
        return Err("jisuno-platform: macOS only".to_string());
    }
    #[cfg(target_os = "macos")]
    {
        for _ in 0..WAIT_POLLS {
            if window_present(resolved) {
                return Ok(());
            }
            std::thread::sleep(WAIT_POLL);
        }
        Err("timed out after 5 s".to_string())
    }
}

/// One poll: the window resolves (workspace def or running app) and has at
/// least the title-matching window when a title is given.
#[cfg(target_os = "macos")]
fn window_present(resolved: &ResolvedWindow) -> bool {
    use super::ax::macos as axm;

    let Some(pid) = axm::pid_for_app(&resolved.app) else {
        return false;
    };
    let Ok(windows) = axm::app_windows(pid) else {
        return false;
    };
    let titles = axm::window_titles(&windows);
    axm::pick_window(&windows, &titles, resolved.title.as_deref()).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wait_budget_is_five_seconds() {
        assert_eq!(WAIT_POLL, Duration::from_millis(100));
        assert_eq!(WAIT_POLLS, 50);
        assert_eq!(WAIT_POLL * WAIT_POLLS as u32, Duration::from_secs(5));
    }
}
