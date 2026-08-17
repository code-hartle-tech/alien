//! The Windows-side method schema, and the marshalling rules that go with it.
//!
//! # Why a schema table is needed at all
//!
//! On Linux every call looks the same: write `{method} 0x0 {id} {buffer}` to
//! `/proc/acpi/call` and read bytes back. The ACPI method takes an opaque
//! buffer, so one generic `(function, &[u8]) -> Vec<u8>` covers everything.
//!
//! Windows reaches the *same* AML through WMI, and WMI is typed. The MOF fixes
//! each method's name, its parameter names, and each parameter's width — so a
//! generic byte-buffer shim cannot express it. Hence this table: it is the
//! translation between Alien's function ids and what the WMI provider expects.
//!
//! # The marshalling trap
//!
//! CIM `uint64` does **not** marshal as a 64-bit integer. Microsoft's own
//! documentation says 64-bit values must be encoded as strings, and Acer's own
//! service does exactly that — it builds a `VT_UI8` VARIANT and then calls
//! `VariantChangeType` to `VT_BSTR` before invoking the method.
//!
//! A hand-rolled implementation that passes `VT_UI8` will be accepted by the
//! type system and rejected, or silently misread, by the provider. This is the
//! single most likely way to get a Windows port subtly wrong, so the shape is
//! encoded here rather than left to each call site.
//!
//! # Status
//!
//! Pure data and pure functions, so it is testable on any host. The COM plumbing
//! that consumes it is separate and is **not yet verified on Windows hardware** —
//! see the port notes in the repository docs.

use crate::wmi::Function;

/// How a parameter crosses the WMI boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamKind {
    /// CIM `uint32`. Marshals as `VT_I4`/`VT_UI4` directly.
    U32,
    /// CIM `uint64`. **Must** be marshalled as `VT_BSTR` — a decimal string.
    U64AsString,
    /// CIM `uint8[16]`. A SAFEARRAY of `VT_UI1`.
    U8Array16,
    /// The method takes no input at all.
    None,
}

/// How a reply comes back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReturnKind {
    /// A single `uint64` out-parameter, little-endian once decoded.
    U64,
    /// A status byte plus a 15-byte payload, concatenated by the caller.
    StatusPlusBytes15,
    /// Nothing beyond the status word.
    StatusOnly,
}

/// One row of the translation table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodSchema {
    /// Alien's function id, as used on the Linux path.
    pub function: u32,
    /// The WMI method name declared in the MOF.
    pub wmi_name: &'static str,
    pub input: ParamKind,
    pub output: ReturnKind,
}

/// The Acer gaming WMI class, from the decoded MOF.
pub const GAMING_CLASS: &str = "AcerGamingFunction";
/// The APGe class that carries CoolBoost and friends.
pub const APGE_CLASS: &str = "APGeAction";
/// Namespace both live in.
pub const WMI_NAMESPACE: &str = r"root\WMI";

/// Every gaming method Alien uses, mapped to its WMI shape.
///
/// Derived from the decoded MOF for this hardware. Two rows are not
/// mechanically translatable and are called out in their comments.
pub const GAMING_METHODS: &[MethodSchema] = &[
    MethodSchema {
        function: Function::SetGamingLed as u32,
        wmi_name: "SetGamingLED",
        input: ParamKind::U64AsString,
        output: ReturnKind::StatusOnly,
    },
    MethodSchema {
        function: Function::GetSysInfo as u32,
        wmi_name: "GetGamingSysInfo",
        input: ParamKind::U32,
        output: ReturnKind::U64,
    },
    MethodSchema {
        function: Function::SetStaticLed as u32,
        wmi_name: "SetGamingRgbKb",
        // Exactly 32 bits, which is why `zone_word` is a u32 on both paths.
        input: ParamKind::U32,
        output: ReturnKind::StatusOnly,
    },
    MethodSchema {
        function: Function::GetStaticLed as u32,
        wmi_name: "GetGamingRgbKb",
        input: ParamKind::U32,
        output: ReturnKind::U64,
    },
    MethodSchema {
        function: Function::SetFanBehaviour as u32,
        wmi_name: "SetGamingFanBehavior",
        input: ParamKind::U64AsString,
        output: ReturnKind::StatusOnly,
    },
    MethodSchema {
        function: Function::SetFanSpeed as u32,
        wmi_name: "SetGamingFanSpeed",
        input: ParamKind::U64AsString,
        output: ReturnKind::StatusOnly,
    },
    MethodSchema {
        function: Function::GetFanSpeed as u32,
        wmi_name: "GetGamingFanSpeed",
        input: ParamKind::U32,
        output: ReturnKind::U64,
    },
    MethodSchema {
        function: Function::SetFanTable as u32,
        wmi_name: "SetGamingFanTable",
        input: ParamKind::U64AsString,
        output: ReturnKind::StatusOnly,
    },
    MethodSchema {
        function: Function::GetFanTable as u32,
        wmi_name: "GetGamingFanTable",
        // No input parameter at all. The Linux path already sends an empty
        // buffer here, so the two agree.
        input: ParamKind::None,
        output: ReturnKind::U64,
    },
    MethodSchema {
        function: Function::SetKbBacklight as u32,
        wmi_name: "SetGamingKBBacklight",
        // Widened from Alien's 8 bytes by zero-padding — which is exactly what
        // Acer's own service does.
        input: ParamKind::U8Array16,
        output: ReturnKind::StatusOnly,
    },
    MethodSchema {
        function: Function::GetKbBacklight as u32,
        wmi_name: "GetGamingKBBacklight",
        // The one row needing a real adapter: the Linux path sends an input
        // selector, the WMI method takes none, and the reply arrives as a
        // status byte plus a separate 15-byte array rather than one buffer.
        input: ParamKind::None,
        output: ReturnKind::StatusPlusBytes15,
    },
    MethodSchema {
        function: Function::SetMiscSetting as u32,
        wmi_name: "SetGamingMiscSetting",
        input: ParamKind::U64AsString,
        output: ReturnKind::StatusOnly,
    },
    MethodSchema {
        function: Function::GetMiscSetting as u32,
        wmi_name: "GetGamingMiscSetting",
        input: ParamKind::U32,
        output: ReturnKind::U64,
    },
];

/// Look up a function's WMI shape.
pub fn schema_for(function: u32) -> Option<&'static MethodSchema> {
    GAMING_METHODS.iter().find(|m| m.function == function)
}

/// Render a `u64` the way the WMI provider expects it.
///
/// A decimal string, not a number. See the module docs — this is the marshalling
/// rule that a hand-rolled port gets wrong.
pub fn u64_param(word: u64) -> String {
    word.to_string()
}

/// Widen an 8-byte payload to the `uint8[16]` the backlight setter declares.
///
/// Zero-padded on the right. Rejects anything longer, because silently
/// truncating a lighting frame would produce wrong colours rather than an error.
pub fn pad_to_16(payload: &[u8]) -> Result<[u8; 16], String> {
    if payload.len() > 16 {
        return Err(format!(
            "payload is {} bytes; the WMI method declares uint8[16]",
            payload.len()
        ));
    }
    let mut out = [0u8; 16];
    out[..payload.len()].copy_from_slice(payload);
    Ok(out)
}

/// Rebuild Alien's flat reply buffer from the backlight getter's split output.
///
/// The WMI method returns `gmReturn` (the status byte) and `gmOutput[15]`
/// separately, whereas every Alien decoder expects one buffer with the status
/// in byte 0.
pub fn join_status_and_payload(status: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + payload.len());
    out.push(status);
    out.extend_from_slice(payload);
    out
}

/// Decode a `uint64` out-parameter into Alien's byte-buffer convention.
///
/// The Linux transport hands decoders a little-endian buffer whose byte 0 is the
/// status, so the same layout is reproduced here and every existing decoder —
/// `sensor_u16`, the duty getter, the status check — works unchanged.
pub fn u64_reply_to_buffer(value: u64) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_function_alien_uses_has_a_windows_mapping() {
        // The set the policy allowlist admits, minus the APGe ones which live
        // on a different class.
        for f in [
            Function::SetGamingLed,
            Function::GetSysInfo,
            Function::SetStaticLed,
            Function::GetStaticLed,
            Function::SetFanBehaviour,
            Function::SetFanSpeed,
            Function::GetFanSpeed,
            Function::SetKbBacklight,
            Function::GetKbBacklight,
            Function::SetMiscSetting,
            Function::GetMiscSetting,
        ] {
            assert!(
                schema_for(f as u32).is_some(),
                "{f:?} has no Windows schema; the port would silently lack it"
            );
        }
    }

    #[test]
    fn sixty_four_bit_parameters_marshal_as_decimal_strings() {
        // Not hex, not a number. `0x820009` is both fans at maximum.
        assert_eq!(u64_param(0x0082_0009), "8519689");
        assert_eq!(u64_param(0), "0");
        assert_eq!(u64_param(u64::MAX), "18446744073709551615");
    }

    #[test]
    fn the_fan_word_setter_is_marked_as_a_string_parameter() {
        // The single most consequential row: passing this as VT_UI8 is the
        // documented way to get a Windows port subtly wrong.
        let s = schema_for(Function::SetFanBehaviour as u32).expect("mapped");
        assert_eq!(s.input, ParamKind::U64AsString);
        assert_eq!(s.wmi_name, "SetGamingFanBehavior");
    }

    #[test]
    fn getters_taking_a_selector_use_u32_not_u64() {
        for f in [
            Function::GetSysInfo,
            Function::GetFanSpeed,
            Function::GetMiscSetting,
        ] {
            assert_eq!(schema_for(f as u32).unwrap().input, ParamKind::U32);
        }
    }

    #[test]
    fn the_two_methods_with_no_input_are_marked_as_such() {
        assert_eq!(
            schema_for(Function::GetFanTable as u32).unwrap().input,
            ParamKind::None
        );
        assert_eq!(
            schema_for(Function::GetKbBacklight as u32).unwrap().input,
            ParamKind::None
        );
    }

    #[test]
    fn backlight_payload_is_zero_padded_to_sixteen() {
        let got = pad_to_16(&[1, 2, 3, 4, 5, 6, 7, 8]).expect("fits");
        assert_eq!(&got[..8], &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(&got[8..], &[0u8; 8], "the tail must be zeros, not garbage");
    }

    #[test]
    fn an_oversized_payload_is_refused_rather_than_truncated() {
        let err = pad_to_16(&[0u8; 17]).unwrap_err();
        assert!(err.contains("uint8[16]"));
    }

    #[test]
    fn the_split_backlight_reply_rebuilds_aliens_buffer_layout() {
        let joined = join_status_and_payload(0, &[0xAA, 0xBB, 0xCC]);
        assert_eq!(joined[0], 0, "status must land in byte 0");
        assert_eq!(&joined[1..], &[0xAA, 0xBB, 0xCC]);
    }

    #[test]
    fn a_u64_reply_decodes_the_way_existing_parsers_expect() {
        // Byte 0 is status, bytes 1..=2 are a little-endian sensor value —
        // exactly what sensor_u16 reads on the Linux path.
        let buf = u64_reply_to_buffer(0x0000_0000_0000_5A00);
        assert_eq!(buf[0], 0x00, "status byte");
        assert_eq!(u16::from_le_bytes([buf[1], buf[2]]), 0x005A);
        assert_eq!(buf.len(), 8);
    }

    #[test]
    fn method_names_match_the_decoded_mof_spelling() {
        // A typo here fails at runtime on Windows only, where it is expensive to
        // notice. Pinned against the MOF.
        let expected = [
            (Function::SetFanBehaviour as u32, "SetGamingFanBehavior"),
            (Function::GetSysInfo as u32, "GetGamingSysInfo"),
            (Function::SetStaticLed as u32, "SetGamingRgbKb"),
            (Function::SetKbBacklight as u32, "SetGamingKBBacklight"),
            (Function::GetFanTable as u32, "GetGamingFanTable"),
        ];
        for (f, name) in expected {
            assert_eq!(schema_for(f).unwrap().wmi_name, name);
        }
    }

    #[test]
    fn no_two_rows_claim_the_same_function() {
        let mut seen: Vec<u32> = GAMING_METHODS.iter().map(|m| m.function).collect();
        let before = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(
            seen.len(),
            before,
            "duplicate function id in the schema table"
        );
    }
}
