//! Launch at login: register or unregister the app through
//! `SMAppService.mainAppService` (macOS 13+).
//!
//! Registration works only for a bundled, signed app. The caller checks
//! [`in_app_bundle`] first and logs instead of failing when the app runs
//! unbundled (`cargo run`).

use std::path::Path;

/// True when `exe` sits in `<Name>.app/Contents/MacOS/`. Pure: unit-tested on all OSes.
pub fn is_app_bundle_exe(exe: &Path) -> bool {
    let Some(macos) = exe.parent() else {
        return false;
    };
    let Some(contents) = macos.parent() else {
        return false;
    };
    let Some(app) = contents.parent() else {
        return false;
    };
    macos.file_name().is_some_and(|n| n == "MacOS")
        && contents.file_name().is_some_and(|n| n == "Contents")
        && app.extension().is_some_and(|e| e == "app")
}

/// True when the running executable is inside an app bundle. False on non-macOS.
pub fn in_app_bundle() -> bool {
    #[cfg(target_os = "macos")]
    {
        std::env::current_exe().is_ok_and(|exe| is_app_bundle_exe(&exe))
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Register (`true`) or unregister (`false`) the main app as a login item.
pub fn set_enabled(enabled: bool) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        macos::set_enabled(enabled)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = enabled;
        Err(anyhow::anyhow!("jisuno-platform: macOS only"))
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use anyhow::{anyhow, Result};
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject, Bool};
    use objc2_foundation::NSError;

    #[link(name = "ServiceManagement", kind = "framework")]
    extern "C" {}

    pub(super) fn set_enabled(enabled: bool) -> Result<()> {
        let class = AnyClass::get(c"SMAppService")
            .ok_or_else(|| anyhow!("SMAppService is unavailable (macOS 13 or later)"))?;
        // SAFETY: `mainAppService` is a class method returning the shared service object.
        let service: Retained<AnyObject> = unsafe { msg_send![class, mainAppService] };
        // SAFETY: `status` is an NSInteger property (0 not registered, 1 enabled,
        // 2 requires approval, 3 not found).
        let status: isize = unsafe { msg_send![&*service, status] };
        let registered = status == 1 || status == 2;
        if registered == enabled {
            return Ok(());
        }
        let mut error: *mut NSError = std::ptr::null_mut();
        // SAFETY: both selectors take one `NSError **` out-parameter and return BOOL.
        let ok: Bool = unsafe {
            if enabled {
                msg_send![&*service, registerAndReturnError: &mut error]
            } else {
                msg_send![&*service, unregisterAndReturnError: &mut error]
            }
        };
        if ok.as_bool() {
            return Ok(());
        }
        // SAFETY: on failure the out-parameter holds an autoreleased NSError; retain it.
        let reason = unsafe { Retained::retain(error) }
            .map(|e| e.localizedDescription().to_string())
            .unwrap_or_else(|| "unknown error".to_string());
        Err(anyhow!("{reason}"))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn bundle_exe_path_is_detected() {
        assert!(is_app_bundle_exe(Path::new(
            "/Applications/Jisuno.app/Contents/MacOS/jisuno"
        )));
        assert!(!is_app_bundle_exe(Path::new(
            "/Users/me/github/jisuno/target/release/jisuno"
        )));
        assert!(!is_app_bundle_exe(Path::new(
            "/Applications/Jisuno/Contents/MacOS/jisuno"
        )));
        assert!(!is_app_bundle_exe(Path::new("/Jisuno.app/MacOS/jisuno")));
    }

    #[test]
    fn stubs_on_other_oses() {
        if !cfg!(target_os = "macos") {
            assert!(!in_app_bundle());
            assert!(set_enabled(true).is_err());
        }
    }
}
