//! Global key tap: hold keys, global chords, Esc/Return capture.
//!
//! The tap feeds [`jisuno_logic::hold::HoldMachine`]: hold keys and chords in,
//! capture decisions out. Design rules are `docs/design/keybindings.md` §4-§5:
//! a hold counts only if no other key is pressed during it, the first hold
//! key wins, Esc is captured only in `listening`/`confirming`, Return only in
//! `confirming`.
//!
//! macOS details:
//!
//! * Active (not listen-only) tap at [`CGEventTapLocation::HID`][hid] with head
//!   insert, so Esc/Return/global chords can be swallowed by returning NULL.
//!   Hold-key modifiers are never swallowed.
//! * Right modifiers are told apart from left ones via the device-dependent
//!   flag bits; left and right ⌘ share a keycode, so keycodes alone lie.
//! * The tap runs on its own thread with its own CFRunLoop; events travel to
//!   the host over a [`std::sync::mpsc`] channel.
//! * `kCGEventTapDisabledByTimeout` / `kCGEventTapDisabledByUserInput`
//!   re-enable the tap and emit [`KeyEvent::TapReenabled`]; the host resyncs
//!   its state machine on that event.
//! * The callback stays minimal: integer compares and a linear scan over the
//!   (few) configured chords. [`Key`] values are cloned only on a match.
//! * Events the crate posts itself (see [`crate::insert`]) carry a user-data
//!   tag and are passed through untouched, so synthetic Cmd+V can never
//!   re-trigger a chord.
//!
//! [hid]: https://developer.apple.com/documentation/coregraphics/cgeventtaplocation/hid

use std::sync::mpsc;

use jisuno_logic::keybindings::{HoldSide, Key, Modifier};

/// Device-dependent flag bits telling right modifiers from left ones.
/// Left and right ⌘ (and ⌥/⇧/⌃) share keycodes; these bits do not lie.
pub const NX_DEVICERCTLKEYMASK: u64 = 0x2000;
pub const NX_DEVICERALTKEYMASK: u64 = 0x40;
pub const NX_DEVICERCMDKEYMASK: u64 = 0x10;
pub const NX_DEVICERSHIFTKEYMASK: u64 = 0x04;

/// Tag written into `kCGEventSourceUserData` on events this crate posts, so
/// the tap can tell its own synthetic events apart from the user's keys.
pub const POSTED_EVENT_TAG: i64 = 0x6a69_7375_6e6f;

/// A right-side modifier key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RightKey {
    RightCtrl,
    RightOption,
    RightShift,
    RightCmd,
}

impl RightKey {
    /// The device-dependent flag bit identifying this key.
    pub fn device_bit(self) -> u64 {
        match self {
            Self::RightCtrl => NX_DEVICERCTLKEYMASK,
            Self::RightOption => NX_DEVICERALTKEYMASK,
            Self::RightCmd => NX_DEVICERCMDKEYMASK,
            Self::RightShift => NX_DEVICERSHIFTKEYMASK,
        }
    }

    /// All four, in a fixed order used when several bits flip at once.
    pub fn all() -> [RightKey; 4] {
        [
            Self::RightCtrl,
            Self::RightOption,
            Self::RightShift,
            Self::RightCmd,
        ]
    }
}

impl From<HoldSide> for RightKey {
    fn from(side: HoldSide) -> Self {
        match side {
            HoldSide::RightCtrl => Self::RightCtrl,
            HoldSide::RightOption => Self::RightOption,
            HoldSide::RightShift => Self::RightShift,
            HoldSide::RightCmd => Self::RightCmd,
        }
    }
}

impl From<RightKey> for HoldSide {
    fn from(key: RightKey) -> Self {
        match key {
            RightKey::RightCtrl => Self::RightCtrl,
            RightKey::RightOption => Self::RightOption,
            RightKey::RightShift => Self::RightShift,
            RightKey::RightCmd => Self::RightCmd,
        }
    }
}

/// Right modifiers currently held, decoded from raw `CGEventFlags` bits.
pub fn right_keys_in(flags: u64) -> Vec<RightKey> {
    RightKey::all()
        .into_iter()
        .filter(|key| flags & key.device_bit() != 0)
        .collect()
}

/// Hold transitions between two `CGEventFlags` snapshots: `(key, down)` for
/// each right modifier whose device bit flipped. Fixed order (Ctrl, Option,
/// Shift, Cmd); empty when nothing changed.
pub fn right_transitions(old_flags: u64, new_flags: u64) -> Vec<(RightKey, bool)> {
    RightKey::all()
        .into_iter()
        .filter_map(|key| {
            let was = old_flags & key.device_bit() != 0;
            let is = new_flags & key.device_bit() != 0;
            if was == is {
                None
            } else {
                Some((key, is))
            }
        })
        .collect()
}

/// US-ANSI keycode to keybindings file name (`w`, `space`, `f5`, ...).
/// Physical positions, not labels: other layouts move the labels around.
pub fn keycode_name(keycode: u16) -> Option<&'static str> {
    Some(match keycode {
        0x00 => "a",
        0x01 => "s",
        0x02 => "d",
        0x03 => "f",
        0x04 => "h",
        0x05 => "g",
        0x06 => "z",
        0x07 => "x",
        0x08 => "c",
        0x09 => "v",
        0x0B => "b",
        0x0C => "q",
        0x0D => "w",
        0x0E => "e",
        0x0F => "r",
        0x10 => "y",
        0x11 => "t",
        0x12 => "1",
        0x13 => "2",
        0x14 => "3",
        0x15 => "4",
        0x16 => "6",
        0x17 => "5",
        0x18 => "=",
        0x19 => "9",
        0x1A => "7",
        0x1B => "-",
        0x1C => "8",
        0x1D => "0",
        0x1E => "]",
        0x1F => "o",
        0x20 => "u",
        0x21 => "[",
        0x22 => "i",
        0x23 => "p",
        0x25 => "l",
        0x26 => "j",
        0x27 => "'",
        0x28 => "k",
        0x29 => ";",
        0x2A => "\\",
        0x2B => ",",
        0x2C => "/",
        0x2D => "n",
        0x2E => "m",
        0x2F => ".",
        0x32 => "`",
        0x24 => "return",
        0x30 => "tab",
        0x31 => "space",
        0x33 => "delete",
        0x35 => "escape",
        0x7A => "f1",
        0x78 => "f2",
        0x63 => "f3",
        0x76 => "f4",
        0x60 => "f5",
        0x61 => "f6",
        0x62 => "f7",
        0x64 => "f8",
        0x65 => "f9",
        0x6D => "f10",
        0x67 => "f11",
        0x6F => "f12",
        0x69 => "f13",
        0x6B => "f14",
        0x71 => "f15",
        0x6A => "f16",
        0x40 => "f17",
        0x4F => "f18",
        0x50 => "f19",
        0x5A => "f20",
        _ => return None,
    })
}

/// Reverse of [`keycode_name`]: file name to US-ANSI keycode.
pub fn keycode_for_name(name: &str) -> Option<u16> {
    Some(match name {
        "a" => 0x00,
        "s" => 0x01,
        "d" => 0x02,
        "f" => 0x03,
        "h" => 0x04,
        "g" => 0x05,
        "z" => 0x06,
        "x" => 0x07,
        "c" => 0x08,
        "v" => 0x09,
        "b" => 0x0B,
        "q" => 0x0C,
        "w" => 0x0D,
        "e" => 0x0E,
        "r" => 0x0F,
        "y" => 0x10,
        "t" => 0x11,
        "1" => 0x12,
        "2" => 0x13,
        "3" => 0x14,
        "4" => 0x15,
        "6" => 0x16,
        "5" => 0x17,
        "=" => 0x18,
        "9" => 0x19,
        "7" => 0x1A,
        "-" => 0x1B,
        "8" => 0x1C,
        "0" => 0x1D,
        "]" => 0x1E,
        "o" => 0x1F,
        "u" => 0x20,
        "[" => 0x21,
        "i" => 0x22,
        "p" => 0x23,
        "l" => 0x25,
        "j" => 0x26,
        "'" => 0x27,
        "k" => 0x28,
        ";" => 0x29,
        "\\" => 0x2A,
        "," => 0x2B,
        "/" => 0x2C,
        "n" => 0x2D,
        "m" => 0x2E,
        "." => 0x2F,
        "`" => 0x32,
        "return" => 0x24,
        "tab" => 0x30,
        "space" => 0x31,
        "delete" => 0x33,
        "escape" => 0x35,
        "f1" => 0x7A,
        "f2" => 0x78,
        "f3" => 0x63,
        "f4" => 0x76,
        "f5" => 0x60,
        "f6" => 0x61,
        "f7" => 0x62,
        "f8" => 0x64,
        "f9" => 0x65,
        "f10" => 0x6D,
        "f11" => 0x67,
        "f12" => 0x6F,
        "f13" => 0x69,
        "f14" => 0x6B,
        "f15" => 0x71,
        "f16" => 0x6A,
        "f17" => 0x40,
        "f18" => 0x4F,
        "f19" => 0x50,
        "f20" => 0x5A,
        _ => return None,
    })
}

/// Translate a key-down into a keybindings [`Key`]: modifiers in canonical
/// `ctrl+option+shift+cmd` order plus the unshifted key name. `None` when the
/// keycode has no entry in the US table.
pub fn translate_chord(shift: bool, ctrl: bool, alt: bool, cmd: bool, keycode: u16) -> Option<Key> {
    let name = keycode_name(keycode)?;
    let mut modifiers = Vec::with_capacity(4);
    if ctrl {
        modifiers.push(Modifier::Ctrl);
    }
    if alt {
        modifiers.push(Modifier::Opt);
    }
    if shift {
        modifiers.push(Modifier::Shift);
    }
    if cmd {
        modifiers.push(Modifier::Cmd);
    }
    Some(Key::Chord {
        modifiers,
        key: name.to_string(),
    })
}

/// Event delivered to the host on the channel from [`KeyTap::start`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyEvent {
    /// A configured hold key went down.
    HoldDown(RightKey),
    /// A configured hold key went up.
    HoldUp(RightKey),
    /// Any other key-down (or left-modifier press): breaks a hold-alone hold.
    OtherKeyDown,
    /// A configured non-hold global chord was pressed.
    Chord(Key),
    /// Esc was pressed (swallowed iff the host captures it).
    Escape,
    /// Return was pressed (swallowed iff the host captures it).
    Return,
    /// The tap was disabled and re-enabled; keys may have been missed.
    TapReenabled,
}

/// What the host may need to capture right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Captured {
    Escape,
    Return,
    Chord(Key),
}

/// Decides whether [`Captured`] input is swallowed (return NULL from the tap)
/// or passed through to the app. Called on the tap thread: keep it fast and
/// side-effect free. Hold-key modifiers are never swallowed. The usual body
/// consults the hold state machine (`captures()`).
pub type ShouldCapture = std::sync::Arc<dyn Fn(Captured) -> bool + Send + Sync>;

/// Tap configuration: which right modifiers are hold keys, which chords are
/// global. Esc/Return need no entry: they are always reported and the
/// [`ShouldCapture`] callback decides.
#[derive(Debug, Clone, Default)]
pub struct TapConfig {
    /// Right modifiers that count as hold keys (`HoldDown`/`HoldUp`).
    /// Any other right modifier press is [`KeyEvent::OtherKeyDown`].
    pub holds: Vec<RightKey>,
    /// Non-hold global chords to report as [`KeyEvent::Chord`].
    pub chords: Vec<Key>,
}

/// The running tap. Owns the tap thread; [`KeyTap::shutdown`] stops it.
/// Dropping without shutdown signals the runloop to stop but does not join.
pub struct KeyTap {
    #[cfg(target_os = "macos")]
    inner: macos::RunningTap,
}

impl KeyTap {
    /// Start the tap on its own CFRunLoop thread. Fails (without prompting)
    /// when Input Monitoring / Accessibility is not granted.
    pub fn start(
        config: TapConfig,
        should_capture: ShouldCapture,
    ) -> anyhow::Result<(KeyTap, mpsc::Receiver<KeyEvent>)> {
        #[cfg(target_os = "macos")]
        {
            macos::start(config, should_capture).map(|(inner, rx)| (KeyTap { inner }, rx))
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (config, should_capture);
            Err(anyhow::anyhow!("jisuno-platform: macOS only"))
        }
    }

    /// Stop the runloop and join the tap thread.
    pub fn shutdown(self) -> anyhow::Result<()> {
        #[cfg(target_os = "macos")]
        {
            self.inner.shutdown()
        }
        #[cfg(not(target_os = "macos"))]
        {
            Ok(())
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{mpsc, Arc, Mutex};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
    use core_graphics::event::{
        CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
        CallbackResult, EventField,
    };

    use super::*;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRunLoopStop(rl: *mut c_void);
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventTapEnable(tap: *const c_void, enable: bool);
    }

    /// One configured chord, precomputed to integers for the hot path.
    struct ChordMatch {
        mask: u8,
        keycode: u16,
        key: Key,
    }

    /// Device-independent modifier bits (the low word of `CGEventFlags`).
    const FLAG_SHIFT: u64 = 0x0002_0000;
    const FLAG_CTRL: u64 = 0x0004_0000;
    const FLAG_ALT: u64 = 0x0008_0000;
    const FLAG_CMD: u64 = 0x0010_0000;

    struct Shared {
        tx: mpsc::Sender<KeyEvent>,
        holds: Vec<RightKey>,
        chords: Vec<ChordMatch>,
        should_capture: ShouldCapture,
        last_flags: AtomicU64,
    }

    /// Modifier mask from raw flags: bit0 ctrl, bit1 opt, bit2 shift, bit3 cmd.
    fn chord_mask(flags: u64) -> u8 {
        (u8::from(flags & FLAG_CTRL != 0))
            | (u8::from(flags & FLAG_ALT != 0) << 1)
            | (u8::from(flags & FLAG_SHIFT != 0) << 2)
            | (u8::from(flags & FLAG_CMD != 0) << 3)
    }

    fn precompute(chords: &[Key]) -> Vec<ChordMatch> {
        let mut out = Vec::with_capacity(chords.len());
        for key in chords {
            let Key::Chord {
                modifiers,
                key: name,
            } = key
            else {
                continue;
            };
            let Some(keycode) = keycode_for_name(name) else {
                continue;
            };
            let mut mask = 0u8;
            for m in modifiers {
                mask |= match m {
                    Modifier::Ctrl => 1,
                    Modifier::Opt => 2,
                    Modifier::Shift => 4,
                    Modifier::Cmd => 8,
                };
            }
            out.push(ChordMatch {
                mask,
                keycode,
                key: key.clone(),
            });
        }
        out
    }

    fn emit(shared: &Shared, event: KeyEvent) {
        // The host went away: nothing to do.
        let _ = shared.tx.send(event);
    }

    fn on_flags_changed(shared: &Shared, flags: u64) -> CallbackResult {
        let old = shared.last_flags.swap(flags, Ordering::Relaxed);
        let mut saw_right = false;
        for (key, down) in right_transitions(old, flags) {
            saw_right = true;
            if shared.holds.contains(&key) {
                emit(
                    shared,
                    if down {
                        KeyEvent::HoldDown(key)
                    } else {
                        KeyEvent::HoldUp(key)
                    },
                );
            } else if down {
                // A non-hold right modifier press breaks hold-alone, like any
                // other key. Releases are silent.
                emit(shared, KeyEvent::OtherKeyDown);
            }
        }
        if saw_right {
            return CallbackResult::Keep;
        }
        // Left-modifier (or caps-lock) press breaks hold-alone too. Only
        // 0->1 transitions count; releases never break a hold.
        let newly = !old & flags;
        if newly & (FLAG_SHIFT | FLAG_CTRL | FLAG_ALT | FLAG_CMD | 0x0001_0000) != 0 {
            emit(shared, KeyEvent::OtherKeyDown);
        }
        CallbackResult::Keep
    }

    fn on_key_down(shared: &Shared, flags: u64, keycode: u16, repeat: bool) -> CallbackResult {
        // Esc and Return are always reported; swallowed only when captured.
        if keycode == 0x35 {
            if !repeat {
                emit(shared, KeyEvent::Escape);
            }
            return capture(shared, Captured::Escape);
        }
        if keycode == 0x24 {
            if !repeat {
                emit(shared, KeyEvent::Return);
            }
            return capture(shared, Captured::Return);
        }
        let mask = chord_mask(flags);
        let found = shared
            .chords
            .iter()
            .find(|c| c.mask == mask && c.keycode == keycode);
        match found {
            Some(chord) if !repeat => {
                emit(shared, KeyEvent::Chord(chord.key.clone()));
                capture(shared, Captured::Chord(chord.key.clone()))
            }
            Some(chord) => capture(shared, Captured::Chord(chord.key.clone())),
            None => {
                if !repeat {
                    emit(shared, KeyEvent::OtherKeyDown);
                }
                CallbackResult::Keep
            }
        }
    }

    fn capture(shared: &Shared, captured: Captured) -> CallbackResult {
        if (shared.should_capture)(captured) {
            CallbackResult::Drop
        } else {
            CallbackResult::Keep
        }
    }

    /// A runloop pointer that only ever crosses threads to be stopped.
    /// `CFRunLoopStop` is safe to call from any thread.
    #[derive(Clone, Copy)]
    struct RunLoopPtr(*mut c_void);

    // SAFETY: the pointer is only ever passed to `CFRunLoopStop`.
    unsafe impl Send for RunLoopPtr {}
    unsafe impl Sync for RunLoopPtr {}

    pub(super) struct RunningTap {
        runloop: Arc<Mutex<Option<RunLoopPtr>>>,
        thread: Option<JoinHandle<()>>,
    }

    // SAFETY: the pointer is only ever passed to CFRunLoopStop.
    unsafe impl Send for RunningTap {}
    unsafe impl Sync for RunningTap {}

    pub(super) fn start(
        config: TapConfig,
        should_capture: ShouldCapture,
    ) -> anyhow::Result<(RunningTap, mpsc::Receiver<KeyEvent>)> {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared {
            tx,
            holds: config.holds,
            chords: precompute(&config.chords),
            should_capture,
            last_flags: AtomicU64::new(0),
        });

        // CGEventTap is !Send, so it is created on the tap thread itself and
        // only the init result crosses back.
        let runloop: Arc<Mutex<Option<RunLoopPtr>>> = Arc::new(Mutex::new(None));
        let runloop_w = Arc::clone(&runloop);
        let (init_tx, init_rx) = mpsc::channel::<anyhow::Result<()>>();
        let thread = thread::spawn(move || {
            let tap = match CGEventTap::new(
                CGEventTapLocation::HID,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::Default,
                vec![
                    CGEventType::KeyDown,
                    CGEventType::KeyUp,
                    CGEventType::FlagsChanged,
                ],
                move |proxy, etype, event| {
                    if matches!(
                        etype,
                        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
                    ) {
                        // Re-enable and tell the host keys may have been missed.
                        // NOTE: `event` may be NULL here; never touch it.
                        unsafe { CGEventTapEnable(proxy, true) };
                        emit(&shared, KeyEvent::TapReenabled);
                        return CallbackResult::Keep;
                    }
                    // Our own synthetic events (insert) pass through untouched.
                    if event.get_integer_value_field(EventField::EVENT_SOURCE_USER_DATA)
                        == POSTED_EVENT_TAG
                    {
                        return CallbackResult::Keep;
                    }
                    if matches!(etype, CGEventType::FlagsChanged) {
                        return on_flags_changed(&shared, event.get_flags().bits());
                    }
                    if matches!(etype, CGEventType::KeyDown) {
                        let keycode = event
                            .get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE)
                            as u16;
                        let repeat = event
                            .get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT)
                            != 0;
                        return on_key_down(&shared, event.get_flags().bits(), keycode, repeat);
                    }
                    CallbackResult::Keep
                },
            ) {
                Ok(tap) => tap,
                Err(()) => {
                    let _ = init_tx.send(Err(anyhow::anyhow!(
                        "CGEventTapCreate failed: grant Input Monitoring (and Accessibility) to this process, then retry"
                    )));
                    return;
                }
            };
            let source = match tap.mach_port().create_runloop_source(0) {
                Ok(source) => source,
                Err(()) => {
                    let _ = init_tx.send(Err(anyhow::anyhow!(
                        "CFMachPort runloop source creation failed"
                    )));
                    return;
                }
            };
            let current = CFRunLoop::get_current();
            *runloop_w.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(RunLoopPtr(current.as_concrete_TypeRef() as *mut c_void));
            unsafe { current.add_source(&source, kCFRunLoopCommonModes) };
            tap.enable();
            let _ = init_tx.send(Ok(()));
            CFRunLoop::run_current();
            // `tap` drops here and invalidates the mach port.
        });

        init_rx
            .recv()
            .map_err(|_| anyhow::anyhow!("key tap thread died during startup"))??;
        Ok((
            RunningTap {
                runloop,
                thread: Some(thread),
            },
            rx,
        ))
    }

    impl RunningTap {
        pub(super) fn shutdown(mut self) -> anyhow::Result<()> {
            // Wait (briefly) for the tap thread to publish its runloop.
            for _ in 0..200 {
                let has = runloop_set(&self.runloop);
                if has {
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
            if let Some(rl) = runloop_take(&self.runloop) {
                unsafe { CFRunLoopStop(rl.0) };
            }
            if let Some(handle) = self.thread.take() {
                handle
                    .join()
                    .map_err(|_| anyhow::anyhow!("key tap thread panicked"))?;
            }
            Ok(())
        }
    }

    fn runloop_set(runloop: &Arc<Mutex<Option<RunLoopPtr>>>) -> bool {
        runloop.lock().unwrap_or_else(|e| e.into_inner()).is_some()
    }

    fn runloop_take(runloop: &Arc<Mutex<Option<RunLoopPtr>>>) -> Option<RunLoopPtr> {
        runloop.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    use core_foundation::base::TCFType as _;
}

#[cfg(test)]
mod tests {
    use super::*;
    use jisuno_logic::keybindings::defaults;

    #[test]
    fn right_device_bits_decode() {
        assert_eq!(right_keys_in(0), vec![]);
        assert_eq!(
            right_keys_in(NX_DEVICERALTKEYMASK),
            vec![RightKey::RightOption]
        );
        assert_eq!(
            right_keys_in(NX_DEVICERCMDKEYMASK),
            vec![RightKey::RightCmd]
        );
        assert_eq!(
            right_keys_in(NX_DEVICERSHIFTKEYMASK),
            vec![RightKey::RightShift]
        );
        assert_eq!(
            right_keys_in(NX_DEVICERCTLKEYMASK),
            vec![RightKey::RightCtrl]
        );
        // Left-side bits must not decode as right keys.
        assert_eq!(right_keys_in(0x20 | 0x08 | 0x02 | 0x01), vec![]);
    }

    #[test]
    fn right_transitions_press_and_release() {
        assert_eq!(
            right_transitions(0, NX_DEVICERALTKEYMASK),
            vec![(RightKey::RightOption, true)]
        );
        assert_eq!(
            right_transitions(NX_DEVICERALTKEYMASK, 0),
            vec![(RightKey::RightOption, false)]
        );
        assert_eq!(
            right_transitions(NX_DEVICERALTKEYMASK, NX_DEVICERALTKEYMASK),
            vec![]
        );
        // Simultaneous flips come out in fixed Ctrl/Option/Shift/Cmd order.
        assert_eq!(
            right_transitions(
                0,
                NX_DEVICERCMDKEYMASK | NX_DEVICERALTKEYMASK | NX_DEVICERSHIFTKEYMASK
            ),
            vec![
                (RightKey::RightOption, true),
                (RightKey::RightShift, true),
                (RightKey::RightCmd, true),
            ]
        );
    }

    #[test]
    fn chord_translation_matches_keybindings_spellings() {
        // translate_chord output must equal what keybindings.json parses to,
        // so tap matches and config lookups agree.
        let cases = [
            ((false, true, true, false, 0x0D), "ctrl+option+w"),
            ((false, true, true, false, 0x31), "ctrl+option+space"),
            ((false, true, true, false, 0x2B), "ctrl+option+,"),
            ((false, false, false, false, 0x35), "escape"),
            ((false, false, false, false, 0x24), "return"),
            ((true, true, true, true, 0x65), "ctrl+option+shift+cmd+f9"),
        ];
        for ((shift, ctrl, alt, cmd, code), file) in cases {
            let got = translate_chord(shift, ctrl, alt, cmd, code)
                .unwrap_or_else(|| panic!("no translation for {file}"));
            let want = defaults()
                .bindings
                .iter()
                .find(|b| b.key.file() == file)
                .map(|b| b.key.clone())
                .unwrap_or_else(|| parse_single(file));
            assert_eq!(got, want, "translation of {file}");
        }
    }

    fn parse_single(file: &str) -> Key {
        let text =
            format!("{{\"bindings\": [{{\"key\": \"{file}\", \"command\": \"model.warm\"}}]}}");
        jisuno_logic::keybindings::parse(&text)
            .expect("test key must parse")
            .bindings
            .pop()
            .expect("one binding")
            .key
    }

    #[test]
    fn chord_translation_modifiers_canonical_and_unknown() {
        // Modifier order is canonical whatever the press order.
        let key = translate_chord(true, true, true, true, 0x00).expect("a maps");
        assert_eq!(key.file(), "ctrl+option+shift+cmd+a");
        assert_eq!(key.display(), "⌃⌥⇧⌘A");
        // No modifiers: bare typing key.
        let bare = translate_chord(false, false, false, false, 0x00).expect("a maps");
        assert_eq!(bare.file(), "a");
        // Unknown keycode: no translation.
        assert_eq!(translate_chord(false, false, false, false, 0xFF), None);
    }

    #[test]
    fn keycode_tables_roundtrip() {
        // Every name with a keycode maps back to itself.
        let names = [
            "a", "w", "0", "9", "=", "-", "]", "o", "[", "'", ";", "\\", ",", "/", ".", "`",
            "return", "tab", "space", "delete", "escape", "f1", "f12", "f20",
        ];
        for name in names {
            let code = keycode_for_name(name).unwrap_or_else(|| panic!("no keycode for {name}"));
            assert_eq!(keycode_name(code), Some(name), "roundtrip {name}");
        }
        assert_eq!(keycode_for_name("hald"), None);
        assert_eq!(keycode_name(0xFF), None);
    }

    #[test]
    fn right_key_hold_side_roundtrip() {
        for side in [
            HoldSide::RightCtrl,
            HoldSide::RightOption,
            HoldSide::RightShift,
            HoldSide::RightCmd,
        ] {
            let key = RightKey::from(side);
            assert_eq!(HoldSide::from(key), side);
        }
    }
}
