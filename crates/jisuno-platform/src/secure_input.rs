//! Secure-input detection: is a password field (or another app) holding the
//! keyboard, and if so, which app?
//!
//! While Secure Event Input is on, the event tap gets no keys and holds
//! cannot fire (`docs/design/keybindings.md` §8). The menu bar shows the lock
//! badge and the HUD names the holding app.

/// True while any process holds Secure Event Input. False on non-macOS.
pub fn is_secure_input_enabled() -> bool {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: IsSecureEventInputEnabled takes no arguments.
        unsafe { IsSecureEventInputEnabled() != 0 }
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

#[cfg(target_os = "macos")]
#[link(name = "Carbon", kind = "framework")]
extern "C" {
    fn IsSecureEventInputEnabled() -> u8;
}

/// The app holding Secure Event Input, if one could be identified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecureInputHolder {
    /// Process id of the holding app.
    pub pid: i32,
    /// Human-readable process name (`ps -o comm=`), if it could be read.
    pub name: Option<String>,
}

/// Best-effort holder lookup: parses `ioreg -l -w 0` for
/// `kCGSSessionSecureInputPID`, then resolves the pid to a process name.
///
/// Cost: spawns two subprocesses (tens of milliseconds). Do NOT call on a hot
/// path; poll at most ~1 Hz, only while [`is_secure_input_enabled`]. Returns
/// `None` when secure input is off, when the pid cannot be found, or on
/// non-macOS.
pub fn secure_input_holder() -> Option<SecureInputHolder> {
    #[cfg(target_os = "macos")]
    {
        if !is_secure_input_enabled() {
            return None;
        }
        let output = std::process::Command::new("ioreg")
            .args(["-l", "-w", "0"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let pid = parse_secure_input_pid(&text)?;
        let name = std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "comm="])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .filter(|name| !name.is_empty());
        Some(SecureInputHolder { pid, name })
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Parse the secure-input pid out of `ioreg -l -w 0` output.
/// Lines look like: `| |   "kCGSSessionSecureInputPID" = 501`.
/// Used by the macOS holder lookup and by unit tests on every OS.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_secure_input_pid(text: &str) -> Option<i32> {
    text.lines().find_map(|line| {
        let (_, rest) = line.split_once("kCGSSessionSecureInputPID")?;
        let (_, value) = rest.split_once('=')?;
        value.trim().parse::<i32>().ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ioreg_pid_line() {
        let text = "    | |   \"kCGSSessionSecureInputPID\" = 501\n    | |   \"other\" = 1\n";
        assert_eq!(parse_secure_input_pid(text), Some(501));
    }

    #[test]
    fn no_pid_means_none() {
        assert_eq!(parse_secure_input_pid("nothing here\n"), None);
        assert_eq!(
            parse_secure_input_pid("\"kCGSSessionSecureInputPID\" = abc\n"),
            None
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn stubs_on_other_oses() {
        assert!(!is_secure_input_enabled());
        assert_eq!(secure_input_holder(), None);
    }
}
