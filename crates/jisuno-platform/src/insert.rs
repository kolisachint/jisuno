//! Text insertion into the focused app (`jisuno.md` §10 round 4.5-6).
//!
//! * [`InsertMode::Paste`]: save every pasteboard item (types + data), write
//!   the text, post Cmd+V to the target pid, then restore the saved items
//!   after ~500 ms on a background thread — but only if the pasteboard
//!   changeCount is still ours. If writing the pasteboard fails, fall back to
//!   [`InsertMode::Type`]. If posting Cmd+V fails, the clipboard is restored
//!   and the error returned (no fallback: the text may already be inserted).
//! * [`InsertMode::Type`]: synthetic key events via
//!   `CGEventKeyboardSetUnicodeString`, in chunks of at most
//!   [`TYPE_CHUNK_UNITS`] UTF-16 units (never splitting a surrogate pair).
//!
//! Text goes to the app that had focus at key-down. If that pid is gone by
//! key-up, nothing is inserted anywhere: [`insert_text`] returns
//! [`FocusLost`]. Call from the main thread (AppKit pasteboard access).

/// How the text reaches the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InsertMode {
    /// Paste via the clipboard with save/restore (default).
    #[default]
    Paste,
    /// Synthetic key events. Fallback for `insert_type_apps` and failed paste.
    Type,
}

/// The target app quit (or its window closed) before key-up, or there was no
/// target app at all (`pid` is `None`): nothing was inserted anywhere; the
/// host keeps the text in Recent and shows "Focus lost: text in Recent".
/// Match with `anyhow::Error::downcast_ref`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusLost {
    pub pid: Option<i32>,
}

impl std::fmt::Display for FocusLost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.pid {
            Some(pid) => write!(f, "focus lost: target pid {pid} is gone"),
            None => write!(f, "focus lost: no target app"),
        }
    }
}

impl std::error::Error for FocusLost {}

/// Insert `text` into `target_pid`. `None` returns [`FocusLost`] and posts
/// nothing: events are never sent to the session, where they would land in
/// whatever app happens to be focused. Empty text is a no-op. When
/// `restore_clipboard` is false, Paste leaves the inserted text on the
/// clipboard.
///
/// Clipboard restore runs on a background thread ~500 ms after the paste, so
/// the calling process must outlive the call by at least that long or the
/// restore is lost (the text is already inserted either way).
pub fn insert_text(
    text: &str,
    target_pid: Option<i32>,
    mode: InsertMode,
    restore_clipboard: bool,
) -> anyhow::Result<()> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (text, target_pid, mode, restore_clipboard);
        return Err(anyhow::anyhow!("jisuno-platform: macOS only"));
    }
    #[cfg(target_os = "macos")]
    {
        use anyhow::Context;

        if text.is_empty() {
            return Ok(());
        }
        let Some(pid) = target_pid else {
            return Err(FocusLost { pid: None }.into());
        };
        if !macos::pid_alive(pid) {
            return Err(FocusLost { pid: Some(pid) }.into());
        }
        match mode {
            InsertMode::Paste => match macos::insert_via_paste(text, pid, restore_clipboard) {
                Ok(()) => Ok(()),
                Err(paste_err) => {
                    if paste_err
                        .downcast_ref::<macos::ClipboardWriteFailed>()
                        .is_none()
                    {
                        return Err(paste_err);
                    }
                    // Nothing was posted: fall back to typing.
                    macos::insert_via_type(text, pid).context("paste failed, type failed")
                }
            },
            InsertMode::Type => macos::insert_via_type(text, pid),
        }
    }
}

/// Max UTF-16 code units per synthetic key event.
pub const TYPE_CHUNK_UNITS: usize = 20;

/// Split `text` into chunks of at most `max_units` UTF-16 code units, never
/// splitting a surrogate pair. Pure: unit-tested on all OSes.
pub fn utf16_chunks(text: &str, max_units: usize) -> Vec<Vec<u16>> {
    let max_units = max_units.max(1);
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < units.len() {
        let mut end = (start + max_units).min(units.len());
        // Never split a surrogate pair: back off if `end` lands between a
        // lead surrogate and its trail.
        if end < units.len()
            && units[end - 1] >= 0xD800
            && units[end - 1] <= 0xDBFF
            && units[end] >= 0xDC00
            && units[end] <= 0xDFFF
        {
            end -= 1;
        }
        chunks.push(units[start..end].to_vec());
        start = end;
    }
    chunks
}

#[cfg(target_os = "macos")]
mod macos {
    use std::thread;
    use std::time::Duration;

    use core_graphics::event::{CGEvent, EventField};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    use objc2::rc::Retained;
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
    use objc2_foundation::{NSArray, NSData, NSString};

    use super::super::keys::POSTED_EVENT_TAG;

    /// Restore delay after Cmd+V. The target app reads the pasteboard
    /// asynchronously; if it is slower than this it pastes the restored
    /// content instead. Raised from 150 ms for that reason.
    const RESTORE_DELAY: Duration = Duration::from_millis(500);

    /// Tag every posted event so the key tap passes our own events through.
    const PASTE_KEYCODE: u16 = 0x09; // ANSI_V
    const CMD_FLAG: u64 = 0x0010_0000;

    /// The pasteboard refused our text. Nothing was posted, so the caller may
    /// fall back to typing.
    #[derive(Debug)]
    pub(super) struct ClipboardWriteFailed;

    impl std::fmt::Display for ClipboardWriteFailed {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("writing the text to the pasteboard failed")
        }
    }

    impl std::error::Error for ClipboardWriteFailed {}

    pub(super) fn pid_alive(pid: i32) -> bool {
        // SAFETY: kill(pid, 0) performs no action; ESRCH means the pid is gone
        // (EPERM means it exists but is not ours).
        let result = unsafe { libc::kill(pid, 0) };
        if result == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }

    fn post_key(keycode: u16, down: bool, flags: u64, pid: i32) -> anyhow::Result<()> {
        use core_graphics::event::CGEventFlags;
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|()| anyhow::anyhow!("CGEventSourceCreate failed"))?;
        let event = CGEvent::new_keyboard_event(source, keycode, down)
            .map_err(|()| anyhow::anyhow!("CGEventCreateKeyboardEvent failed"))?;
        event.set_flags(CGEventFlags::from_bits_retain(flags));
        event.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, POSTED_EVENT_TAG);
        event.post_to_pid(pid);
        Ok(())
    }

    fn post_unicode(chunk: &[u16], down: bool, pid: i32) -> anyhow::Result<()> {
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|()| anyhow::anyhow!("CGEventSourceCreate failed"))?;
        let event = CGEvent::new_keyboard_event(source, 0, down)
            .map_err(|()| anyhow::anyhow!("CGEventCreateKeyboardEvent failed"))?;
        event.set_string_from_utf16_unchecked(chunk);
        event.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, POSTED_EVENT_TAG);
        event.post_to_pid(pid);
        Ok(())
    }

    pub(super) fn insert_via_type(text: &str, pid: i32) -> anyhow::Result<()> {
        if crate::permissions::accessibility() != crate::permissions::PermissionState::Granted {
            anyhow::bail!("Can't insert: Accessibility is off");
        }
        for chunk in super::utf16_chunks(text, super::TYPE_CHUNK_UNITS) {
            post_unicode(&chunk, true, pid)?;
            post_unicode(&chunk, false, pid)?;
        }
        Ok(())
    }

    pub(super) fn insert_via_paste(
        text: &str,
        pid: i32,
        restore_clipboard: bool,
    ) -> anyhow::Result<()> {
        if crate::permissions::accessibility() != crate::permissions::PermissionState::Granted {
            anyhow::bail!("Can't insert: Accessibility is off");
        }
        let pasteboard = NSPasteboard::generalPasteboard();

        // Save every item: types plus raw data.
        let saved: Vec<(String, Vec<u8>)> = pasteboard
            .types()
            .map(|types| {
                types
                    .iter()
                    .filter_map(|kind| {
                        pasteboard
                            .dataForType(&kind)
                            .map(|data| (kind.to_string(), data.to_vec()))
                    })
                    .collect()
            })
            .unwrap_or_default();

        pasteboard.clearContents();
        let string = NSString::from_str(text);
        // SAFETY: reading a framework-provided constant.
        let string_type = unsafe { NSPasteboardTypeString };
        let wrote = pasteboard.setString_forType(&string, string_type);
        if !wrote {
            // Restore what was there and let the caller fall back to Type.
            restore_items(&pasteboard, &saved);
            return Err(ClipboardWriteFailed.into());
        }
        let ours = pasteboard.changeCount();

        let posted = post_key(PASTE_KEYCODE, true, CMD_FLAG, pid)
            .and_then(|()| post_key(PASTE_KEYCODE, false, CMD_FLAG, pid));
        if let Err(e) = posted {
            // Nothing reached the app: put the old clipboard back. No typing
            // fallback, so the text cannot be inserted twice.
            restore_items(&pasteboard, &saved);
            return Err(e.context(format!("posting Cmd+V to pid {pid} failed")));
        }

        if restore_clipboard {
            // Limits: the check and the restore are not atomic, so a copy in
            // the gap between them can be overwritten; and a target app that
            // reads the pasteboard after RESTORE_DELAY pastes the old content.
            thread::spawn(move || {
                thread::sleep(RESTORE_DELAY);
                // Only restore if nobody else touched the clipboard since.
                let pasteboard = NSPasteboard::generalPasteboard();
                if pasteboard.changeCount() == ours {
                    restore_items(&pasteboard, &saved);
                }
            });
        }
        Ok(())
    }

    /// Best-effort clipboard restore: re-declare the saved types, write back
    /// each blob. Partial restores are acceptable; the clipboard is shared.
    fn restore_items(pasteboard: &NSPasteboard, saved: &[(String, Vec<u8>)]) {
        pasteboard.clearContents();
        if saved.is_empty() {
            return;
        }
        let types: Vec<Retained<NSString>> = saved
            .iter()
            .map(|(kind, _)| NSString::from_str(kind))
            .collect();
        let array = NSArray::from_retained_slice(&types);
        // SAFETY: NSPasteboardType is an NSString; owner nil promises nothing.
        unsafe { pasteboard.declareTypes_owner(&array, None) };
        for ((_, bytes), ns_kind) in saved.iter().zip(types.iter()) {
            let data = NSData::with_bytes(bytes);
            pasteboard.setData_forType(Some(&data), ns_kind);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_ascii_by_limit() {
        let chunks = utf16_chunks("hello world, this is long", 20);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), 20);
        assert_eq!(chunks[1].len(), 5);
        let back: String = chunks
            .iter()
            .flat_map(|c| char::decode_utf16(c.iter().copied()))
            .collect::<Result<String, _>>()
            .expect("valid utf-16");
        assert_eq!(back, "hello world, this is long");
    }

    #[test]
    fn chunks_never_split_surrogate_pairs() {
        // "a" + grinning face (2 units) + "b".
        let text = "a\u{1F600}b";
        assert_eq!(text.encode_utf16().count(), 4);
        let chunks = utf16_chunks(text, 2);
        // [a], [face], [b]: the face never straddles a boundary.
        assert_eq!(chunks.len(), 3);
        for chunk in &chunks {
            let s = String::from_utf16(chunk).expect("no lone surrogates");
            assert!(!s.is_empty());
        }
        let back: String = chunks
            .iter()
            .flat_map(|c| char::decode_utf16(c.iter().copied()))
            .collect::<Result<String, _>>()
            .expect("valid utf-16");
        assert_eq!(back, text);
    }

    #[test]
    fn chunks_edge_cases() {
        assert_eq!(utf16_chunks("", 20), Vec::<Vec<u16>>::new());
        assert_eq!(utf16_chunks("ab", 20).len(), 1);
        // max_units of 0 is clamped to 1 rather than looping forever.
        assert_eq!(utf16_chunks("ab", 0).len(), 2);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn stub_on_other_oses() {
        let err = insert_text("hi", Some(1), InsertMode::Paste, true).unwrap_err();
        assert_eq!(err.to_string(), "jisuno-platform: macOS only");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn dead_pid_is_focus_lost_and_live_pid_is_not() {
        // A reaped child is gone; our own pid is alive.
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("spawn true");
        let pid = child.id() as i32;
        child.wait().expect("reap true");
        assert!(
            !super::macos::pid_alive(pid),
            "reaped child {pid} must read as dead"
        );
        let err = insert_text("hi", Some(pid), InsertMode::Paste, true).unwrap_err();
        assert!(
            err.downcast_ref::<FocusLost>().is_some(),
            "dead pid must surface FocusLost, got {err:?}"
        );
        assert!(super::macos::pid_alive(std::process::id() as i32));
    }
}
