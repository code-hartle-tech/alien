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
//! So Alien probes. Everything here uses **getters only** — reads cannot change
//! machine state, so probing is safe to run at startup and safe to run often.
//! The one hazard on this interface, misc-setting sub-index 6, is never touched
//! (see [`crate::policy`]).

use crate::perkey;
use crate::transport::Transport;
use crate::rgb::Zone;
use crate::wmi::{Function, OverclockTarget, Sensor};

/// Three-state, because "we could not tell" is a real and different answer
/// from "no". Presenting an unknown as unsupported hides working features;
/// presenting it as supported is how you get a dead control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    Yes,
    No,
    /// The firmware accepts the call but the effect could not be confirmed.
    Unverified,
}

impl Support {
    pub fn symbol(self) -> &'static str {
        match self {
            Support::Yes => "yes",
            Support::No => "no",
            Support::Unverified => "unverified",
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
    /// Misc-setting sub-indices the firmware accepts, minus the hazard.
    pub misc_subindices: Vec<u8>,
    pub notes: Vec<String>,
}

/// Probe the machine.
pub fn probe(t: &dyn Transport) -> Capabilities {
    let sensor = |s: Sensor| -> Support {
        match t.call_bytes(Function::GetSysInfo as u32, &[0x01, s as u8]) {
            Ok(r) if r.first() == Some(&0) => {
                let v = crate::transport::sensor_u16(&r).unwrap_or(0);
                if v > 0 { Support::Yes } else { Support::No }
            }
            _ => Support::No,
        }
    };

    let misc = |sub: u8| -> bool {
        // Getter only. Sub-index 6 is never probed — on the setter it writes a
        // byte that survives power cycles, and there is nothing to learn from
        // the getter worth normalising that risk.
        if sub == 6 {
            return false;
        }
        matches!(
            t.call_bytes(Function::GetMiscSetting as u32, &[sub, 0]),
            Ok(r) if r.first() == Some(&0)
        )
    };

    let mut misc_subindices: Vec<u8> = (0u8..16).filter(|s| misc(*s)).collect();
    misc_subindices.sort_unstable();

    let backlight_effects = match t.call_bytes(Function::GetKbBacklight as u32, &[0; 8]) {
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

    let per_key = if perkey::detect().is_some() { Support::Yes } else { Support::No };

    let cpu_overclock = if misc_subindices.contains(&(OverclockTarget::Cpu as u8)) {
        // Accepted by firmware — but on models where Feature.ini disables CPU
        // overclock the write is inert, and we cannot tell the difference from
        // here. Never claim more than that.
        Support::Unverified
    } else {
        Support::No
    };
    let gpu_overclock = if misc_subindices.contains(&(OverclockTarget::Gpu as u8)) {
        Support::Unverified
    } else {
        Support::No
    };

    let fan_control = match t.call_u64(
        Function::GetFanSpeed as u32,
        u64::from(crate::wmi::Fan::Cpu as u8),
    ) {
        Ok(r) if r.first() == Some(&0) => Support::Yes,
        _ => Support::Unverified,
    };

    let mut notes = Vec::new();
    if per_key == Support::No {
        notes.push(
            "Per-key colour needs an ITE 8291 USB controller; this keyboard is four-zone, \
             so individual keys cannot be addressed at all."
                .into(),
        );
    }
    if cpu_overclock == Support::Unverified {
        notes.push(
            "The CPU overclock flag is settable, but PredatorSense gates CPU overclock on \
             Feature.ini per model and the write is inert where it is disabled. Measure before \
             trusting it."
                .into(),
        );
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
            ("gpu overclock flag", self.gpu_overclock),
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
        Fake { ok_functions: ok.to_vec(), calls: Mutex::new(Vec::new()) }
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
        assert_eq!(c.gpu_overclock, Support::Unverified);
        assert!(c.notes.iter().any(|n| n.contains("Feature.ini")));
    }

    #[test]
    fn the_cmos_subindex_is_never_probed() {
        let f = fake(&[Function::GetMiscSetting as u32]);
        let c = probe(&f);
        assert!(!c.misc_subindices.contains(&6), "sub-6 must never be reported");
        let calls = f.calls.lock().unwrap();
        assert!(
            !calls.iter().any(|(fun, buf)| *fun == Function::GetMiscSetting as u32
                && buf.first() == Some(&6)),
            "sub-6 must never even be called"
        );
    }

    #[test]
    fn per_zone_is_detected_when_the_getter_answers() {
        let c = probe(&fake(&[Function::GetStaticLed as u32]));
        assert_eq!(c.per_zone_static, Support::Yes);
    }

    #[test]
    fn every_row_is_populated() {
        let c = probe(&fake(&[]));
        assert_eq!(c.rows().len(), 12);
    }
}
