//! Hold-key / utterance state machine.
//!
//! Pure and platform-independent: the macOS event tap feeds key events and
//! timestamps in via [`HoldMachine::handle`], calls [`HoldMachine::tick`]
//! periodically for timeouts, and executes the returned [`Effect`]s.
//!
//! Time is passed in as `u64` milliseconds; there are no clocks here, so the
//! machine is fully deterministic. Rules implemented are
//! `docs/design/keybindings.md` §4 (hold-alone, first key wins, 150 ms
//! minimum, 2 min cap, Esc/Return capture) and §5 (contexts),
//! `docs/design/jisuno.md` §7 (model lifecycle) and §10 rounds 4–5, and
//! `voice-control.md` §6 (confirm flow).

/// Which hold key started the utterance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldKind {
    /// Hold Right ⌥: dictate into the focused app.
    Dictate,
    /// Hold Right ⌘: speak a command phrase.
    Command,
}

/// Transcription mode requested from the host on release / cap / hands-free end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Dictate,
    Command,
}

impl From<HoldKind> for Mode {
    fn from(kind: HoldKind) -> Self {
        match kind {
            HoldKind::Dictate => Mode::Dictate,
            HoldKind::Command => Mode::Command,
        }
    }
}

/// HUD content to show. The host owns pixels; this only names the state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HudState {
    /// Hold passed the minimum; live utterance in progress.
    Listening(Mode),
    /// Hands-free session recording.
    HandsFree,
}

/// Input event fed by the event tap / host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    /// A hold key went down.
    HoldDown(HoldKind),
    /// A hold key went up.
    HoldUp(HoldKind),
    /// Any other key (or chord involving one) went down while a hold is active.
    /// A hold counts only if no other key is pressed during it.
    ///
    /// Never send this for the other hold key: feed HoldDown/HoldUp for it
    /// instead. The first hold key wins; the other hold key neither switches
    /// mode nor breaks the hold.
    OtherKeyDown,
    /// Hands-free toggle chord (⌃⌥Space).
    HandsFreeToggle,
    /// Esc in a listening/confirming context.
    Cancel,
    /// Return in a confirming context.
    Confirm,
    /// Voice activity judged the line silent (hands-free only).
    SilenceDetected,
    /// The host finished transcribing a dictation utterance. Ignored unless
    /// the machine is transcribing a dictate utterance with this `id`: in
    /// Command mode the host must end the utterance with [`Input::CommandResolved`]
    /// or [`Input::Finished`] instead.
    TranscriptReady { text: String, id: u64 },
    /// The host transcribed a command utterance and resolved it. Ignored
    /// unless the machine is transcribing a command utterance with this `id`.
    CommandResolved {
        needs_confirm: bool,
        label: String,
        id: u64,
    },
    /// The host is done with this utterance without running anything: no
    /// match ("Did you mean …"), empty transcript, or a transcription/model
    /// error. Carries the host's result line to show, if any. Ignored unless
    /// the machine is transcribing with this `id`.
    Finished { hud: Option<HudState>, id: u64 },
    /// The host started a model load on its own (e.g. the warm key ⌃⌥W).
    /// Recorded as an unowned load: later taps never cancel it.
    LoadStarted,
    /// A model load failed. Drops back to unloaded; a live session keeps
    /// recording and the host reports the failure at Transcribe time.
    LoadFailed,
    /// The model finished loading and is warm.
    ModelWarm,
    /// The event tap was re-enabled or Secure Input state changed. Any live
    /// capture (hold or hands-free) is discarded; a pending confirm is
    /// cancelled. An utterance already handed to the host is left alone.
    Resync,
}

/// Effect for the host to execute, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    OpenMic,
    CloseMic {
        discard: bool,
    },
    StartModelLoad,
    CancelModelLoad,
    ShowHud(HudState),
    HideHud,
    /// Hand audio to the host for transcription. `id` pairs this handoff
    /// with the host's reply; replies are [`Input`]s carrying the same `id`.
    Transcribe {
        mode: Mode,
        id: u64,
    },
    RunCommand,
    ShowConfirm {
        label: String,
    },
    /// The chord must reach the app untouched. The tap never swallowed it, so
    /// this is a no-op for the tap; it is emitted for clarity.
    PassThrough,
    Countdown {
        secs_left: u64,
    },
}

/// Key the tap may need to capture, per `keybindings.md` §5.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapturedKey {
    Escape,
    Return,
}

/// Tunable thresholds. Defaults are the signed design values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// Release before this is a tap: discard, no HUD, nothing transcribed.
    pub min_hold_ms: u64,
    /// Confirm prompt expires after this with no answer.
    pub confirm_timeout_ms: u64,
    /// Dictation/hands-free cap; at the cap Jisuno stops and transcribes.
    pub dictation_limit_s: u64,
    /// Countdown effects start this far before the cap, one per whole second.
    pub countdown_lead_s: u64,
    /// Safety net base: if the host never answers a `Transcribe` (crash, lost
    /// reply), `tick` recovers to Idle after this plus the clip duration, so a
    /// lost reply cannot wedge the machine.
    pub transcribe_timeout_ms: u64,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            min_hold_ms: 150,
            confirm_timeout_ms: 6000,
            dictation_limit_s: 120,
            countdown_lead_s: 15,
            transcribe_timeout_ms: 10_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    /// Key down, under the minimum hold. Mic open, no HUD yet.
    Pending {
        kind: HoldKind,
        since: u64,
    },
    /// Hold passed the minimum. HUD visible.
    Listening {
        kind: HoldKind,
        since: u64,
    },
    /// Hands-free session. HUD visible from the start.
    HandsFree {
        since: u64,
    },
    /// Audio handed to the host for transcription, awaiting its reply.
    /// `since` is when the handoff happened; `clip_ms` is the captured audio
    /// length (mic open to close). The transcribe timeout runs from the
    /// handoff for `transcribe_timeout_ms + clip_ms`; `id` pairs the handoff
    /// with the host's reply so a stale reply cannot disturb a later
    /// utterance.
    Transcribing {
        mode: Mode,
        since: u64,
        id: u64,
        clip_ms: u64,
    },
    /// Command awaiting Return / Esc / timeout.
    Confirming {
        since: u64,
    },
}

/// Model load tracking. `Loading` carries the press that started it, if any:
/// only that press may cancel it. A host-started load (warm key) has no owner
/// and is never cancelled by a tap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoadState {
    Unloaded,
    Loading { owner: Option<u64> },
    Warm,
}

/// Pure hold-key state machine. See the module docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoldMachine {
    timing: Timing,
    state: State,
    load: LoadState,
    /// Next utterance id handed out by `Transcribe`.
    next_utterance_id: u64,
    /// Next press/session id handed out to a mic session.
    next_press_id: u64,
    /// The press/session currently holding the mic, if any.
    active_press: Option<u64>,
    /// Last `secs_left` emitted for the current session, to emit each whole
    /// second exactly once.
    last_countdown: Option<u64>,
}

impl HoldMachine {
    pub fn new(timing: Timing) -> Self {
        Self {
            timing,
            state: State::Idle,
            load: LoadState::Unloaded,
            next_utterance_id: 0,
            next_press_id: 0,
            active_press: None,
            last_countdown: None,
        }
    }

    /// Explicit setter for the warm state, alongside the [`Input::ModelWarm`]
    /// event (e.g. the host pre-warmed the model under us).
    pub fn set_model_warm(&mut self, warm: bool) {
        self.load = if warm {
            LoadState::Warm
        } else {
            LoadState::Unloaded
        };
    }

    pub fn is_model_warm(&self) -> bool {
        self.load == LoadState::Warm
    }

    /// Whether a hold, hands-free session, utterance or confirm is in
    /// progress (any state but idle). The host logs other keys only then.
    pub fn in_session(&self) -> bool {
        self.state != State::Idle
    }

    /// Whether the tap must capture `key` right now (`keybindings.md` §5):
    /// Esc only in `listening`/`confirming`, Return only in `confirming`.
    /// Everywhere else both pass through to the app untouched.
    pub fn captures(&self, key: CapturedKey) -> bool {
        match key {
            CapturedKey::Escape => matches!(
                self.state,
                State::Listening { .. } | State::HandsFree { .. } | State::Confirming { .. }
            ),
            CapturedKey::Return => matches!(self.state, State::Confirming { .. }),
        }
    }

    pub fn handle(&mut self, ev: Input, now_ms: u64) -> Vec<Effect> {
        match ev {
            Input::HoldDown(kind) => self.on_hold_down(kind, now_ms),
            Input::HoldUp(kind) => self.on_hold_up(kind, now_ms),
            Input::OtherKeyDown => self.on_other_key_down(),
            Input::HandsFreeToggle => self.on_hands_free_toggle(now_ms),
            Input::Cancel => self.on_cancel(),
            Input::Confirm => self.on_confirm(),
            Input::SilenceDetected => self.on_silence(now_ms),
            Input::TranscriptReady { id, .. } => self.on_transcript_ready(id),
            Input::CommandResolved {
                needs_confirm,
                label,
                id,
            } => self.on_command_resolved(needs_confirm, label, id, now_ms),
            Input::Finished { hud, id } => self.on_finished(hud, id),
            Input::LoadStarted => {
                if self.load == LoadState::Unloaded {
                    self.load = LoadState::Loading { owner: None };
                }
                Vec::new()
            }
            Input::LoadFailed => {
                self.load = LoadState::Unloaded;
                Vec::new()
            }
            Input::ModelWarm => {
                self.set_model_warm(true);
                Vec::new()
            }
            Input::Resync => self.on_resync(),
        }
    }

    /// Periodic timeout driver: minimum-hold promotion, countdown, cap,
    /// confirm expiry, and transcribe recovery. Idempotent; call as often as
    /// convenient.
    pub fn tick(&mut self, now_ms: u64) -> Vec<Effect> {
        match self.state {
            State::Pending { kind, since } => {
                if elapsed(now_ms, since) >= self.timing.min_hold_ms {
                    self.state = State::Listening { kind, since };
                    let mut fx = vec![Effect::ShowHud(HudState::Listening(kind.into()))];
                    fx.extend(self.check_cap(now_ms, kind.into(), since));
                    fx
                } else {
                    Vec::new()
                }
            }
            State::Listening { kind, since } => self.check_cap(now_ms, kind.into(), since),
            State::HandsFree { since } => self.check_cap(now_ms, Mode::Dictate, since),
            State::Confirming { since } => {
                if elapsed(now_ms, since) >= self.timing.confirm_timeout_ms {
                    self.state = State::Idle;
                    vec![Effect::HideHud]
                } else {
                    Vec::new()
                }
            }
            State::Transcribing { since, clip_ms, .. } => {
                let budget = self.timing.transcribe_timeout_ms.saturating_add(clip_ms);
                if elapsed(now_ms, since) >= budget {
                    self.state = State::Idle;
                    vec![Effect::HideHud]
                } else {
                    Vec::new()
                }
            }
            State::Idle => Vec::new(),
        }
    }

    /// Open the mic for a new press/session. Starts a model load only from
    /// [`LoadState::Unloaded`]; a host-started (warm key) or already-running
    /// load is left alone and can never be cancelled by this session.
    fn begin_session(&mut self) -> Vec<Effect> {
        let press = self.next_press_id;
        self.next_press_id = self.next_press_id.saturating_add(1);
        self.active_press = Some(press);
        self.last_countdown = None;
        let mut fx = vec![Effect::OpenMic];
        if self.load == LoadState::Unloaded {
            self.load = LoadState::Loading { owner: Some(press) };
            fx.push(Effect::StartModelLoad);
        }
        fx
    }

    /// Whether the active session owns the in-flight load.
    fn owns_load(&self) -> bool {
        matches!(self.load, LoadState::Loading { owner } if owner == self.active_press)
            && self.active_press.is_some()
    }

    /// Cancel the in-flight load iff the active session owns it.
    fn cancel_owned_load(&mut self) -> Option<Effect> {
        if self.owns_load() {
            self.load = LoadState::Unloaded;
            Some(Effect::CancelModelLoad)
        } else {
            None
        }
    }

    /// Move to [`State::Transcribing`], handing out the next utterance id and
    /// dropping session ownership (a still-running load is left for the
    /// transcription). Returns the utterance id for the `Transcribe` effect.
    fn enter_transcribing(&mut self, mode: Mode, now_ms: u64, clip_ms: u64) -> u64 {
        let id = self.next_utterance_id;
        self.next_utterance_id = self.next_utterance_id.saturating_add(1);
        self.state = State::Transcribing {
            mode,
            since: now_ms,
            id,
            clip_ms,
        };
        self.active_press = None;
        id
    }

    fn idle(&mut self) {
        self.state = State::Idle;
        self.active_press = None;
        self.last_countdown = None;
    }

    fn on_hold_down(&mut self, kind: HoldKind, now_ms: u64) -> Vec<Effect> {
        // First hold key wins: any down outside Idle is ignored entirely, so
        // the other hold key neither switches mode nor breaks the hold.
        // Holds during hands-free are likewise ignored.
        if self.state != State::Idle {
            return Vec::new();
        }
        self.state = State::Pending {
            kind,
            since: now_ms,
        };
        self.begin_session()
    }

    fn on_hold_up(&mut self, kind: HoldKind, now_ms: u64) -> Vec<Effect> {
        let (pending, active, since) = match self.state {
            State::Pending { kind: k, since } => (true, k, since),
            State::Listening { kind: k, since } => (false, k, since),
            _ => return Vec::new(),
        };
        if active != kind {
            return Vec::new();
        }
        if elapsed(now_ms, since) < self.timing.min_hold_ms {
            // Tap: discard silently; a load this press started is cancelled.
            // A load that already finished (Warm) or that the host started
            // (warm key, unowned) is left alone. Ownership is checked before
            // going idle, which clears the active press.
            let cancel = self.cancel_owned_load();
            self.idle();
            let mut fx = vec![Effect::CloseMic { discard: true }];
            if let Some(effect) = cancel {
                fx.push(effect);
            }
            fx
        } else {
            // Release at or past the minimum: stop and transcribe. A load
            // still in flight is left running for the transcription. If no
            // tick promoted Pending -> Listening yet, show the HUD first so
            // it appears for every real utterance.
            let clip_ms = elapsed(now_ms, since);
            let mode = active.into();
            let id = self.enter_transcribing(mode, now_ms, clip_ms);
            let mut fx = Vec::new();
            if pending {
                fx.push(Effect::ShowHud(HudState::Listening(mode)));
            }
            fx.push(Effect::CloseMic { discard: false });
            fx.push(Effect::Transcribe { mode, id });
            fx
        }
    }

    fn on_other_key_down(&mut self) -> Vec<Effect> {
        match self.state {
            State::Pending { .. } | State::Listening { .. } => {
                // Hold-alone rule: abandon silently and let the chord reach
                // the app untouched.
                let cancel = self.cancel_owned_load();
                self.idle();
                let mut fx = vec![
                    Effect::CloseMic { discard: true },
                    Effect::HideHud,
                    Effect::PassThrough,
                ];
                if let Some(effect) = cancel {
                    fx.push(effect);
                }
                fx
            }
            _ => Vec::new(),
        }
    }

    fn on_hands_free_toggle(&mut self, now_ms: u64) -> Vec<Effect> {
        match self.state {
            State::Idle => {
                self.state = State::HandsFree { since: now_ms };
                let mut fx = self.begin_session();
                fx.push(Effect::ShowHud(HudState::HandsFree));
                fx
            }
            State::HandsFree { since } => {
                // Toggle again ends the session: stop and transcribe.
                let clip_ms = elapsed(now_ms, since);
                let id = self.enter_transcribing(Mode::Dictate, now_ms, clip_ms);
                vec![
                    Effect::CloseMic { discard: false },
                    Effect::Transcribe {
                        mode: Mode::Dictate,
                        id,
                    },
                ]
            }
            // A hold utterance owns the mic; toggles elsewhere are ignored.
            _ => Vec::new(),
        }
    }

    fn on_cancel(&mut self) -> Vec<Effect> {
        match self.state {
            State::Listening { .. } | State::HandsFree { .. } => {
                let cancel = self.cancel_owned_load();
                self.idle();
                let mut fx = vec![Effect::CloseMic { discard: true }, Effect::HideHud];
                if let Some(effect) = cancel {
                    fx.push(effect);
                }
                fx
            }
            State::Confirming { .. } => {
                self.idle();
                vec![Effect::HideHud]
            }
            // Elsewhere (including Pending) Esc is not captured: ignore.
            _ => Vec::new(),
        }
    }

    fn on_confirm(&mut self) -> Vec<Effect> {
        match self.state {
            State::Confirming { .. } => {
                self.idle();
                // The host dismisses the prompt as it runs the command.
                vec![Effect::RunCommand]
            }
            _ => Vec::new(),
        }
    }

    fn on_silence(&mut self, now_ms: u64) -> Vec<Effect> {
        match self.state {
            State::HandsFree { since } => {
                let clip_ms = elapsed(now_ms, since);
                let id = self.enter_transcribing(Mode::Dictate, now_ms, clip_ms);
                vec![
                    Effect::CloseMic { discard: false },
                    Effect::Transcribe {
                        mode: Mode::Dictate,
                        id,
                    },
                ]
            }
            _ => Vec::new(),
        }
    }

    fn on_transcript_ready(&mut self, id: u64) -> Vec<Effect> {
        match self.state {
            State::Transcribing {
                mode: Mode::Dictate,
                id: current,
                ..
            } if id == current => {
                self.idle();
                vec![Effect::HideHud]
            }
            _ => Vec::new(),
        }
    }

    fn on_command_resolved(
        &mut self,
        needs_confirm: bool,
        label: String,
        id: u64,
        now_ms: u64,
    ) -> Vec<Effect> {
        match self.state {
            State::Transcribing {
                mode: Mode::Command,
                id: current,
                ..
            } if id == current => {
                if needs_confirm {
                    self.state = State::Confirming { since: now_ms };
                    vec![Effect::ShowConfirm { label }]
                } else {
                    self.idle();
                    vec![Effect::RunCommand]
                }
            }
            _ => Vec::new(),
        }
    }

    fn on_finished(&mut self, hud: Option<HudState>, id: u64) -> Vec<Effect> {
        match self.state {
            State::Transcribing { id: current, .. } if id == current => {
                self.idle();
                match hud {
                    Some(h) => vec![Effect::ShowHud(h)],
                    None => vec![Effect::HideHud],
                }
            }
            _ => Vec::new(),
        }
    }

    fn on_resync(&mut self) -> Vec<Effect> {
        match self.state {
            State::Pending { .. } | State::Listening { .. } | State::HandsFree { .. } => {
                let cancel = self.cancel_owned_load();
                let mut fx = vec![Effect::CloseMic { discard: true }, Effect::HideHud];
                if let Some(effect) = cancel {
                    fx.push(effect);
                }
                self.idle();
                fx
            }
            State::Confirming { .. } => {
                self.idle();
                vec![Effect::HideHud]
            }
            // An utterance already handed to the host keeps its reply path.
            State::Transcribing { .. } | State::Idle => Vec::new(),
        }
    }

    /// Countdown seconds then cap stop for a live session started at `since`.
    fn check_cap(&mut self, now_ms: u64, mode: Mode, since: u64) -> Vec<Effect> {
        let limit_ms = self.timing.dictation_limit_s.saturating_mul(1000);
        // With a short cap the countdown window shrinks with the limit, so no
        // value above the true remaining seconds is ever emitted.
        let window = self
            .timing
            .countdown_lead_s
            .min(self.timing.dictation_limit_s);
        let window_ms = window.saturating_mul(1000);
        let remaining = limit_ms.saturating_sub(elapsed(now_ms, since));
        if remaining == 0 {
            let clip_ms = elapsed(now_ms, since);
            let id = self.enter_transcribing(mode, now_ms, clip_ms);
            return vec![
                Effect::CloseMic { discard: false },
                Effect::Transcribe { mode, id },
            ];
        }
        if remaining > window_ms {
            return Vec::new();
        }
        // Whole seconds left, rounded down: the HUD never shows more than the
        // true remaining whole seconds.
        let secs = remaining / 1000;
        let prev = self.last_countdown.unwrap_or(window + 1);
        let mut fx = Vec::new();
        // Emit every whole second crossed since the last tick, newest last, so
        // coarse ticks still produce each second exactly once.
        for s in (secs..prev).rev() {
            if (1..=window).contains(&s) {
                fx.push(Effect::Countdown { secs_left: s });
            }
        }
        if secs < prev {
            self.last_countdown = Some(secs);
        }
        fx
    }
}

impl Default for HoldMachine {
    fn default() -> Self {
        Self::new(Timing::default())
    }
}

fn elapsed(now_ms: u64, since: u64) -> u64 {
    now_ms.saturating_sub(since)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn warm() -> HoldMachine {
        let mut m = HoldMachine::default();
        m.set_model_warm(true);
        m
    }

    fn transcribe(mode: Mode, id: u64) -> Effect {
        Effect::Transcribe { mode, id }
    }

    #[test]
    fn key_down_opens_mic_and_starts_cold_load() {
        // Rule 1: mic opens immediately; load starts iff the model is cold.
        let mut cold = HoldMachine::default();
        assert_eq!(
            cold.handle(Input::HoldDown(HoldKind::Dictate), 0),
            vec![Effect::OpenMic, Effect::StartModelLoad]
        );

        let mut m = warm();
        assert_eq!(
            m.handle(Input::HoldDown(HoldKind::Dictate), 0),
            vec![Effect::OpenMic]
        );
    }

    #[test]
    fn tap_at_149_discards_but_hold_at_150_transcribes() {
        // Rule 2 vs rule 6: the boundary is exact. The 150 ms release never
        // saw a promotion tick, so the HUD is shown first (every real
        // utterance shows it).
        let mut m = warm();
        m.handle(Input::HoldDown(HoldKind::Dictate), 1000);
        assert_eq!(
            m.handle(Input::HoldUp(HoldKind::Dictate), 1000 + 149),
            vec![Effect::CloseMic { discard: true }]
        );

        let mut m = warm();
        m.handle(Input::HoldDown(HoldKind::Dictate), 1000);
        assert_eq!(
            m.handle(Input::HoldUp(HoldKind::Dictate), 1000 + 150),
            vec![
                Effect::ShowHud(HudState::Listening(Mode::Dictate)),
                Effect::CloseMic { discard: false },
                transcribe(Mode::Dictate, 0),
            ]
        );
    }

    #[test]
    fn release_without_promotion_tick_still_shows_hud() {
        // A fast release past the minimum but before any tick promoted
        // Pending -> Listening still shows the HUD, so it appears for every
        // real utterance.
        let mut m = warm();
        m.handle(Input::HoldDown(HoldKind::Command), 0);
        assert_eq!(
            m.handle(Input::HoldUp(HoldKind::Command), 200),
            vec![
                Effect::ShowHud(HudState::Listening(Mode::Command)),
                Effect::CloseMic { discard: false },
                transcribe(Mode::Command, 0),
            ]
        );

        // After a promotion tick the HUD is already up: no duplicate.
        let mut m = warm();
        m.handle(Input::HoldDown(HoldKind::Dictate), 0);
        assert_eq!(
            m.tick(200),
            vec![Effect::ShowHud(HudState::Listening(Mode::Dictate))]
        );
        assert_eq!(
            m.handle(Input::HoldUp(HoldKind::Dictate), 300),
            vec![
                Effect::CloseMic { discard: false },
                transcribe(Mode::Dictate, 0),
            ]
        );
    }

    #[test]
    fn hud_appears_only_past_the_minimum() {
        // Rule 3: no HUD under 150 ms; ShowHud on the tick that passes it.
        let mut m = warm();
        m.handle(Input::HoldDown(HoldKind::Dictate), 0);
        assert!(m.tick(149).is_empty());
        assert_eq!(
            m.tick(150),
            vec![Effect::ShowHud(HudState::Listening(Mode::Dictate))]
        );
        // And only once: the next tick is quiet.
        assert!(m.tick(200).is_empty());
    }

    #[test]
    fn in_session_only_during_a_hold() {
        // The host logs other keys only while a session is live.
        let mut m = warm();
        assert!(!m.in_session());
        m.handle(Input::HoldDown(HoldKind::Dictate), 0);
        assert!(m.in_session());
        m.handle(Input::OtherKeyDown, 50);
        assert!(!m.in_session());
    }

    #[test]
    fn chord_mid_hold_abandons_and_passes_through() {
        // Rule 4: Right ⌘+C types Cmd+C in the app; Jisuno does nothing.
        let mut m = warm();
        m.handle(Input::HoldDown(HoldKind::Command), 0);
        assert_eq!(
            m.tick(200),
            vec![Effect::ShowHud(HudState::Listening(Mode::Command))]
        );
        assert_eq!(
            m.handle(Input::OtherKeyDown, 250),
            vec![
                Effect::CloseMic { discard: true },
                Effect::HideHud,
                Effect::PassThrough,
            ]
        );
        // Abandoned: a late release is ignored, machine is idle.
        assert!(m.handle(Input::HoldUp(HoldKind::Command), 300).is_empty());
    }

    #[test]
    fn first_hold_key_wins() {
        // Rule 5: Right ⌥ down, then Right ⌘ down/up, then Right ⌥ up.
        let mut m = warm();
        m.handle(Input::HoldDown(HoldKind::Dictate), 0);
        assert!(m.handle(Input::HoldDown(HoldKind::Command), 10).is_empty());
        assert!(m.handle(Input::HoldUp(HoldKind::Command), 200).is_empty());
        assert_eq!(
            m.tick(200),
            vec![Effect::ShowHud(HudState::Listening(Mode::Dictate))]
        );
        assert_eq!(
            m.handle(Input::HoldUp(HoldKind::Dictate), 300),
            vec![
                Effect::CloseMic { discard: false },
                transcribe(Mode::Dictate, 0),
            ]
        );
    }

    #[test]
    fn release_after_hold_transcribes_with_mode() {
        // Rule 6, command flavour. No promotion tick ran, so the HUD is shown
        // first.
        let mut m = warm();
        m.handle(Input::HoldDown(HoldKind::Command), 0);
        assert_eq!(
            m.handle(Input::HoldUp(HoldKind::Command), 500),
            vec![
                Effect::ShowHud(HudState::Listening(Mode::Command)),
                Effect::CloseMic { discard: false },
                transcribe(Mode::Command, 0),
            ]
        );
    }

    #[test]
    fn cold_tap_cancels_load_but_cold_hold_does_not() {
        // Rule 1+2: tap cancels a load this press started.
        let mut m = HoldMachine::default();
        assert_eq!(
            m.handle(Input::HoldDown(HoldKind::Dictate), 0),
            vec![Effect::OpenMic, Effect::StartModelLoad]
        );
        assert_eq!(
            m.handle(Input::HoldUp(HoldKind::Dictate), 100),
            vec![Effect::CloseMic { discard: true }, Effect::CancelModelLoad,]
        );

        // A hold release leaves a still-running load alone.
        let mut m = HoldMachine::default();
        m.handle(Input::HoldDown(HoldKind::Dictate), 0);
        assert_eq!(
            m.handle(Input::HoldUp(HoldKind::Dictate), 500),
            vec![
                Effect::ShowHud(HudState::Listening(Mode::Dictate)),
                Effect::CloseMic { discard: false },
                transcribe(Mode::Dictate, 0),
            ]
        );
        // And a load that already finished is never "cancelled" by a tap.
        let mut m = HoldMachine::default();
        m.handle(Input::HoldDown(HoldKind::Dictate), 0);
        m.handle(Input::ModelWarm, 50);
        assert_eq!(
            m.handle(Input::HoldUp(HoldKind::Dictate), 100),
            vec![Effect::CloseMic { discard: true }]
        );
    }

    #[test]
    fn warm_key_load_survives_tap() {
        // A load the host started (warm key ⌃⌥W) is unowned: a hold starts no
        // second load and a tap cancels nothing.
        let mut m = HoldMachine::default();
        assert!(m.handle(Input::LoadStarted, 0).is_empty());
        assert_eq!(
            m.handle(Input::HoldDown(HoldKind::Dictate), 5),
            vec![Effect::OpenMic]
        );
        assert_eq!(
            m.handle(Input::HoldUp(HoldKind::Dictate), 50),
            vec![Effect::CloseMic { discard: true }]
        );
        // Still loading, not unloaded: the next hold starts no load either.
        assert_eq!(
            m.handle(Input::HoldDown(HoldKind::Dictate), 100),
            vec![Effect::OpenMic]
        );
        // The load finishing warms the machine as usual.
        m.handle(Input::ModelWarm, 150);
        assert!(m.is_model_warm());
    }

    #[test]
    fn double_key_down_does_not_double_start() {
        // Second down of either hold key while not idle is ignored: exactly
        // one StartModelLoad for the press.
        let mut m = HoldMachine::default();
        assert_eq!(
            m.handle(Input::HoldDown(HoldKind::Dictate), 0),
            vec![Effect::OpenMic, Effect::StartModelLoad]
        );
        assert!(m.handle(Input::HoldDown(HoldKind::Command), 10).is_empty());
        assert!(m.handle(Input::HoldDown(HoldKind::Dictate), 20).is_empty());
        assert_eq!(
            m.handle(Input::HoldUp(HoldKind::Dictate), 300),
            vec![
                Effect::ShowHud(HudState::Listening(Mode::Dictate)),
                Effect::CloseMic { discard: false },
                transcribe(Mode::Dictate, 0),
            ]
        );
    }

    #[test]
    fn load_failed_unloads_but_keeps_listening() {
        // The session keeps recording; the host reports the failure when the
        // Transcribe runs.
        let mut m = HoldMachine::default();
        m.handle(Input::HoldDown(HoldKind::Dictate), 0);
        assert_eq!(
            m.tick(200),
            vec![Effect::ShowHud(HudState::Listening(Mode::Dictate))]
        );
        assert!(m.handle(Input::LoadFailed, 250).is_empty());
        assert!(!m.is_model_warm());
        assert_eq!(
            m.handle(Input::HoldUp(HoldKind::Dictate), 300),
            vec![
                Effect::CloseMic { discard: false },
                transcribe(Mode::Dictate, 0),
            ]
        );

        // After a failure there is no owned load left for a tap to cancel.
        let mut m = HoldMachine::default();
        m.handle(Input::HoldDown(HoldKind::Dictate), 0);
        m.handle(Input::LoadFailed, 50);
        assert_eq!(
            m.handle(Input::HoldUp(HoldKind::Dictate), 100),
            vec![Effect::CloseMic { discard: true }]
        );
    }

    #[test]
    fn hands_free_toggle_starts_immediately() {
        // Rule 7: no 150 ms rule; HUD at once.
        let mut m = warm();
        assert_eq!(
            m.handle(Input::HandsFreeToggle, 0),
            vec![Effect::OpenMic, Effect::ShowHud(HudState::HandsFree)]
        );
    }

    #[test]
    fn hands_free_ends_on_silence_or_toggle_and_cancels_on_esc() {
        // Rule 7: silence end and second-toggle end transcribe; Cancel discards.
        let mut m = warm();
        m.handle(Input::HandsFreeToggle, 0);
        assert_eq!(
            m.handle(Input::SilenceDetected, 5000),
            vec![
                Effect::CloseMic { discard: false },
                transcribe(Mode::Dictate, 0),
            ]
        );

        let mut m = warm();
        m.handle(Input::HandsFreeToggle, 0);
        assert_eq!(
            m.handle(Input::HandsFreeToggle, 5000),
            vec![
                Effect::CloseMic { discard: false },
                transcribe(Mode::Dictate, 0),
            ]
        );

        let mut m = warm();
        m.handle(Input::HandsFreeToggle, 0);
        assert_eq!(
            m.handle(Input::Cancel, 5000),
            vec![Effect::CloseMic { discard: true }, Effect::HideHud]
        );
    }

    #[test]
    fn hold_keys_ignored_during_hands_free() {
        // Rule 7: hold downs/ups change nothing mid-session.
        let mut m = warm();
        m.handle(Input::HandsFreeToggle, 0);
        assert!(m.handle(Input::HoldDown(HoldKind::Dictate), 100).is_empty());
        assert!(m.handle(Input::HoldUp(HoldKind::Dictate), 200).is_empty());
        assert!(m.handle(Input::HoldDown(HoldKind::Command), 300).is_empty());
        // Session still alive: silence still ends it.
        assert_eq!(
            m.handle(Input::SilenceDetected, 400),
            vec![
                Effect::CloseMic { discard: false },
                transcribe(Mode::Dictate, 0),
            ]
        );
    }

    #[test]
    fn cap_stops_and_transcribes_with_countdown_15_to_1() {
        // Rule 8: 120 s cap, countdown from limit-15 s each whole second.
        let mut m = HoldMachine::new(Timing {
            dictation_limit_s: 120,
            ..Timing::default()
        });
        m.set_model_warm(true);
        m.handle(Input::HoldDown(HoldKind::Dictate), 0);
        let mut fx = Vec::new();
        for s in 0..15 {
            fx.extend(m.tick((105 + s) * 1000));
        }
        let expected: Vec<Effect> = (1..=15)
            .rev()
            .map(|s| Effect::Countdown { secs_left: s })
            .collect();
        assert_eq!(fx[0], Effect::ShowHud(HudState::Listening(Mode::Dictate)));
        assert_eq!(&fx[1..], &expected[..]);
        // At exactly the cap: stop and transcribe as if released.
        assert_eq!(
            m.tick(120 * 1000),
            vec![
                Effect::CloseMic { discard: false },
                transcribe(Mode::Dictate, 0),
            ]
        );
    }

    #[test]
    fn countdown_with_short_cap_never_exceeds_remaining() {
        // Window is min(lead, limit): with a 10 s cap no value above 10 is
        // ever emitted, and every whole second 10..1 appears exactly once.
        let mut m = HoldMachine::new(Timing {
            dictation_limit_s: 10,
            ..Timing::default()
        });
        m.set_model_warm(true);
        m.handle(Input::HoldDown(HoldKind::Dictate), 0);
        let mut seen = Vec::new();
        let mut saw_hud = false;
        let mut t = 100;
        while t < 10 * 1000 {
            for e in m.tick(t) {
                match e {
                    Effect::ShowHud(HudState::Listening(Mode::Dictate)) => saw_hud = true,
                    Effect::Countdown { secs_left } => seen.push(secs_left),
                    other => panic!("unexpected effect before cap: {other:?}"),
                }
            }
            t += 100;
        }
        assert!(saw_hud, "HUD shows for the capped utterance");
        assert!(
            seen.iter().all(|&s| (1..=10).contains(&s)),
            "no countdown above the true remaining seconds: {seen:?}"
        );
        let mut sorted = seen.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted, (1..=10).collect::<Vec<_>>());
        assert_eq!(
            m.tick(10 * 1000),
            vec![
                Effect::CloseMic { discard: false },
                transcribe(Mode::Dictate, 0),
            ]
        );
    }

    #[test]
    fn cap_applies_to_hands_free_too() {
        // Rule 8 covers hands-free alike; mode is always Dictate there.
        let mut m = HoldMachine::new(Timing {
            dictation_limit_s: 120,
            ..Timing::default()
        });
        m.set_model_warm(true);
        m.handle(Input::HandsFreeToggle, 0);
        // No countdown yet at 15 s into a 120 s session.
        assert!(m.tick(15 * 1000).is_empty());
        assert_eq!(
            m.tick(120 * 1000),
            vec![
                Effect::CloseMic { discard: false },
                transcribe(Mode::Dictate, 0),
            ]
        );
    }

    #[test]
    fn cancel_only_meaningful_while_live() {
        // Rule 9: Esc cancels a live hold; elsewhere it is ignored.
        let mut m = warm();
        m.handle(Input::HoldDown(HoldKind::Dictate), 0);
        assert_eq!(
            m.tick(200),
            vec![Effect::ShowHud(HudState::Listening(Mode::Dictate))]
        );
        assert_eq!(
            m.handle(Input::Cancel, 250),
            vec![Effect::CloseMic { discard: true }, Effect::HideHud]
        );

        // Idle, pending (< 150 ms), transcribing: ignored.
        let mut m = warm();
        assert!(m.handle(Input::Cancel, 0).is_empty());
        m.handle(Input::HoldDown(HoldKind::Dictate), 0);
        assert!(m.handle(Input::Cancel, 100).is_empty());
    }

    #[test]
    fn captures_matches_keybindings_contexts() {
        // Rule 9 (`keybindings.md` §5): Esc in listening/confirming, Return
        // only in confirming; both free everywhere else.
        let mut m = warm();
        assert!(!m.captures(CapturedKey::Escape));
        assert!(!m.captures(CapturedKey::Return));

        m.handle(Input::HoldDown(HoldKind::Dictate), 0);
        assert!(!m.captures(CapturedKey::Escape), "pending is not listening");
        m.tick(200);
        assert!(m.captures(CapturedKey::Escape));
        assert!(!m.captures(CapturedKey::Return));

        m.handle(Input::HoldUp(HoldKind::Dictate), 300);
        assert!(
            !m.captures(CapturedKey::Escape),
            "transcribing captures nothing"
        );

        let mut m = warm();
        m.handle(Input::HandsFreeToggle, 0);
        assert!(m.captures(CapturedKey::Escape));
        assert!(!m.captures(CapturedKey::Return));
    }

    #[test]
    fn command_confirm_flow() {
        // Rule 10: resolve -> confirm prompt -> run / cancel / timeout.
        let mut m = warm();
        m.handle(Input::HoldDown(HoldKind::Command), 0);
        m.handle(Input::HoldUp(HoldKind::Command), 500);

        // Needs confirmation: prompt shows.
        assert_eq!(
            m.handle(
                Input::CommandResolved {
                    needs_confirm: true,
                    label: "clean build".to_string(),
                    id: 0,
                },
                600
            ),
            vec![Effect::ShowConfirm {
                label: "clean build".to_string(),
            }]
        );
        assert!(m.captures(CapturedKey::Escape));
        assert!(m.captures(CapturedKey::Return));
        // Return runs it.
        assert_eq!(m.handle(Input::Confirm, 700), vec![Effect::RunCommand]);

        // Esc cancels: prompt hides, nothing runs.
        let mut m = warm();
        m.handle(Input::HoldDown(HoldKind::Command), 0);
        m.handle(Input::HoldUp(HoldKind::Command), 500);
        m.handle(
            Input::CommandResolved {
                needs_confirm: true,
                label: "clean build".to_string(),
                id: 0,
            },
            600,
        );
        assert_eq!(m.handle(Input::Cancel, 700), vec![Effect::HideHud]);

        // 6 s with no answer cancels.
        let mut m = warm();
        m.handle(Input::HoldDown(HoldKind::Command), 0);
        m.handle(Input::HoldUp(HoldKind::Command), 500);
        m.handle(
            Input::CommandResolved {
                needs_confirm: true,
                label: "clean build".to_string(),
                id: 0,
            },
            600,
        );
        assert!(m.tick(600 + 5999).is_empty());
        assert_eq!(m.tick(600 + 6000), vec![Effect::HideHud]);
        assert!(!m.captures(CapturedKey::Return));

        // No confirmation needed: runs straight away.
        let mut m = warm();
        m.handle(Input::HoldDown(HoldKind::Command), 0);
        m.handle(Input::HoldUp(HoldKind::Command), 500);
        assert_eq!(
            m.handle(
                Input::CommandResolved {
                    needs_confirm: false,
                    label: String::new(),
                    id: 0,
                },
                600
            ),
            vec![Effect::RunCommand]
        );
    }

    #[test]
    fn command_no_match_finishes_and_new_hold_works() {
        // Liveness: "Did you mean …" / no match ends the utterance via
        // Finished; the machine is Idle and the next hold works.
        let mut m = warm();
        m.handle(Input::HoldDown(HoldKind::Command), 0);
        m.handle(Input::HoldUp(HoldKind::Command), 500);
        assert_eq!(
            m.handle(Input::Finished { hud: None, id: 0 }, 900),
            vec![Effect::HideHud]
        );
        assert!(!m.captures(CapturedKey::Escape));
        assert_eq!(
            m.handle(Input::HoldDown(HoldKind::Dictate), 1000),
            vec![Effect::OpenMic]
        );
    }

    #[test]
    fn finished_with_result_line_shows_it() {
        // The host's result line (e.g. "Did you mean …") replaces the HUD.
        let mut m = warm();
        m.handle(Input::HoldDown(HoldKind::Command), 0);
        m.handle(Input::HoldUp(HoldKind::Command), 500);
        let hud = HudState::Listening(Mode::Command);
        assert_eq!(
            m.handle(
                Input::Finished {
                    hud: Some(hud),
                    id: 0
                },
                900
            ),
            vec![Effect::ShowHud(hud)]
        );
        // Finished outside Transcribing is ignored.
        assert!(m
            .handle(Input::Finished { hud: None, id: 0 }, 950)
            .is_empty());
    }

    #[test]
    fn dictate_error_finishes_and_new_hold_works() {
        // Transcription/model error on the dictate path: hide, Idle, live.
        let mut m = warm();
        m.handle(Input::HoldDown(HoldKind::Dictate), 0);
        m.tick(200);
        m.handle(Input::HoldUp(HoldKind::Dictate), 300);
        assert_eq!(
            m.handle(Input::Finished { hud: None, id: 0 }, 1200),
            vec![Effect::HideHud]
        );
        assert_eq!(
            m.handle(Input::HoldDown(HoldKind::Dictate), 1300),
            vec![Effect::OpenMic]
        );
    }

    #[test]
    fn transcribe_timeout_recovers_to_idle() {
        // Safety net: a lost host reply cannot wedge the machine. The budget
        // is the base timeout plus the 500 ms clip, counted from the key-up
        // handoff.
        let mut m = warm();
        m.handle(Input::HoldDown(HoldKind::Dictate), 0);
        m.handle(Input::HoldUp(HoldKind::Dictate), 500);
        assert!(m.tick(500 + 10_000 + 500 - 1).is_empty());
        assert_eq!(m.tick(500 + 10_000 + 500), vec![Effect::HideHud]);
        assert!(!m.captures(CapturedKey::Escape));
        assert_eq!(
            m.handle(Input::HoldDown(HoldKind::Dictate), 11_000),
            vec![Effect::OpenMic]
        );
    }

    #[test]
    fn transcribe_timeout_includes_long_clip() {
        // A 120 s capped dictation gets 120 s of clip on top of the base
        // timeout before the machine recovers.
        let mut m = HoldMachine::new(Timing {
            dictation_limit_s: 120,
            ..Timing::default()
        });
        m.set_model_warm(true);
        m.handle(Input::HoldDown(HoldKind::Dictate), 0);
        m.tick(150);
        assert_eq!(
            m.tick(120 * 1000),
            vec![
                Effect::CloseMic { discard: false },
                transcribe(Mode::Dictate, 0),
            ]
        );
        let handoff = 120 * 1000;
        assert!(m.tick(handoff + 10_000 + 120_000 - 1).is_empty());
        assert_eq!(m.tick(handoff + 10_000 + 120_000), vec![Effect::HideHud]);
        assert_eq!(
            m.handle(Input::HoldDown(HoldKind::Dictate), handoff + 130_001),
            vec![Effect::OpenMic]
        );
    }

    #[test]
    fn stale_reply_after_timeout_does_not_disturb_new_utterance() {
        // Timeout recovers to Idle; a new hold transcribes as id 1; the late
        // reply for id 0 is ignored and the new utterance completes normally.
        let mut m = warm();
        m.handle(Input::HoldDown(HoldKind::Dictate), 0);
        m.tick(200);
        m.handle(Input::HoldUp(HoldKind::Dictate), 300);
        m.tick(300 + 10_000 + 300); // budget spent: Idle.

        m.handle(Input::HoldDown(HoldKind::Dictate), 11_000);
        m.tick(11_200);
        assert_eq!(
            m.handle(Input::HoldUp(HoldKind::Dictate), 11_300),
            vec![
                Effect::CloseMic { discard: false },
                transcribe(Mode::Dictate, 1),
            ]
        );
        // Late replies for the old utterance: all ignored.
        assert!(m
            .handle(
                Input::TranscriptReady {
                    text: "old".to_string(),
                    id: 0,
                },
                11_400
            )
            .is_empty());
        assert!(m
            .handle(Input::Finished { hud: None, id: 0 }, 11_400)
            .is_empty());
        assert!(m
            .handle(
                Input::CommandResolved {
                    needs_confirm: false,
                    label: String::new(),
                    id: 0,
                },
                11_400
            )
            .is_empty());
        // The current utterance still completes.
        assert_eq!(
            m.handle(
                Input::TranscriptReady {
                    text: "new".to_string(),
                    id: 1,
                },
                11_500
            ),
            vec![Effect::HideHud]
        );
        assert_eq!(
            m.handle(Input::HoldDown(HoldKind::Dictate), 11_600),
            vec![Effect::OpenMic]
        );
    }

    #[test]
    fn stale_command_reply_is_ignored() {
        // Same id pairing on the command path: a late resolve for the old
        // utterance does not pop a confirm prompt over the new one.
        let mut m = warm();
        m.handle(Input::HoldDown(HoldKind::Command), 0);
        m.handle(Input::HoldUp(HoldKind::Command), 500);
        m.tick(500 + 10_000 + 500); // budget spent: Idle.

        m.handle(Input::HoldDown(HoldKind::Command), 12_000);
        m.handle(Input::HoldUp(HoldKind::Command), 12_500);
        assert!(m
            .handle(
                Input::CommandResolved {
                    needs_confirm: true,
                    label: "old".to_string(),
                    id: 0,
                },
                12_600
            )
            .is_empty());
        assert_eq!(
            m.handle(
                Input::CommandResolved {
                    needs_confirm: true,
                    label: "new".to_string(),
                    id: 1,
                },
                12_700
            ),
            vec![Effect::ShowConfirm {
                label: "new".to_string(),
            }]
        );
    }

    #[test]
    fn resync_closes_pending_and_listening() {
        // Cold pending hold: mic closes, HUD hides, the owned load cancels,
        // and the next hold starts a fresh load.
        let mut m = HoldMachine::default();
        m.handle(Input::HoldDown(HoldKind::Dictate), 0);
        assert_eq!(
            m.handle(Input::Resync, 50),
            vec![
                Effect::CloseMic { discard: true },
                Effect::HideHud,
                Effect::CancelModelLoad,
            ]
        );
        assert_eq!(
            m.handle(Input::HoldDown(HoldKind::Dictate), 100),
            vec![Effect::OpenMic, Effect::StartModelLoad]
        );

        // Listening with a warm model: no load to cancel.
        let mut m = warm();
        m.handle(Input::HoldDown(HoldKind::Dictate), 0);
        m.tick(200);
        assert_eq!(
            m.handle(Input::Resync, 250),
            vec![Effect::CloseMic { discard: true }, Effect::HideHud]
        );
        assert_eq!(
            m.handle(Input::HoldDown(HoldKind::Dictate), 300),
            vec![Effect::OpenMic]
        );
    }

    #[test]
    fn resync_discards_hands_free() {
        let mut m = HoldMachine::default();
        m.handle(Input::HandsFreeToggle, 0);
        assert_eq!(
            m.handle(Input::Resync, 5000),
            vec![
                Effect::CloseMic { discard: true },
                Effect::HideHud,
                Effect::CancelModelLoad,
            ]
        );
        assert_eq!(
            m.handle(Input::HandsFreeToggle, 5100),
            vec![
                Effect::OpenMic,
                Effect::StartModelLoad,
                Effect::ShowHud(HudState::HandsFree),
            ]
        );
    }

    #[test]
    fn resync_cancels_confirming_but_leaves_transcribing() {
        // Confirming is cancelled to Idle.
        let mut m = warm();
        m.handle(Input::HoldDown(HoldKind::Command), 0);
        m.handle(Input::HoldUp(HoldKind::Command), 500);
        m.handle(
            Input::CommandResolved {
                needs_confirm: true,
                label: "clean build".to_string(),
                id: 0,
            },
            600,
        );
        assert_eq!(m.handle(Input::Resync, 700), vec![Effect::HideHud]);
        assert!(!m.captures(CapturedKey::Return));
        assert_eq!(
            m.handle(Input::HoldDown(HoldKind::Dictate), 800),
            vec![Effect::OpenMic]
        );

        // Transcribing keeps its reply path: Resync is a no-op and the
        // matching reply still completes the utterance.
        let mut m = warm();
        m.handle(Input::HoldDown(HoldKind::Dictate), 0);
        m.tick(200);
        m.handle(Input::HoldUp(HoldKind::Dictate), 300);
        assert!(m.handle(Input::Resync, 400).is_empty());
        assert_eq!(
            m.handle(
                Input::TranscriptReady {
                    text: "hello".to_string(),
                    id: 0,
                },
                500
            ),
            vec![Effect::HideHud]
        );
    }

    #[test]
    fn run_command_leaves_machine_idle() {
        // Confirm -> RunCommand must leave Idle so the next hold works.
        let mut m = warm();
        m.handle(Input::HoldDown(HoldKind::Command), 0);
        m.handle(Input::HoldUp(HoldKind::Command), 500);
        m.handle(
            Input::CommandResolved {
                needs_confirm: true,
                label: "clean build".to_string(),
                id: 0,
            },
            600,
        );
        assert_eq!(m.handle(Input::Confirm, 700), vec![Effect::RunCommand]);
        assert!(!m.captures(CapturedKey::Escape));
        assert!(!m.captures(CapturedKey::Return));
        assert_eq!(
            m.handle(Input::HoldDown(HoldKind::Dictate), 800),
            vec![Effect::OpenMic]
        );

        // Direct-run path is Idle too.
        let mut m = warm();
        m.handle(Input::HoldDown(HoldKind::Command), 0);
        m.handle(Input::HoldUp(HoldKind::Command), 500);
        assert_eq!(
            m.handle(
                Input::CommandResolved {
                    needs_confirm: false,
                    label: String::new(),
                    id: 0,
                },
                600
            ),
            vec![Effect::RunCommand]
        );
        assert_eq!(
            m.handle(Input::HoldDown(HoldKind::Dictate), 700),
            vec![Effect::OpenMic]
        );
    }

    #[test]
    fn transcript_ready_hides_hud_and_wrong_context_inputs_are_ignored() {
        // Dictate path completion.
        let mut m = warm();
        m.handle(Input::HoldDown(HoldKind::Dictate), 0);
        m.tick(200);
        m.handle(Input::HoldUp(HoldKind::Dictate), 300);
        assert_eq!(
            m.handle(
                Input::TranscriptReady {
                    text: "hello".to_string(),
                    id: 0,
                },
                400
            ),
            vec![Effect::HideHud]
        );
        assert!(!m.captures(CapturedKey::Escape));

        // Stray host events outside their state are ignored.
        let mut m = warm();
        assert!(m.handle(Input::SilenceDetected, 0).is_empty());
        assert!(m.handle(Input::Confirm, 0).is_empty());
        assert!(m
            .handle(
                Input::TranscriptReady {
                    text: "x".to_string(),
                    id: 0,
                },
                0
            )
            .is_empty());
        assert!(m
            .handle(
                Input::CommandResolved {
                    needs_confirm: false,
                    label: String::new(),
                    id: 0,
                },
                0
            )
            .is_empty());
    }

    #[test]
    fn transcript_ready_ignored_in_command_mode() {
        // In Command mode the host must end with CommandResolved or Finished.
        let mut m = warm();
        m.handle(Input::HoldDown(HoldKind::Command), 0);
        m.handle(Input::HoldUp(HoldKind::Command), 500);
        assert!(m
            .handle(
                Input::TranscriptReady {
                    text: "clean build".to_string(),
                    id: 0,
                },
                600
            )
            .is_empty());
        assert_eq!(
            m.handle(Input::Finished { hud: None, id: 0 }, 700),
            vec![Effect::HideHud]
        );
    }
}
