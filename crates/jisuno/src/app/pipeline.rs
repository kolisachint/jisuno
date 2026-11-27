//! Hold/tap/mic/transcribe pipeline: the 20 ms driver and its effects.
//!
//! Split from `app.rs` with no behavior change (step 1).

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait};
use gpui::App;
use jisuno_asr::setup::Model as AsrModel;
use jisuno_audio::mic::{choose_input, InputChoice};
use jisuno_audio::tones::{self, Tone};
use jisuno_logic::hold::{CapturedKey, Effect, HoldKind, Input, Mode};
use jisuno_logic::keybindings::{Command, Key};
use jisuno_logic::settings::{self};
use jisuno_platform::{
    frontmost::{self, AppInfo},
    insert::{self, FocusLost, InsertMode},
    keys::KeyEvent,
    permissions::{self},
    secure_input::{self, SecureInputHolder},
    status_item::MenuAction,
};
use jisuno_ui::hud_model::HudState as UiHud;

use super::config::take_tap_restart_failed;
use super::hold_kind_for;
use super::model::ModelState;
use super::state::{
    text_summary, AppState, LastInsert, ListeningUi, PendingTranscribe, RecentEntry,
};
use super::worker::{AudioSource, ToWorker};

/// Where a machine effect goes when there is no `&mut App`.
/// `CloseMic`/`CancelModelLoad` run inline (no `App` needed); `OpenMic`
/// stays suppressed (a headless reopen would transcribe without a HUD);
/// `HideHud` is deferred to the next tick. Everything else has no headless
/// path and is logged loudly — never silently dropped. The `match` is
/// exhaustive over [`Effect`], so a new variant fails to compile until it
/// is routed here.
/// Secure-input holder lookup (spawns `ioreg` + `ps`): cached for 5 s.
static SECURE_HOLDER_CACHE: OnceLock<Mutex<(Option<Instant>, Option<SecureInputHolder>)>> =
    OnceLock::new();

pub(crate) fn cached_secure_input_holder() -> Option<SecureInputHolder> {
    if !secure_input::is_secure_input_enabled() {
        return None;
    }
    let now = Instant::now();
    if let Ok(cache) = SECURE_HOLDER_CACHE
        .get_or_init(|| Mutex::new((None, None)))
        .lock()
    {
        if let (Some(at), holder) = (&cache.0, &cache.1) {
            if now.duration_since(*at) < Duration::from_secs(5) {
                return holder.clone();
            }
        }
    }
    let holder = secure_input::secure_input_holder();
    if let Ok(mut cache) = SECURE_HOLDER_CACHE
        .get_or_init(|| Mutex::new((None, None)))
        .lock()
    {
        *cache = (Some(now), holder.clone());
    }
    holder
}

/// `cpal` input-device enumeration: cached for 5 s.
static INPUT_DEVICE_CACHE: OnceLock<Mutex<(Option<Instant>, Vec<String>)>> = OnceLock::new();

pub(crate) fn cached_input_device_names() -> Vec<String> {
    let now = Instant::now();
    if let Ok(cache) = INPUT_DEVICE_CACHE
        .get_or_init(|| Mutex::new((None, Vec::new())))
        .lock()
    {
        if let (Some(at), names) = (&cache.0, &cache.1) {
            if now.duration_since(*at) < Duration::from_secs(5) {
                return names.clone();
            }
        }
    }
    let names: Vec<String> = cpal::default_host()
        .input_devices()
        .map(|devices| devices.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default();
    if let Ok(mut cache) = INPUT_DEVICE_CACHE
        .get_or_init(|| Mutex::new((None, Vec::new())))
        .lock()
    {
        *cache = (Some(now), names.clone());
    }
    names
}

pub(crate) enum NoCxRoute {
    CloseMic { discard: bool },
    CancelLoad,
    ReopenSuppressed,
    HideDeferred,
    Dropped,
}

/// Route one machine effect for the headless contexts; see [`NoCxRoute`].
pub(crate) fn no_cx_route(effect: &Effect) -> NoCxRoute {
    match effect {
        Effect::CloseMic { discard } => NoCxRoute::CloseMic { discard: *discard },
        Effect::CancelModelLoad => NoCxRoute::CancelLoad,
        Effect::OpenMic => NoCxRoute::ReopenSuppressed,
        Effect::HideHud => NoCxRoute::HideDeferred,
        Effect::StartModelLoad
        | Effect::ShowHud(_)
        | Effect::Transcribe { .. }
        | Effect::RunCommand
        | Effect::ShowConfirm { .. }
        | Effect::PassThrough
        | Effect::Countdown { .. } => NoCxRoute::Dropped,
    }
}

impl AppState {
    pub(crate) fn tick(&mut self, cx: &mut App) -> bool {
        if self.quit_requested {
            self.shutdown();
            cx.quit();
            return true;
        }
        if let Some(which) = self.debug_open.take() {
            match which {
                super::DebugOpen::Popover => self.toggle_popover(cx),
                super::DebugOpen::Settings => self.open_settings(cx),
                super::DebugOpen::Welcome => self.open_welcome(cx),
            }
        }
        self.drain_menu(cx);
        self.drain_download();
        self.drain_bench();
        self.drain_popover(cx);
        self.drain_settings(cx);
        self.drain_welcome(cx);
        self.drain_tap(cx);
        self.drain_worker(cx);
        self.drain_hook_requests(cx);
        self.drain_cmd_done(cx);
        let effects = self
            .machine
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .tick(self.now_ms());
        if !effects.is_empty() {
            self.log(&format!("hold tick -> {} effect(s)", effects.len()));
        }
        self.exec_effects(cx, effects);
        self.drain_mic(cx);
        self.poll_partials();
        self.drain_watch(cx);
        self.apply_pending_timing();
        self.apply_pending_model(cx);
        self.poll_secure(cx);
        self.poll_idle_unload();
        self.refresh_hud_level(cx);
        self.refresh_chrome();
        self.poll_popover(cx);
        self.poll_settings(cx);
        self.poll_welcome(cx);
        if let Some((down, up, sent)) = self.debug_hold {
            let now = Instant::now();
            if !sent && now >= down {
                self.debug_hold = Some((down, up, true));
                self.log("debug hold down (simulated)");
                self.begin_hold(cx, HoldKind::Dictate);
            } else if sent && now >= up {
                self.debug_hold = None;
                self.log("debug hold up (simulated)");
                self.input(cx, Input::HoldUp(HoldKind::Dictate), "up dictate (debug)");
            }
        }
        false
    }

    pub(crate) fn shutdown(&mut self) {
        self.log("quitting");
        let _ = self.to_worker.send(ToWorker::Unload);
        if let Some(tap) = self.tap.take() {
            if let Err(e) = tap.shutdown() {
                self.log(&format!("tap shutdown: {e:#}"));
            }
        }
        if let Some(hud) = self.hud.take() {
            // Best effort: the process is going away anyway.
            let _ = hud;
        }
    }

    // --- Menu ---

    pub(crate) fn drain_menu(&mut self, cx: &mut App) {
        while let Ok(action) = self.menu_rx.try_recv() {
            match action {
                MenuAction::TogglePopover => self.toggle_popover(cx),
                MenuAction::OpenSettings => self.open_settings(cx),
                MenuAction::OpenWelcome => self.open_welcome(cx),
                MenuAction::WarmModel => self.start_load(cx, "menu"),
                MenuAction::UnloadModel => self.unload_model("menu"),
                MenuAction::OpenPane(pane) => {
                    if let Err(e) = permissions::open_settings_pane(pane) {
                        self.log(&format!("open settings pane: {e:#}"));
                    }
                }
                MenuAction::Quit => self.quit_requested = true,
            }
        }
    }

    // --- Event tap ---

    pub(crate) fn drain_tap(&mut self, cx: &mut App) {
        if take_tap_restart_failed() {
            self.show_ephemeral(
                cx,
                UiHud::ConfigError {
                    location: "Keys paused".to_string(),
                    message: "Input Monitoring is off".to_string(),
                },
                "tap restart failed",
            );
        }
        let mut events = Vec::new();
        if let Some(rx) = &self.tap_rx {
            while let Ok(ev) = rx.try_recv() {
                events.push(ev);
            }
        }
        for ev in events {
            self.log(&format!("tap {ev:?}"));
            if self.key_capturing.is_some() {
                self.feed_capture(cx, &ev);
                continue;
            }
            match ev {
                KeyEvent::HoldDown(key) => match hold_kind_for(&self.bindings, key) {
                    Some(kind) => self.begin_hold(cx, kind),
                    None => self.log("tap: hold key with no dictate/command binding — ignored"),
                },
                KeyEvent::HoldUp(key) => {
                    if let Some(kind) = hold_kind_for(&self.bindings, key) {
                        self.input(cx, Input::HoldUp(kind), "up");
                    }
                }
                KeyEvent::OtherKeyDown => self.input(cx, Input::OtherKeyDown, "other-key"),
                KeyEvent::Chord(key) => self.on_chord(cx, &key),
                KeyEvent::Escape => {
                    if self.machine_captures(CapturedKey::Escape) {
                        if self.confirming.is_some() {
                            self.esc_cancel = true;
                        }
                        self.input(cx, Input::Cancel, "cancel (esc)");
                    } else if self.popover.is_some() {
                        self.close_popover(cx, "esc");
                    }
                }
                KeyEvent::Return => {
                    if self.machine_captures(CapturedKey::Return) {
                        self.input(cx, Input::Confirm, "confirm (return)");
                    }
                }
                KeyEvent::TapReenabled => {
                    self.log("tap re-enabled: keys may have been missed");
                    self.input(cx, Input::Resync, "resync");
                }
            }
        }
    }

    pub(crate) fn on_chord(&mut self, cx: &mut App, key: &Key) {
        match self.bindings.lookup(key, None) {
            Some(Command::HandsFreeToggle) => {
                if self.check_hold_ready(cx) {
                    self.handsfree_armed = true;
                    self.input(cx, Input::HandsFreeToggle, "hands-free toggle");
                }
            }
            Some(Command::Warm) => self.start_load(cx, "warm key"),
            Some(Command::Unload) => self.unload_model("unload key"),
            Some(Command::Settings) => self.open_settings(cx),
            Some(Command::Menu) => self.toggle_popover(cx),
            Some(command) => self.log(&format!("chord for {command:?} — no app action yet")),
            None => {}
        }
    }

    /// HoldDown path with the spec's pre-checks: the machine never sees a
    /// hold the host cannot serve (missing mic/model), so a dead press shows
    /// the error HUD and is silently ignored on release.
    pub(crate) fn begin_hold(&mut self, cx: &mut App, kind: HoldKind) {
        if !self.check_hold_ready(cx) {
            return;
        }
        let tag = match kind {
            HoldKind::Dictate => "down dictate",
            HoldKind::Command => "down command",
        };
        self.input(cx, Input::HoldDown(kind), tag);
    }

    pub(crate) fn check_hold_ready(&mut self, cx: &mut App) -> bool {
        if secure_input::is_secure_input_enabled() {
            let holder = cached_secure_input_holder();
            let name = holder
                .as_ref()
                .and_then(|h| h.name.clone())
                .unwrap_or_else(|| "unknown".to_string());
            if !self.announced_secure.contains(&name) {
                self.announced_secure.push(name.clone());
                self.show_ephemeral(
                    cx,
                    UiHud::SecureInput { app: name.clone() },
                    &format!("secure input on ({name})"),
                );
            } else {
                self.log(&format!(
                    "secure input on ({name}) · keys paused (already shown)"
                ));
            }
            return false;
        }
        if self.debug_wav.is_none() && default_input_missing() {
            self.show_ephemeral(cx, UiHud::NoMicrophone, "no microphone");
            return false;
        }
        if !self.model_files_present() {
            self.show_ephemeral(cx, UiHud::ModelMissing, "model missing");
            return false;
        }
        true
    }

    pub(crate) fn model_files_present(&self) -> bool {
        AsrModel::parse(&self.model_name)
            .map(|m| m.is_ready())
            .unwrap_or(false)
    }

    // --- Mic ---

    pub(crate) fn open_mic(&mut self, cx: &mut App) {
        // Captured only here: OpenMic runs exactly when the machine
        // accepted the press, so ignored presses can no longer overwrite
        // the target the utterance will insert into.
        self.target = frontmost::frontmost_app();
        if let Some(app) = &self.target {
            self.log(&format!("frontmost on hold: {app:?}"));
        }
        self.mic_open = true;
        self.buffer.clear();
        self.heard_speech = false;
        self.level = 0.0;
        self.resampler = None;
        self.debug_pcm = None;
        self.debug_pos = 0;
        self.debug_next_at = None;
        self.resolve_audio_notes();
        if let Some(path) = self.debug_wav.clone() {
            // Debug WAV as a fake real-time mic: the clip feeds in 100 ms
            // chunks on the wall clock while the hold is down, so
            // pause-split segments decode during the hold like a real mic.
            match jisuno_audio::audio::load_wav_16k_mono(&path) {
                Ok(pcm) => {
                    self.debug_pcm = Some(pcm);
                    self.debug_next_at = Some(Instant::now());
                    self.heard_speech = true;
                    self.level = 0.5;
                    self.log("mic: debug wav substitutes for the microphone");
                }
                Err(e) => {
                    self.log(&format!("debug wav failed: {e:#}"));
                    self.mic_open = false;
                    self.input_no_cx(Input::Resync, "resync (debug wav failed)");
                    return;
                }
            }
        } else {
            // Measure the synchronous open; it only moves off the tick with
            // evidence (> 10 ms).
            let started = Instant::now();
            let names = cached_input_device_names();
            let opened = match choose_input(&self.settings.input_device, &names) {
                InputChoice::Named(name) => jisuno_audio::mic::start_named(name).or_else(|e| {
                    self.log(&format!("named input failed, using system input: {e:#}"));
                    jisuno_audio::mic::start()
                }),
                InputChoice::System | InputChoice::Missing => jisuno_audio::mic::start(),
            };
            let elapsed = started.elapsed();
            if elapsed > Duration::from_millis(10) {
                self.log(&format!(
                    "mic start blocked tick for {} ms",
                    elapsed.as_millis()
                ));
            }
            match opened {
                Ok(capture) => {
                    self.mic_rate = capture.input_rate;
                    self.capture = Some(capture);
                    self.log(&format!("mic open ({} Hz)", self.mic_rate));
                }
                Err(e) => {
                    self.log(&format!("mic open failed: {e:#}"));
                    self.mic_open = false;
                    self.input_no_cx(Input::Resync, "resync (mic failed)");
                }
            }
            self.resampler = Some(jisuno_audio::audio::StreamingResampler::new(
                self.mic_rate,
                jisuno_audio::audio::TARGET_RATE,
            ));
        }
        // Speech detection for hold sessions: skip the decode round-trip when
        // the press captured no speech at all.
        self.handsfree_vad = None;
        if self.expect_handsfree() {
            self.handsfree_vad = Some(jisuno_logic::vad::Vad::new(u64::from(
                self.settings.hands_free_silence_ms,
            )));
        }
        // Streaming utterance against the helper child: begin now when warm
        // (pushes flow from `drain_mic`); when cold the stream id is
        // reserved and audio only accumulates until the Warm flush.
        // Id 0 is reserved for the worker one-shot path, never begun.
        self.stream_seq = self.stream_seq.wrapping_add(1).max(1);
        self.stream = Some(self.stream_seq);
        self.stream_begun = false;
        self.partial_text = None;
        self.partial_in_flight = false;
        if self.model_state == ModelState::Warm {
            let stream = self.stream_seq;
            if self.to_worker.send(ToWorker::Begin { stream }).is_ok() {
                self.stream_begun = true;
                self.last_partial_poll = Instant::now();
                self.log(&format!("stream {stream} begun (warm)"));
            } else {
                // The worker thread is gone: respawn it and reload. The
                // stream stays reserved and starts when the load lands
                // (flush on Warm); a key-up before then decodes from
                // fallback audio instead.
                self.log("model worker gone at mic open — respawning");
                self.recover_worker(cx, "mic open");
            }
        }
    }

    /// True when the press that just opened the mic was the hands-free
    /// toggle. The machine does not report the mode in OpenMic, so the flag
    /// is set when the toggle is fed and consumed here.
    pub(crate) fn expect_handsfree(&mut self) -> bool {
        // Armed by HandsFreeToggle handling and consumed when the mic opens
        // for that press. A stray arm (toggle ignored mid-hold) is harmless:
        // the silence input is ignored outside hands-free state.
        std::mem::replace(&mut self.handsfree_armed, false)
    }

    pub(crate) fn close_mic(&mut self, discard: bool) {
        // NB: the raw drain below (no VAD, no machine input) — never the
        // effect-executing `drain_mic`, which would re-enter through
        // CloseMic while an outer chunk loop is still iterating.
        if discard && self.listening.is_some() {
            // A discard while the HUD shows a hold (Esc, other key): the
            // cancel cue. A tap (no HUD yet) is silent.
            self.play_tone(Tone::Cancel);
        }
        if discard {
            // Tap / Esc / other-key: drop the utterance without decoding.
            self.cancel_stream();
            self.resampler = None;
            self.debug_pcm = None;
            self.debug_next_at = None;
        }
        let _ = self.drain_mic_raw();
        if !discard {
            self.flush_stream_tail();
        }
        if let Some(capture) = self.capture.take() {
            capture.stop();
        }
        self.mic_open = false;
        self.handsfree_vad = None;
        if discard {
            self.buffer.clear();
            self.heard_speech = false;
            self.log("mic closed (discarded)");
        } else {
            self.log(&format!("mic closed ({} samples kept)", self.buffer.len()));
        }
    }

    /// Drop the helper stream without decoding (silent hold, failed load):
    /// cancel only what was begun — cancelling a never-begun stream would
    /// error the helper child — then clear the local stream state.
    pub(crate) fn cancel_stream(&mut self) {
        let begun = std::mem::replace(&mut self.stream_begun, false);
        if let Some(stream) = self.stream.take() {
            if begun {
                let _ = self.to_worker.send(ToWorker::CancelStream { stream });
            }
        }
        self.partial_in_flight = false;
        self.partial_text = None;
    }

    /// Push any filter remainder still held by the streaming resampler
    /// (plus any unfed debug-WAV tail) so key-up `finish` sees everything.
    pub(crate) fn flush_stream_tail(&mut self) {
        if self.debug_pcm.is_some() {
            self.push_debug_chunks(true);
        }
        if self.stream_begun {
            if let Some(stream) = self.stream {
                if let Some(resampler) = self.resampler.as_mut() {
                    let tail = resampler.flush();
                    if !tail.is_empty() {
                        let _ = self.to_worker.send(ToWorker::Push { stream, pcm: tail });
                    }
                }
            }
        }
    }

    /// Feed due 100 ms debug-WAV chunks (16 kHz already). With `catch_up`,
    /// push everything remaining (key-up); otherwise only what the wall
    /// clock has reached (real-time simulation mid-hold).
    pub(crate) fn push_debug_chunks(&mut self, catch_up: bool) {
        let total = self.debug_pcm.as_ref().map(Vec::len).unwrap_or(0);
        if total == 0 || !self.mic_open {
            return;
        }
        let now = Instant::now();
        loop {
            let due = match self.debug_next_at {
                Some(t) => catch_up || now >= t,
                None => false,
            };
            if !due || self.debug_pos >= total {
                break;
            }
            let end = (self.debug_pos + 1600).min(total);
            let chunk = self.debug_pcm.as_ref().unwrap()[self.debug_pos..end].to_vec();
            self.debug_pos = end;
            self.debug_next_at = Some(self.debug_next_at.unwrap() + Duration::from_millis(100));
            let rms = jisuno_logic::vad::rms(&chunk);
            if rms > self.level {
                self.level = rms;
            }
            if !self.stream_begun {
                self.buffer.extend_from_slice(&chunk);
            }
            if self.stream_begun {
                if let Some(stream) = self.stream {
                    let _ = self.to_worker.send(ToWorker::Push { stream, pcm: chunk });
                }
            }
        }
    }

    /// Channel + debug-WAV audio into the buffer/stream, without VAD or
    /// machine input: re-entrancy-safe, so `close_mic` can call it while a
    /// `drain_mic` chunk loop is on the stack. Returns true when the
    /// hands-free VAD judged the line silent (the caller feeds the machine).
    pub(crate) fn drain_mic_raw(&mut self) -> bool {
        self.push_debug_chunks(false);
        let mut chunks = Vec::new();
        if let Some(capture) = &self.capture {
            while let Ok(chunk) = capture.chunks.try_recv() {
                chunks.push(chunk);
            }
        }
        let mut silence = false;
        for chunk in &chunks {
            let rms = jisuno_logic::vad::rms(chunk);
            if rms > self.level {
                self.level = rms;
            }
            if !chunk.iter().all(|s| *s == 0.0) && rms > HEARD_SPEECH_RMS {
                self.heard_speech = true;
            }
            if let Some(vad) = &mut self.handsfree_vad {
                if vad.push(chunk) == jisuno_logic::vad::VadEvent::SilenceTimeout {
                    self.handsfree_vad = None;
                    silence = true;
                }
            }
            // Resample once per chunk into 16 kHz: the buffer stays at the
            // decode rate, so a later Warm flush sends it without a
            // whole-buffer resample on the tick. The streaming resampler
            // keeps filter state across chunks (a per-chunk one-shot would
            // corrupt edges).
            let pcm16 = match self.resampler.as_mut() {
                Some(r) => r.push(chunk),
                None => jisuno_audio::audio::resample_to_16k(chunk, self.mic_rate),
            };
            self.buffer.extend_from_slice(&pcm16);
            if self.stream_begun {
                if let Some(stream) = self.stream {
                    if !pcm16.is_empty() {
                        let _ = self.to_worker.send(ToWorker::Push { stream, pcm: pcm16 });
                    }
                }
            }
        }
        silence
    }

    /// Mic drain on the tick: audio first (re-entrancy-safe), then any
    /// hands-free silence stop through the machine with effects executed
    /// here via `exec_effects` — no effect is dropped, so the hands-free
    /// `Transcribe` always runs.
    pub(crate) fn drain_mic(&mut self, cx: &mut App) {
        if self.drain_mic_raw() {
            let effects = self
                .machine
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .handle(Input::SilenceDetected, self.now_ms());
            self.log(&format!(
                "hold hands-free silence -> {} effect(s)",
                effects.len()
            ));
            self.exec_effects(cx, effects);
        }
    }

    /// Ask the worker for live partial text every 400 ms while listening
    /// (H4). One request at a time; a slow decode self-throttles.
    pub(crate) fn poll_partials(&mut self) {
        if !self.settings.partials || self.pending.is_some() {
            return;
        }
        if !self.mic_open || !self.stream_begun || self.partial_in_flight {
            return;
        }
        let Some(stream) = self.stream else {
            return;
        };
        if self.last_partial_poll.elapsed() < Duration::from_millis(400) {
            return;
        }
        if self.to_worker.send(ToWorker::PartialReq { stream }).is_ok() {
            self.partial_in_flight = true;
            self.last_partial_poll = Instant::now();
        }
    }

    /// Start, stop or cancel cue (`settings.json` `sounds`). Non-blocking:
    /// playback runs on its own thread.
    pub(crate) fn play_tone(&self, tone: Tone) {
        if self.settings.sounds {
            self.log(&format!("tone: {}", tone.log_name()));
            tones::play(tone);
        }
    }

    /// Resolve the Bluetooth / missing-device notes for this utterance (H16/H17).
    /// The device opened is the named one when connected, else the system input.
    pub(crate) fn resolve_audio_notes(&mut self) {
        self.bluetooth_device = None;
        self.missing_device_note = None;
        let wanted = self.settings.input_device.clone();
        let names = cached_input_device_names();
        let named = match choose_input(&wanted, &names) {
            InputChoice::Named(name) => Some(name.to_string()),
            InputChoice::System => None,
            InputChoice::Missing => {
                self.missing_device_note = Some(wanted);
                None
            }
        };
        let device = named.or_else(default_input_name);
        if let Some(name) = device.filter(|name| is_bluetooth_name(name)) {
            self.bluetooth_device = Some(name);
        }
    }

    /// H16: the Bluetooth note shows on the first dictation on each Bluetooth
    /// input in a session. True when this hold should carry it.
    pub(crate) fn take_bluetooth_note(&mut self) -> bool {
        let Some(device) = self.bluetooth_device.clone() else {
            return false;
        };
        if self.announced_bluetooth.contains(&device) {
            return false;
        }
        self.announced_bluetooth.push(device);
        self.log("bluetooth mic note (once per device per session)");
        true
    }

    // --- Effects ---

    pub(crate) fn exec_effects(&mut self, cx: &mut App, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::OpenMic => self.open_mic(cx),
                Effect::CloseMic { discard } => self.close_mic(discard),
                Effect::StartModelLoad => self.start_load(cx, "hold"),
                Effect::CancelModelLoad => self.cancel_load(),
                Effect::ShowHud(state) => self.show_machine_hud(cx, state),
                Effect::HideHud => {
                    if self.confirming.take().is_some() {
                        self.pending_plan = None;
                        let expired = !std::mem::replace(&mut self.esc_cancel, false);
                        if !expired {
                            self.play_tone(Tone::Cancel);
                        }
                        self.hide_hud(cx);
                        let state = if expired {
                            UiHud::ConfirmExpired
                        } else {
                            UiHud::Cancelled
                        };
                        let tag = if expired {
                            "confirm expired"
                        } else {
                            "cancelled"
                        };
                        self.show_ephemeral(cx, state, tag);
                    } else {
                        self.hide_hud(cx);
                    }
                }
                Effect::Transcribe { mode, id } => self.begin_transcribe(cx, mode, id),
                Effect::RunCommand => self.run_command(cx),
                Effect::ShowConfirm { label } => self.show_confirm(cx, label),
                Effect::PassThrough => {}
                Effect::Countdown { secs_left } => {
                    if let Some(listening) = &mut self.listening {
                        listening.countdown_ms = Some(secs_left * 1000);
                    }
                    self.log(&format!("countdown {secs_left}s left"));
                }
            }
        }
    }

    /// Where a machine effect goes when there is no `&mut App` (secure-input
    /// poll, mic-open failures): the cx-free effects run inline, the rest
    /// are routed by [`no_cx_route`] so none is silently dropped. Silence
    /// stops no longer come through here — `drain_mic` executes them with
    /// effects. HideHud cannot close the window without `App`, so it marks
    /// the HUD for closing on the next tick instead.
    pub(crate) fn input_no_cx(&mut self, ev: Input, tag: &str) {
        let effects = self
            .machine
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .handle(ev, self.now_ms());
        self.log(&format!("hold {tag} -> {} effect(s)", effects.len()));
        for effect in effects {
            match no_cx_route(&effect) {
                NoCxRoute::CloseMic { discard } => self.close_mic(discard),
                NoCxRoute::CancelLoad => self.cancel_load(),
                NoCxRoute::ReopenSuppressed => self.open_mic_no_cx(),
                NoCxRoute::HideDeferred => {
                    self.listening = None;
                    self.confirming = None;
                    self.pending_plan = None;
                    if let Some(hud) = &mut self.hud {
                        hud.sticky = false;
                        hud.dismiss_at = Some(Instant::now());
                    }
                }
                NoCxRoute::Dropped => {
                    self.log(&format!("hold {tag}: {effect:?} has no headless path"));
                }
            }
        }
    }

    pub(crate) fn open_mic_no_cx(&mut self) {
        // Only reached from Resync paths; keep the state consistent without
        // touching UI: mic stays closed until the next real press.
        self.mic_open = false;
        self.buffer.clear();
    }

    pub(crate) fn show_machine_hud(&mut self, cx: &mut App, state: jisuno_logic::hold::HudState) {
        use jisuno_logic::hold::HudState as LogicHud;
        match state {
            LogicHud::Listening(mode) => {
                let dictate = mode == Mode::Dictate;
                self.play_tone(if dictate {
                    Tone::StartDictate
                } else {
                    Tone::StartCommand
                });
                let bluetooth_note = dictate && self.take_bluetooth_note();
                self.listening = Some(ListeningUi {
                    dictate,
                    hands_free: false,
                    level: 0.5,
                    loading_since: (self.model_state != ModelState::Warm).then(Instant::now),
                    countdown_ms: None,
                    partial: None,
                    bluetooth_note,
                });
                if dictate {
                    self.log("hud: listening dictate");
                } else {
                    self.log("hud: listening command");
                }
                self.open_listening_hud(cx);
            }
            LogicHud::HandsFree => {
                self.play_tone(Tone::StartDictate);
                let bluetooth_note = self.take_bluetooth_note();
                self.listening = Some(ListeningUi {
                    dictate: true,
                    hands_free: true,
                    level: 0.5,
                    loading_since: (self.model_state != ModelState::Warm).then(Instant::now),
                    countdown_ms: None,
                    partial: None,
                    bluetooth_note,
                });
                self.log("hud: hands-free");
                self.open_listening_hud(cx);
            }
        }
    }

    // --- Transcribe ---

    /// Resolve one utterance: finish the helper stream (warm) or one-shot
    /// the buffered clip behind the in-flight load (cold, or wav-debug).
    pub(crate) fn begin_transcribe(&mut self, cx: &mut App, mode: Mode, id: u64) {
        let key_up = Instant::now();
        self.play_tone(if mode == Mode::Dictate {
            Tone::StopDictate
        } else {
            Tone::StopCommand
        });
        self.pending = Some(PendingTranscribe {
            id,
            mode,
            key_up,
            target: self.target.clone(),
        });
        self.last_active = Instant::now();
        if mode == Mode::Dictate && !self.heard_speech && self.debug_wav.is_none() {
            self.log("no speech captured — skipping decode");
            self.pending = None;
            self.cancel_stream();
            self.input(cx, Input::Finished { hud: None, id }, "finished (silence)");
            return;
        }
        if self.model_state == ModelState::Failed {
            self.log("transcribe with failed load — model missing HUD");
            self.pending = None;
            self.input(
                cx,
                Input::Finished { hud: None, id },
                "finished (load failed)",
            );
            self.show_ephemeral(cx, UiHud::ModelMissing, "model load failed");
            return;
        }
        let dictate = mode == Mode::Dictate;
        // Warm and streamed (mic or debug WAV): the child already holds
        // the audio — finish only decodes the tail. Otherwise hand the
        // whole clip over; the worker one-shots it behind the in-flight
        // load.
        self.flush_stream_tail();
        let (stream, fallback) = if self.stream_begun {
            (self.stream.unwrap_or(0), None)
        } else {
            match &self.debug_wav {
                Some(path) => (0, Some(AudioSource::Wav { path: path.clone() })),
                None => (
                    self.stream.unwrap_or(0),
                    Some(AudioSource::Mic {
                        rate: jisuno_audio::audio::TARGET_RATE,
                        pcm: std::mem::take(&mut self.buffer),
                    }),
                ),
            }
        };
        self.show_decoding(cx, dictate, self.model_state != ModelState::Warm);
        // The stream ends here either way: the answer arrives as
        // Transcript / PendingFailed / Crashed.
        self.stream = None;
        self.stream_begun = false;
        self.partial_in_flight = false;
        self.partial_text = None;
        if self
            .to_worker
            .send(ToWorker::Finish {
                stream,
                reply: id,
                fallback,
            })
            .is_err()
        {
            // The worker thread is gone: respawn it and re-drive this
            // utterance from the fallback audio, which the cold path decodes
            // behind the fresh load. The buffer still holds the whole clip
            // here (cleared only after a successful handoff).
            self.log("model worker gone — respawning, decoding from fallback audio");
            let fallback = match &self.debug_wav {
                Some(path) => Some(AudioSource::Wav { path: path.clone() }),
                None => Some(AudioSource::Mic {
                    rate: jisuno_audio::audio::TARGET_RATE,
                    pcm: std::mem::take(&mut self.buffer),
                }),
            };
            self.clear_stream();
            self.recover_worker(cx, "worker gone");
            let _ = self.to_worker.send(ToWorker::Finish {
                stream: 0,
                reply: id,
                fallback,
            });
        }
        self.buffer.clear();
    }

    pub(crate) fn on_transcript(
        &mut self,
        cx: &mut App,
        id: u64,
        text: String,
        decode_ms: u64,
        child_rss_mb: u64,
    ) {
        let Some(pending) = self.pending.take() else {
            self.log(&format!("stale transcript {id} — ignored"));
            return;
        };
        if pending.id != id {
            self.log(&format!(
                "stale transcript {id} (want {}) — ignored",
                pending.id
            ));
            self.pending = Some(pending);
            return;
        }
        let key_up_ms = pending.key_up.elapsed().as_millis();
        self.child_rss_mb = Some(child_rss_mb);
        self.log(&format!(
            "key-up -> text: {key_up_ms} ms (finish {decode_ms} ms, child {child_rss_mb} MB)"
        ));
        self.last_active = Instant::now();
        match pending.mode {
            Mode::Dictate => {
                // Probe the machine first: if the transcribe timeout already
                // fired (or the utterance moved on), the reply is ignored —
                // never insert stale text. Acceptance releases the machine,
                // so the text below needs no second feed.
                if !self.input_checked(
                    cx,
                    Input::TranscriptReady {
                        text: String::new(),
                        id,
                    },
                    "transcript ready",
                ) {
                    self.log(&format!(
                        "stale transcript {id} (machine moved on) — kept in Recent"
                    ));
                    self.push_recent(text, Some("not inserted".to_string()));
                    return;
                }
                if text.trim().is_empty() {
                    return;
                }
                if pending.target.is_none() {
                    self.log("focus lost: text in Recent");
                    self.push_recent(text.clone(), Some("not inserted".to_string()));
                    self.show_ephemeral(cx, UiHud::FocusLost, "insert failed");
                    return;
                }
                let (pid, bundle) = Self::insert_dest(&pending.target);
                match self.insert_text(&text, pid, bundle) {
                    Ok(()) => {
                        let words = text.split_whitespace().count();
                        self.push_recent(text.clone(), None);
                        let latency = self.show_latency.then_some(key_up_ms as u64);
                        self.show_ephemeral(
                            cx,
                            UiHud::Inserted {
                                words: words as u32,
                                latency_ms: latency,
                            },
                            &format!("inserted {words} words"),
                        );
                    }
                    Err(kind) => {
                        let hud = match kind {
                            InsertFailure::FocusLost => UiHud::FocusLost,
                            InsertFailure::Blocked => UiHud::InsertBlocked,
                        };
                        self.show_ephemeral(cx, hud, "insert failed");
                    }
                }
            }
            Mode::Command => {
                let frontmost = pending.target.as_ref().and_then(|t| t.bundle_id.clone());
                self.resolve_command(cx, id, text, frontmost);
            }
        }
    }

    /// Key-down target resolved to an insert destination: the app focused
    /// when the machine accepted the press, or nowhere (focus was already
    /// lost at key-down) — never the app focused at key-up.
    pub(crate) fn insert_dest(target: &Option<AppInfo>) -> (Option<i32>, Option<String>) {
        match target {
            Some(t) => (Some(t.pid), t.bundle_id.clone()),
            None => (None, None),
        }
    }

    pub(crate) fn insert_text(
        &mut self,
        text: &str,
        pid: Option<i32>,
        bundle: Option<String>,
    ) -> Result<(), InsertFailure> {
        let mut mode = match self.settings.insert {
            settings::Insert::Paste => InsertMode::Paste,
            settings::Insert::Type => InsertMode::Type,
        };
        if let Some(bundle) = bundle {
            if self.settings.insert_type_apps.contains(&bundle) {
                mode = InsertMode::Type;
            }
        }
        // Consecutive dictations into one app read as one sentence: a single
        // leading space when the previous insert went to the same pid.
        let spaced;
        let spoken = if join_space_needed(self.last_insert, Instant::now(), pid, text) {
            spaced = format!(" {text}");
            spaced.as_str()
        } else {
            text
        };
        // No post-paste wait here: the platform restores the clipboard on
        // its own background thread (~150 ms), and the app process
        // outlives it. Sleeping would stall the 20 ms tick.
        match insert::insert_text(spoken, pid, mode, self.settings.restore_clipboard) {
            Ok(()) => {
                self.last_insert = pid.map(|pid| LastInsert {
                    pid,
                    at: Instant::now(),
                    ends_ws: text.ends_with(char::is_whitespace),
                });
                Ok(())
            }
            Err(e) => {
                self.last_insert = None;
                if e.downcast_ref::<FocusLost>().is_some() {
                    self.log("focus lost: text in Recent");
                    self.push_recent(text.to_string(), Some("not inserted".to_string()));
                    Err(InsertFailure::FocusLost)
                } else {
                    self.log(&format!("insert failed: {e:#}"));
                    self.push_recent(text.to_string(), Some("not inserted".to_string()));
                    Err(InsertFailure::Blocked)
                }
            }
        }
    }
}

/// Chunk RMS above which a dictate hold counts as speech (skips the decode
/// when false). Same floor as the decoder's silence gate, so quiet speech
/// the decoder can transcribe is not dropped here first.
const HEARD_SPEECH_RMS: f32 = 0.01;

/// How long a previous insert still counts as the same utterance thread.
const JOIN_WINDOW: Duration = Duration::from_secs(60);

/// True when `text` should get one leading space before it is inserted:
/// the previous insert went to the same `pid` less than 60 s ago, that text
/// did not end in whitespace, and `text` does not start with closing
/// punctuation (`.,;:!?)'"`).
pub(crate) fn join_space_needed(
    prev: Option<LastInsert>,
    now: Instant,
    pid: Option<i32>,
    text: &str,
) -> bool {
    let (Some(prev), Some(pid)) = (prev, pid) else {
        return false;
    };
    if prev.pid != pid || prev.ends_ws {
        return false;
    }
    if now.saturating_duration_since(prev.at) >= JOIN_WINDOW {
        return false;
    }
    match text.chars().next() {
        Some(c) => !matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | ')' | '\'' | '"'),
        None => false,
    }
}

#[derive(Debug)]
pub(crate) enum InsertFailure {
    FocusLost,
    Blocked,
}

impl AppState {
    /// Recent holds the last 5 results in memory only (gone on quit): the
    /// popover reads this once it exists; until then it is logged.
    pub(crate) fn push_recent(&mut self, text: String, tag: Option<String>) {
        self.log(&format!(
            "recent{}: {}",
            tag.as_deref()
                .map(|t| format!(" [{t}]"))
                .unwrap_or_default(),
            text_summary(&text)
        ));
        self.recent.push_back(RecentEntry { text, tag });
        while self.recent.len() > 5 {
            self.recent.pop_front();
        }
    }
}

// --- Audio device helpers (cpal enumeration; capture stays default) --------

pub(crate) fn default_input_missing() -> bool {
    cpal::default_host().default_input_device().is_none()
}

pub(crate) fn default_input_name() -> Option<String> {
    cpal::default_host()
        .default_input_device()
        .and_then(|d| d.name().ok())
}

pub(crate) fn is_bluetooth_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.contains("bluetooth") || lower.contains("airpods") || lower.contains("beats")
}

#[cfg(test)]
mod tests {
    use super::join_space_needed;
    use super::no_cx_route;
    use super::NoCxRoute;
    use super::{Duration, Instant};
    use crate::app::state::LastInsert;
    use jisuno_logic::hold::HudState as LogicHud;
    use jisuno_logic::hold::{Effect, Mode};

    /// Insert `ago` before `now`.
    fn prev(now: Instant, pid: i32, ago: Duration, ends_ws: bool) -> Option<LastInsert> {
        Some(LastInsert {
            pid,
            at: now.checked_sub(ago).unwrap_or(now),
            ends_ws,
        })
    }

    #[test]
    fn join_space_for_consecutive_inserts_into_one_app() {
        let now = Instant::now();
        // The reported bug: "...microphone." then "Partial text check".
        assert!(join_space_needed(
            prev(now, 7, Duration::from_secs(3), false),
            now,
            Some(7),
            "Partial text check"
        ));
        // Boundary: 59 s ago joins, 60 s ago does not.
        assert!(join_space_needed(
            prev(now, 7, Duration::from_secs(59), false),
            now,
            Some(7),
            "next"
        ));
        assert!(!join_space_needed(
            prev(now, 7, Duration::from_secs(60), false),
            now,
            Some(7),
            "next"
        ));
    }

    #[test]
    fn no_join_space_when_a_condition_fails() {
        let now = Instant::now();
        let recent = || prev(now, 7, Duration::from_secs(3), false);
        // No previous insert.
        assert!(!join_space_needed(None, now, Some(7), "next"));
        // Another target.
        assert!(!join_space_needed(recent(), now, Some(8), "next"));
        // No target at key-down (focus lost).
        assert!(!join_space_needed(recent(), now, None, "next"));
        // Previous text already ended in whitespace or a newline.
        assert!(!join_space_needed(
            prev(now, 7, Duration::from_secs(3), true),
            now,
            Some(7),
            "next"
        ));
        // New text starts with punctuation.
        for start in [".", ",", ";", ":", "!", "?", ")", "'", "\""] {
            assert!(
                !join_space_needed(recent(), now, Some(7), &format!("{start}next")),
                "{start} must not get a space"
            );
        }
        // Opening brackets and digits still join.
        assert!(join_space_needed(recent(), now, Some(7), "(next"));
        assert!(join_space_needed(recent(), now, Some(7), "2 items"));
        // Empty text never gets a space.
        assert!(!join_space_needed(recent(), now, Some(7), ""));
    }

    fn is_dropped(effect: &Effect) -> bool {
        matches!(no_cx_route(effect), NoCxRoute::Dropped)
    }

    /// The utterance inserts into the app focused at key-down (captured
    /// when the machine accepted the press), never the key-up frontmost
    /// app; `None` means focus was already lost.
    #[test]
    fn insert_uses_key_down_target() {
        use super::AppState;
        use jisuno_platform::frontmost::AppInfo;
        assert_eq!(
            AppState::insert_dest(&Some(AppInfo {
                bundle_id: Some("com.example.App".to_string()),
                name: "App".to_string(),
                pid: 123,
            })),
            (Some(123), Some("com.example.App".to_string()))
        );
        assert_eq!(AppState::insert_dest(&None), (None, None));
    }

    /// Every [`Effect`] variant has an explicit headless route: cx-free
    /// effects run inline, the rest are loudly dropped. If a variant is
    /// added to the machine, this fails to compile until it is routed.
    #[test]
    fn every_effect_has_a_headless_route() {
        assert!(matches!(
            no_cx_route(&Effect::CloseMic { discard: true }),
            NoCxRoute::CloseMic { discard: true }
        ));
        assert!(matches!(
            no_cx_route(&Effect::CloseMic { discard: false }),
            NoCxRoute::CloseMic { discard: false }
        ));
        assert!(matches!(
            no_cx_route(&Effect::CancelModelLoad),
            NoCxRoute::CancelLoad
        ));
        assert!(matches!(
            no_cx_route(&Effect::OpenMic),
            NoCxRoute::ReopenSuppressed
        ));
        assert!(matches!(
            no_cx_route(&Effect::HideHud),
            NoCxRoute::HideDeferred
        ));
        for effect in [
            Effect::StartModelLoad,
            Effect::ShowHud(LogicHud::Listening(Mode::Dictate)),
            Effect::ShowHud(LogicHud::HandsFree),
            Effect::Transcribe {
                mode: Mode::Dictate,
                id: 0,
            },
            Effect::RunCommand,
            Effect::ShowConfirm {
                label: String::new(),
            },
            Effect::PassThrough,
            Effect::Countdown { secs_left: 3 },
        ] {
            assert!(
                is_dropped(&effect),
                "{effect:?} must be an explicit Dropped"
            );
        }
    }
}
