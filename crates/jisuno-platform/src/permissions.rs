//! Permission checks, prompts, and System Settings deep links.
//!
//! Jisuno needs Microphone (record only while a hold key is down),
//! Accessibility (insert text, move windows), and Input Monitoring (the hold
//! keys work in every app). First run checks each live and links the matching
//! pane (`docs/design/app-ui.md` §6).

/// The state of one permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionState {
    Granted,
    Denied,
    /// Never asked, or the system could not tell (treated as not granted).
    NotDetermined,
}

/// Which System Settings pane to open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Microphone,
    Accessibility,
    InputMonitoring,
}

/// Deep-link URL for `pane` (`app-ui.md` §6). Pure: unit-tested on all OSes.
pub fn settings_url(pane: Pane) -> &'static str {
    match pane {
        Pane::Microphone => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
        }
        Pane::Accessibility => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        }
        Pane::InputMonitoring => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
        }
    }
}

/// Open the System Settings pane for `pane` so the user can grant it.
pub fn open_settings_pane(pane: Pane) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let status = std::process::Command::new("open")
            .arg(settings_url(pane))
            .status()
            .map_err(|e| anyhow::anyhow!("opening System Settings failed: {e:#}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(anyhow::anyhow!("opening System Settings failed: {status}"))
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pane;
        Err(anyhow::anyhow!("jisuno-platform: macOS only"))
    }
}

/// Microphone record permission (`AVCaptureDevice audio`).
pub fn microphone() -> PermissionState {
    #[cfg(target_os = "macos")]
    {
        macos::microphone_state()
    }
    #[cfg(not(target_os = "macos"))]
    {
        PermissionState::NotDetermined
    }
}

/// Show the microphone prompt if never asked, then report the outcome.
/// Blocks until the user answers; returns the fresh state.
pub fn request_microphone() -> anyhow::Result<PermissionState> {
    #[cfg(target_os = "macos")]
    {
        macos::request_microphone()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(anyhow::anyhow!("jisuno-platform: macOS only"))
    }
}

/// Accessibility trust (`AXIsProcessTrusted`).
pub fn accessibility() -> PermissionState {
    #[cfg(target_os = "macos")]
    {
        if macos::ax_trusted(false) {
            PermissionState::Granted
        } else {
            // NotDetermined vs Denied is indistinguishable here; the Settings
            // pane shows the prompt state. Report Denied so the host links
            // Settings rather than waiting.
            PermissionState::Denied
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        PermissionState::NotDetermined
    }
}

/// Show the Accessibility prompt (System Settings) if not trusted.
/// Returns whether the process is trusted now (false until the user grants
/// and relaunches attention to the dialog; re-check with [`accessibility`]).
pub fn request_accessibility() -> anyhow::Result<bool> {
    #[cfg(target_os = "macos")]
    {
        Ok(macos::ax_trusted(true))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(anyhow::anyhow!("jisuno-platform: macOS only"))
    }
}

/// Input Monitoring (`IOHIDCheckAccess(ListenEvent)`).
pub fn input_monitoring() -> PermissionState {
    #[cfg(target_os = "macos")]
    {
        match macos::hid_check_access() {
            0 => PermissionState::Granted,       // kIOHIDAccessTypeGranted
            1 => PermissionState::Denied,        // kIOHIDAccessTypeDenied
            _ => PermissionState::NotDetermined, // kIOHIDAccessTypeUnknown
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        PermissionState::NotDetermined
    }
}

/// Show the Input Monitoring prompt. Returns the system's answer; the grant
/// only lands after the user approves in Settings, so re-check with
/// [`input_monitoring`] afterwards.
pub fn request_input_monitoring() -> anyhow::Result<bool> {
    #[cfg(target_os = "macos")]
    {
        Ok(macos::hid_request_access())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(anyhow::anyhow!("jisuno-platform: macOS only"))
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::sync::mpsc;

    use anyhow::Context;
    use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};

    pub(super) const HID_LISTEN_EVENT: i32 = 1; // kIOHIDRequestTypeListenEvent

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
        fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
        static kAXTrustedCheckOptionPrompt: *const c_void;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFBooleanTrue: *const c_void;
        // Addresses only; the structs are never read from Rust.
        static kCFTypeDictionaryKeyCallBacks: u8;
        static kCFTypeDictionaryValueCallBacks: u8;
        fn CFDictionaryCreate(
            allocator: *const c_void,
            keys: *const *const c_void,
            values: *const *const c_void,
            num_values: isize,
            key_callbacks: *const c_void,
            value_callbacks: *const c_void,
        ) -> *mut c_void;
        fn CFRelease(cf: *const c_void);
    }

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOHIDCheckAccess(request: i32) -> i32;
        fn IOHIDRequestAccess(request: i32) -> bool;
    }

    pub(super) fn hid_check_access() -> i32 {
        // SAFETY: pure query, no arguments to get wrong.
        unsafe { IOHIDCheckAccess(HID_LISTEN_EVENT) }
    }

    pub(super) fn hid_request_access() -> bool {
        // SAFETY: pure prompt call.
        unsafe { IOHIDRequestAccess(HID_LISTEN_EVENT) }
    }

    /// `prompt`: show the System Settings dialog when not trusted.
    pub(super) fn ax_trusted(prompt: bool) -> bool {
        if !prompt {
            // SAFETY: pure query.
            return unsafe { AXIsProcessTrusted() };
        }
        // SAFETY: one documented key/value pair under the default callbacks;
        // the dictionary is released after the call.
        unsafe {
            let keys = [kAXTrustedCheckOptionPrompt];
            let values = [kCFBooleanTrue];
            let options = CFDictionaryCreate(
                std::ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                1,
                &kCFTypeDictionaryKeyCallBacks as *const u8 as *const c_void,
                &kCFTypeDictionaryValueCallBacks as *const u8 as *const c_void,
            );
            if options.is_null() {
                return AXIsProcessTrusted();
            }
            let trusted = AXIsProcessTrustedWithOptions(options);
            CFRelease(options);
            trusted
        }
    }

    fn audio_media_type() -> anyhow::Result<&'static objc2_av_foundation::AVMediaType> {
        // SAFETY: reading a framework-provided constant.
        unsafe { AVMediaTypeAudio }.context("AVMediaTypeAudio is missing")
    }

    pub(super) fn microphone_state() -> super::PermissionState {
        use super::PermissionState;
        let Ok(media) = audio_media_type() else {
            return PermissionState::NotDetermined;
        };
        // SAFETY: media is a valid AVMediaType; the call is a pure query.
        match unsafe { AVCaptureDevice::authorizationStatusForMediaType(media) } {
            AVAuthorizationStatus::Authorized => PermissionState::Granted,
            AVAuthorizationStatus::NotDetermined => PermissionState::NotDetermined,
            _ => PermissionState::Denied, // Restricted and Denied alike.
        }
    }

    pub(super) fn request_microphone() -> anyhow::Result<super::PermissionState> {
        let media = audio_media_type()?;
        let (tx, rx) = mpsc::channel();
        let handler = block2::RcBlock::new(move |granted: objc2::runtime::Bool| {
            let _ = tx.send(granted.as_bool());
        });
        // SAFETY: AVFoundation copies the block; the completion runs on an
        // arbitrary queue and only touches the channel.
        unsafe {
            AVCaptureDevice::requestAccessForMediaType_completionHandler(media, &handler);
        }
        let granted = rx
            .recv()
            .map_err(|_| anyhow::anyhow!("microphone prompt answered after receiver was gone"))?;
        Ok(if granted {
            super::PermissionState::Granted
        } else {
            super::PermissionState::Denied
        })
    }

    use std::ffi::c_void;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_urls_match_app_ui_spec() {
        assert_eq!(
            settings_url(Pane::Microphone),
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
        );
        assert_eq!(
            settings_url(Pane::Accessibility),
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        );
        assert_eq!(
            settings_url(Pane::InputMonitoring),
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn stubs_on_other_oses() {
        assert_eq!(microphone(), PermissionState::NotDetermined);
        assert!(request_microphone().is_err());
        assert!(request_accessibility().is_err());
        assert!(request_input_monitoring().is_err());
        assert!(open_settings_pane(Pane::Microphone).is_err());
    }
}
