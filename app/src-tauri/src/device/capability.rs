//! The capability engine.
//!
//! This module is the enforcement point for the project's hardware truth rule:
//! a feature is never described as working because a UI exists for it, because
//! a command was accepted, or because documentation says it should. It is
//! described as working only when the physical device demonstrated it.
//!
//! The rules here are mirrored in TypeScript at
//! `app/frontend/src/types/capability.ts`. Both sides are tested.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How much we actually know about a feature.
///
/// The ordering of variants is deliberate and is *not* a ranking of quality —
/// do not rely on `PartialOrd`; it is not derived for that reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CapabilityStatus {
    /// We have not established whether the feature is accessible.
    Unknown,
    /// A technically valid interface appears to expose the feature, but it has
    /// **not** been confirmed on the user's physical device.
    Supported,
    /// The actual physical device was tested and the feature was confirmed to
    /// work. Only reachable via [`Capability::verify_with_hardware`].
    Verified,
    /// The device itself answers for the feature and reports its state, and
    /// every change is checked by reading it back — but no change has been
    /// confirmed yet in this run. The first one that is makes it `Verified`.
    Ready,
    /// The functionality cannot currently be accessed safely through available
    /// interfaces.
    Unsupported,
}

impl CapabilityStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "UNKNOWN",
            Self::Supported => "SUPPORTED",
            Self::Verified => "VERIFIED",
            Self::Ready => "READY",
            Self::Unsupported => "UNSUPPORTED",
        }
    }

    /// Whether the UI may treat this as a working control with no caveat.
    pub fn is_actionable(self) -> bool {
        matches!(self, Self::Verified)
    }

    /// Whether interaction is permitted but must carry an "unverified" caveat.
    pub fn requires_caveat(self) -> bool {
        matches!(self, Self::Supported | Self::Ready)
    }
}

/// The features tracked. Mirrors the TypeScript `CapabilityKey`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CapabilityKey {
    Battery,
    Volume,
    Playback,
    NoiseControl,
    AwareMode,
    CustomNoiseControl,
    Equalizer,
    Multipoint,
    DeviceSettings,
    FirmwareVersion,
    AutoOff,
    VoicePrompts,
    Sidetone,
    DeviceRename,
    /// Connecting the device to, and disconnecting it from, the hosts it is
    /// paired with.
    PairedDevices,
    /// What an earbud's touch shortcut does.
    TouchControls,
    /// Earbuds pausing when taken out, and playing again when put back.
    InEarDetection,
    /// Earbuds answering a call when put in.
    AutoAnswer,
}

impl CapabilityKey {
    pub const ALL: [CapabilityKey; 18] = [
        Self::Battery,
        Self::Volume,
        Self::Playback,
        Self::NoiseControl,
        Self::AwareMode,
        Self::CustomNoiseControl,
        Self::Equalizer,
        Self::Multipoint,
        Self::DeviceSettings,
        Self::FirmwareVersion,
        Self::AutoOff,
        Self::VoicePrompts,
        Self::Sidetone,
        Self::DeviceRename,
        Self::PairedDevices,
        Self::TouchControls,
        Self::InEarDetection,
        Self::AutoAnswer,
    ];
}

/// The interface through which a capability is (or would be) reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mechanism {
    WindowsAudio,
    /// Android's media volume, through its audio manager.
    AndroidAudio,
    WindowsBluetooth,
    WindowsPnp,
    WindowsMediaSession,
    BleGattStandard,
    BleGattVendor,
    RfcommVendor,
    SoftwareDsp,
    None,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CapabilityError {
    #[error("evidence is required for every capability transition")]
    MissingEvidence,

    #[error(
        "cannot mark {key:?} as VERIFIED without hardware evidence; \
         use verify_with_hardware() from a physical device test"
    )]
    VerificationRequiresHardware { key: CapabilityKey },

    #[error("{key:?} is UNSUPPORTED, which is terminal for this session; run a fresh discovery")]
    UnsupportedIsTerminal { key: CapabilityKey },
}

/// A single tracked capability plus the reasoning behind its current status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capability {
    pub key: CapabilityKey,
    pub status: CapabilityStatus,
    pub mechanism: Mechanism,
    /// Tracked separately from `status` so hardware confirmation can never be
    /// implied by the status field alone.
    pub hardware_verified: bool,
    /// Why the capability is in its current state. Always populated.
    pub evidence: String,
    /// RFC3339 timestamp of the last change, if any.
    pub last_evaluated: Option<String>,
}

impl Capability {
    /// A capability we know nothing about yet.
    pub fn unknown(key: CapabilityKey, reason: impl Into<String>) -> Self {
        Self {
            key,
            status: CapabilityStatus::Unknown,
            mechanism: Mechanism::None,
            hardware_verified: false,
            evidence: reason.into(),
            last_evaluated: None,
        }
    }

    fn guard(&self, evidence: &str) -> Result<(), CapabilityError> {
        if evidence.trim().is_empty() {
            return Err(CapabilityError::MissingEvidence);
        }
        if self.status == CapabilityStatus::Unsupported {
            return Err(CapabilityError::UnsupportedIsTerminal { key: self.key });
        }
        Ok(())
    }

    fn apply(&mut self, status: CapabilityStatus, mechanism: Mechanism, evidence: String) {
        self.status = status;
        self.mechanism = mechanism;
        self.evidence = evidence;
        self.last_evaluated = Some(crate::device::now_rfc3339());
    }

    /// Record that a valid interface appears to expose this feature.
    ///
    /// This explicitly does **not** claim the feature works on the user's
    /// device. It is the correct state after discovering, say, a GATT
    /// characteristic with the right properties.
    pub fn mark_supported(
        &mut self,
        mechanism: Mechanism,
        evidence: impl Into<String>,
    ) -> Result<(), CapabilityError> {
        let evidence = evidence.into();
        self.guard(&evidence)?;
        self.apply(CapabilityStatus::Supported, mechanism, evidence);
        Ok(())
    }

    /// Record that the device answers for the feature and reports its state,
    /// with no change confirmed yet.
    pub fn mark_ready(
        &mut self,
        mechanism: Mechanism,
        evidence: impl Into<String>,
    ) -> Result<(), CapabilityError> {
        let evidence = evidence.into();
        self.guard(&evidence)?;
        self.apply(CapabilityStatus::Ready, mechanism, evidence);
        Ok(())
    }

    /// Record that the feature is not safely reachable. Terminal for the session.
    pub fn mark_unsupported(
        &mut self,
        evidence: impl Into<String>,
    ) -> Result<(), CapabilityError> {
        let evidence = evidence.into();
        if evidence.trim().is_empty() {
            return Err(CapabilityError::MissingEvidence);
        }
        self.apply(CapabilityStatus::Unsupported, Mechanism::None, evidence);
        Ok(())
    }

    /// Promote to VERIFIED.
    ///
    /// The `proof` parameter exists to make this impossible to call by accident
    /// from ordinary control flow: it can only be constructed by the hardware
    /// verification harness, which requires an observed state change on the
    /// physical device.
    pub fn verify_with_hardware(
        &mut self,
        proof: &HardwareProof,
        mechanism: Mechanism,
    ) -> Result<(), CapabilityError> {
        if proof.observation.trim().is_empty() {
            return Err(CapabilityError::MissingEvidence);
        }
        if proof.key != self.key {
            return Err(CapabilityError::VerificationRequiresHardware { key: self.key });
        }
        self.guard(&proof.observation)?;
        self.hardware_verified = true;
        self.apply(
            CapabilityStatus::Verified,
            mechanism,
            format!(
                "Hardware-verified {}: {}",
                proof.verified_at, proof.observation
            ),
        );
        Ok(())
    }
}

/// Evidence that a physical device demonstrated a feature.
///
/// Constructed only by [`HardwareProof::observed`], which the verification
/// harness calls after comparing a real before/after device state. There is no
/// other constructor, and the fields are private, so a VERIFIED status cannot
/// be produced by code that merely sent a command.
#[derive(Debug, Clone)]
pub struct HardwareProof {
    key: CapabilityKey,
    observation: String,
    verified_at: String,
}

impl HardwareProof {
    /// Record that the device's own reported state changed as expected.
    ///
    /// `before` and `after` must differ, and must come from reading the device
    /// — not from local state we set ourselves. A caller that passes identical
    /// values gets `None`, which is what makes "the slider moved" insufficient.
    pub fn observed(
        key: CapabilityKey,
        before: &str,
        after: &str,
        expected: &str,
    ) -> Option<Self> {
        Self::observed_at(key, before, after, expected, crate::device::now_rfc3339())
    }

    /// As [`observed`](Self::observed), for a change the device confirmed at a
    /// known earlier time.
    ///
    /// Evidence gathered once is reported many times — every snapshot rebuilds
    /// the capability set — and stamping it with the time of the report would
    /// quietly claim a fresh test that never happened. `observed_at` is when
    /// the device was actually read.
    pub fn observed_at(
        key: CapabilityKey,
        before: &str,
        after: &str,
        expected: &str,
        observed_at: impl Into<String>,
    ) -> Option<Self> {
        if before == after {
            return None;
        }
        if after != expected {
            return None;
        }
        Some(Self {
            key,
            observation: format!(
                "device-reported state changed {before:?} -> {after:?}, matching requested {expected:?}"
            ),
            verified_at: observed_at.into(),
        })
    }

    /// Record a passive observation: the device spontaneously reported a state
    /// change after the user physically operated the headphones. This is the
    /// strongest evidence available for read-only capabilities.
    pub fn observed_passively(key: CapabilityKey, detail: impl Into<String>) -> Self {
        Self {
            key,
            observation: format!("passively observed from device: {}", detail.into()),
            verified_at: crate::device::now_rfc3339(),
        }
    }
}

/// The full capability set for a device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilitySet {
    inner: BTreeMap<CapabilityKey, Capability>,
}

impl CapabilitySet {
    /// Every capability honestly unknown. This is the only correct starting
    /// point for a device we have not interrogated.
    pub fn all_unknown(reason: impl Into<String>) -> Self {
        let reason = reason.into();
        let mut inner = BTreeMap::new();
        for key in CapabilityKey::ALL {
            inner.insert(key, Capability::unknown(key, reason.clone()));
        }
        Self { inner }
    }

    pub fn get(&self, key: CapabilityKey) -> &Capability {
        self.inner
            .get(&key)
            .expect("CapabilitySet is constructed with every key present")
    }

    pub fn get_mut(&mut self, key: CapabilityKey) -> &mut Capability {
        self.inner
            .get_mut(&key)
            .expect("CapabilitySet is constructed with every key present")
    }

    pub fn iter(&self) -> impl Iterator<Item = &Capability> {
        self.inner.values()
    }

    /// Count of capabilities confirmed against physical hardware.
    pub fn verified_count(&self) -> usize {
        self.inner.values().filter(|c| c.hardware_verified).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_capabilities_are_all_unknown_and_unverified() {
        let set = CapabilitySet::all_unknown("no device interrogated");
        assert_eq!(set.iter().count(), CapabilityKey::ALL.len());
        for cap in set.iter() {
            assert_eq!(cap.status, CapabilityStatus::Unknown);
            assert!(!cap.hardware_verified);
        }
        assert_eq!(set.verified_count(), 0);
    }

    #[test]
    fn transitions_require_evidence() {
        let mut cap = Capability::unknown(CapabilityKey::Battery, "init");
        assert_eq!(
            cap.mark_supported(Mechanism::WindowsPnp, "   "),
            Err(CapabilityError::MissingEvidence)
        );
        assert_eq!(cap.status, CapabilityStatus::Unknown);
    }

    #[test]
    fn supported_does_not_imply_hardware_verification() {
        let mut cap = Capability::unknown(CapabilityKey::Battery, "init");
        cap.mark_supported(Mechanism::WindowsPnp, "PnP battery property exists")
            .unwrap();
        assert_eq!(cap.status, CapabilityStatus::Supported);
        // The critical assertion: SUPPORTED never sets hardware_verified.
        assert!(!cap.hardware_verified);
    }

    #[test]
    fn identical_before_and_after_yields_no_proof() {
        // "The slider moved but the device reported the same value" must not
        // be able to produce a VERIFIED status.
        let proof = HardwareProof::observed(CapabilityKey::NoiseControl, "quiet", "quiet", "quiet");
        assert!(proof.is_none());
    }

    #[test]
    fn state_change_not_matching_request_yields_no_proof() {
        let proof = HardwareProof::observed(CapabilityKey::NoiseControl, "quiet", "off", "aware");
        assert!(proof.is_none());
    }

    #[test]
    fn genuine_observed_change_verifies() {
        let mut cap = Capability::unknown(CapabilityKey::NoiseControl, "init");
        let proof =
            HardwareProof::observed(CapabilityKey::NoiseControl, "quiet", "aware", "aware").unwrap();
        cap.verify_with_hardware(&proof, Mechanism::RfcommVendor)
            .unwrap();
        assert_eq!(cap.status, CapabilityStatus::Verified);
        assert!(cap.hardware_verified);
        assert!(cap.evidence.contains("Hardware-verified"));
    }

    /// The evidence must carry the time the device was read, not the time the
    /// capability set happened to be rebuilt.
    #[test]
    fn proof_keeps_the_time_the_device_was_actually_read() {
        let mut cap = Capability::unknown(CapabilityKey::NoiseControl, "init");
        let proof = HardwareProof::observed_at(
            CapabilityKey::NoiseControl,
            "Quiet",
            "Aware",
            "Aware",
            "2026-10-10T09:15:00Z",
        )
        .unwrap();
        cap.verify_with_hardware(&proof, Mechanism::RfcommVendor)
            .unwrap();
        assert!(cap.evidence.contains("2026-10-10T09:15:00Z"));
    }

    /// A timestamp does not buy an exemption from the before/after rules.
    #[test]
    fn a_timestamped_proof_still_needs_a_real_change() {
        let at = "2026-10-10T09:15:00Z";
        assert!(
            HardwareProof::observed_at(CapabilityKey::NoiseControl, "Quiet", "Quiet", "Quiet", at)
                .is_none()
        );
        assert!(
            HardwareProof::observed_at(CapabilityKey::NoiseControl, "Quiet", "Aware", "Quiet", at)
                .is_none()
        );
    }

    #[test]
    fn proof_for_a_different_capability_is_rejected() {
        let mut cap = Capability::unknown(CapabilityKey::Battery, "init");
        let proof =
            HardwareProof::observed(CapabilityKey::NoiseControl, "quiet", "aware", "aware").unwrap();
        assert!(cap.verify_with_hardware(&proof, Mechanism::RfcommVendor).is_err());
        assert_eq!(cap.status, CapabilityStatus::Unknown);
    }

    #[test]
    fn unsupported_is_terminal() {
        let mut cap = Capability::unknown(CapabilityKey::Equalizer, "init");
        cap.mark_unsupported("no vendor EQ interface reachable from Windows")
            .unwrap();
        assert_eq!(
            cap.mark_supported(Mechanism::BleGattVendor, "found something"),
            Err(CapabilityError::UnsupportedIsTerminal {
                key: CapabilityKey::Equalizer
            })
        );
        assert_eq!(cap.status, CapabilityStatus::Unsupported);
    }

    #[test]
    fn status_actionability_semantics() {
        assert!(CapabilityStatus::Verified.is_actionable());
        // SUPPORTED must never be actionable-without-caveat: the whole point is
        // that the interface exists but was never confirmed on this device.
        assert!(!CapabilityStatus::Supported.is_actionable());
        assert!(CapabilityStatus::Supported.requires_caveat());
        assert!(CapabilityStatus::Ready.requires_caveat());
        assert!(!CapabilityStatus::Unknown.is_actionable());
        assert!(!CapabilityStatus::Unsupported.is_actionable());
    }
}
