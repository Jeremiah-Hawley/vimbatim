/*
 * Speech timer: a countdown/stopwatch popup that opens over the middle of the
 * formatting ribbon, plus the WPM readout that turns a selection and a
 * duration into "how fast would I have to read this".
 *
 * `TimerState` (held by `AppState`) is plain data and arithmetic with no GPUI
 * in it, so the parsing, formatting and elapsed maths below are unit-tested
 * directly. The `Timer` view at the bottom is the thin rendering shell.
 */

use gpui::prelude::*;
use gpui::*;
use std::time::{Duration, Instant};

use crate::state::AppState;
use crate::theme::{palette, radius, space, Palette};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimerMode {
    /// Counts down from the duration typed into the box.
    Countdown,
    /// Counts up from zero.
    Stopwatch,
}

/// The timer's whole model: how much time has been banked from previous run
/// segments, and when the current one (if any) started.
///
/// Storing `running_since: Option<Instant>` rather than decrementing a counter
/// on a tick means the displayed time is always derived from the real clock —
/// a dropped or late repaint can't make the timer drift, which is the failure
/// mode that matters when someone is timing a speech with it.
#[derive(Clone, Debug)]
pub struct TimerState {
    pub visible: bool,
    pub mode: TimerMode,
    /// Text in the countdown's duration box, parsed by `parse_duration`.
    pub input: String,
    /// Time banked from run segments that have already ended.
    accumulated: Duration,
    /// Start of the current run segment; `None` when stopped or paused.
    running_since: Option<Instant>,
    /// Elapsed time at each Lap press, oldest first.
    pub laps: Vec<Duration>,
}

impl Default for TimerState {
    fn default() -> Self {
        TimerState {
            visible: false,
            mode: TimerMode::Countdown,
            // A plausible speech length, so Start works without typing first.
            input: "5:00".to_string(),
            accumulated: Duration::ZERO,
            running_since: None,
            laps: Vec::new(),
        }
    }
}

impl TimerState {
    pub fn is_running(&self) -> bool {
        self.running_since.is_some()
    }

    /// No-op when already running, so a double Start can't bank time twice or
    /// (worse) leave a second tick task running against the same state.
    pub fn start(&mut self) {
        self.start_at(Instant::now());
    }

    pub fn stop(&mut self) {
        self.stop_at(Instant::now());
    }

    /// Banks the lap's *elapsed* time, not the displayed one: in countdown mode
    /// the display runs backwards, and "lap at 4:12 remaining" is far less
    /// useful than "this lap took 48s".
    pub fn lap(&mut self) {
        self.laps.push(self.elapsed());
    }

    /// Clears the clock and the laps but keeps the mode and the typed duration
    /// — Reset means "run it again", not "start configuring from scratch".
    pub fn reset(&mut self) {
        self.accumulated = Duration::ZERO;
        self.running_since = None;
        self.laps.clear();
    }

    pub fn set_mode(&mut self, mode: TimerMode) {
        if self.mode != mode {
            self.mode = mode;
            // A stopwatch's elapsed time means nothing as a countdown's, and
            // vice versa; carrying it over just produces a confusing number.
            self.reset();
        }
    }

    /// The countdown's configured length, or `None` when the box holds
    /// something unparseable. Meaningless in stopwatch mode.
    pub fn target(&self) -> Option<Duration> {
        parse_duration(&self.input)
    }

    pub fn elapsed(&self) -> Duration {
        self.elapsed_at(Instant::now())
    }

    /// What the big readout shows: time remaining when counting down (floored
    /// at zero, so an overrun sits at 0:00 rather than going negative), time
    /// elapsed when counting up.
    pub fn displayed(&self) -> Duration {
        self.displayed_at(Instant::now())
    }

    // ── `_at` variants: the same logic with the clock injected, so tests can
    //    drive it without sleeping ────────────────────────────────────────────

    pub fn start_at(&mut self, now: Instant) {
        if self.running_since.is_none() {
            self.running_since = Some(now);
        }
    }

    pub fn stop_at(&mut self, now: Instant) {
        if let Some(since) = self.running_since.take() {
            self.accumulated += now.saturating_duration_since(since);
        }
    }

    pub fn elapsed_at(&self, now: Instant) -> Duration {
        match self.running_since {
            Some(since) => self.accumulated + now.saturating_duration_since(since),
            None => self.accumulated,
        }
    }

    pub fn displayed_at(&self, now: Instant) -> Duration {
        let elapsed = self.elapsed_at(now);
        match self.mode {
            TimerMode::Stopwatch => elapsed,
            TimerMode::Countdown => self
                .target()
                .unwrap_or(Duration::ZERO)
                .saturating_sub(elapsed),
        }
    }
}

/// Parses the duration box: `"90"` (seconds), `"1:30"` (m:ss), `"1:02:03"`
/// (h:mm:ss). Returns `None` for anything else — empty, non-numeric, or more
/// than three parts — so the caller can leave the timer unstartable rather
/// than guess at a length.
///
/// Deliberately does *not* require the minutes/seconds parts to be < 60:
/// `"0:90"` is a perfectly clear way to type ninety seconds.
pub fn parse_duration(text: &str) -> Option<Duration> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    let mut secs: u64 = 0;
    for part in &parts {
        secs = secs.checked_mul(60)?.checked_add(part.trim().parse::<u64>().ok()?)?;
    }
    Some(Duration::from_secs(secs))
}

/// `m:ss`, widening to `h:mm:ss` only once there's an hour to show — a speech
/// timer spends all its time in the two-field form.
pub fn format_duration(d: Duration) -> String {
    let total = d.as_secs();
    let (hours, minutes, seconds) = (total / 3600, (total % 3600) / 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// Words per minute needed to read `words` in `over`. `None` when the duration
/// is zero — the answer is "infinity", which is not a number to show someone.
pub fn words_per_minute(words: usize, over: Duration) -> Option<u32> {
    let secs = over.as_secs_f64();
    if secs <= 0.0 {
        return None;
    }
    Some((words as f64 * 60.0 / secs).round() as u32)
}

/// The timer popup. Rendered by `MainWindow` inside the ribbon's own stacking
/// context (and `deferred`, so it paints over the editor below), which is what
/// puts it in the middle of the ribbon.
pub struct Timer {
    state: Entity<AppState>,
    /// Focus for the countdown duration box — same typable-box pattern as the
    /// ribbon's font-size control.
    input_focus: FocusHandle,
    /// True while a repaint loop is running, so a second Start can't spawn a
    /// second one.
    ticking: bool,
}

impl Timer {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        Timer { state, input_focus: cx.focus_handle(), ticking: false }
    }

    /// Repaints while the clock runs. GPUI has no "render every frame" mode:
    /// nothing about the app changes when a second passes, so the view has to
    /// wake itself. Exits as soon as the timer stops, so a paused or closed
    /// timer costs nothing.
    fn start_ticking(&mut self, cx: &mut Context<Self>) {
        if self.ticking {
            return;
        }
        self.ticking = true;
        cx.spawn(async move |this, cx| {
            loop {
                // Fast enough that the seconds digit never looks stuck,
                // slow enough to be invisible on a battery.
                cx.background_executor().timer(Duration::from_millis(200)).await;
                let keep_going = this.update(cx, |this: &mut Timer, cx| {
                    let running = this.state.read(cx).timer.is_running();
                    if !running {
                        this.ticking = false;
                    }
                    cx.notify();
                    running
                });
                // `Err` means the view is gone (window closed) — stop rather
                // than spin forever against a dead entity.
                match keep_going {
                    Ok(true) => continue,
                    _ => return,
                }
            }
        })
        .detach();
    }

    fn handle_input_key(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.clone();
        self.state.update(cx, |state, _cx| {
            let input = &mut state.timer.input;
            match key.as_str() {
                "backspace" => {
                    input.pop();
                }
                "escape" | "enter" => {}
                // Digits and the separator only — anything else would just
                // make `parse_duration` fail.
                k if k.len() == 1
                    && input.len() < 8
                    && (k.chars().all(|c| c.is_ascii_digit()) || k == ":") =>
                {
                    input.push_str(k);
                }
                _ => {}
            }
        });
        cx.notify();
    }

    fn button(
        &self,
        id: &'static str,
        label: &'static str,
        enabled: bool,
        p: Palette,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut TimerState) + 'static,
    ) -> AnyElement {
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .h(px(26.0))
            .px(px(12.0))
            .rounded(px(radius::MD))
            .text_xs()
            .border_1()
            .border_color(rgb(p.border_subtle))
            .bg(rgb(p.chrome_elevated))
            .text_color(rgb(if enabled { p.text } else { p.text_faint }))
            .when(enabled, |d| {
                d.cursor_pointer()
                    .hover(move |s| s.bg(rgb(p.chrome_hover)))
                    .active(move |s| s.bg(rgb(p.chrome_active)))
                    .on_click(cx.listener(move |this, _ev, _window, cx| {
                        this.state.update(cx, |state, cx| {
                            on_click(&mut state.timer);
                            cx.notify();
                        });
                        // Cheap to call when nothing started: it returns
                        // immediately unless the clock is actually running.
                        if this.state.read(cx).timer.is_running() {
                            this.start_ticking(cx);
                        }
                        cx.notify();
                    }))
            })
            .child(label)
            .into_any_element()
    }

    fn mode_tab(&self, id: &'static str, label: &'static str, mode: TimerMode, current: TimerMode, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let selected = mode == current;
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .h(px(22.0))
            .px(px(10.0))
            .rounded(px(radius::MD))
            .text_xs()
            .cursor_pointer()
            .bg(rgb(if selected { p.accent } else { p.chrome_elevated }))
            .text_color(rgb(if selected { 0xffffff } else { p.text_muted }))
            .hover(move |s| s.text_color(rgb(if selected { 0xffffff } else { p.text })))
            .on_click(cx.listener(move |this, _ev, _window, cx| {
                this.state.update(cx, |state, cx| {
                    state.timer.set_mode(mode);
                    cx.notify();
                });
                cx.notify();
            }))
            .child(label)
            .into_any_element()
    }

    /// The WPM line. Only shown while the clock is stopped: the whole point is
    /// to answer "can I read this in that time", which is a question about a
    /// duration sitting still.
    fn wpm_row(&self, p: Palette, cx: &Context<Self>) -> AnyElement {
        let state = self.state.read(cx);
        let words = state.spoken_words_in_selection();
        let over = state.timer.displayed();

        let (label, muted) = match words {
            // Deliberately the user's own wording — the hint has to say what to
            // do, and "select text read to calculate wpm" says it.
            None => ("select text read to calculate wpm".to_string(), true),
            Some(0) => ("selection has no text read aloud".to_string(), true),
            Some(words) => match words_per_minute(words, over) {
                Some(wpm) => (format!("{wpm} wpm to read {words} words in {}", format_duration(over)), false),
                None => ("set a time to calculate wpm".to_string(), true),
            },
        };

        div()
            .text_xs()
            .text_color(rgb(if muted { p.text_faint } else { p.text }))
            .child(label)
            .into_any_element()
    }
}

impl Render for Timer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let p = palette(state.theme, state.theme_mode);
        let mode = state.timer.mode;
        let running = state.timer.is_running();
        let display = format_duration(state.timer.displayed());
        let input = state.timer.input.clone();
        let unparseable = mode == TimerMode::Countdown && state.timer.target().is_none();
        // Newest first: the lap you just took is the one you're reading.
        let laps: Vec<(usize, String)> = state
            .timer
            .laps
            .iter()
            .enumerate()
            .rev()
            .take(4)
            .map(|(i, d)| (i + 1, format_duration(*d)))
            .collect();

        div()
            .w(px(280.0))
            .bg(rgb(p.chrome))
            .border_1()
            .border_color(rgb(p.border))
            .rounded(px(8.0))
            .shadow_lg()
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .p(px(12.0))
            // Without this every click inside the panel also reaches
            // `MainWindow`'s window-spanning handlers (which blur focus and
            // close menus) — the same trap documented in `settings_modal.rs`.
            .on_mouse_down(MouseButton::Left, |_ev, _window, cx| cx.stop_propagation())
            // ── Title row ────────────────────────────────────────────────
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(space::XXS))
                            .child(self.mode_tab("timer-mode-countdown", "Timer", TimerMode::Countdown, mode, p, cx))
                            .child(self.mode_tab("timer-mode-stopwatch", "Stopwatch", TimerMode::Stopwatch, mode, p, cx)),
                    )
                    .child(
                        div()
                            .id("timer-close")
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(20.0))
                            .h(px(20.0))
                            .rounded(px(radius::MD))
                            .cursor_pointer()
                            .text_color(rgb(p.text_muted))
                            .hover(move |s| s.bg(rgb(p.chrome_hover)).text_color(rgb(p.text)))
                            .on_click(cx.listener(|this, _ev, _window, cx| {
                                this.state.update(cx, |s, cx| {
                                    s.timer.visible = false;
                                    cx.notify();
                                });
                            }))
                            .child("×"),
                    ),
            )
            // ── The clock ────────────────────────────────────────────────
            .child(
                div()
                    .flex()
                    .justify_center()
                    .py(px(2.0))
                    .text_size(px(34.0))
                    .font_family(crate::text_editor::FONT_FAMILY)
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(if running { p.text } else { p.text_muted }))
                    .child(display),
            )
            // ── Countdown length ─────────────────────────────────────────
            .when(mode == TimerMode::Countdown, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_center()
                        .gap(px(space::XS))
                        .child(div().text_xs().text_color(rgb(p.text_muted)).child("Count down from"))
                        .child(
                            div()
                                .id("timer-input")
                                .track_focus(&self.input_focus)
                                .on_key_down(cx.listener(Self::handle_input_key))
                                .flex()
                                .items_center()
                                .justify_center()
                                .w(px(64.0))
                                .h(px(24.0))
                                .rounded(px(radius::MD))
                                .bg(rgb(p.chrome_elevated))
                                .text_sm()
                                .font_family(crate::text_editor::FONT_FAMILY)
                                .text_color(rgb(if unparseable { p.accent_alt } else { p.text }))
                                .cursor_pointer()
                                .border_1()
                                .border_color(rgb(if unparseable { p.accent_alt } else { p.border_subtle }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _ev, window, cx| {
                                        cx.stop_propagation();
                                        this.input_focus.clone().focus(window, cx);
                                        cx.notify();
                                    }),
                                )
                                .child(input),
                        ),
                )
            })
            // ── Transport ────────────────────────────────────────────────
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_center()
                    .gap(px(space::XXS))
                    .child(self.button("timer-start", "Start", !running, p, cx, |t| t.start()))
                    .child(self.button("timer-stop", "Stop", running, p, cx, |t| t.stop()))
                    .child(self.button("timer-lap", "Lap", running, p, cx, |t| t.lap()))
                    .child(self.button("timer-reset", "Reset", true, p, cx, |t| t.reset())),
            )
            // ── Laps ─────────────────────────────────────────────────────
            .when(!laps.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(1.0))
                        .children(laps.into_iter().map(|(n, time)| {
                            div()
                                .flex()
                                .flex_row()
                                .justify_between()
                                .text_xs()
                                .text_color(rgb(p.text_muted))
                                .child(format!("Lap {n}"))
                                .child(time)
                        })),
                )
            })
            // ── WPM ──────────────────────────────────────────────────────
            .when(!running, |d| {
                d.child(div().h(px(1.0)).bg(rgb(p.border_subtle)))
                    .child(self.wpm_row(p, cx))
            })
    }
}

#[cfg(test)]
mod tests {
    // Named imports, not `use super::*` — that would re-glob `gpui::*`, whose
    // own `test` attribute macro then shadows the standard one and expands
    // into itself ("recursion limit reached while expanding `#[test]`").
    use super::{format_duration, parse_duration, words_per_minute, TimerMode, TimerState};
    use std::time::{Duration, Instant};

    #[test]
    fn parses_every_accepted_duration_shape() {
        assert_eq!(parse_duration("90"), Some(Duration::from_secs(90)));
        assert_eq!(parse_duration("1:30"), Some(Duration::from_secs(90)));
        assert_eq!(parse_duration("1:02:03"), Some(Duration::from_secs(3723)));
        assert_eq!(parse_duration(" 5:00 "), Some(Duration::from_secs(300)));
        // Over-60 parts are legal: "0:90" is an unambiguous ninety seconds.
        assert_eq!(parse_duration("0:90"), Some(Duration::from_secs(90)));
    }

    #[test]
    fn rejects_input_it_cannot_turn_into_a_time() {
        for bad in ["", "  ", "abc", "1:2:3:4", "1:", "-5", "5.5"] {
            assert_eq!(parse_duration(bad), None, "{bad:?} should not parse");
        }
    }

    #[test]
    fn formats_minutes_and_only_widens_for_hours() {
        assert_eq!(format_duration(Duration::from_secs(0)), "0:00");
        assert_eq!(format_duration(Duration::from_secs(9)), "0:09");
        assert_eq!(format_duration(Duration::from_secs(605)), "10:05");
        assert_eq!(format_duration(Duration::from_secs(3600)), "1:00:00");
        assert_eq!(format_duration(Duration::from_secs(3723)), "1:02:03");
    }

    /// Stop/start banks each segment instead of restarting the clock — the
    /// behaviour that makes Stop a pause rather than a reset.
    #[test]
    fn pausing_and_resuming_accumulates_both_segments() {
        let t0 = Instant::now();
        let mut timer = TimerState { mode: TimerMode::Stopwatch, ..TimerState::default() };

        timer.start_at(t0);
        timer.stop_at(t0 + Duration::from_secs(10));
        assert_eq!(timer.elapsed_at(t0 + Duration::from_secs(999)), Duration::from_secs(10));

        timer.start_at(t0 + Duration::from_secs(20));
        assert_eq!(timer.elapsed_at(t0 + Duration::from_secs(25)), Duration::from_secs(15));
    }

    #[test]
    fn a_second_start_does_not_rebase_a_running_clock() {
        let t0 = Instant::now();
        let mut timer = TimerState { mode: TimerMode::Stopwatch, ..TimerState::default() };

        timer.start_at(t0);
        timer.start_at(t0 + Duration::from_secs(5));

        assert_eq!(timer.elapsed_at(t0 + Duration::from_secs(10)), Duration::from_secs(10));
    }

    #[test]
    fn countdown_shows_time_remaining_and_floors_at_zero() {
        let t0 = Instant::now();
        let mut timer = TimerState { input: "1:00".into(), ..TimerState::default() };
        timer.start_at(t0);

        assert_eq!(timer.displayed_at(t0 + Duration::from_secs(20)), Duration::from_secs(40));
        // Overrunning parks at 0:00 rather than going negative (or panicking
        // on a `Duration` subtraction overflow).
        assert_eq!(timer.displayed_at(t0 + Duration::from_secs(90)), Duration::ZERO);
    }

    #[test]
    fn an_unparseable_countdown_length_reads_as_zero_rather_than_panicking() {
        let timer = TimerState { input: "oops".into(), ..TimerState::default() };
        assert_eq!(timer.target(), None);
        assert_eq!(timer.displayed_at(Instant::now()), Duration::ZERO);
    }

    /// Laps record elapsed time, so they read the same in both modes.
    #[test]
    fn laps_record_elapsed_not_the_countdown_display() {
        let t0 = Instant::now();
        let mut timer = TimerState { input: "5:00".into(), ..TimerState::default() };
        timer.start_at(t0);
        timer.stop_at(t0 + Duration::from_secs(48));

        timer.lap();

        assert_eq!(timer.laps, vec![Duration::from_secs(48)]);
    }

    #[test]
    fn switching_mode_clears_a_clock_that_would_be_meaningless_in_the_other() {
        let t0 = Instant::now();
        let mut timer = TimerState { mode: TimerMode::Stopwatch, ..TimerState::default() };
        timer.start_at(t0);
        timer.stop_at(t0 + Duration::from_secs(30));
        timer.lap();

        timer.set_mode(TimerMode::Countdown);

        assert_eq!(timer.elapsed(), Duration::ZERO);
        assert!(timer.laps.is_empty());
        assert!(!timer.is_running());
        // The typed length survives — Reset means "run it again".
        assert_eq!(timer.input, "5:00");
    }

    #[test]
    fn wpm_is_words_scaled_to_a_minute() {
        assert_eq!(words_per_minute(300, Duration::from_secs(60)), Some(300));
        assert_eq!(words_per_minute(300, Duration::from_secs(120)), Some(150));
        assert_eq!(words_per_minute(100, Duration::from_secs(30)), Some(200));
        // No duration means no answer, rather than a division by zero.
        assert_eq!(words_per_minute(300, Duration::ZERO), None);
    }
}
