//! Bose-specific device implementations.
//!
//! `mock` is the simulated device used for development and regression tests.
//!
//! `real` is the hardware-backed device. It reports what standard Windows
//! interfaces can tell us about the headphones (identity, connection, battery)
//! and, for supported models, drives noise control over Bose's BMAP protocol.
//!
//! `bmap` is the Windows transport and verification layer for that protocol.
//! The protocol implementation itself is vendored from upstream; see
//! `vendor/bose-bmap/VENDORED.md` and `docs/protocol-notes.md`.

pub mod bmap;
pub mod choice;
pub mod mock;
pub mod real;

pub use mock::{MockBehaviour, MockBoseDevice};
pub use real::RealBoseDevice;

/// Name fragments that identify a Bose device from a Windows friendly name.
///
/// Matching is case-insensitive and substring-based because Windows friendly
/// names vary by pairing method and firmware ("Bose QuietComfort Headphones",
/// "LE_Bose QC Ultra", a user-renamed device, and so on).
pub const BOSE_NAME_HINTS: &[&str] = &[
    "bose",
    "quietcomfort",
    "quiet comfort",
    "qc45",
    "qc35",
    "qc ultra",
];

/// Bose Corporation's Bluetooth SIG company identifier.
///
/// Observed on a real device: every profile child node of the test headphones
/// carries `VID&0001009E`, where `0001` is the SIG namespace and `009E` is the
/// company id. This is authoritative in a way the friendly name is not.
pub const BOSE_SIG_COMPANY_ID: u16 = 0x009E;

/// Whether a Windows friendly name plausibly belongs to a Bose device.
///
/// This is a weak *hint*, not an identification, and it fails completely for
/// renamed devices — the development test unit is named "Aurora" and matches
/// none of the hints. Prefer [`is_bose_vendor`]; use this only as a fallback
/// when no vendor id is available.
pub fn looks_like_bose(friendly_name: &str) -> bool {
    let lowered = friendly_name.to_lowercase();
    BOSE_NAME_HINTS.iter().any(|hint| lowered.contains(hint))
}

/// Whether a SIG company identifier belongs to Bose.
pub fn is_bose_vendor(vendor_id: Option<u16>) -> bool {
    vendor_id == Some(BOSE_SIG_COMPANY_ID)
}

/// Whether a device is a Bose device, preferring the vendor id.
///
/// The vendor id is definitive when Windows exposes one. The name hint is a
/// fallback for devices whose profile nodes carry no `VID&` field.
pub fn is_bose_device(vendor_id: Option<u16>, friendly_name: &str) -> bool {
    is_bose_vendor(vendor_id) || (vendor_id.is_none() && looks_like_bose(friendly_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_plausible_bose_names() {
        assert!(looks_like_bose("Bose QuietComfort Headphones"));
        assert!(looks_like_bose("LE_Bose QC Ultra Headphones"));
        assert!(looks_like_bose("BOSE QC45"));
        assert!(looks_like_bose("quietcomfort 45"));
    }

    #[test]
    fn does_not_match_unrelated_devices() {
        assert!(!looks_like_bose("Jaisal's S24 Ultra"));
        assert!(!looks_like_bose("Legion M600s Mouse"));
        assert!(!looks_like_bose("ZEB-DUKE"));
        assert!(!looks_like_bose("Speakers (Realtek(R) Audio)"));
    }

    /// "Ultra" alone must not match — several unrelated devices use it, as the
    /// development machine's own paired phone demonstrates.
    #[test]
    fn ultra_alone_is_not_a_bose_hint() {
        assert!(!looks_like_bose("Galaxy S24 Ultra"));
    }

    /// The case that motivated vendor-id detection: a real Bose QuietComfort
    /// renamed to "Aurora". Name matching cannot find it; the vendor id can.
    #[test]
    fn renamed_bose_device_is_identified_by_vendor_id() {
        assert!(!looks_like_bose("Aurora"));
        assert!(is_bose_device(Some(0x009E), "Aurora"));
    }

    #[test]
    fn non_bose_vendor_is_not_matched() {
        // 0x0075 is Samsung; the paired phone on the development machine.
        assert!(!is_bose_device(Some(0x0075), "Jaisal's S24 Ultra"));
        // A Bose-sounding name must not override a known non-Bose vendor id.
        assert!(!is_bose_device(Some(0x0075), "Bose QuietComfort"));
    }

    #[test]
    fn name_hint_is_used_only_when_no_vendor_id_exists() {
        assert!(is_bose_device(None, "Bose QuietComfort Headphones"));
        assert!(!is_bose_device(None, "Aurora"));
    }
}
