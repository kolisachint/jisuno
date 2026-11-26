//! Hold-key / utterance state machine.
//!
//! Pure and platform-independent: the macOS event tap feeds key events and
//! timestamps in via [`HoldMachine::handle`], calls [`HoldMachine::tick`]
//! periodically for timeouts, and executes the returned [`Effect`]s.
//!
//! Time is passed in as `u64` milliseconds; there are no clocks here, so the
//! machine is fully deterministic. Rules implemented are
//! `docs/design/keybindings.md` §2 (principles 2, 4), §4, §5 and
//! `docs/design/jisuno.md` §7, §8, §10 round 4 (items 6, 8, 9).

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
    OtherKeyDown,
    /// Hands-free toggle chord (⌃⌥Space).
    HandsFreeToggle,
    /// Esc in a listening/confirming context.
    Cancel,
    /// Return in a confirming context.
    Confirm,
    /// Voice activity judged the line silent (hands-free only).
    SilenceDetected,
    /// The host finished transcribing a dictation utterance.
    TranscriptReady { text: String },
    /// The host transcribed a command utterance and resolved it.
    CommandResolved { needs_confirm: bool, label: String },
    /// The host is done with this utterance without running anything: no
    /// match ("Did you mean …"), empty transcript, or a transcription/model
    /// error. Carries the host's result line to show, if any.
    Finished { hud: Option<HudState> },
    /// The model finished loading and is warm.
    ModelWarm,
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
    Transcribe(Mode),
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
    /// Safety net: if the host never answers a `Transcribe` (crash, lost
    /// reply), `tick` recovers to Idle after this.
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
    /// `since` is when the handoff happened; the transcribe timeout runs
    /// from here so a lost reply cannot wedge the machine.
    Transcribing {
        mode: Mode,
        since: u64,
    },
    /// Command awaiting Return / Esc / timeout.
    Confirming {
        since: u64,
    },
}

/// Pure hold-key state machine. See the module docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoldMachine {
    timing: Timing,
    state: State,
    model_warm: bool,
    /// The current press/session emitted `StartModelLoad` and the load has not
    /// finished since. Only then may a discard path emit `CancelModelLoad`.
    load_started: bool,
    /// Last `secs_left` emitted for the current session, to emit each whole
    /// second exactly once.
    last_countdown: Option<u64>,
}

impl HoldMachine {
    pub fn new(timing: Timing) -> Self {
        Self {
            timing,
            state: State::Idle,
            model_warm: false,
            load_started: false,
            last_countdown: None,
        }
    }

    /// Explicit setter for `model_warm`, alongside the [`Input::ModelWarm`]
    /// event (e.g. the host pre-warmed the model under us).
    pub fn set_model_warm(&mut self, warm: bool) {
        self.model_warm = warm;
        if warm {
            self.load_started = false;
        }
    }

    pub fn is_model_warm(&self) -> bool {
        self.model_warm
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
            Input::TranscriptReady { .. } => self.on_transcript_ready(),
            Input::CommandResolved {
                needs_confirm,
                label,
            } => self.on_command_resolved(needs_confirm, label, now_ms),
            Input::Finished { hud } => self.on_finished(hud),
            Input::ModelWarm => {
                self.set_model_warm(true);
                Vec::new()
            }
        }
    }

    /// Periodic timeout driver: minimum-hold promotion, countdown, cap, and
    /// confirm expiry. Idempotent; call as often as convenient.
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
            State::Transcribing { since, .. } => {
                if elapsed(now_ms, since) >= self.timing.transcribe_timeout_ms {
                    self.state = State::Idle;
                    vec![Effect::HideHud]
                } else {
                    Vec::new()
                }
            }
            State::Idle => Vec::new(),
        }
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
        self.last_countdown = None;
        let mut fx = vec![Effect::OpenMic];
        if !self.model_warm {
            self.load_started = true;
            fx.push(Effect::StartModelLoad);
        }
        fx
    }

    fn on_hold_up(&mut self, kind: HoldKind, now_ms: u64) -> Vec<Effect> {
        let (active, since) = match self.state {
            State::Pending { kind: k, since } | State::Listening { kind: k, since }
                if k == kind =>
            {
                (k, since)
            }
            _ => return Vec::new(),
        };
        let _ = active;
        if elapsed(now_ms, since) < self.timing.min_hold_ms {
            // Tap: discard silently; a load this press started is cancelled.
            // A load that already finished (`ModelWarm` clears the flag) is
            // left alone.
            self.state = State::Idle;
            let mut fx = vec![Effect::CloseMic { discard: true }];
            if self.load_started {
                self.load_started = false;
                fx.push(Effect::CancelModelLoad);
            }
            fx
        } else {
            // Release at or past the minimum: stop and transcribe. A load
            // still in flight is left running for the transcription.
            self.state = State::Transcribing {
                mode: kind.into(),
                since: now_ms,
            };
            self.load_started = false;
            vec![
                Effect::CloseMic { discard: false },
                Effect::Transcribe(kind.into()),
            ]
        }
    }

    fn on_other_key_down(&mut self) -> Vec<Effect> {
        match self.state {
            State::Pending { .. } | State::Listening { .. } => {
                // Hold-alone rule: abandon silently and let the chord reach
                // the app untouched.
                self.state = State::Idle;
                let mut fx = vec![
                    Effect::CloseMic { discard: true },
                    Effect::HideHud,
                    Effect::PassThrough,
                ];
                if self.load_started {
                    self.load_started = false;
                    fx.push(Effect::CancelModelLoad);
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
                self.last_countdown = None;
                let mut fx = vec![Effect::OpenMic];
                if !self.model_warm {
                    self.load_started = true;
                    fx.push(Effect::StartModelLoad);
                }
                fx.push(Effect::ShowHud(HudState::HandsFree));
                fx
            }
            State::HandsFree { .. } => {
                // Toggle again ends the session: stop and transcribe.
                self.state = State::Transcribing {
                    mode: Mode::Dictate,
                    since: now_ms,
                };
                self.load_started = false;
                vec![
                    Effect::CloseMic { discard: false },
                    Effect::Transcribe(Mode::Dictate),
                ]
            }
            // A hold utterance owns the mic; toggles elsewhere are ignored.
            _ => Vec::new(),
        }
    }

    fn on_cancel(&mut self) -> Vec<Effect> {
        match self.state {
            State::Listening { .. } | State::HandsFree { .. } => {
                self.state = State::Idle;
                let mut fx = vec![Effect::CloseMic { discard: true }, Effect::HideHud];
                if self.load_started {
                    self.load_started = false;
                    fx.push(Effect::CancelModelLoad);
                }
                fx
            }
            State::Confirming { .. } => {
                self.state = State::Idle;
                vec![Effect::HideHud]
            }
            // Elsewhere (including Pending) Esc is not captured: ignore.
            _ => Vec::new(),
        }
    }

    fn on_confirm(&mut self) -> Vec<Effect> {
        match self.state {
            State::Confirming { .. } => {
                self.state = State::Idle;
                // The host dismisses the prompt as it runs the command.
                vec![Effect::RunCommand]
            }
            _ => Vec::new(),
        }
    }

    fn on_silence(&mut self, now_ms: u64) -> Vec<Effect> {
        match self.state {
            State::HandsFree { .. } => {
                self.state = State::Transcribing {
                    mode: Mode::Dictate,
                    since: now_ms,
                };
                self.load_started = false;
                vec![
                    Effect::CloseMic { discard: false },
                    Effect::Transcribe(Mode::Dictate),
                ]
            }
            _ => Vec::new(),
        }
    }

    fn on_transcript_ready(&mut self) -> Vec<Effect> {
        match self.state {
            State::Transcribing {
                mode: Mode::Dictate,
                ..
            } => {
                self.state = State::Idle;
                vec![Effect::HideHud]
            }
            _ => Vec::new(),
        }
    }

    fn on_command_resolved(
        &mut self,
        needs_confirm: bool,
        label: String,
        now_ms: u64,
    ) -> Vec<Effect> {
        match self.state {
            State::Transcribing {
                mode: Mode::Command,
                ..
            } => {
                if needs_confirm {
                    self.state = State::Confirming { since: now_ms };
                    vec![Effect::ShowConfirm { label }]
                } else {
                    self.state = State::Idle;
                    vec![Effect::RunCommand]
                }
            }
            _ => Vec::new(),
        }
    }

    fn on_finished(&mut self, hud: Option<HudState>) -> Vec<Effect> {
        match self.state {
            State::Transcribing { .. } => {
                self.state = State::Idle;
                match hud {
                    Some(h) => vec![Effect::ShowHud(h)],
                    None => vec![Effect::HideHud],
                }
            }
            _ => Vec::new(),
        }
    }

    /// Countdown seconds then cap stop for a live session started at `since`.
    fn check_cap(&mut self, now_ms: u64, mode: Mode, since: u64) -> Vec<Effect> {
        let limit_ms = self.timing.dictation_limit_s.saturating_mul(1000);
        let lead_ms = self.timing.countdown_lead_s.saturating_mul(1000);
        let remaining = limit_ms.saturating_sub(elapsed(now_ms, since));
        if remaining == 0 {
            self.state = State::Transcribing {
                mode,
                since: now_ms,
            };
            self.load_started = false;
            return vec![
                Effect::CloseMic { discard: false },
                Effect::Transcribe(mode),
            ];
        }
        if remaining > lead_ms {
            return Vec::new();
        }
        // Whole seconds left, rounded up: at exactly lead the HUD shows
        // `countdown_lead_s`, at 1..1000 ms it shows 1.
        let secs = remaining.div_ceil(1000);
        let prev = self
            .last_countdown
            .unwrap_or(self.timing.countdown_lead_s + 1);
        let mut fx = Vec::new();
        // Emit every whole second crossed since the last tick, newest last, so
        // coarse ticks still produce each second exactly once.
        for s in (secs..prev).rev() {
            if s >= 1 && s <= self.timing.countdown_lead_s {
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
        // Rule 2 vs rule 6: the boundary is exact.
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
                Effect::CloseMic { discard: false },
                Effect::Transcribe(Mode::Dictate),
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
                Effect::Transcribe(Mode::Dictate),
            ]
        );
    }

    #[test]
    fn release_after_hold_transcribes_with_mode() {
        // Rule 6, command flavour.
        let mut m = warm();
        m.handle(Input::HoldDown(HoldKind::Command), 0);
        assert_eq!(
            m.handle(Input::HoldUp(HoldKind::Command), 500),
            vec![
                Effect::CloseMic { discard: false },
                Effect::Transcribe(Mode::Command),
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
                Effect::CloseMic { discard: false },
                Effect::Transcribe(Mode::Dictate),
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
                Effect::Transcribe(Mode::Dictate),
            ]
        );

        let mut m = warm();
        m.handle(Input::HandsFreeToggle, 0);
        assert_eq!(
            m.handle(Input::HandsFreeToggle, 5000),
            vec![
                Effect::CloseMic { discard: false },
                Effect::Transcribe(Mode::Dictate),
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
                Effect::Transcribe(Mode::Dictate),
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
                Effect::Transcribe(Mode::Dictate),
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
                Effect::Transcribe(Mode::Dictate),
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
            m.handle(Input::Finished { hud: None }, 900),
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
            m.handle(Input::Finished { hud: Some(hud) }, 900),
            vec![Effect::ShowHud(hud)]
        );
        // Finished outside Transcribing is ignored.
        assert!(m.handle(Input::Finished { hud: None }, 950).is_empty());
    }

    #[test]
    fn dictate_error_finishes_and_new_hold_works() {
        // Transcription/model error on the dictate path: hide, Idle, live.
        let mut m = warm();
        m.handle(Input::HoldDown(HoldKind::Dictate), 0);
        m.tick(200);
        m.handle(Input::HoldUp(HoldKind::Dictate), 300);
        assert_eq!(
            m.handle(Input::Finished { hud: None }, 1200),
            vec![Effect::HideHud]
        );
        assert_eq!(
            m.handle(Input::HoldDown(HoldKind::Dictate), 1300),
            vec![Effect::OpenMic]
        );
    }

    #[test]
    fn transcribe_timeout_recovers_to_idle() {
        // Safety net: a lost host reply cannot wedge the machine.
        let mut m = warm();
        m.handle(Input::HoldDown(HoldKind::Dictate), 0);
        m.handle(Input::HoldUp(HoldKind::Dictate), 500);
        assert!(m.tick(500 + 9999).is_empty());
        assert_eq!(m.tick(500 + 10_000), vec![Effect::HideHud]);
        assert!(!m.captures(CapturedKey::Escape));
        assert_eq!(
            m.handle(Input::HoldDown(HoldKind::Dictate), 11_000),
            vec![Effect::OpenMic]
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
                },
                0
            )
            .is_empty());
        assert!(m
            .handle(
                Input::CommandResolved {
                    needs_confirm: false,
                    label: String::new(),
                },
                0
            )
            .is_empty());
    }
}
