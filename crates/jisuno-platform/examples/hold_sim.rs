//! Synthetic Right-Option hold at HID level: posts a FlagsChanged down/up
//! pair with the device-dependent right-alt bit set, so the [`KeyTap`]
//!// sees a genuine hold without touching the keyboard.
//!
//! Usage: `cargo run -p jisuno-platform --example hold_sim -- <ms>`.
//! The `<ms>` hold must exceed the 150 ms minimum to count.
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

    const RIGHT_OPTION_KEYCODE: u16 = 61;
    const FLAG_ALT: u64 = 0x0008_0000;

    fn post(flags: u64) -> anyhow::Result<()> {
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|()| anyhow::anyhow!("CGEventSourceCreate failed"))?;
        let event = CGEvent::new(source).map_err(|()| anyhow::anyhow!("CGEventCreate failed"))?;
        event.set_type(CGEventType::FlagsChanged);
        event.set_flags(CGEventFlags::from_bits_retain(flags));
        event.set_integer_value_field(
            EventField::KEYBOARD_EVENT_KEYCODE,
            i64::from(RIGHT_OPTION_KEYCODE),
        );
        event.post(CGEventTapLocation::HID);
        Ok(())
    }

    pub(super) fn run(ms: u64) -> anyhow::Result<()> {
        println!("posting Right-Option down at HID level (flags {NX_DEVICERALTKEYMASK:#x} | alt)");
        post(NX_DEVICERALTKEYMASK | FLAG_ALT)?;
        std::thread::sleep(std::time::Duration::from_millis(ms));
        println!("posting Right-Option up");
        post(0)?;
        Ok(())
    }
}

fn main() -> anyhow::Result<()> {
    let ms: u64 = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: hold_sim <ms>"))?
        .parse()
        .map_err(|_| anyhow::anyhow!("usage: hold_sim <ms>"))?;
    #[cfg(target_os = "macos")]
    {
        real::run(ms)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = ms;
        Err(anyhow::anyhow!(
            "hold_sim needs macOS: HID event posts are macOS-only"
        ))
    }
}
