//! `keys.send {keys}`: post the chord to the focused app.
//!
//! The chord spelling is parsed with
//! [`Key::parse`][jisuno_logic::keybindings::Key::parse]; the resulting
//! keycode (US-ANSI, see [`crate::keys`]) plus modifier flags are posted as
//! key-down/up to the frontmost app's pid. No focused app is
//! `no focused app`. A lone hold key (`right_option` et al.) posts its bare
//! modifier keycode.

use jisuno_logic::keybindings::{HoldSide, Key, Modifier};

/// Modifier flag bits for `CGEventFlags` (same as `NSEvent` masks).
pub(crate) const FLAG_SHIFT: u64 = 0x0002_0000;
pub(crate) const FLAG_CTRL: u64 = 0x0004_0000;
pub(crate) const FLAG_ALT: u64 = 0x0008_0000;
pub(crate) const FLAG_CMD: u64 = 0x0010_0000;

/// Bare modifier keycodes (left side; the tap cannot tell sides apart and
/// `keys.send` targets the focused app, so the side does not matter).
pub(crate) const KEYCODE_SHIFT: u16 = 0x38;
pub(crate) const KEYCODE_CTRL: u16 = 0x3B;
pub(crate) const KEYCODE_OPTION: u16 = 0x3A;
pub(crate) const KEYCODE_CMD: u16 = 0x37;

/// Map a parsed key to (US-ANSI keycode, modifier flags). `None` when the
/// key name has no keycode. Pure: unit-tested on all OSes.
pub(crate) fn chord_keycode_flags(key: &Key) -> Option<(u16, u64)> {
    match key {
        Key::Hold(side) => {
            let keycode = match side {
                HoldSide::RightCtrl => KEYCODE_CTRL,
                HoldSide::RightOption => KEYCODE_OPTION,
                HoldSide::RightShift => KEYCODE_SHIFT,
                HoldSide::RightCmd => KEYCODE_CMD,
            };
            Some((keycode, 0))
        }
        Key::Chord { modifiers, key } => {
            let keycode = crate::keys::keycode_for_name(key)?;
            let mut flags = 0u64;
            for modifier in modifiers {
                flags |= match modifier {
                    Modifier::Ctrl => FLAG_CTRL,
                    Modifier::Opt => FLAG_ALT,
                    Modifier::Shift => FLAG_SHIFT,
                    Modifier::Cmd => FLAG_CMD,
                };
            }
            Some((keycode, flags))
        }
    }
}

/// Post `keys` to the focused app.
pub(crate) fn send_keys(keys: &str) -> Result<(), String> {
    let key = Key::parse(keys).map_err(|error| error.to_string())?;
    let Some((keycode, flags)) = chord_keycode_flags(&key) else {
        return Err(format!("unknown key \"{keys}\""));
    };
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (keycode, flags);
        return Err("jisuno-platform: macOS only".to_string());
    }
    #[cfg(target_os = "macos")]
    {
        macos::post_key(keycode, flags)
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use core_graphics::event::{CGEvent, CGEventTapLocation, EventField};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    use super::super::super::keys::POSTED_EVENT_TAG;

    pub(super) fn post_key(keycode: u16, flags: u64) -> Result<(), String> {
        let Some(pid) = crate::frontmost::frontmost_app().map(|app| app.pid) else {
            return Err("no focused app".to_string());
        };
        post(keycode, true, flags, pid)?;
        post(keycode, false, flags, pid)?;
        Ok(())
    }

    fn post(keycode: u16, down: bool, flags: u64, pid: i32) -> Result<(), String> {
        use core_graphics::event::CGEventFlags;

        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|()| "keys.send failed: no event source".to_string())?;
        let event = CGEvent::new_keyboard_event(source, keycode, down)
            .map_err(|()| "keys.send failed: event creation failed".to_string())?;
        event.set_flags(CGEventFlags::from_bits_retain(flags));
        event.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, POSTED_EVENT_TAG);
        event.post_to_pid(pid);
        // `post_to_pid` is fire-and-forget; reaching it is success.
        let _ = CGEventTapLocation::Session;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(raw: &str) -> Key {
        Key::parse(raw).unwrap_or_else(|error| panic!("{raw} must parse: {error}"))
    }

    #[test]
    fn chord_maps_to_keycode_and_flags() {
        assert_eq!(
            chord_keycode_flags(&parse("ctrl+option+w")),
            Some((0x0D, FLAG_CTRL | FLAG_ALT))
        );
        assert_eq!(chord_keycode_flags(&parse("cmd+c")), Some((0x08, FLAG_CMD)));
        assert_eq!(
            chord_keycode_flags(&parse("ctrl+option+shift+cmd+f5")),
            Some((0x60, FLAG_CTRL | FLAG_ALT | FLAG_SHIFT | FLAG_CMD))
        );
        assert_eq!(chord_keycode_flags(&parse("escape")), Some((0x35, 0)));
        assert_eq!(chord_keycode_flags(&parse("return")), Some((0x24, 0)));
        assert_eq!(chord_keycode_flags(&parse("space")), Some((0x31, 0)));
        // Bare typing key: no modifiers.
        assert_eq!(
            chord_keycode_flags(&parse("ctrl+a")),
            Some((0x00, FLAG_CTRL))
        );
    }

    #[test]
    fn hold_maps_to_bare_modifier_keycode() {
        assert_eq!(
            chord_keycode_flags(&parse("right_option")),
            Some((KEYCODE_OPTION, 0))
        );
        assert_eq!(
            chord_keycode_flags(&parse("right_cmd")),
            Some((KEYCODE_CMD, 0))
        );
    }

    #[test]
    fn bad_spelling_is_a_parse_error() {
        assert!(send_keys("ctrl+hald").is_err());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn stub_on_other_oses() {
        assert_eq!(
            send_keys("cmd+c"),
            Err("jisuno-platform: macOS only".to_string())
        );
    }
}
