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
//! Icon states follow `app-ui.md` §2. The listening colours need a
//! non-template image; until custom-drawn glyphs land, the template
//! `waveform` image stays and the state is carried by the VoiceOver label
//! (via the tooltip) plus a text badge suffix (`!` warning, lock while
//! Secure Input holds the keyboard). The badge is a placeholder for the
//! 9x9pt drawn badges in the spec.

use std::sync::mpsc;

use crate::permissions::Pane;

/// An action from the status-item menu. Sent to the app thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    WarmModel,
    UnloadModel,
    OpenPane(Pane),
    Quit,
}

/// Icon states from `app-ui.md` §2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconState {
    /// Nothing running. Dimmed while the model is not in memory.
    Idle { model_warm: bool },
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
            self.inner
                .set_item_title(tag, &format!("{name} — {state}"));
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
        NSImage, NSMenu, NSMenuItem, NSEventModifierFlags, NSStatusBar, NSStatusItem,
    };
    use objc2_foundation::NSString;

    use super::*;

    pub(super) const ITEM_MODEL: isize = 1;
    const ITEM_WARM: isize = 2;
    const ITEM_UNLOAD: isize = 3;
    pub(super) const ITEM_MIC: isize = 4;
    pub(super) const ITEM_AX: isize = 5;
    pub(super) const ITEM_HID: isize = 6;
    const ITEM_QUIT: isize = 7;

    fn tag_action(tag: isize) -> Option<MenuAction> {
        match tag {
            ITEM_WARM => Some(MenuAction::WarmModel),
            ITEM_UNLOAD => Some(MenuAction::UnloadModel),
            ITEM_MIC => Some(MenuAction::OpenPane(Pane::Microphone)),
            ITEM_AX => Some(MenuAction::OpenPane(Pane::Accessibility)),
            ITEM_HID => Some(MenuAction::OpenPane(Pane::InputMonitoring)),
            ITEM_QUIT => Some(MenuAction::Quit),
            _ => None,
        }
    }

    struct TargetIvars {
        sender: Mutex<mpsc::Sender<MenuAction>>,
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
        }
    );

    impl MenuTarget {
        fn new(sender: mpsc::Sender<MenuAction>, mtm: MainThreadMarker) -> Retained<Self> {
            let this = mtm.alloc::<Self>();
            let this = this.set_ivars(TargetIvars {
                sender: Mutex::new(sender),
            });
            unsafe { objc2::msg_send![super(this), init] }
        }

        fn as_object(&self) -> &AnyObject {
            self.as_super().as_super()
        }
    }

    pub(super) struct Inner {
        item: Retained<NSStatusItem>,
        _target: Retained<MenuTarget>,
        mtm: MainThreadMarker,
    }

    impl Inner {
        pub(super) fn set_item_title(&self, tag: isize, text: &str) {
            if let Some(menu) = self.item.menu(self.mtm) {
                for child in menu.itemArray().iter() {
                    if child.tag() == tag {
                        child.setTitle(&NSString::from_str(text));
                        return;
                    }
                }
            }
        }

        pub(super) fn set_icon(&self, state: IconState, warning: bool, secure: bool) {
            let Some(button) = self.item.button(self.mtm) else {
                return;
            };
            let mut badge = String::new();
            if secure {
                badge.push_str(" \u{1F512}");
            }
            if warning {
                badge.push_str(" !");
            }
            button.setTitle(&NSString::from_str(&badge));
            let label = match (warning, secure) {
                (true, true) => "Jisuno, needs attention, secure input on",
                (_, true) => "Jisuno, idle, secure input on",
                _ => state.label(),
            };
            button.setToolTip(Some(&NSString::from_str(label)));
        }
    }

    pub(super) fn create(sender: mpsc::Sender<MenuAction>) -> anyhow::Result<Inner> {
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| anyhow::anyhow!("status item must be created on the main thread"))?;
        let target = MenuTarget::new(sender, mtm);
        let target_obj: &AnyObject = target.as_object();

        let bar = NSStatusBar::systemStatusBar();
        let item = bar.statusItemWithLength(objc2_app_kit::NSVariableStatusItemLength);

        if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("waveform"),
            Some(&NSString::from_str("Jisuno")),
        ) {
            image.setTemplate(true);
            if let Some(button) = item.button(mtm) {
                button.setImage(Some(&image));
                button.setTitle(&NSString::from_str(""));
            }
        }
        if let Some(button) = item.button(mtm) {
            button.setToolTip(Some(&NSString::from_str(
                IconState::Idle { model_warm: false }.label(),
            )));
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
            add("Microphone — …", ITEM_MIC, "", NSEventModifierFlags::empty());
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
            add("Quit Jisuno", ITEM_QUIT, "q", NSEventModifierFlags::empty());
        }
        item.setMenu(Some(&menu));

        Ok(Inner {
            item,
            _target: target,
            mtm,
        })
    }
}
