//! Hold/tap/mic/transcribe pipeline: the 20 ms driver and its effects.
//!
//! Split from `app.rs` with no behavior change (step 1).

use std::time::{Duration, Instant};

use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait};
use gpui::App;
use jisuno_asr::setup::Model as AsrModel;
use jisuno_logic::hold::{CapturedKey, Effect, HoldKind, Input, Mode};
use jisuno_logic::keybindings::{Command, Key};
use jisuno_logic::settings::{self};
use jisuno_platform::{
    frontmost::{self},
    insert::{self, FocusLost, InsertMode},
    keys::KeyEvent,
    permissions::{self},
    secure_input,
    status_item::MenuAction,
};
use jisuno_ui::hud_model::HudState as UiHud;

use super::hold_kind_for;
use super::model::ModelState;
use super::state::{AppState, ListeningUi, PendingTranscribe, RecentEntry};
use super::worker::{AudioSource, ToWorker};

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
        self.drain_mic();
        self.poll_partials();
        self.drain_watch(cx);
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
                KeyEvent::HoldDown(key) => {
                    self.target = frontmost::frontmost_app();
                    if let Some(app) = &self.target {
                        self.log(&format!("frontmost on hold: {app:?}"));
                    }
                    match hold_kind_for(&self.bindings, key) {
                        Some(kind) => self.begin_hold(cx, kind),
                        None => self.log("tap: hold key with no dictate/command binding — ignored"),
                    }
                }
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
                        self.close_popover("esc");
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
                self.target = frontmost::frontmost_app();
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
            let holder = secure_input::secure_input_holder();
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

    pub(crate) fn open_mic(&mut self, _cx: &mut App) {
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
            match jisuno_audio::mic::start() {
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
                self.stream = None;
                self.log("model worker gone — stream dropped");
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
        if discard {
            // Tap / Esc / other-key: drop the utterance without decoding.
            if let Some(stream) = self.stream.take() {
                let _ = self.to_worker.send(ToWorker::CancelStream { stream });
            }
            self.stream_begun = false;
            self.partial_in_flight = false;
            self.partial_text = None;
            self.resampler = None;
            self.debug_pcm = None;
            self.debug_next_at = None;
        }
        self.drain_mic();
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
            self.buffer.extend_from_slice(&chunk);
            if self.stream_begun {
                if let Some(stream) = self.stream {
                    let _ = self.to_worker.send(ToWorker::Push { stream, pcm: chunk });
                }
            }
        }
    }

    pub(crate) fn drain_mic(&mut self) {
        self.push_debug_chunks(false);
        let mut chunks = Vec::new();
        if let Some(capture) = &self.capture {
            while let Ok(chunk) = capture.chunks.try_recv() {
                chunks.push(chunk);
            }
        }
        for chunk in chunks {
            let rms = jisuno_logic::vad::rms(&chunk);
            if rms > self.level {
                self.level = rms;
            }
            if !chunk.iter().all(|s| *s == 0.0) && rms > 0.02 {
                self.heard_speech = true;
            }
            if let Some(vad) = &mut self.handsfree_vad {
                if vad.push(&chunk) == jisuno_logic::vad::VadEvent::SilenceTimeout {
                    self.handsfree_vad = None;
                    self.input_no_cx(Input::SilenceDetected, "hands-free silence");
                }
            }
            self.buffer.extend_from_slice(&chunk);
            // Stream to the helper without blocking: the worker queues it
            // for the child. A failed send means the worker thread is
            // gone; the crash surfaces through its next reply. The
            // streaming resampler keeps filter state across chunks (a
            // per-chunk one-shot would corrupt edges).
            if self.stream_begun {
                if let Some(stream) = self.stream {
                    let pcm16 = match self.resampler.as_mut() {
                        Some(r) => r.push(&chunk),
                        None => jisuno_audio::audio::resample_to_16k(&chunk, self.mic_rate),
                    };
                    if !pcm16.is_empty() {
                        let _ = self.to_worker.send(ToWorker::Push { stream, pcm: pcm16 });
                    }
                }
            }
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

    /// Resolve the Bluetooth / missing-device notes for this utterance (H16/H17).
    pub(crate) fn resolve_audio_notes(&mut self) {
        self.bluetooth_now = false;
        self.missing_device_note = None;
        let wanted = self.settings.input_device.clone();
        if wanted != "system" {
            match find_input_device(&wanted) {
                Some(name) => {
                    if is_bluetooth_name(&name) {
                        self.bluetooth_now = true;
                    }
                    // Named-device capture is not wired yet (mic::start opens
                    // the default); fall back with the H17 note either way.
                    self.missing_device_note = Some(wanted);
                }
                None => self.missing_device_note = Some(wanted),
            }
            return;
        }
        if let Some(name) = default_input_name() {
            if is_bluetooth_name(&name) {
                self.bluetooth_now = true;
            }
        }
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

    /// Machine input from a context without `&mut App` (mic drain,
    /// secure-input poll): execute App-free effects inline. HideHud cannot
    /// close the window without `App`, so it marks the HUD for closing on
    /// the next tick instead.
    pub(crate) fn input_no_cx(&mut self, ev: Input, tag: &str) {
        let effects = self
            .machine
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .handle(ev, self.now_ms());
        self.log(&format!("hold {tag} -> {} effect(s)", effects.len()));
        for effect in effects {
            match effect {
                Effect::CloseMic { discard } => self.close_mic(discard),
                Effect::CancelModelLoad => self.cancel_load(),
                Effect::OpenMic => self.open_mic_no_cx(),
                Effect::HideHud => {
                    self.listening = None;
                    self.confirming = None;
                    self.pending_plan = None;
                    if let Some(hud) = &mut self.hud {
                        hud.sticky = false;
                        hud.dismiss_at = Some(Instant::now());
                    }
                }
                _ => self.log(&format!("hold {tag}: deferred effect {effect:?} to tick")),
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
                self.listening = Some(ListeningUi {
                    dictate,
                    hands_free: false,
                    level: 0.5,
                    loading_since: (self.model_state != ModelState::Warm).then(Instant::now),
                    countdown_ms: None,
                    partial: None,
                });
                if dictate {
                    self.log("hud: listening dictate");
                } else {
                    self.log("hud: listening command");
                }
                self.open_listening_hud(cx);
            }
            LogicHud::HandsFree => {
                self.listening = Some(ListeningUi {
                    dictate: true,
                    hands_free: true,
                    level: 0.5,
                    loading_since: (self.model_state != ModelState::Warm).then(Instant::now),
                    countdown_ms: None,
                    partial: None,
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
        self.pending = Some(PendingTranscribe { id, mode, key_up });
        self.last_active = Instant::now();
        if mode == Mode::Dictate && !self.heard_speech && self.debug_wav.is_none() {
            self.log("no speech captured — skipping decode");
            self.pending = None;
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
                        rate: self.mic_rate,
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
            self.log("model worker gone");
            self.pending = None;
            self.input(
                cx,
                Input::Finished { hud: None, id },
                "finished (worker gone)",
            );
        }
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
                if text.trim().is_empty() {
                    self.input(cx, Input::Finished { hud: None, id }, "finished (empty)");
                    return;
                }
                match self.insert_text(&text) {
                    Ok(()) => {
                        let words = text.split_whitespace().count();
                        self.push_recent(text.clone(), None);
                        self.input(cx, Input::TranscriptReady { text, id }, "transcript ready");
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
                        self.input(
                            cx,
                            Input::TranscriptReady {
                                text: String::new(),
                                id,
                            },
                            "transcript ready (insert failed)",
                        );
                        let hud = match kind {
                            InsertFailure::FocusLost => UiHud::FocusLost,
                            InsertFailure::Blocked => UiHud::InsertBlocked,
                        };
                        self.show_ephemeral(cx, hud, "insert failed");
                    }
                }
            }
            Mode::Command => self.resolve_command(cx, id, text),
        }
    }

    pub(crate) fn insert_text(&mut self, text: &str) -> Result<(), InsertFailure> {
        let pid = self.target.as_ref().map(|t| t.pid);
        let bundle = self.target.as_ref().and_then(|t| t.bundle_id.clone());
        let mut mode = match self.settings.insert {
            settings::Insert::Paste => InsertMode::Paste,
            settings::Insert::Type => InsertMode::Type,
        };
        if let Some(bundle) = bundle {
            if self.settings.insert_type_apps.contains(&bundle) {
                mode = InsertMode::Type;
            }
        }
        // No post-paste wait here: the platform restores the clipboard on
        // its own background thread (~150 ms), and the app process
        // outlives it. Sleeping would stall the 20 ms tick.
        match insert::insert_text(text, pid, mode, self.settings.restore_clipboard) {
            Ok(()) => Ok(()),
            Err(e) => {
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
            "recent{}: {text:?}",
            tag.as_deref()
                .map(|t| format!(" [{t}]"))
                .unwrap_or_default()
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

pub(crate) fn find_input_device(wanted: &str) -> Option<String> {
    let host = cpal::default_host();
    host.input_devices().ok()?.find_map(|device| {
        let name = device.name().ok()?;
        (name == wanted).then_some(name)
    })
}

pub(crate) fn is_bluetooth_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.contains("bluetooth") || lower.contains("airpods") || lower.contains("beats")
}
