//! Windows transport — the same firmware, reached through WMI instead of ACPI.
//!
//! # Validation status
//!
//! **The COM path in this file has not been executed on Windows hardware.** It
//! is written against the decoded MOF for this machine and against Acer's own
//! service binary, and it type-checks for `x86_64-pc-windows-msvc`, but that is
//! not the same as working. Everything genuinely testable was pushed into
//! [`crate::wmi_schema`], whose tests run on every host.
//!
//! Saying so here rather than only in a commit message is deliberate: this
//! project's whole discipline is refusing to collapse *accepted*, *confirmed*
//! and *measured* into one green tick, and a transport is the worst possible
//! place to start.
//!
//! Open questions only a Windows machine can settle:
//!
//! 1. whether `uint8[16]` survives [`Variant::from`] into `put_property` — this
//!    is `SetGamingKBBacklight`, and it is the highest-risk unknown;
//! 2. whether method invocation works without elevation (the namespace grants
//!    `WBEM_METHOD_EXECUTE` to all users, but the WDM provider may still gate);
//! 3. whether both classes appear with no Acer software installed — the `_WDG`
//!    decode says the schema is firmware-supplied, which predicts yes.
//!
//! # Why there is no daemon here
//!
//! On Linux `alien-daemon` exists because `/proc/acpi/call` is a single global
//! kernel buffer whose write-then-read is not atomic: two callers interleave and
//! each reads the other's answer, with no error anywhere. That is a procfs
//! problem, not a firmware one. The AML declares `WMBH` and `WMAA`
//! `Serialized`, so two concurrent callers cannot interleave here and a broker
//! has nothing to broker. A single elevated process talks to WMI directly.
//!
//! One caveat worth recording: `WMAA` and `WMBH` hold *different* mutexes while
//! writing the same underlying mailbox before triggering SMI. Cross-method
//! exclusion is not declared in firmware, so Alien keeps one in-process lock
//! spanning both classes — the same discipline the Linux transport applies.
//!
//! # Why the low-level method API
//!
//! The crate's ergonomic `exec_class_method` is serde-driven: it derives the
//! class name from a Rust type and serialises a statically-typed parameter
//! struct. Alien dispatches *dynamically* on a function id, and [`Variant`] is
//! not `Serialize`, so that route cannot express this at all. The documented
//! escape hatch — `get_method` for the input signature, `spawn_instance` to
//! build one, `put_property` to fill it — takes `impl Into<Variant>` and does.

use std::sync::Mutex;

use serde::Deserialize;
use wmi::{Variant, WMIConnection};

use crate::transport::{Transport, TransportError};
use crate::wmi_schema::{
    self, MethodSchema, ParamKind, ReturnKind, GAMING_CLASS, WMI_NAMESPACE,
};

/// Firmware access over Windows WMI.
pub struct WmiTransport {
    connection: WMIConnection,
    /// Spans both WMI classes, not one — see the module docs.
    call_lock: Mutex<()>,
}

// SAFETY: every use of the connection is serialised through `call_lock`.
unsafe impl Send for WmiTransport {}
unsafe impl Sync for WmiTransport {}

fn wmi_err(context: &str, e: impl std::fmt::Display) -> TransportError {
    TransportError::AcpiFailure(format!("{context}: {e}"))
}

impl WmiTransport {
    /// Connect to `root\WMI`.
    ///
    /// Fails loudly when the Acer class is absent rather than degrading into a
    /// transport whose every call returns nothing — this interface already has
    /// enough ways to succeed without doing anything.
    pub fn connect() -> Result<Self, TransportError> {
        let connection = WMIConnection::with_namespace_path(WMI_NAMESPACE)
            .map_err(|e| wmi_err(&format!("cannot open {WMI_NAMESPACE}"), e))?;
        let transport = WmiTransport {
            connection,
            call_lock: Mutex::new(()),
        };
        // Prove the class and its instance exist now, not on the first fan
        // write — a transport that connects and then fails every call is the
        // worst shape this could take.
        transport.class_object()?;
        transport.instance_path()?;
        Ok(transport)
    }

    /// The class *definition*. `GetMethod` may only be called on one of these,
    /// not on an instance.
    fn class_object(&self) -> Result<wmi::IWbemClassWrapper, TransportError> {
        self.connection
            .get_object(GAMING_CLASS)
            .map_err(|e| wmi_err(&format!("get_object {GAMING_CLASS}"), e))
    }

    /// The `__Path` of the single instance these methods are invoked on.
    ///
    /// `_WDG` declares exactly one instance for this GUID, so more than one
    /// means the machine is not what the protocol notes describe — and guessing
    /// which to drive would be worse than stopping.
    fn instance_path(&self) -> Result<String, TransportError> {
        let mut rows: Vec<GamingInstance> = self
            .connection
            .raw_query(format!("SELECT __Path FROM {GAMING_CLASS}"))
            .map_err(|e| wmi_err(&format!("query {GAMING_CLASS}"), e))?;
        match rows.len() {
            0 => Err(TransportError::MethodNotFound),
            1 => Ok(std::mem::take(&mut rows[0].__Path)),
            n => Err(TransportError::AcpiFailure(format!(
                "{GAMING_CLASS} reports {n} instances; _WDG declares exactly one"
            ))),
        }
    }

    fn invoke(&self, schema: &MethodSchema, payload: &[u8]) -> Result<Vec<u8>, TransportError> {
        let _guard = self
            .call_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let definition = self.class_object()?;
        let path = self.instance_path()?;

        let in_params = match schema.input {
            ParamKind::None => None,
            _ => {
                let signature = definition
                    .get_method(schema.wmi_name)
                    .map_err(|e| wmi_err(&format!("GetMethod {}", schema.wmi_name), e))?
                    .ok_or_else(|| {
                        TransportError::AcpiFailure(format!(
                            "{} declares no input parameters, but the schema expects {:?}",
                            schema.wmi_name, schema.input
                        ))
                    })?;
                let instance = signature
                    .spawn_instance()
                    .map_err(|e| wmi_err("SpawnInstance", e))?;
                let value = build_param(schema.input, payload)?;
                instance
                    .put_property("gmInput", value)
                    .map_err(|e| wmi_err("put gmInput", e))?;
                Some(instance)
            }
        };

        let reply = self
            .connection
            .exec_method(&path, schema.wmi_name, in_params.as_ref())
            .map_err(|e| wmi_err(schema.wmi_name, e))?;

        decode_reply(schema, reply.as_ref())
    }
}

/// Turn Alien's byte payload into the VARIANT the method declares.
fn build_param(kind: ParamKind, payload: &[u8]) -> Result<Variant, TransportError> {
    Ok(match kind {
        // Unreachable: callers skip parameter construction entirely for None.
        ParamKind::None => return Err(TransportError::AcpiFailure(
            "no parameter to build for a method that declares none".into(),
        )),
        // Selector getters carry the sub-index in byte 1 of the Linux wire
        // format; WMI wants a plain uint32.
        ParamKind::U32 => Variant::UI4(u32::from(payload.get(1).copied().unwrap_or(0))),
        // The rule a hand-rolled port gets wrong: CIM uint64 crosses as a
        // decimal string, not as an integer.
        ParamKind::U64AsString => Variant::String(wmi_schema::u64_param(le_u64(payload))),
        ParamKind::U8Array16 => {
            let padded = wmi_schema::pad_to_16(payload).map_err(TransportError::AcpiFailure)?;
            Variant::from(padded.to_vec())
        }
    })
}

/// Just enough of the class to read an instance path out of a query.
#[derive(Deserialize)]
#[allow(non_snake_case, non_camel_case_types)]
struct GamingInstance {
    __Path: String,
}

/// Little-endian u64 from however many bytes the caller supplied.
fn le_u64(payload: &[u8]) -> u64 {
    let mut buf = [0u8; 8];
    let n = payload.len().min(8);
    buf[..n].copy_from_slice(&payload[..n]);
    u64::from_le_bytes(buf)
}

fn variant_u64(v: &Variant) -> Option<u64> {
    match v {
        Variant::UI8(n) => Some(*n),
        Variant::UI4(n) => Some(u64::from(*n)),
        Variant::UI2(n) => Some(u64::from(*n)),
        Variant::UI1(n) => Some(u64::from(*n)),
        Variant::I4(n) => u64::try_from(*n).ok(),
        // uint64 comes back as a string for the same reason it goes out as one.
        Variant::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn property_u64(obj: &wmi::IWbemClassWrapper, name: &str) -> Option<u64> {
    obj.get_property(name).ok().as_ref().and_then(variant_u64)
}

fn decode_reply(
    schema: &MethodSchema,
    reply: Option<&wmi::IWbemClassWrapper>,
) -> Result<Vec<u8>, TransportError> {
    let Some(obj) = reply else {
        // A method with no out-parameters. Report success in Alien's layout
        // rather than an empty buffer, which every decoder reads as 0xFF.
        return Ok(wmi_schema::u64_reply_to_buffer(0));
    };
    match schema.output {
        ReturnKind::U64 | ReturnKind::StatusOnly => {
            let value = property_u64(obj, "gmOutput")
                .or_else(|| property_u64(obj, "gmReturn"))
                .or_else(|| property_u64(obj, "ReturnValue"))
                .unwrap_or(0);
            Ok(wmi_schema::u64_reply_to_buffer(value))
        }
        ReturnKind::StatusPlusBytes15 => {
            // The one method whose reply is split across two out-parameters.
            let status = property_u64(obj, "gmReturn").unwrap_or(0xFF) as u8;
            let payload = match obj.get_property("gmOutput") {
                Ok(Variant::Array(items)) => items
                    .iter()
                    .map(|v| variant_u64(v).unwrap_or(0) as u8)
                    .collect::<Vec<u8>>(),
                _ => Vec::new(),
            };
            Ok(wmi_schema::join_status_and_payload(status, &payload))
        }
    }
}

impl Transport for WmiTransport {
    fn call_bytes(&self, function: u32, buf: &[u8]) -> Result<Vec<u8>, TransportError> {
        // The same allowlist the daemon enforces on Linux. It is pure logic over
        // (function, payload), so it moves here unchanged rather than being
        // reimplemented — and running on Windows is not a reason to widen it.
        if let crate::policy::Verdict::Deny(reason) = crate::policy::check(function, buf) {
            return Err(TransportError::AcpiFailure(format!(
                "policy denied fn {function:#04x}: {reason}"
            )));
        }
        let schema = wmi_schema::schema_for(function).ok_or_else(|| {
            TransportError::AcpiFailure(format!("fn {function:#04x} has no Windows mapping"))
        })?;
        self.invoke(schema, buf)
    }

    fn describe(&self) -> String {
        format!("Windows WMI ({WMI_NAMESPACE}, {GAMING_CLASS})")
    }
}

#[cfg(test)]
mod tests {
    use super::le_u64;

    #[test]
    fn short_payloads_widen_without_losing_their_low_bytes() {
        assert_eq!(le_u64(&[0x09, 0x00, 0x82]), 0x0082_0009);
        assert_eq!(le_u64(&[]), 0);
    }

    #[test]
    fn oversized_payloads_take_the_low_eight_bytes() {
        assert_eq!(
            le_u64(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]).to_le_bytes(),
            [1, 2, 3, 4, 5, 6, 7, 8]
        );
    }

    #[test]
    fn the_both_fans_max_word_survives_the_round_trip() {
        assert_eq!(le_u64(&0x0082_0009u64.to_le_bytes()), 0x0082_0009);
    }
}
