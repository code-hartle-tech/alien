//! What this particular machine can actually do.
//!
//! Acer's gaming WMI interface is one protocol across many models, and the
//! models differ enormously in what they implement. The firmware answers every
//! call either way — with a status byte saying "supported" or "rejected" — so
//! a tool that does not ask ends up offering controls that do nothing.
//!
//! That is exactly what the vendor's own software does on this hardware: the
//! Overclocking tab is present on a machine where `Feature.ini` disables CPU
//! overclock, and the switch is inert.
//!
//! So Alien probes, but "getter" does not always mean side-effect-free on this
//! firmware. Misc-setting sub-index 5 sends an OEM discrete-GPU notification
//! and sub-index 6 belongs to a persistent setter path; neither is touched by
//! automatic capability discovery (see [`crate::policy`]).

use crate::perkey;
use crate::rgb::Zone;
use crate::transport::Transport;
use crate::wmi::{Function, OverclockTarget, Sensor};

/// Four-state, because protocol acceptance and physical proof are separate,
/// and "we could not tell" is different again from an explicit "no".
/// Presenting an unknown as unsupported hides working features; presenting it
/// as supported is how you get a dead control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    Yes,
    No,
    /// The firmware accepts the call but the effect could not be confirmed.
    Unverified,
    /// The probe could not distinguish support from a transient or malformed
    /// response. This is not the same as a firmware rejection.
    Unknown,
}

impl Support {
    pub fn symbol(self) -> &'static str {
        match self {
            Support::Yes => "yes",
            Support::No => "no",
            Support::Unverified => "unverified",
            Support::Unknown => "unknown",
        }
    }
}

/// What a machine can do, before anyone has asked it.
///
/// Every field is [`Support::Unknown`], which is the literally true answer
/// for an unprobed machine. Deriving `Default` instead would pick `Yes` — the
/// first variant — and claim full support for hardware nothing has touched
/// yet, which is exactly the dead-control failure the three-state enum exists
/// to prevent.
impl Default for Capabilities {
    fn default() -> Self {
        Capabilities {
            cpu_temp: Support::Unknown,
            gpu_temp: Support::Unknown,
            system_temp: Support::Unknown,
            cpu_fan: Support::Unknown,
            gpu_fan: Support::Unknown,
            fan_control: Support::Unknown,
            manual_fan_duty: Support::Unknown,
            backlight_effects: Support::Unknown,
            per_zone_static: Support::Unknown,
            per_key: Support::Unknown,
            cpu_overclock: Support::Unknown,
            gpu_overclock: Support::Unknown,
            coolboost: Support::Unknown,
            keyboard_timeout: Support::Unknown,
            lcd_overdrive: Support::Unknown,
            misc_subindices: Vec::new(),
            notes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Capabilities {
    pub cpu_temp: Support,
    pub gpu_temp: Support,
    pub system_temp: Support,
    pub cpu_fan: Support,
    pub gpu_fan: Support,
    pub fan_control: Support,
    pub manual_fan_duty: Support,
    pub backlight_effects: Support,
    pub per_zone_static: Support,
    pub per_key: Support,
    pub cpu_overclock: Support,
    pub gpu_overclock: Support,
    pub coolboost: Support,
    pub keyboard_timeout: Support,
    pub lcd_overdrive: Support,
    /// Misc-setting sub-indices the automatic, non-notifying probe accepts.
    /// Deliberately excludes GPU sub-index 5 and persistent hazard 6.
    pub misc_subindices: Vec<u8>,
    pub notes: Vec<String>,
}

fn per_key_support(controller_present: bool) -> Support {
    if controller_present {
        // Detection proves only that a known USB id exists. The ITE transport
        // is source-mapped but has not been exercised on project-owned
        // hardware, and packages intentionally install no hidraw permission
        // rule yet. Never turn USB presence into a supported-control claim.
        Support::Unverified
    } else {
        Support::No
    }
}

/// Probe the machine.
pub fn probe(t: &dyn Transport) -> Capabilities {
    let sensor = |s: Sensor| -> Support {
        match t.call_bytes(Function::GetSysInfo as u32, &[0x01, s as u8]) {
            Ok(r) if r.first() == Some(&0) => {
                let v = crate::transport::sensor_u16(&r).unwrap_or(0);
                if v > 0 {
                    Support::Yes
                } else {
                    Support::No
                }
            }
            _ => Support::No,
        }
    };

    let misc = |sub: u8| -> bool {
        // Sub-index 5 is nominally a getter but sends Notify(PEGP, 0xC0) on
        // this target. Sub-index 6 belongs to a setter that writes a byte that
        // survives power cycles. Neither belongs in an automatic probe.
        if matches!(sub, 5 | 6) {
            return false;
        }
        matches!(
            t.call_bytes(Function::GetMiscSetting as u32, &[sub, 0]),
            Ok(r) if r.first() == Some(&0)
        )
    };

    let mut misc_subindices: Vec<u8> = [OverclockTarget::Cpu as u8]
        .into_iter()
        .filter(|subindex| misc(*subindex))
        .collect();
    misc_subindices.sort_unstable();

    let backlight_effects = match t.call_bytes(Function::GetKbBacklight as u32, &1u64.to_le_bytes())
    {
        // The one control with trustworthy readback: if it answers with a
        // populated buffer, it genuinely works.
        Ok(r) if r.first() == Some(&0) && r.len() >= 9 => Support::Yes,
        _ => Support::No,
    };

    // ── Per-zone static colour ─────────────────────────────────────────────
    //
    // Probed by reading a zone back. This used to report "no" everywhere, with
    // an elaborate residue-detection scheme to explain the garbage coming
    // back. Both were wrong: the payload being sent was malformed, so the
    // firmware ignored the write and function 7 returned whatever the previous
    // call had left in the buffer. Send it correctly and it answers properly —
    // four zones set to four colours read back as four colours.
    //
    // The lesson generalises: before concluding a machine lacks a feature,
    // make sure the request was well-formed. "Unsupported" and "malformed" look
    // identical from the outside.
    let per_zone_static = match t.call_bytes(
        Function::GetStaticLed as u32,
        &[Zone::One as u8, 0, 0, 0, 0, 0, 0, 0],
    ) {
        Ok(r) if r.first() == Some(&0) && r.len() >= 4 => Support::Yes,
        _ => Support::No,
    };

    let per_key = per_key_support(perkey::detect().is_some());

    let cpu_overclock = if misc_subindices.contains(&(OverclockTarget::Cpu as u8)) {
        // Accepted by firmware — but on models where Feature.ini disables CPU
        // overclock the write is inert, and we cannot tell the difference from
        // here. Never claim more than that.
        Support::Unverified
    } else {
        Support::No
    };
    // Do not derive "no" from a getter we deliberately did not send. Explicit
    // `gpu-flag status` / OEM-mode refresh owns the notification boundary.
    let gpu_overclock = Support::Unknown;

    let fan_control = match t.call_u64(
        Function::GetFanSpeed as u32,
        u64::from(crate::wmi::Fan::Cpu as u8),
    ) {
        Ok(r) if r.first() == Some(&0) => Support::Yes,
        _ => Support::Unverified,
    };

    let advanced_error = |error: crate::transport::TransportError| match error {
        crate::transport::TransportError::FirmwareStatus { .. }
        | crate::transport::TransportError::UnsupportedEndpoint(_) => Support::No,
        _ => Support::Unknown,
    };

    let coolboost = match t.coolboost() {
        Ok(_) => Support::Unverified,
        Err(error) => advanced_error(error),
    };
    let keyboard_timeout = match t.keyboard_timeout() {
        Ok(_) => Support::Unverified,
        Err(error) => advanced_error(error),
    };
    let lcd_overdrive = match t.lcd_overdrive() {
        Ok(Some(_)) => Support::Unverified,
        Ok(None) => Support::No,
        Err(error) => advanced_error(error),
    };

    let mut notes = Vec::new();
    match per_key {
        Support::No => notes.push(
            "Per-key colour needs an ITE 8291 USB controller; this keyboard is four-zone, \
             so individual keys cannot be addressed at all."
                .into(),
        ),
        Support::Unverified => notes.push(
            "An ITE per-key USB id is present, but Alien's transport has no live hardware \
             validation and packages install no hidraw access rule; per-key writes remain \
             experimental and unavailable."
                .into(),
        ),
        _ => {}
    }
    if cpu_overclock == Support::Unverified {
        notes.push(
            "The CPU overclock flag is settable, but PredatorSense gates CPU overclock on \
             Feature.ini per model and the write is inert where it is disabled. Measure before \
             trusting it."
                .into(),
        );
    }
    notes.push(
        "Raw GPU-flag support is not probed automatically because its nominal getter sends an OEM GPU notification; use an explicitly labelled manual GPU status refresh."
            .into(),
    );
    if coolboost == Support::Unverified {
        notes.push(
            "CoolBoost setter reinitialization is confirmed on PH315-53; a controlled A/B/A found no sustained cooling lift."
                .into(),
        );
    }
    if keyboard_timeout == Support::Unverified {
        notes.push(
            "The APGe timeout field is readable; a real 30-second light-off/wake observation is still required."
                .into(),
        );
    }
    if lcd_overdrive == Support::Unverified {
        notes.push(
            "LCD overdrive is getter-confirmed but panel timing/ghosting has not been measured."
                .into(),
        );
    }
    for (name, support) in [
        ("CoolBoost", coolboost),
        ("keyboard timeout", keyboard_timeout),
        ("LCD overdrive", lcd_overdrive),
    ] {
        if support == Support::Unknown {
            notes.push(format!(
                "{name} support is unknown because its getter did not produce a valid supported/unsupported result; no write is enabled."
            ));
        }
    }

    Capabilities {
        cpu_temp: sensor(Sensor::CpuTemp),
        gpu_temp: sensor(Sensor::GpuTemp),
        system_temp: sensor(Sensor::SystemTemp),
        cpu_fan: sensor(Sensor::CpuFanRpm),
        gpu_fan: sensor(Sensor::GpuFanRpm),
        fan_control,
        manual_fan_duty: fan_control,
        backlight_effects,
        per_zone_static,
        per_key,
        cpu_overclock,
        gpu_overclock,
        coolboost,
        keyboard_timeout,
        lcd_overdrive,
        misc_subindices,
        notes,
    }
}

impl Capabilities {
    /// Rows for display, in the order a user would want them.
    pub fn rows(&self) -> Vec<(&'static str, Support)> {
        vec![
            ("cpu temperature", self.cpu_temp),
            ("gpu temperature", self.gpu_temp),
            ("board temperature", self.system_temp),
            ("cpu fan rpm", self.cpu_fan),
            ("gpu fan rpm", self.gpu_fan),
            ("fan mode (max/auto)", self.fan_control),
            ("manual fan duty", self.manual_fan_duty),
            ("keyboard effects", self.backlight_effects),
            ("per-zone static colour", self.per_zone_static),
            ("per-key colour", self.per_key),
            ("cpu overclock flag", self.cpu_overclock),
            ("raw GPU firmware flag", self.gpu_overclock),
            ("CoolBoost protocol", self.coolboost),
            ("30-second keyboard timeout", self.keyboard_timeout),
            ("LCD overdrive protocol", self.lcd_overdrive),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::TransportError;
    use std::sync::Mutex;

    /// A transport that answers from a script, so capability logic can be
    /// tested without hardware — which matters because the interesting cases
    /// are the *unsupported* ones, and the reference machine supports things.
    struct Fake {
        ok_functions: Vec<u32>,
        calls: Mutex<Vec<(u32, Vec<u8>)>>,
    }

    impl Transport for Fake {
        fn call_bytes(&self, function: u32, buf: &[u8]) -> Result<Vec<u8>, TransportError> {
            self.calls.lock().unwrap().push((function, buf.to_vec()));
            if self.ok_functions.contains(&function) {
                // status 0 then a plausible reading
                Ok(vec![0x00, 0x4a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00])
            } else {
                Ok(vec![0x01, 0, 0, 0, 0, 0, 0, 0])
            }
        }
        fn describe(&self) -> String {
            "fake".into()
        }
    }

    fn fake(ok: &[u32]) -> Fake {
        Fake {
            ok_functions: ok.to_vec(),
            calls: Mutex::new(Vec::new()),
        }
    }

    struct BrokenAdvanced;

    impl Transport for BrokenAdvanced {
        fn call_bytes(&self, _function: u32, _buf: &[u8]) -> Result<Vec<u8>, TransportError> {
            Ok(vec![1; 8])
        }

        fn describe(&self) -> String {
            "broken-advanced".into()
        }

        fn coolboost(&self) -> Result<bool, TransportError> {
            Err(TransportError::AcpiFailure("malformed getter reply".into()))
        }
    }

    struct RejectedAdvanced;

    impl Transport for RejectedAdvanced {
        fn call_bytes(&self, _function: u32, _buf: &[u8]) -> Result<Vec<u8>, TransportError> {
            Ok(vec![1; 8])
        }

        fn describe(&self) -> String {
            "rejected-advanced".into()
        }

        fn coolboost(&self) -> Result<bool, TransportError> {
            Err(TransportError::FirmwareStatus {
                operation: "CoolBoost getter".into(),
                status: 0xe2,
            })
        }
    }

    #[test]
    fn a_machine_that_answers_nothing_reports_no_capabilities() {
        let c = probe(&fake(&[]));
        assert_eq!(c.cpu_temp, Support::No);
        assert_eq!(c.backlight_effects, Support::No);
        assert_eq!(c.cpu_overclock, Support::No);
        assert!(c.misc_subindices.is_empty());
    }

    #[test]
    fn advanced_probe_keeps_malformed_unknown_separate_from_firmware_no() {
        assert_eq!(probe(&BrokenAdvanced).coolboost, Support::Unknown);
        assert_eq!(probe(&RejectedAdvanced).coolboost, Support::No);
    }

    #[test]
    fn sensors_are_detected_from_a_plausible_reading() {
        let c = probe(&fake(&[Function::GetSysInfo as u32]));
        assert_eq!(c.cpu_temp, Support::Yes);
        assert_eq!(c.gpu_temp, Support::Yes);
    }

    #[test]
    fn overclock_is_never_reported_as_confirmed() {
        // It is settable-but-possibly-inert, and claiming Yes would be the
        // exact lie this project exists to avoid.
        let c = probe(&fake(&[Function::GetMiscSetting as u32]));
        assert_eq!(c.cpu_overclock, Support::Unverified);
        assert_eq!(c.gpu_overclock, Support::Unknown);
        assert!(c.notes.iter().any(|n| n.contains("Feature.ini")));
        assert!(c
            .notes
            .iter()
            .any(|n| n.contains("not probed automatically")));
    }

    #[test]
    fn the_cmos_subindex_is_never_probed() {
        let f = fake(&[Function::GetMiscSetting as u32]);
        let c = probe(&f);
        assert!(
            !c.misc_subindices.contains(&6),
            "sub-6 must never be reported"
        );
        let calls = f.calls.lock().unwrap();
        assert!(
            !calls.iter().any(
                |(fun, buf)| *fun == Function::GetMiscSetting as u32 && buf.first() == Some(&6)
            ),
            "sub-6 must never even be called"
        );
    }

    #[test]
    fn the_notifying_gpu_subindex_is_never_probed_automatically() {
        let f = fake(&[Function::GetMiscSetting as u32]);
        let c = probe(&f);
        assert_eq!(c.gpu_overclock, Support::Unknown);
        assert!(!c.misc_subindices.contains(&5));
        let calls = f.calls.lock().unwrap();
        assert!(
            !calls.iter().any(
                |(fun, buf)| *fun == Function::GetMiscSetting as u32 && buf.first() == Some(&5)
            ),
            "sub-5 getter sends a GPU notification and must never be called automatically"
        );
    }

    #[test]
    fn per_zone_is_detected_when_the_getter_answers() {
        let c = probe(&fake(&[Function::GetStaticLed as u32]));
        assert_eq!(c.per_zone_static, Support::Yes);
    }

    #[test]
    fn per_key_can_never_be_reported_as_verified_by_detection_alone() {
        assert_eq!(per_key_support(false), Support::No);
        assert_eq!(per_key_support(true), Support::Unverified);
    }

    #[test]
    fn every_row_is_populated() {
        let c = probe(&fake(&[]));
        assert_eq!(c.rows().len(), 15);
    }
}
