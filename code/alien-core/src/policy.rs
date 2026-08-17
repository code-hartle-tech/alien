//! What the daemon is willing to forward.
//!
//! The daemon runs as root and its socket is reachable by every member of the
//! `alien` group. If it forwarded arbitrary `(function, payload)` pairs it
//! would be a general-purpose "call any firmware method as root" service, which
//! is a much larger thing than "let me set my fan speed" — and it would hand
//! out the two hazards this project documents:
//!
//! * **Function 22 (`0x16`) sub-index 6 writes a persistent CMOS byte.** It
//!   survives reboots and power cycles. Nothing in Alien needs it.
//! * **Unknown functions are unknown.** This interface routes through SMM;
//!   functions we have not characterised may do anything, and "it returned
//!   status 0" tells us nothing about what it did.
//!
//! So the policy is an allowlist of the functions Alien actually uses, with a
//! payload check where the payload is what makes a call dangerous. Everything
//! else is refused with a reason.
//!
//! This lives in `alien-core`, not in the daemon, so the daemon policy can be
//! unit-tested without root. Direct `AcpiCall` access remains an explicit
//! root-only trust boundary and does not consult this group-socket allowlist.

use crate::wmi::Function;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Deny(&'static str),
}

impl Verdict {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Verdict::Allow)
    }
}

/// The functions Alien issues, and nothing else.
const ALLOWED: &[u32] = &[
    Function::SetGamingLed as u32,
    Function::GetSysInfo as u32,
    Function::SetStaticLed as u32,
    Function::GetStaticLed as u32,
    Function::SetFanBehaviour as u32,
    Function::SetFanSpeed as u32,
    Function::GetFanSpeed as u32,
    Function::SetKbBacklight as u32,
    Function::GetKbBacklight as u32,
    Function::SetMiscSetting as u32,
    Function::GetMiscSetting as u32,
];

/// Sub-index of `SetMiscSetting` that writes persistent CMOS.
const CMOS_SUBINDEX: u8 = 6;

/// Decide whether a call may be forwarded.
pub fn check(function: u32, payload: &[u8]) -> Verdict {
    if !ALLOWED.contains(&function) {
        return Verdict::Deny(
            "function is not on Alien's allowlist; this daemon is not a general-purpose \
             firmware proxy",
        );
    }

    if function == Function::GetSysInfo as u32 {
        return match payload {
            [1, sensor] if matches!(*sensor, 1 | 2 | 3 | 6 | 10) => Verdict::Allow,
            _ => Verdict::Deny("system-info reads must select one characterized sensor"),
        };
    }

    if function == Function::SetStaticLed as u32 {
        let Ok(bytes) = <[u8; 8]>::try_from(payload) else {
            return Verdict::Deny("static-zone writes must be exactly eight bytes");
        };
        return if matches!(bytes[0], 1 | 2 | 4 | 8) && bytes[4..].iter().all(|byte| *byte == 0) {
            Verdict::Allow
        } else {
            Verdict::Deny("static-zone writes require one Covini zone bit and zero high padding")
        };
    }

    if function == Function::GetStaticLed as u32 {
        let Ok(bytes) = <[u8; 8]>::try_from(payload) else {
            return Verdict::Deny("static-zone reads must be exactly eight bytes");
        };
        return if matches!(bytes[0], 1 | 2 | 4 | 8) && bytes[1..].iter().all(|byte| *byte == 0) {
            Verdict::Allow
        } else {
            Verdict::Deny("static-zone reads require one Covini zone bit and zero padding")
        };
    }

    if function == Function::SetFanBehaviour as u32 {
        let Ok(bytes) = <[u8; 8]>::try_from(payload) else {
            return Verdict::Deny("fan-behaviour writes must be exactly eight bytes");
        };
        let word = u64::from_le_bytes(bytes);
        return if matches!(
            word,
            0x0041_0009
                | 0x0082_0009
                | 0x00c3_0009
                | 0x0001_0001
                | 0x0002_0001
                | 0x0003_0001
                | 0x0040_0008
                | 0x0080_0008
                | 0x00c0_0008
        ) {
            Verdict::Allow
        } else {
            Verdict::Deny("fan-behaviour word is not an exact CPU/GPU Auto/Max/Manual encoding")
        };
    }

    if function == Function::SetFanSpeed as u32 {
        let Ok(bytes) = <[u8; 8]>::try_from(payload) else {
            return Verdict::Deny("fan-speed writes must be exactly eight bytes");
        };
        return if matches!(bytes[0], 1 | 4)
            && bytes[1] <= 100
            && bytes[2..].iter().all(|byte| *byte == 0)
        {
            Verdict::Allow
        } else {
            Verdict::Deny("fan-speed writes require CPU/GPU id, 0..100 percent and zero padding")
        };
    }

    if function == Function::GetFanSpeed as u32 {
        let Ok(bytes) = <[u8; 8]>::try_from(payload) else {
            return Verdict::Deny("fan-speed reads must be exactly eight bytes");
        };
        return if matches!(bytes[0], 1 | 4) && bytes[1..].iter().all(|byte| *byte == 0) {
            Verdict::Allow
        } else {
            Verdict::Deny("fan-speed reads require CPU/GPU id and zero padding")
        };
    }

    if function == Function::SetKbBacklight as u32 {
        let Ok(bytes) = <[u8; 8]>::try_from(payload) else {
            return Verdict::Deny("Covini backlight writes must be exactly eight bytes");
        };
        let brightness_ok = crate::rgb::BRIGHTNESS_STEPS.contains(&bytes[2]);
        let shape_ok = match bytes[0] {
            0 => bytes[1] == 0 && bytes[3..].iter().all(|byte| *byte == 0),
            1 | 5 => (1..=9).contains(&bytes[1]) && bytes[3] == 0 && bytes[4] == 0,
            2 => (1..=9).contains(&bytes[1]) && bytes[3..].iter().all(|byte| *byte == 0),
            3 => {
                (1..=9).contains(&bytes[1])
                    && bytes[3] == 8
                    && matches!(bytes[4], 1 | 2)
                    && bytes[5..].iter().all(|byte| *byte == 0)
            }
            4 => (1..=9).contains(&bytes[1]) && bytes[3] == 0 && matches!(bytes[4], 1 | 2),
            _ => false,
        };
        return if brightness_ok && shape_ok {
            Verdict::Allow
        } else {
            Verdict::Deny("backlight payload is not an exact PH315-53 Covini effect encoding")
        };
    }

    if function == Function::GetKbBacklight as u32 {
        return if payload == 1u64.to_le_bytes() {
            Verdict::Allow
        } else {
            Verdict::Deny("backlight reads require the exact selector word 1")
        };
    }

    // Misc settings share one broad firmware function. Constrain each direction
    // to the exact shapes Alien issues so group membership cannot be used as a
    // generic sub-index/value write primitive.
    if function == Function::SetMiscSetting as u32 {
        let Ok(bytes) = <[u8; 8]>::try_from(payload) else {
            return Verdict::Deny("misc-setting writes must be exactly eight bytes");
        };
        if bytes[0] == CMOS_SUBINDEX {
            return Verdict::Deny(
                "misc-setting sub-index 6 writes a byte that persists across power cycles; \
                 Alien never issues it",
            );
        }
        if bytes[0] == 5 {
            return Verdict::Deny(
                "raw GPU-flag writes would split OEM GPU-mode state; use the guarded typed endpoint",
            );
        }
        if bytes[0] != 7 {
            return Verdict::Deny("Alien writes only the characterized legacy CPU flag sub-index");
        }
        if !matches!(bytes[1], 0 | 2) || bytes[2..].iter().any(|byte| *byte != 0) {
            return Verdict::Deny(
                "turbo flag payload must be value 0 or 2 followed by zero padding",
            );
        }
        return Verdict::Allow;
    }
    if function == Function::GetMiscSetting as u32 {
        let Ok([subindex, zero]) = <[u8; 2]>::try_from(payload) else {
            return Verdict::Deny("misc-setting reads must be exactly two bytes");
        };
        if subindex == CMOS_SUBINDEX {
            return Verdict::Deny(
                "misc-setting sub-index 6 writes a byte that persists across power cycles; \
                 Alien never issues it",
            );
        }
        if subindex == 5 {
            return Verdict::Deny(
                "raw GPU-flag getter sends a GPU notification; use the rate-limited typed endpoint",
            );
        }
        if zero != 0 {
            return Verdict::Deny("misc-setting read padding byte must be zero");
        }
        if subindex != 7 {
            return Verdict::Deny(
                "Alien raw reads only the characterized legacy CPU flag sub-index",
            );
        }
        return Verdict::Allow;
    }

    // Function 2 enters the firmware's SMI dispatcher and is used for many
    // unrelated LED/keyboard behaviours. Alien needs exactly one recovered
    // sub-operation: Covini static-zone status, low byte 8 with only bits
    // 40..43 populated. Keep every other function-2 word out of the daemon.
    if function == Function::SetGamingLed as u32 {
        let Ok(bytes) = <[u8; 8]>::try_from(payload) else {
            return Verdict::Deny("Covini zone-status payload must be exactly eight bytes");
        };
        return if crate::rgb::is_zone_enable_word(u64::from_le_bytes(bytes)) {
            Verdict::Allow
        } else {
            Verdict::Deny("function 2 is allowed only for the recovered Covini zone-status mask")
        };
    }

    Verdict::Deny("allowlisted function has no exact payload rule")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_the_calls_alien_actually_makes() {
        assert!(check(Function::GetSysInfo as u32, &[0x01, 0x02]).is_allowed());
        assert!(check(Function::SetFanBehaviour as u32, &0x820009u64.to_le_bytes()).is_allowed());
        assert!(check(Function::SetKbBacklight as u32, &[1, 5, 100, 0, 0, 1, 2, 3]).is_allowed());
        assert!(check(Function::GetKbBacklight as u32, &1u64.to_le_bytes()).is_allowed());
        assert!(check(Function::SetStaticLed as u32, &[4, 1, 2, 3, 0, 0, 0, 0]).is_allowed());
        assert!(check(Function::GetStaticLed as u32, &[4, 0, 0, 0, 0, 0, 0, 0]).is_allowed());
        assert!(check(Function::SetFanSpeed as u32, &[1, 60, 0, 0, 0, 0, 0, 0]).is_allowed());
        assert!(check(Function::GetFanSpeed as u32, &[4, 0, 0, 0, 0, 0, 0, 0]).is_allowed());
        assert!(check(
            Function::SetGamingLed as u32,
            &crate::rgb::zone_enable_word([true, false, true, true]).to_le_bytes(),
        )
        .is_allowed());
    }

    #[test]
    fn refuses_functions_we_have_not_characterised() {
        // 0x0A is real on this interface and we have no idea what it does.
        assert!(!check(0x0A, &[]).is_allowed());
        assert!(!check(0xFF, &[]).is_allowed());
    }

    #[test]
    fn refuses_the_persistent_cmos_subindex_both_ways() {
        assert!(!check(
            Function::SetMiscSetting as u32,
            &crate::wmi::misc_word(6, 1).to_le_bytes()
        )
        .is_allowed());
        assert!(!check(Function::GetMiscSetting as u32, &[6, 0]).is_allowed());
    }

    #[test]
    fn raw_gpu_setter_is_closed_but_legacy_cpu_flag_remains_exact() {
        // 5 = GPU and must use the typed compound-mode endpoint. 7 = the
        // legacy CPU flag. Adjacent to the persistent hazard, which is exactly
        // why the guard tests exact values rather than a range.
        assert!(!check(
            Function::SetMiscSetting as u32,
            &crate::wmi::misc_word(5, 2).to_le_bytes()
        )
        .is_allowed());
        assert!(check(
            Function::SetMiscSetting as u32,
            &crate::wmi::misc_word(7, 2).to_le_bytes()
        )
        .is_allowed());
    }

    #[test]
    fn misc_setter_rejects_wrong_shape_value_and_subindex() {
        assert!(!check(Function::SetMiscSetting as u32, &[5, 2]).is_allowed());
        assert!(!check(
            Function::SetMiscSetting as u32,
            &crate::wmi::misc_word(5, 1).to_le_bytes()
        )
        .is_allowed());
        assert!(!check(
            Function::SetMiscSetting as u32,
            &crate::wmi::misc_word(4, 2).to_le_bytes()
        )
        .is_allowed());
        assert!(!check(Function::SetMiscSetting as u32, &[5, 2, 0, 0, 0, 0, 0, 1]).is_allowed());
    }

    #[test]
    fn misc_getter_is_read_only_but_still_shape_checked() {
        assert!(!check(Function::GetMiscSetting as u32, &[5, 0]).is_allowed());
        assert!(check(Function::GetMiscSetting as u32, &[7, 0]).is_allowed());
        assert!(!check(Function::GetMiscSetting as u32, &[1, 0]).is_allowed());
        assert!(!check(Function::GetMiscSetting as u32, &[5]).is_allowed());
        assert!(!check(Function::GetMiscSetting as u32, &[5, 1]).is_allowed());
    }

    #[test]
    fn every_allowlisted_function_has_an_exact_payload_shape() {
        for function in ALLOWED {
            assert!(
                !check(*function, &[]).is_allowed(),
                "function {function} accepted an empty payload"
            );
            assert!(
                !check(*function, &[0xff; 16]).is_allowed(),
                "function {function} accepted an oversized arbitrary payload"
            );
        }
        assert!(!check(Function::SetKbBacklight as u32, &[3, 5, 100, 8, 2, 1, 2, 3]).is_allowed());
        assert!(!check(Function::SetFanSpeed as u32, &[1, 101, 0, 0, 0, 0, 0, 0]).is_allowed());
        assert!(!check(Function::GetSysInfo as u32, &[1, 4]).is_allowed());
    }

    #[test]
    fn function_two_is_limited_to_the_exact_covini_zone_mask() {
        assert!(!check(Function::SetGamingLed as u32, &[8]).is_allowed());
        assert!(!check(Function::SetGamingLed as u32, &9u64.to_le_bytes()).is_allowed());
        assert!(!check(
            Function::SetGamingLed as u32,
            &(8u64 | (1u64 << 39)).to_le_bytes(),
        )
        .is_allowed());
        assert!(check(
            Function::SetGamingLed as u32,
            &crate::rgb::zone_enable_word([false; 4]).to_le_bytes(),
        )
        .is_allowed());
    }
}
