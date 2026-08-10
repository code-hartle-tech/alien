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
//! This lives in `alien-core`, not in the daemon, so it can be unit-tested
//! without root and so a direct-transport caller enforces the same rules.

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
    Function::GetGamingLed as u32,
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

    // The only payload-dependent hazard we know of. Guarded on both the setter
    // and the getter: reading sub-6 is harmless, but allowing it invites
    // someone to discover the index and then reach for the setter.
    if function == Function::SetMiscSetting as u32 || function == Function::GetMiscSetting as u32 {
        if payload.first() == Some(&CMOS_SUBINDEX) {
            return Verdict::Deny(
                "misc-setting sub-index 6 writes a byte that persists across power cycles; \
                 Alien never issues it",
            );
        }
    }

    Verdict::Allow
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_the_calls_alien_actually_makes() {
        assert!(check(Function::GetSysInfo as u32, &[0x01, 0x02]).is_allowed());
        assert!(check(Function::SetFanBehaviour as u32, &0x820009u64.to_le_bytes()).is_allowed());
        assert!(check(Function::SetKbBacklight as u32, &[0, 0, 100, 0, 2, 1, 2, 3]).is_allowed());
    }

    #[test]
    fn refuses_functions_we_have_not_characterised() {
        // 0x0A is real on this interface and we have no idea what it does.
        assert!(!check(0x0A, &[]).is_allowed());
        assert!(!check(0xFF, &[]).is_allowed());
    }

    #[test]
    fn refuses_the_persistent_cmos_subindex_both_ways() {
        assert!(!check(Function::SetMiscSetting as u32, &[6, 1]).is_allowed());
        assert!(!check(Function::GetMiscSetting as u32, &[6, 0]).is_allowed());
    }

    #[test]
    fn still_allows_the_turbo_subindices() {
        // 5 = GPU, 7 = CPU. Adjacent to the hazard, which is exactly why the
        // guard has to test the value and not a range.
        assert!(check(Function::SetMiscSetting as u32, &[5, 2]).is_allowed());
        assert!(check(Function::SetMiscSetting as u32, &[7, 2]).is_allowed());
    }
}
