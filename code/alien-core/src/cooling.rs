//! Temperature-driven fan curve — the control law, with no I/O in it.
//!
//! # Why this exists
//!
//! The EC's own automatic curve is not a curve on this machine. Measured on the
//! reference PH315-53 over a 4×20-minute ABBA run: the CPU fan sat at
//! 3529–3942 RPM at 61 °C, at 74 °C, at 93 °C, at 95 °C and through sustained
//! twelve-thread load. It never ramped. The cost is not subtle — under
//! sustained load, fans at maximum beat the EC curve by **61.8 %** sustained
//! throughput (26 721 → 43 232 MIPS, sum of 7-zip compression and decompression
//! ratings).
//!
//! So Alien's two shipped options are both wrong most of the time: the EC curve
//! throttles the machine, and pinning both fans at maximum spends 40 % more RPM
//! than necessary to hold 61 °C at zero load. This module is the third option.
//!
//! # Why a stepped table and not a PID
//!
//! Three properties of this hardware each independently rule out a PID:
//!
//! - **Actuator gain varies about 10:1 across the range.** 30 → 45 % duty moves
//!   the CPU fan 2100 → 4700 RPM (~173 RPM per point); 45 → 90 % moves it
//!   4700 → 5450 (~17 RPM per point). A fixed-gain loop tuned in the middle is
//!   an order of magnitude wrong at both ends.
//! - **Temperatures arrive as integer °C**, so a derivative term differentiates
//!   quantisation noise. The Linux kernel's own `power_allocator` defaults
//!   `k_d = 0` for the same reason.
//! - **The CPU pins at 92 °C under load regardless of fan speed**, so any
//!   setpoint below that produces permanent positive error at 100 % output —
//!   guaranteed integral windup.
//!
//! Of nine fan controllers surveyed at source level, none closes a PID on
//! temperature by default.
//!
//! # Which sensor drives it
//!
//! Both. They answer different questions, and using either alone gets it wrong.
//!
//! `max(cpu, gpu)` is the **ramp trigger**: it moves quickly and separates idle
//! from load. But it cannot tell you whether the fans are winning, because it
//! reads 92 °C under load whether the fans are at 3529 RPM or 5928 RPM.
//!
//! Board temperature is the **saturation detector**. It is the only sensor that
//! separated the two conditions in measurement — 83.4 °C on the EC curve versus
//! 77.7 °C at maximum — because it tracks heat accumulating in the chassis
//! rather than at the die. Once it climbs, throughput has already been lost and
//! recovering it takes minutes, so crossing [`Curve::board_override_c`] forces
//! maximum regardless of what the step table says.
//!
//! # Failing safe means failing *loud*
//!
//! Handing the fans back to the EC is not a safe state here — it is the state
//! that permits 95 °C and 1446 MHz. The kernel agrees: `acer-wmi` maps
//! `pwm_enable = 0` to `ACER_WMID_FAN_MODE_TURBO`, so Acer's own driver treats
//! "nobody is controlling this" as *turbo*, not as automatic. Every failure
//! path in this module therefore escalates toward [`Action::Max`], never toward
//! auto.
//!
//! The one graduation: losing tachometer readings might mean a dead fan, which
//! is unbounded risk, so it goes straight to maximum. Losing a temperature
//! reading only means lost visibility — the cooling system still works — so it
//! holds a high fixed duty instead of shouting. That split is lifted from OCP
//! OpenNetworkLinux, which distinguishes the two faults for exactly this reason.

use std::time::{Duration, Instant};

/// A single row of the curve: cross `up_c` going up, fall below `down_c` to
/// leave, run at `duty` while here.
///
/// The gap between `up_c` and the next row down's `down_c` is the hysteresis
/// band. Keep it at 4 °C or more. nbfc-linux ships a config for this exact
/// machine whose bands are 0–2 °C wide, and one row has no hysteresis at all —
/// take its duty domain, not its thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    /// Rise to at least this temperature to enter this step.
    pub up_c: u16,
    /// Fall strictly below this temperature to leave it.
    pub down_c: u16,
    /// Duty for both fans while this step is active, 0-100.
    pub duty: u8,
}

/// The curve plus its guard rails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Curve {
    /// Ascending by `up_c`. The duty below the first step's `up_c` is
    /// [`Curve::floor_duty`].
    pub steps: Vec<Step>,
    /// Duty below the first step.
    ///
    /// **40 %, not 30 %.** The EC curve idles near 2608 RPM but climbs to about
    /// 3947 under load; 30 % duty is 2100–3300 RPM, so a controller resting at
    /// 30 % that then meets load would move *less* air than doing nothing at
    /// all. 40 % is roughly the EC's own loaded plateau.
    pub floor_duty: u8,
    /// Board temperature that forces maximum regardless of the step table.
    pub board_override_c: u16,
    /// CPU temperature that latches [`Action::Max`].
    pub critical_c: u16,
    /// Release the critical latch below this. Must be well under
    /// `critical_c` — this gap is what stops the latch chattering.
    pub critical_release_c: u16,
    /// Duty held when temperatures cannot be read but fans still report.
    pub blind_duty: u8,
}

impl Default for Curve {
    /// Derived from measurements on the reference PH315-53.
    ///
    /// Only 30/45/90/100 % have measured RPM; the rest are interpolated and
    /// want calibrating per chassis. The bands are 6–7 °C, matching the median
    /// across nbfc-linux's 317 shipped configs rather than the 0–2 °C in its
    /// PH315-53 entry.
    fn default() -> Self {
        Curve {
            steps: vec![
                Step { up_c: 58, down_c: 51, duty: 50 },
                Step { up_c: 66, down_c: 59, duty: 60 },
                Step { up_c: 73, down_c: 66, duty: 70 },
                Step { up_c: 79, down_c: 72, duty: 80 },
                Step { up_c: 84, down_c: 77, duty: 90 },
                Step { up_c: 88, down_c: 82, duty: 100 },
            ],
            floor_duty: 40,
            // Idle board temp measured 68-71 °C and saturated load 83.4 °C, so
            // 80 fires while there is still throughput left to save.
            board_override_c: 80,
            critical_c: 93,
            critical_release_c: 85,
            blind_duty: 75,
        }
    }
}

impl Curve {
    /// Lowest step index whose `up_c` this temperature has reached, if any.
    fn step_for_rising(&self, t: u16) -> Option<usize> {
        self.steps.iter().rposition(|s| t >= s.up_c)
    }

    /// Duty for a step index, or the floor for `None`.
    fn duty_at(&self, idx: Option<usize>) -> u8 {
        idx.map_or(self.floor_duty, |i| self.steps[i].duty)
    }

    /// Reject curves that would chatter or invert.
    pub fn validate(&self) -> Result<(), String> {
        if self.steps.is_empty() {
            return Err("curve has no steps".into());
        }
        if self.floor_duty > 100 || self.blind_duty > 100 {
            return Err("duties must be 0..=100".into());
        }
        if self.critical_release_c >= self.critical_c {
            return Err(format!(
                "critical release {} must be below critical {}",
                self.critical_release_c, self.critical_c
            ));
        }
        let mut prev: Option<&Step> = None;
        for (i, s) in self.steps.iter().enumerate() {
            if s.duty > 100 {
                return Err(format!("step {i} duty {} exceeds 100", s.duty));
            }
            if s.down_c >= s.up_c {
                return Err(format!(
                    "step {i} down {} must be below up {}",
                    s.down_c, s.up_c
                ));
            }
            if let Some(p) = prev {
                if s.up_c <= p.up_c {
                    return Err(format!("step {i} up {} is not ascending", s.up_c));
                }
                if s.duty < p.duty {
                    return Err(format!("step {i} duty {} decreases", s.duty));
                }
                // The band that actually prevents oscillation is between this
                // step's entry and the *previous* step's exit.
                if s.down_c <= p.down_c {
                    return Err(format!("step {i} down {} is not ascending", s.down_c));
                }
            }
            prev = Some(s);
        }
        Ok(())
    }
}

/// What the caller should do to the hardware this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Both fans to this duty, via manual mode.
    Duty(u8),
    /// Both fans to firmware maximum.
    Max,
}

/// Why the controller chose what it chose. Surfaced to the user; never used to
/// make decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// Still proving sensors are sane after start.
    Priming,
    /// Following the step table.
    Curve,
    /// Board temperature crossed `board_override_c`.
    BoardSaturation,
    /// CPU crossed `critical_c` and the latch is engaged.
    Critical,
    /// Fan tachometers stopped reporting — a fan may be dead.
    TachLost,
    /// Temperatures stopped reporting but fans still report.
    SensorsLost,
}

/// One telemetry sample, already filtered for "did this sensor report at all".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reading {
    pub cpu_c: Option<u16>,
    pub gpu_c: Option<u16>,
    pub board_c: Option<u16>,
    pub cpu_rpm: Option<u16>,
    pub gpu_rpm: Option<u16>,
}

impl Reading {
    fn hottest_die(&self) -> Option<u16> {
        match (self.cpu_c, self.gpu_c) {
            (Some(c), Some(g)) => Some(c.max(g)),
            (Some(c), None) => Some(c),
            (None, Some(g)) => Some(g),
            (None, None) => None,
        }
    }

    fn any_tach(&self) -> bool {
        self.cpu_rpm.is_some() || self.gpu_rpm.is_some()
    }
}

/// Samples averaged before the die temperature is allowed to move the curve.
///
/// # Why the curve cannot read the raw sensor
///
/// Measured on the reference machine at genuine idle, with nothing running: the
/// CPU reports 65 °C, then 88 °C, then 71 °C, then 93 °C, within a few seconds.
/// Those are single-sample turbo excursions — the chip boosts to 4.4-4.6 GHz,
/// touches the thermal ceiling and backs off, all inside one 2-second sampling
/// window. The package throttle counter confirms them, advancing only on the
/// spikes and going flat in between.
///
/// A curve reading that sensor directly sits at 100 % duty on an idle machine,
/// which is precisely the behaviour this module exists to remove. Averaging
/// across five samples (ten seconds) reduces an isolated 88 °C spike over a
/// 65 °C baseline to about 70 °C, while a genuine sustained load — which is what
/// actually accumulates heat — pulls the whole window up within one fan settle
/// time.
///
/// **Only the curve input is smoothed.** The critical latch and the board
/// override both read the instantaneous sample, because a safety response must
/// never wait on an average.
pub const SMOOTHING_SAMPLES: usize = 5;

/// How long the controller must see good readings before it stops running at
/// maximum and starts following the curve.
///
/// Borrowed from `phosphor-pid-control`, which marks every sensor missing
/// *before* any reading exists so a zone is in failsafe until proven otherwise.
/// This closes the window where a controller restarting after a crash would run
/// the curve on data it has not yet validated.
pub const PRIME_SAMPLES: u8 = 3;

/// Consecutive bad samples tolerated before escalating.
pub const FAULT_TOLERANCE: u8 = 3;

/// Minimum time at a step before stepping up. Short, because heat is the risk.
pub const MIN_DWELL_UP: Duration = Duration::from_secs(10);

/// Minimum time before stepping down. Long, because coming down early is what
/// causes hunting, and quiet is worth less than stability.
pub const MIN_DWELL_DOWN: Duration = Duration::from_secs(30);

/// The controller. Pure: feed it readings and a clock, it yields actions.
///
/// Time is a parameter rather than read from the clock inside, following
/// `admit_after_interval` in the daemon — it is what makes dwell and hysteresis
/// testable over a synthetic series without sleeping.
#[derive(Debug, Clone)]
pub struct Controller {
    curve: Curve,
    /// `None` until priming completes.
    step: Option<Option<usize>>,
    changed_at: Option<Instant>,
    good_samples: u8,
    bad_temp_samples: u8,
    bad_tach_samples: u8,
    critical_latched: bool,
    last_action: Option<Action>,
    /// Recent die temperatures, most recent last. Only the curve reads this.
    window: Vec<u16>,
}

impl Controller {
    pub fn new(curve: Curve) -> Self {
        Controller {
            curve,
            step: None,
            changed_at: None,
            good_samples: 0,
            bad_temp_samples: 0,
            bad_tach_samples: 0,
            critical_latched: false,
            last_action: None,
            window: Vec::with_capacity(SMOOTHING_SAMPLES),
        }
    }

    /// Rolling mean of the die temperature, rounded to the nearest degree.
    ///
    /// Fills from the first sample rather than waiting for a full window, so a
    /// freshly started controller is not blind — it is simply less damped for
    /// the first few seconds, during which it is still priming at maximum
    /// anyway.
    fn smoothed(&self) -> Option<u16> {
        if self.window.is_empty() {
            return None;
        }
        let sum: u32 = self.window.iter().map(|&t| u32::from(t)).sum();
        let n = self.window.len() as u32;
        Some(((sum + n / 2) / n) as u16)
    }

    pub fn curve(&self) -> &Curve {
        &self.curve
    }

    /// Whether the controller has finished priming and is following the curve.
    pub fn is_primed(&self) -> bool {
        self.step.is_some()
    }

    /// Decide for this sample.
    ///
    /// Returns the action *and* whether it differs from the last one. Callers
    /// should only write to firmware when it changed — in steady state this
    /// controller issues no writes at all, which matters because every write is
    /// an SMM mailbox round trip.
    pub fn step(&mut self, r: Reading, now: Instant) -> Decision {
        let (action, reason) = self.decide(r, now);
        let changed = self.last_action != Some(action);
        self.last_action = Some(action);
        Decision { action, reason, changed }
    }

    fn decide(&mut self, r: Reading, now: Instant) -> (Action, Reason) {
        // Tachometer loss first: it may mean a dead fan, which is the only
        // fault here with unbounded consequences.
        if !r.any_tach() {
            self.bad_tach_samples = self.bad_tach_samples.saturating_add(1);
            if self.bad_tach_samples >= FAULT_TOLERANCE {
                return (Action::Max, Reason::TachLost);
            }
        } else {
            self.bad_tach_samples = 0;
        }

        let Some(die) = r.hottest_die() else {
            // Temperatures gone but fans still turning: we have lost sight, not
            // cooling. Hold a high duty rather than escalating to maximum.
            self.bad_temp_samples = self.bad_temp_samples.saturating_add(1);
            if self.bad_temp_samples >= FAULT_TOLERANCE {
                return (Action::Duty(self.curve.blind_duty), Reason::SensorsLost);
            }
            return (
                self.last_action
                    .unwrap_or(Action::Duty(self.curve.blind_duty)),
                Reason::SensorsLost,
            );
        };
        self.bad_temp_samples = 0;

        if self.window.len() == SMOOTHING_SAMPLES {
            self.window.remove(0);
        }
        self.window.push(die);

        // Start at maximum and earn the right to come down.
        if self.step.is_none() {
            self.good_samples = self.good_samples.saturating_add(1);
            if self.good_samples < PRIME_SAMPLES {
                return (Action::Max, Reason::Priming);
            }
            self.step = Some(self.curve.step_for_rising(self.smoothed().unwrap_or(die)));
            self.changed_at = Some(now);
        }

        // Critical latch. Engages on CPU alone — it is the die that pins.
        let cpu = r.cpu_c.unwrap_or(die);
        if self.critical_latched {
            if cpu < self.curve.critical_release_c {
                self.critical_latched = false;
            } else {
                return (Action::Max, Reason::Critical);
            }
        } else if cpu >= self.curve.critical_c {
            self.critical_latched = true;
            return (Action::Max, Reason::Critical);
        }

        // Chassis saturation. Bypasses dwell: by the time board temperature is
        // up, throughput is already gone.
        if r.board_c.is_some_and(|b| b >= self.curve.board_override_c) {
            return (Action::Max, Reason::BoardSaturation);
        }

        let current = self.step.expect("primed above");
        // Smoothed, unlike the two guards above: an isolated turbo excursion
        // must not move the fans, but it must still be able to trip safety.
        let curve_input = self.smoothed().unwrap_or(die);
        let target = self.target_step(current, curve_input);
        if target != current && self.dwell_elapsed(current, target, now) {
            self.step = Some(target);
            self.changed_at = Some(now);
            return (Action::Duty(self.curve.duty_at(target)), Reason::Curve);
        }
        (Action::Duty(self.curve.duty_at(current)), Reason::Curve)
    }

    /// Where the table says we should be, honouring both thresholds.
    ///
    /// Rising uses `>= up_c`; falling uses strict `< down_c`. The asymmetry is
    /// deliberate — the kernel shipped a fan-oscillation fix that was literally
    /// one character, `<=` to `<`, because at zero hysteresis both conditions
    /// were simultaneously satisfiable.
    fn target_step(&self, current: Option<usize>, t: u16) -> Option<usize> {
        let rising = self.curve.step_for_rising(t);
        match (current, rising) {
            // Above where we are: follow immediately.
            (Some(c), Some(r)) if r > c => Some(r),
            (None, Some(r)) => Some(r),
            // At or below: only leave once past this step's own exit.
            (Some(c), _) => {
                if t < self.curve.steps[c].down_c {
                    // Drop one step at a time. Falling straight to the floor
                    // after a load ends is how a controller ends up ramping
                    // right back up.
                    if c == 0 {
                        None
                    } else {
                        Some(c - 1)
                    }
                } else {
                    Some(c)
                }
            }
            (None, None) => None,
        }
    }

    fn dwell_elapsed(&self, from: Option<usize>, to: Option<usize>, now: Instant) -> bool {
        let up = match (from, to) {
            (Some(a), Some(b)) => b > a,
            (None, Some(_)) => true,
            _ => false,
        };
        let need = if up { MIN_DWELL_UP } else { MIN_DWELL_DOWN };
        self.changed_at
            .is_none_or(|t| now.saturating_duration_since(t) >= need)
    }
}

/// The outcome of one controller tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decision {
    pub action: Action,
    pub reason: Reason,
    /// Whether this differs from the previous tick's action. Only write to
    /// firmware when true.
    pub changed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(base: Instant, secs: u64) -> Instant {
        base + Duration::from_secs(secs)
    }

    fn ok(cpu: u16, gpu: u16, board: u16) -> Reading {
        Reading {
            cpu_c: Some(cpu),
            gpu_c: Some(gpu),
            board_c: Some(board),
            cpu_rpm: Some(3500),
            gpu_rpm: Some(4100),
        }
    }

    /// Drive past priming so a test can start from the curve.
    fn primed(c: &mut Controller, r: Reading, base: Instant) {
        for i in 0..PRIME_SAMPLES {
            c.step(r, at(base, i as u64));
        }
    }

    /// Hold a reading until the controller stops moving, then return the final
    /// decision.
    ///
    /// Two things have to be waited out, and both are deliberate: the smoothing
    /// window (the curve reads a rolling mean, so one sample never moves it)
    /// and the per-transition dwell timers (the controller climbs and descends
    /// one step at a time). Samples are spaced wider than MIN_DWELL_DOWN so a
    /// multi-step move is not mistaken for a stuck controller.
    fn hold(c: &mut Controller, r: Reading, base: Instant, from_sec: u64) -> Decision {
        const SPACING: u64 = 35;
        let mut last = c.step(r, at(base, from_sec));
        for i in 1..12 {
            last = c.step(r, at(base, from_sec + SPACING * i));
        }
        last
    }

    /// Fill the smoothing window without letting the clock advance.
    ///
    /// `now` is a parameter, so a test can feed several samples at one instant.
    /// That separates the two things a decision waits on — the rolling mean and
    /// the dwell timers — so a test can assert on one without the other quietly
    /// satisfying itself in the background.
    fn fill_window(c: &mut Controller, r: Reading, when: Instant) {
        for _ in 0..SMOOTHING_SAMPLES {
            c.step(r, when);
        }
    }

    #[test]
    fn default_curve_is_valid() {
        assert!(Curve::default().validate().is_ok());
    }

    #[test]
    fn validate_rejects_inverted_hysteresis() {
        let mut c = Curve::default();
        c.steps[0].down_c = c.steps[0].up_c;
        assert!(c.validate().unwrap_err().contains("must be below up"));
    }

    #[test]
    fn validate_rejects_decreasing_duty() {
        let mut c = Curve::default();
        c.steps[2].duty = 10;
        assert!(c.validate().unwrap_err().contains("decreases"));
    }

    #[test]
    fn validate_rejects_release_at_or_above_critical() {
        let mut c = Curve::default();
        c.critical_release_c = c.critical_c;
        assert!(c.validate().unwrap_err().contains("must be below critical"));
    }

    #[test]
    fn starts_at_max_and_only_then_follows_the_curve() {
        let base = Instant::now();
        let mut c = Controller::new(Curve::default());
        // Cold machine: the curve would say floor, but priming says max.
        for i in 0..(PRIME_SAMPLES - 1) {
            let d = c.step(ok(40, 40, 60), at(base, i as u64));
            assert_eq!(d.action, Action::Max, "sample {i} must still be priming");
            assert_eq!(d.reason, Reason::Priming);
        }
        assert!(!c.is_primed());
        let d = c.step(ok(40, 40, 60), at(base, PRIME_SAMPLES as u64));
        assert!(c.is_primed());
        assert_eq!(d.action, Action::Duty(40));
        assert_eq!(d.reason, Reason::Curve);
    }

    #[test]
    fn rises_through_steps_and_reports_change_only_once() {
        let base = Instant::now();
        let mut c = Controller::new(Curve::default());
        primed(&mut c, ok(40, 40, 60), base);

        let d = hold(&mut c, ok(67, 50, 60), base, 20);
        assert_eq!(d.action, Action::Duty(60));

        // Same conditions again: no new write.
        let d = c.step(ok(67, 50, 60), at(base, 60));
        assert_eq!(d.action, Action::Duty(60));
        assert!(!d.changed, "steady state must not re-write firmware");
    }

    #[test]
    fn hysteresis_holds_the_step_between_up_and_down() {
        let base = Instant::now();
        let mut c = Controller::new(Curve::default());
        primed(&mut c, ok(40, 40, 60), base);
        hold(&mut c, ok(67, 40, 60), base, 20); // -> step 1, duty 60

        // 60 °C is below this step's up_c (66) but not below its down_c (59):
        // it must stay put rather than oscillate.
        let d = hold(&mut c, ok(60, 40, 60), base, 120);
        assert_eq!(d.action, Action::Duty(60), "inside the band, must not drop");
    }

    #[test]
    fn falls_one_step_at_a_time_below_down_threshold() {
        let base = Instant::now();
        let mut c = Controller::new(Curve::default());
        primed(&mut c, ok(40, 40, 60), base);
        assert_eq!(hold(&mut c, ok(90, 40, 60), base, 20).action, Action::Duty(100));

        // Cold suddenly. Descent must be one step per dwell — collapsing
        // straight to the floor is how a controller ends up ramping right back
        // up. Stepping the clock by exactly one dwell at a time proves the rate
        // rather than just the destination.
        let t = 500;
        fill_window(&mut c, ok(40, 40, 60), at(base, t));
        for (i, expected) in [90u8, 80, 70, 60, 50].into_iter().enumerate() {
            let d = c.step(ok(40, 40, 60), at(base, t + 31 * i as u64));
            assert_eq!(
                d.action,
                Action::Duty(expected),
                "step {i}: must descend exactly one step per dwell"
            );
        }
    }

    #[test]
    fn down_dwell_is_longer_than_up_dwell() {
        let base = Instant::now();
        let mut c = Controller::new(Curve::default());
        primed(&mut c, ok(40, 40, 60), base);
        assert_eq!(hold(&mut c, ok(90, 40, 60), base, 20).action, Action::Duty(100));

        // Refill the window at a single instant so the first descent lands at
        // exactly `t` and the dwell clock provably starts there.
        let t = 500;
        fill_window(&mut c, ok(40, 40, 60), at(base, t));
        assert_eq!(c.step(ok(40, 40, 60), at(base, t)).action, Action::Duty(90));

        // 12 s on: past MIN_DWELL_UP (10 s), well short of MIN_DWELL_DOWN
        // (30 s). Coming down early is what causes hunting, so quiet waits.
        assert_eq!(
            c.step(ok(40, 40, 60), at(base, t + 12)).action,
            Action::Duty(90),
            "must not descend again before the longer down dwell"
        );
        assert_eq!(
            c.step(ok(40, 40, 60), at(base, t + 31)).action,
            Action::Duty(80),
            "past MIN_DWELL_DOWN, one more step down"
        );
    }

    #[test]
    fn rising_is_not_blocked_by_the_long_down_dwell() {
        let base = Instant::now();
        let mut c = Controller::new(Curve::default());
        primed(&mut c, ok(40, 40, 60), base);
        hold(&mut c, ok(60, 40, 60), base, 20);
        // Past MIN_DWELL_UP, far short of MIN_DWELL_DOWN.
        let d = hold(&mut c, ok(85, 40, 60), base, 31);
        assert_eq!(d.action, Action::Duty(90), "heat must not wait on the down timer");
    }

    #[test]
    fn board_saturation_forces_max_even_when_die_is_cool() {
        let base = Instant::now();
        let mut c = Controller::new(Curve::default());
        primed(&mut c, ok(40, 40, 60), base);
        let d = c.step(ok(50, 50, 81), at(base, 20));
        assert_eq!(d.action, Action::Max);
        assert_eq!(d.reason, Reason::BoardSaturation);
    }

    #[test]
    fn critical_latches_and_releases_with_hysteresis() {
        let base = Instant::now();
        let mut c = Controller::new(Curve::default());
        primed(&mut c, ok(40, 40, 60), base);

        let d = c.step(ok(93, 40, 60), at(base, 20));
        assert_eq!(d.action, Action::Max);
        assert_eq!(d.reason, Reason::Critical);

        // 90 is below critical but above release: latch must hold.
        let d = c.step(ok(90, 40, 60), at(base, 40));
        assert_eq!(d.action, Action::Max, "latch must not release early");
        assert_eq!(d.reason, Reason::Critical);

        let d = c.step(ok(84, 40, 60), at(base, 60));
        assert_ne!(d.reason, Reason::Critical, "below release, latch must clear");
    }

    #[test]
    fn tach_loss_escalates_to_max_after_tolerance() {
        let base = Instant::now();
        let mut c = Controller::new(Curve::default());
        primed(&mut c, ok(40, 40, 60), base);
        let dead = Reading { cpu_rpm: None, gpu_rpm: None, ..ok(50, 50, 60) };
        for i in 0..(FAULT_TOLERANCE - 1) {
            let d = c.step(dead, at(base, 20 + i as u64));
            assert_ne!(d.reason, Reason::TachLost, "must tolerate a transient");
        }
        let d = c.step(dead, at(base, 60));
        assert_eq!(d.action, Action::Max);
        assert_eq!(d.reason, Reason::TachLost);
    }

    #[test]
    fn temperature_loss_holds_blind_duty_rather_than_max() {
        let base = Instant::now();
        let mut c = Controller::new(Curve::default());
        primed(&mut c, ok(40, 40, 60), base);
        let blind = Reading {
            cpu_c: None,
            gpu_c: None,
            board_c: None,
            cpu_rpm: Some(3500),
            gpu_rpm: Some(4100),
        };
        for i in 0..FAULT_TOLERANCE {
            c.step(blind, at(base, 20 + i as u64));
        }
        let d = c.step(blind, at(base, 60));
        assert_eq!(
            d.action,
            Action::Duty(Curve::default().blind_duty),
            "lost sight is not lost cooling - do not shout"
        );
        assert_eq!(d.reason, Reason::SensorsLost);
    }

    #[test]
    fn tach_loss_outranks_temperature_loss() {
        let base = Instant::now();
        let mut c = Controller::new(Curve::default());
        primed(&mut c, ok(40, 40, 60), base);
        let both_gone = Reading::default();
        for i in 0..FAULT_TOLERANCE {
            c.step(both_gone, at(base, 20 + i as u64));
        }
        let d = c.step(both_gone, at(base, 60));
        assert_eq!(d.action, Action::Max, "a possibly-dead fan wins");
        assert_eq!(d.reason, Reason::TachLost);
    }

    #[test]
    fn transient_sensor_dropout_does_not_change_action() {
        let base = Instant::now();
        let mut c = Controller::new(Curve::default());
        primed(&mut c, ok(40, 40, 60), base);
        let steady = hold(&mut c, ok(67, 40, 60), base, 20).action;
        let blip = Reading { cpu_c: None, gpu_c: None, ..ok(67, 40, 60) };
        let d = c.step(blip, at(base, 40));
        assert_eq!(d.action, steady, "one bad sample must not move the fans");
        assert!(!d.changed);
    }

    #[test]
    fn gpu_can_drive_the_curve_when_hotter_than_cpu() {
        let base = Instant::now();
        let mut c = Controller::new(Curve::default());
        primed(&mut c, ok(40, 40, 60), base);
        let d = hold(&mut c, ok(45, 85, 60), base, 20);
        assert_eq!(d.action, Action::Duty(90), "hottest die drives the ramp");
    }

    #[test]
    fn floor_is_forty_percent_not_thirty() {
        // Regression guard: 30 % duty is 2100-3300 RPM, below the EC's own
        // loaded plateau of ~3947, so a 30 % floor would be worse than doing
        // nothing once load arrives.
        assert_eq!(Curve::default().floor_duty, 40);
    }
}
