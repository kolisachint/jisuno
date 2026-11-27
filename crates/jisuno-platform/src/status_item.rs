//! Menu bar status item: the template `waveform` glyph, state updates, and
//! the minimal menu (model state, Warm/Unload, permission rows, Quit).
//!
//! Runtime choice (see `docs/design/jisuno.md` §10 round 3.2): GPUI is the
//! app runtime (it owns the run loop and the HUD window), because
//! `gpui-pre` 0.3 has no status-item API. The `NSStatusItem` is created
//! alongside it with objc2 on the main thread, inside the GPUI run closure.
//! Menu actions travel back to the app over a channel; the app polls it on
//! its tick.
//!
//! Icon states follow `app-ui.md` §2: an SF Symbol glyph per state, the
//! listening glyphs in the mode colour, and 9x9 pt badges (lock while Secure
//! Input holds the keyboard, warning on an error). Idle and loading glyphs
//! are template images; the listening and badged images are drawn in colour
//! (`app-ui.md` §12). The pure mapping (symbol, tint, badges, label, colours)
//! is unit-tested on every platform.

use std::sync::mpsc;

use crate::permissions::Pane;

/// An action from the status item. Sent to the app thread.
///
/// Left-click toggles the popover ([`MenuAction::TogglePopover`]); the
/// fallback menu (right-click, or VoiceOver) still carries the rows below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    /// The status button was left-clicked: toggle the popover window.
    TogglePopover,
    WarmModel,
    UnloadModel,
    OpenPane(Pane),
    OpenSettings,
    OpenWelcome,
    Quit,
}

/// Icon states from `app-ui.md` §2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconState {
    /// Nothing running. Dimmed while the model is not in memory.
    Idle {
        model_warm: bool,
    },
    ListeningDictate,
    ListeningCommand,
    Loading,
    Error,
}

impl IconState {
    /// VoiceOver label per state (`app-ui.md` §2).
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle { .. } => "Jisuno, idle",
            Self::ListeningDictate => "Jisuno, dictating",
            Self::ListeningCommand => "Jisuno, command",
            Self::Loading => "Jisuno, loading model",
            Self::Error => "Jisuno, needs attention",
        }
    }
}

/// Listening glyph tint: the `dictate_text` and `command_text` tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tint {
    Dictate,
    Command,
}

/// Badges drawn over the glyph, bottom right; the lock sits left of the
/// warning when both show (`app-ui.md` §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Badge {
    Lock,
    Warning,
}

/// SF Symbol for the glyph of each state (names exist on macOS 13+).
pub fn glyph_symbol(state: IconState) -> &'static str {
    match state {
        IconState::Idle { .. } | IconState::Loading | IconState::Error => "waveform",
        IconState::ListeningDictate => "waveform.circle.fill",
        IconState::ListeningCommand => "command.circle.fill",
    }
}

/// The glyph tint: listening states only.
pub fn glyph_tint(state: IconState) -> Option<Tint> {
    match state {
        IconState::ListeningDictate => Some(Tint::Dictate),
        IconState::ListeningCommand => Some(Tint::Command),
        IconState::Idle { .. } | IconState::Loading | IconState::Error => None,
    }
}

/// Badges to draw, in order from left to right.
pub fn badges(warning: bool, secure: bool) -> Vec<Badge> {
    let mut out = Vec::new();
    if secure {
        out.push(Badge::Lock);
    }
    if warning {
        out.push(Badge::Warning);
    }
    out
}

/// SF Symbol for a badge.
pub fn badge_symbol(badge: Badge) -> &'static str {
    match badge {
        Badge::Lock => "lock.fill",
        Badge::Warning => "exclamationmark.triangle.fill",
    }
}

/// `0xRRGGBB` for a tint: `app-ui.md` §1.1 (light, dark).
pub fn tint_hex(tint: Tint, dark: bool) -> u32 {
    match (tint, dark) {
        (Tint::Dictate, false) => 0x09_5a_d7,
        (Tint::Dictate, true) => 0x7f_af_fa,
        (Tint::Command, false) => 0x86_5c_0e,
        (Tint::Command, true) => 0xe8_a0_1c,
    }
}

/// `0xRRGGBB` for a badge fill: `text` for the lock, `warning` for the
/// warning (`app-ui.md` §2, §1.1).
pub fn badge_hex(badge: Badge, dark: bool) -> u32 {
    match (badge, dark) {
        (Badge::Lock, false) => 0x1f_22_25,
        (Badge::Lock, true) => 0xe6_e6_e6,
        (Badge::Warning, false) => 0x9e_5f_09,
        (Badge::Warning, true) => 0xe0_a2_3c,
    }
}

/// VoiceOver label: the state label with the badge suffixes (`app-ui.md` §2).
pub fn accessibility_label(state: IconState, warning: bool, secure: bool) -> String {
    let mut label = state.label().to_string();
    if warning && state != IconState::Error {
        label.push_str(", needs attention");
    }
    if secure {
        label.push_str(", secure input on");
    }
    label
}

/// The running status item. Main-thread only; the app owns it for its whole
/// lifetime. Dropping removes the item from the menu bar.
pub struct StatusItem {
    #[cfg(target_os = "macos")]
    inner: macos_impl::Inner,
}

impl StatusItem {
    /// Create the item, install the menu, and wire `sender` to the menu
    /// actions. Must run on the main thread.
    pub fn new(sender: mpsc::Sender<MenuAction>) -> anyhow::Result<Self> {
        #[cfg(target_os = "macos")]
        {
            Ok(Self {
                inner: macos_impl::create(sender)?,
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = sender;
            Err(anyhow::anyhow!("jisuno-platform: macOS only"))
        }
    }

    /// Update the glyph label and badges. Badges: `warning` shows the `!`
    /// suffix, `secure` the lock suffix (left of the warning when both).
    pub fn set_icon(&self, state: IconState, warning: bool, secure: bool) {
        #[cfg(target_os = "macos")]
        {
            self.inner.set_icon(state, warning, secure);
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (state, warning, secure);
        }
    }

    /// Screen frame of the status button `(x, y, w, h)`, for anchoring the
    /// popover window under the item. Main thread only, like everything here.
    pub fn button_frame(&self) -> Option<(f64, f64, f64, f64)> {
        #[cfg(target_os = "macos")]
        {
            self.inner.button_frame()
        }
        #[cfg(not(target_os = "macos"))]
        {
            None
        }
    }

    /// Click the status button as if the user clicked the item: used by the
    /// `jisuno.menu` voice command (toggles the popover, like a click).
    /// Main thread only, like everything here.
    pub fn pop_menu(&self) {
        #[cfg(target_os = "macos")]
        {
            self.inner.pop_menu();
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = self;
        }
    }

    /// The first menu row: `Warm · ~0.9 GB · unloads in 10 min`,
    /// `Not loaded · loads on first use`, `Loading…`, `Not downloaded`.
    pub fn set_model_line(&self, text: &str) {
        #[cfg(target_os = "macos")]
        {
            self.inner.set_item_title(macos_impl::ITEM_MODEL, text);
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = text;
        }
    }

    /// Permission rows: `Microphone — Allowed` / `Microphone — Not allowed`.
    pub fn set_permission(&self, pane: Pane, granted: bool) {
        #[cfg(target_os = "macos")]
        {
            let tag = match pane {
                Pane::Microphone => macos_impl::ITEM_MIC,
                Pane::Accessibility => macos_impl::ITEM_AX,
                Pane::InputMonitoring => macos_impl::ITEM_HID,
            };
            let name = match pane {
                Pane::Microphone => "Microphone",
                Pane::Accessibility => "Accessibility",
                Pane::InputMonitoring => "Input Monitoring",
            };
            let state = if granted { "Allowed" } else { "Not allowed" };
            self.inner.set_item_title(tag, &format!("{name} — {state}"));
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (pane, granted);
        }
    }
}

#[cfg(target_os = "macos")]
mod macos_impl {
    use std::sync::Mutex;

    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject};
    use objc2::{define_class, sel, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSAccessibility, NSAppearanceCustomization, NSApplication, NSColor, NSEventMask,
        NSEventModifierFlags, NSEventType, NSImage, NSImageSymbolConfiguration, NSMenu, NSMenuItem,
        NSStatusBar, NSStatusBarButton, NSStatusItem, NSView,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

    use super::*;

    pub(super) const ITEM_MODEL: isize = 1;
    const ITEM_WARM: isize = 2;
    const ITEM_UNLOAD: isize = 3;
    pub(super) const ITEM_MIC: isize = 4;
    pub(super) const ITEM_AX: isize = 5;
    pub(super) const ITEM_HID: isize = 6;
    const ITEM_QUIT: isize = 7;
    const ITEM_SETTINGS: isize = 8;
    const ITEM_WELCOME: isize = 9;

    fn tag_action(tag: isize) -> Option<MenuAction> {
        match tag {
            ITEM_WARM => Some(MenuAction::WarmModel),
            ITEM_UNLOAD => Some(MenuAction::UnloadModel),
            ITEM_MIC => Some(MenuAction::OpenPane(Pane::Microphone)),
            ITEM_AX => Some(MenuAction::OpenPane(Pane::Accessibility)),
            ITEM_HID => Some(MenuAction::OpenPane(Pane::InputMonitoring)),
            ITEM_SETTINGS => Some(MenuAction::OpenSettings),
            ITEM_WELCOME => Some(MenuAction::OpenWelcome),
            ITEM_QUIT => Some(MenuAction::Quit),
            _ => None,
        }
    }

    struct TargetIvars {
        sender: Mutex<mpsc::Sender<MenuAction>>,
        /// The fallback menu, popped on right-click. Set once in `create`.
        menu: Mutex<Option<Retained<NSMenu>>>,
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "JisunoMenuTarget"]
        #[ivars = TargetIvars]
        struct MenuTarget;

        impl MenuTarget {
            #[unsafe(method(jisunoMenuAction:))]
            fn on_action(&self, item: Option<&NSMenuItem>) {
                let tag = item.map(|i| i.tag()).unwrap_or(-1);
                if let Some(action) = tag_action(tag) {
                    let sender = self.ivars().sender.lock().unwrap_or_else(|e| e.into_inner());
                    let _ = sender.send(action);
                }
            }

            /// Status button click: left-click toggles the popover,
            /// right-click pops the fallback menu at the button.
            #[unsafe(method(jisunoClick:))]
            fn on_click(&self, button: Option<&NSStatusBarButton>) {
                let right = MainThreadMarker::new()
                    .and_then(|mtm| NSApplication::sharedApplication(mtm).currentEvent())
                    .map(|event| event.r#type() == NSEventType::RightMouseUp)
                    .unwrap_or(false);
                if right {
                    self.pop_fallback(button);
                    return;
                }
                let sender = self.ivars().sender.lock().unwrap_or_else(|e| e.into_inner());
                let _ = sender.send(MenuAction::TogglePopover);
            }
        }
    );

    impl MenuTarget {
        fn new(sender: mpsc::Sender<MenuAction>, mtm: MainThreadMarker) -> Retained<Self> {
            let this = mtm.alloc::<Self>();
            let this = this.set_ivars(TargetIvars {
                sender: Mutex::new(sender),
                menu: Mutex::new(None),
            });
            unsafe { objc2::msg_send![super(this), init] }
        }

        fn as_object(&self) -> &AnyObject {
            self.as_super().as_super()
        }

        /// Pop the fallback menu under `button` (right-click path).
        fn pop_fallback(&self, button: Option<&NSStatusBarButton>) {
            let (Some(button), Some(menu)) = (
                button,
                self.ivars()
                    .menu
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone(),
            ) else {
                return;
            };
            let view: &NSView = button.as_super().as_super().as_super();
            let height = view.bounds().size.height;
            menu.popUpMenuPositioningItem_atLocation_inView(
                None,
                NSPoint { x: 0.0, y: height },
                Some(view),
            );
        }
    }

    /// Badge square, pt (`app-ui.md` §2).
    const BADGE_PT: f64 = 9.0;

    /// SF Symbol image by name, with the accessibility description.
    fn symbol(name: &str, description: &str) -> Option<Retained<NSImage>> {
        NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(name),
            Some(&NSString::from_str(description)),
        )
    }

    fn rgb(hex: u32) -> Retained<NSColor> {
        let channel = |shift: u32| f64::from((hex >> shift) & 0xff) / 255.0;
        NSColor::colorWithSRGBRed_green_blue_alpha(channel(16), channel(8), channel(0), 1.0)
    }

    /// A symbol drawn in one colour (non-template).
    fn colored(image: &NSImage, colour: &NSColor) -> Option<Retained<NSImage>> {
        let config = NSImageSymbolConfiguration::configurationWithHierarchicalColor(colour);
        image.imageWithSymbolConfiguration(&config)
    }

    /// Dark menu bar: the appearance name carries `Dark`.
    fn is_dark(button: &NSStatusBarButton) -> bool {
        button
            .effectiveAppearance()
            .name()
            .to_string()
            .contains("Dark")
    }

    /// Paint the state on the button: the glyph, then the badges, drawn into
    /// one image. A plain idle or loading glyph stays a template.
    fn paint(
        button: &NSStatusBarButton,
        state: IconState,
        warning: bool,
        secure: bool,
        dark: bool,
    ) {
        let label = accessibility_label(state, warning, secure);
        let Some(glyph) = symbol(glyph_symbol(state), state.label()) else {
            return;
        };
        let tint = glyph_tint(state);
        let marks = badges(warning, secure);
        if tint.is_none() && marks.is_empty() {
            glyph.setTemplate(true);
            button.setImage(Some(&glyph));
            return;
        }
        let glyph_colour = match tint {
            Some(Tint::Dictate) => rgb(tint_hex(Tint::Dictate, dark)),
            Some(Tint::Command) => rgb(tint_hex(Tint::Command, dark)),
            None => NSColor::labelColor(),
        };
        let Some(glyph) = colored(&glyph, &glyph_colour) else {
            return;
        };
        let size = glyph.size();
        // Badge i (from the right) sits at x = width - BADGE_PT * (i + 1).
        let placed: Vec<(Retained<NSImage>, NSRect)> = marks
            .iter()
            .rev()
            .enumerate()
            .filter_map(|(i, badge)| {
                let image = symbol(badge_symbol(*badge), &label)?;
                let colour = rgb(badge_hex(*badge, dark));
                let x = size.width - BADGE_PT * (i as f64 + 1.0);
                let rect = NSRect::new(NSPoint::new(x, 0.0), NSSize::new(BADGE_PT, BADGE_PT));
                Some((colored(&image, &colour)?, rect))
            })
            .collect();
        let handler = block2::RcBlock::new(move |rect: NSRect| -> objc2::runtime::Bool {
            glyph.drawInRect(rect);
            for (image, at) in &placed {
                image.drawInRect(*at);
            }
            objc2::runtime::Bool::YES
        });
        let image = NSImage::imageWithSize_flipped_drawingHandler(size, false, &handler);
        image.setTemplate(false);
        image.setAccessibilityDescription(Some(&NSString::from_str(&label)));
        button.setImage(Some(&image));
    }

    pub(super) struct Inner {
        item: Retained<NSStatusItem>,
        _target: Retained<MenuTarget>,
        menu: Retained<NSMenu>,
        mtm: MainThreadMarker,
    }

    impl Inner {
        pub(super) fn set_item_title(&self, tag: isize, text: &str) {
            for child in self.menu.itemArray().iter() {
                if child.tag() == tag {
                    child.setTitle(&NSString::from_str(text));
                    return;
                }
            }
        }

        /// Click the status button (toggles the popover, like a click).
        /// Used by the `jisuno.menu` voice command.
        pub(super) fn pop_menu(&self) {
            if let Some(button) = self.item.button(self.mtm) {
                // SAFETY: the button is a live AppKit object on the main
                // thread; its action reports back over the channel.
                unsafe { button.performClick(None) };
            }
        }

        /// Screen frame of the status button `(x, y, w, h)`.
        pub(super) fn button_frame(&self) -> Option<(f64, f64, f64, f64)> {
            let button = self.item.button(self.mtm)?;
            let view: &NSView = button.as_super().as_super().as_super();
            let window = view.window()?;
            let frame = window.frame();
            Some((
                frame.origin.x,
                frame.origin.y,
                frame.size.width,
                frame.size.height,
            ))
        }

        pub(super) fn set_icon(&self, state: IconState, warning: bool, secure: bool) {
            let Some(button) = self.item.button(self.mtm) else {
                return;
            };
            paint(&button, state, warning, secure, is_dark(&button));
            button.setTitle(&NSString::from_str(""));
            // Idle with the model out of memory: the glyph at 40 % (`app-ui.md` §2).
            let dimmed = state == (IconState::Idle { model_warm: false });
            button.setAlphaValue(if dimmed { 0.4 } else { 1.0 });
            let label = NSString::from_str(&accessibility_label(state, warning, secure));
            button.setToolTip(Some(&label));
            button.setAccessibilityLabel(Some(&label));
        }
    }

    pub(super) fn create(sender: mpsc::Sender<MenuAction>) -> anyhow::Result<Inner> {
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| anyhow::anyhow!("status item must be created on the main thread"))?;
        let target = MenuTarget::new(sender, mtm);
        let target_obj: &AnyObject = target.as_object();

        let bar = NSStatusBar::systemStatusBar();
        let item = bar.statusItemWithLength(objc2_app_kit::NSVariableStatusItemLength);

        if let Some(button) = item.button(mtm) {
            let idle = IconState::Idle { model_warm: false };
            paint(&button, idle, false, false, is_dark(&button));
            button.setTitle(&NSString::from_str(""));
            button.setAlphaValue(0.4);
            button.setToolTip(Some(&NSString::from_str(idle.label())));
            button.setAccessibilityLabel(Some(&NSString::from_str(idle.label())));
            // Left-click toggles the popover; right-click pops the
            // fallback menu. The menu is no longer the button's menu, so
            // a plain click reaches the action instead of opening it.
            unsafe { button.setTarget(Some(target.as_object())) };
            unsafe { button.setAction(Some(sel!(jisunoClick:))) };
            button.sendActionOn(NSEventMask::LeftMouseUp | NSEventMask::RightMouseUp);
        }

        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        unsafe {
            let add = |title: &str, tag: isize, key: &str, mods: NSEventModifierFlags| {
                let child = menu.addItemWithTitle_action_keyEquivalent(
                    &NSString::from_str(title),
                    Some(sel!(jisunoMenuAction:)),
                    &NSString::from_str(key),
                );
                child.setTag(tag);
                child.setKeyEquivalentModifierMask(mods);
                child.setTarget(Some(target_obj));
                child
            };
            let model = add(
                "Not loaded · loads on first use",
                ITEM_MODEL,
                "",
                NSEventModifierFlags::empty(),
            );
            model.setEnabled(false);
            add(
                "Warm model",
                ITEM_WARM,
                "w",
                NSEventModifierFlags::Control | NSEventModifierFlags::Option,
            );
            add(
                "Unload model",
                ITEM_UNLOAD,
                "",
                NSEventModifierFlags::empty(),
            );
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            add(
                "Microphone — …",
                ITEM_MIC,
                "",
                NSEventModifierFlags::empty(),
            );
            add(
                "Accessibility — …",
                ITEM_AX,
                "",
                NSEventModifierFlags::empty(),
            );
            add(
                "Input Monitoring — …",
                ITEM_HID,
                "",
                NSEventModifierFlags::empty(),
            );
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            add(
                "Settings…",
                ITEM_SETTINGS,
                ",",
                NSEventModifierFlags::Control | NSEventModifierFlags::Option,
            );
            add("Welcome…", ITEM_WELCOME, "", NSEventModifierFlags::empty());
            add("Quit Jisuno", ITEM_QUIT, "q", NSEventModifierFlags::empty());
        }
        // No `setMenu`: the button action reports clicks (left toggles the
        // popover, right pops this menu). VoiceOver and the `jisuno.menu`
        // voice command reach the same rows through the popover/click path.
        *target
            .ivars()
            .menu
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(menu.clone());

        Ok(Inner {
            item,
            _target: target,
            menu,
            mtm,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DICTATE: IconState = IconState::ListeningDictate;
    const COMMAND: IconState = IconState::ListeningCommand;
    const IDLE: IconState = IconState::Idle { model_warm: true };

    #[test]
    fn each_state_has_its_symbol() {
        assert_eq!(glyph_symbol(IDLE), "waveform");
        assert_eq!(glyph_symbol(DICTATE), "waveform.circle.fill");
        assert_eq!(glyph_symbol(COMMAND), "command.circle.fill");
        assert_eq!(glyph_symbol(IconState::Loading), "waveform");
        assert_eq!(glyph_symbol(IconState::Error), "waveform");
    }

    /// Only the two listening states carry a colour; the rest are templates.
    #[test]
    fn only_listening_states_are_tinted() {
        assert_eq!(glyph_tint(DICTATE), Some(Tint::Dictate));
        assert_eq!(glyph_tint(COMMAND), Some(Tint::Command));
        assert_eq!(glyph_tint(IDLE), None);
        assert_eq!(glyph_tint(IconState::Loading), None);
        assert_eq!(glyph_tint(IconState::Error), None);
    }

    #[test]
    fn lock_sits_left_of_the_warning() {
        assert_eq!(badges(false, false), Vec::<Badge>::new());
        assert_eq!(badges(true, false), vec![Badge::Warning]);
        assert_eq!(badges(false, true), vec![Badge::Lock]);
        assert_eq!(badges(true, true), vec![Badge::Lock, Badge::Warning]);
        assert_eq!(badge_symbol(Badge::Lock), "lock.fill");
        assert_eq!(
            badge_symbol(Badge::Warning),
            "exclamationmark.triangle.fill"
        );
    }

    #[test]
    fn accent_and_badge_colours_match_the_spec() {
        assert_eq!(tint_hex(Tint::Dictate, false), 0x09_5a_d7);
        assert_eq!(tint_hex(Tint::Dictate, true), 0x7f_af_fa);
        assert_eq!(tint_hex(Tint::Command, false), 0x86_5c_0e);
        assert_eq!(tint_hex(Tint::Command, true), 0xe8_a0_1c);
        assert_eq!(badge_hex(Badge::Lock, false), 0x1f_22_25);
        assert_eq!(badge_hex(Badge::Warning, true), 0xe0_a2_3c);
    }

    #[test]
    fn labels_carry_the_badge_suffixes() {
        assert_eq!(accessibility_label(IDLE, false, false), "Jisuno, idle");
        assert_eq!(
            accessibility_label(IDLE, false, true),
            "Jisuno, idle, secure input on"
        );
        assert_eq!(
            accessibility_label(DICTATE, false, true),
            "Jisuno, dictating, secure input on"
        );
        assert_eq!(
            accessibility_label(IconState::Error, true, true),
            "Jisuno, needs attention, secure input on"
        );
        assert_eq!(
            accessibility_label(IDLE, true, false),
            "Jisuno, idle, needs attention"
        );
    }
}
