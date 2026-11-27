//! Synthetic Right-modifier hold at HID level: posts a FlagsChanged down/up
//! pair with the device-dependent right key bit set, so the [`KeyTap`]
//! sees a genuine hold without touching the keyboard.
//!
//! Usage: `cargo run -p jisuno-platform --example hold_sim -- <ms> [option|command]`.
//! The `<ms>` hold must exceed the 150 ms minimum to count. The second arg
//! chooses the hold key: Right ⌥ (`option`, the default) or Right ⌘
//! (`command`, keycode 54 with `NX_DEVICERCMDKEYMASK | command-mask`).
//!
//! macOS only: posting needs no extra entitlement beyond what the tap
//! itself needs, but `CGEventPost` to the HID tap can be dropped without
//! assistive trust — the caller reports what it posted either way.

#[cfg(target_os = "macos")]
mod real {
    use core_graphics::event::{
        CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, EventField,
    };
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    use jisuno_platform::keys::NX_DEVICERALTKEYMASK;
    use jisuno_platform::keys::NX_DEVICERCMDKEYMASK;

    const RIGHT_OPTION_KEYCODE: u16 = 61;
    const RIGHT_COMMAND_KEYCODE: u16 = 54;
    const FLAG_ALT: u64 = 0x0008_0000;
    const FLAG_CMD: u64 = 0x0010_0000;

    fn post(keycode: u16, flags: u64) -> anyhow::Result<()> {
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|()| anyhow::anyhow!("CGEventSourceCreate failed"))?;
        let event = CGEvent::new(source).map_err(|()| anyhow::anyhow!("CGEventCreate failed"))?;
        event.set_type(CGEventType::FlagsChanged);
        event.set_flags(CGEventFlags::from_bits_retain(flags));
        event.set_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE, i64::from(keycode));
        event.post(CGEventTapLocation::HID);
        Ok(())
    }

    pub(super) fn run(ms: u64, key: &str) -> anyhow::Result<()> {
        let (name, keycode, flags) = match key {
            "command" => (
                "Right-Command",
                RIGHT_COMMAND_KEYCODE,
                NX_DEVICERCMDKEYMASK | FLAG_CMD,
            ),
            "option" => (
                "Right-Option",
                RIGHT_OPTION_KEYCODE,
                NX_DEVICERALTKEYMASK | FLAG_ALT,
            ),
            other => {
                return Err(anyhow::anyhow!(
                    "unknown key {other:?}: want option|command"
                ))
            }
        };
        println!("posting {name} down at HID level (flags {flags:#x} | keycode {keycode})");
        post(keycode, flags)?;
        std::thread::sleep(std::time::Duration::from_millis(ms));
        println!("posting {name} up");
        post(keycode, 0)?;
        Ok(())
    }
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let ms: u64 = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("usage: hold_sim <ms> [option|command]"))?
        .parse()
        .map_err(|_| anyhow::anyhow!("usage: hold_sim <ms> [option|command]"))?;
    let key = args.next().unwrap_or_else(|| "option".to_string());
    #[cfg(target_os = "macos")]
    {
        real::run(ms, &key)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (ms, key);
        Err(anyhow::anyhow!(
            "hold_sim needs macOS: HID event posts are macOS-only"
        ))
    }
}
