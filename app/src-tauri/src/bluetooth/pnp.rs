//! Windows PnP (Configuration Manager) access.
//!
//! Why CfgMgr32 rather than WinRT here: this path is synchronous, needs no COM
//! apartment, and gives direct access to device properties — including the
//! Bluetooth battery property that Windows itself uses to show headphone
//! battery in Settings. WinRT's `DeviceInformation` exposes the same data but
//! requires constructing an `IIterable<HSTRING>` of property keys, which is
//! awkward from Rust for no benefit at this layer.
//!
//! Everything in this module is read-only. Nothing here writes to a device.

#![cfg(windows)]

use std::collections::BTreeMap;
use windows::core::PCWSTR;
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_DevNode_PropertyW, CM_Get_Device_ID_ListW, CM_Get_Device_ID_List_SizeW,
    CM_Locate_DevNodeW, CM_LOCATE_DEVNODE_NORMAL, CM_GETIDLIST_FILTER_PRESENT, CR_SUCCESS,
};
use windows::Win32::Devices::Properties::{
    DEVPROPKEY, DEVPROPTYPE, DEVPROP_TYPE_BOOLEAN, DEVPROP_TYPE_BYTE, DEVPROP_TYPE_GUID,
    DEVPROP_TYPE_STRING, DEVPROP_TYPE_UINT32,
};

/// `DEVPKEY_Device_FriendlyName` — {a45c254e-df1c-4efd-8020-67d146a850e0}, 14
pub const DEVPKEY_DEVICE_FRIENDLY_NAME: DEVPROPKEY = DEVPROPKEY {
    fmtid: windows::core::GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 14,
};

/// `DEVPKEY_Device_DeviceDesc` — {a45c254e-df1c-4efd-8020-67d146a850e0}, 2
pub const DEVPKEY_DEVICE_DESC: DEVPROPKEY = DEVPROPKEY {
    fmtid: windows::core::GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 2,
};

/// Bluetooth device battery level, 0-100.
///
/// {104EA319-6EE2-4701-BD47-8DDBF425BBE5}, 2
///
/// This is the property Windows Settings uses to display headphone battery.
/// It is populated by the HFP battery indication (`AT+IPHONEACCEV` / Android's
/// battery AT command) or by the BLE Battery Service, depending on the device.
/// Whether any given device populates it must be checked per device — the
/// mechanism working is not evidence that a specific device supports it.
pub const DEVPKEY_BLUETOOTH_BATTERY: DEVPROPKEY = DEVPROPKEY {
    fmtid: windows::core::GUID::from_u128(0x104ea319_6ee2_4701_bd47_8ddbf425bbe5),
    pid: 2,
};

/// Device connection state. {83DA6326-97A6-4088-9453-A1923F573B29}, 15
pub const DEVPKEY_DEVICE_IS_CONNECTED: DEVPROPKEY = DEVPROPKEY {
    fmtid: windows::core::GUID::from_u128(0x83da6326_97a6_4088_9453_a1923f573b29),
    pid: 15,
};

/// `DEVPKEY_Device_ContainerId` — {8c7ed206-3f8a-4827-b3ab-ae9e1faefc6c}, 2
///
/// Identifies the physical device. Windows gives every node that belongs to
/// one device the same container id — the Bluetooth entries and the audio
/// endpoints alike — which is what lets an audio endpoint be matched to the
/// headphones without relying on what either is named.
pub const DEVPKEY_DEVICE_CONTAINER_ID: DEVPROPKEY = DEVPROPKEY {
    fmtid: windows::core::GUID::from_u128(0x8c7ed206_3f8a_4827_b3ab_ae9e1faefc6c),
    pid: 2,
};

/// A value read from a PnP property.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropValue {
    Text(String),
    Bool(bool),
    Number(u32),
    Byte(u8),
    /// A GUID, upper-case and hyphenated, without braces.
    Guid(String),
}

impl PropValue {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_guid(&self) -> Option<&str> {
        match self {
            Self::Guid(g) => Some(g.as_str()),
            _ => None,
        }
    }

    /// Battery is reported as a byte on most stacks but a UINT32 on some.
    pub fn as_percent(&self) -> Option<u8> {
        match self {
            Self::Byte(b) => Some(*b),
            Self::Number(n) if *n <= 100 => Some(*n as u8),
            _ => None,
        }
    }
}

/// A Bluetooth-related device node as Windows sees it.
#[derive(Debug, Clone)]
pub struct PnpDevice {
    pub instance_id: String,
    pub friendly_name: Option<String>,
    pub is_connected: Option<bool>,
    pub battery_percent: Option<u8>,
    /// Bluetooth SIG company identifier from the instance id, when present.
    /// `0x009E` is Bose Corporation.
    pub vendor_id: Option<u16>,
    /// Vendor-assigned product identifier from the instance id, when present.
    /// For Bose devices this is the id the BMAP catalog is keyed by.
    pub product_id: Option<u16>,
    /// Service class UUIDs Windows has a node for on this device, upper-case
    /// and without braces. Windows creates these from the device's SDP
    /// records, so this is what the device advertises, as Windows read it.
    pub service_uuids: Vec<String>,
}

/// The 48-bit Bluetooth device address embedded in a PnP instance id.
///
/// Windows writes it as a bare 12-hex-digit run, in several different
/// positions depending on the node kind:
///
/// ```text
/// BTHENUM\DEV_E458BCF9F02E\7&78167D1&0&BLUETOOTHDEVICE_E458BCF9F02E
/// BTHENUM\{0000111E-...}_VID&0001009E_PID&4075\7&78167D1&0&E458BCF9F02E_C00000000
/// ```
///
/// The address is found by taking the **last** maximal 12-character hex run.
///
/// Taking the first run is wrong: service UUIDs contain one too. The Bluetooth
/// SIG base UUID ends `-00805F9B34FB`, and the vendor RFCOMM UUID observed on
/// the test headphones ends `-C4714A518BCC` — both are 12 hex characters and
/// both appear before the address. The address is always last.
pub fn device_address(instance_id: &str) -> Option<String> {
    let upper = instance_id.to_uppercase();
    let chars: Vec<char> = upper.chars().collect();
    let mut found: Option<String> = None;
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_hexdigit() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_hexdigit() {
                i += 1;
            }
            if i - start == 12 {
                found = Some(chars[start..i].iter().collect());
            }
        } else {
            i += 1;
        }
    }
    found
}

/// Parses the Bluetooth SIG company identifier out of a `VID&AAAABBBB` field.
///
/// `AAAA` is the identifier namespace (`0001` = Bluetooth SIG, `0002` = USB-IF)
/// and `BBBB` is the vendor id. Only SIG-assigned ids are returned, because a
/// USB vendor id means something different and must not be compared against
/// the SIG company list.
pub fn vendor_id(instance_id: &str) -> Option<u16> {
    let upper = instance_id.to_uppercase();
    let idx = upper.find("VID&")? + 4;
    let field: String = upper[idx..].chars().take(8).collect();
    if field.len() != 8 || !field.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let namespace = u16::from_str_radix(&field[..4], 16).ok()?;
    if namespace != 1 {
        return None;
    }
    u16::from_str_radix(&field[4..], 16).ok()
}

/// Parses the product identifier out of a `PID&BBBB` field.
///
/// Only returned alongside a SIG-namespace vendor id, for the same reason
/// [`vendor_id`] is: a product id is meaningless without knowing whose it is.
pub fn product_id(instance_id: &str) -> Option<u16> {
    vendor_id(instance_id)?;
    let upper = instance_id.to_uppercase();
    let idx = upper.find("PID&")? + 4;
    let field: String = upper[idx..].chars().take(4).collect();
    if field.len() != 4 || !field.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u16::from_str_radix(&field, 16).ok()
}

/// The service class UUID a Bluetooth Classic profile node belongs to.
///
/// ```text
/// BTHENUM\{0000111E-0000-1000-8000-00805F9B34FB}_VID&0001009E_PID&4075\...
///          ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
/// ```
///
/// Returned upper-case, without braces. `None` for nodes that are not a
/// per-service child of a Classic device.
pub fn service_uuid(instance_id: &str) -> Option<String> {
    let upper = instance_id.to_uppercase();
    let rest = upper.strip_prefix("BTHENUM\\{")?;
    let (uuid, _) = rest.split_once('}')?;
    let well_formed = uuid.len() == 36
        && uuid.chars().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        });
    well_formed.then(|| uuid.to_string())
}

impl PnpDevice {
    /// Whether Windows lists a service with this UUID for the device.
    pub fn advertises_service(&self, uuid: u128) -> bool {
        let wanted = format_uuid(uuid);
        self.service_uuids.contains(&wanted)
    }

    /// Whether this instance id refers to a top-level device rather than one of
    /// the per-profile child nodes Windows creates (AVRCP transport, Handsfree
    /// service, and so on). Those children duplicate the friendly name and
    /// would otherwise produce many spurious "devices".
    pub fn is_top_level(&self) -> bool {
        let id = self.instance_id.to_uppercase();
        // Child profile nodes carry a service GUID in braces.
        let has_service_guid = id.contains('{');
        (id.starts_with("BTHENUM\\DEV_") || id.starts_with("BTHLE\\DEV_")) && !has_service_guid
    }
}

/// A UUID in the form Windows writes it in instance ids: upper-case,
/// hyphenated, no braces.
pub fn format_uuid(uuid: u128) -> String {
    let hex = format!("{uuid:032X}");
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

fn wide_to_string(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Reads a single property from a device node.
///
/// Returns `None` when the property is simply not present, which is the common
/// case and not an error — most devices do not report battery.
pub fn get_property(instance_id: &str, key: &DEVPROPKEY) -> Option<PropValue> {
    unsafe {
        let wide = to_wide(instance_id);
        let mut devinst = 0u32;
        let cr = CM_Locate_DevNodeW(
            &mut devinst,
            PCWSTR(wide.as_ptr()),
            CM_LOCATE_DEVNODE_NORMAL,
        );
        if cr != CR_SUCCESS {
            return None;
        }

        let mut prop_type = DEVPROPTYPE(0);
        let mut size = 0u32;

        // First call sizes the buffer.
        let _ = CM_Get_DevNode_PropertyW(devinst, key, &mut prop_type, None, &mut size, 0);
        if size == 0 {
            return None;
        }

        let mut buf = vec![0u8; size as usize];
        let cr = CM_Get_DevNode_PropertyW(
            devinst,
            key,
            &mut prop_type,
            Some(buf.as_mut_ptr()),
            &mut size,
            0,
        );
        if cr != CR_SUCCESS {
            return None;
        }
        buf.truncate(size as usize);

        match prop_type.0 {
            t if t == DEVPROP_TYPE_STRING.0 => {
                let u16buf: Vec<u16> = buf
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                Some(PropValue::Text(wide_to_string(&u16buf)))
            }
            t if t == DEVPROP_TYPE_BOOLEAN.0 => buf.first().map(|b| PropValue::Bool(*b != 0)),
            t if t == DEVPROP_TYPE_BYTE.0 => buf.first().map(|b| PropValue::Byte(*b)),
            t if t == DEVPROP_TYPE_GUID.0 => guid_from_bytes(&buf).map(PropValue::Guid),
            t if t == DEVPROP_TYPE_UINT32.0 => {
                if buf.len() >= 4 {
                    Some(PropValue::Number(u32::from_le_bytes([
                        buf[0], buf[1], buf[2], buf[3],
                    ])))
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

/// Formats the 16 bytes of a GUID as Windows stores it: the first three
/// fields little-endian, the last eight bytes as they are.
fn guid_from_bytes(bytes: &[u8]) -> Option<String> {
    let b: &[u8; 16] = bytes.get(..16)?.try_into().ok()?;
    let value = (u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as u128) << 96
        | (u16::from_le_bytes([b[4], b[5]]) as u128) << 80
        | (u16::from_le_bytes([b[6], b[7]]) as u128) << 64
        | (u64::from_be_bytes([b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]]) as u128);
    Some(format_uuid(value))
}

/// The container id of the physical device a node belongs to.
pub fn container_id(instance_id: &str) -> Option<String> {
    get_property(instance_id, &DEVPKEY_DEVICE_CONTAINER_ID)
        .and_then(|v| v.as_guid().map(str::to_string))
}

/// Enumerates present device instance ids under the given enumerator prefix.
///
/// `filter` is a PnP enumerator such as `"BTHENUM"` (Bluetooth Classic) or
/// `"BTHLE"` (Bluetooth LE). Only devices currently present are returned.
pub fn enumerate_instance_ids(filter: &str) -> Vec<String> {
    unsafe {
        let wide = to_wide(filter);
        let mut len = 0u32;
        let cr = CM_Get_Device_ID_List_SizeW(
            &mut len,
            PCWSTR(wide.as_ptr()),
            CM_GETIDLIST_FILTER_PRESENT | CM_GETIDLIST_FILTER_ENUMERATOR,
        );
        if cr != CR_SUCCESS || len == 0 {
            return Vec::new();
        }

        let mut buf = vec![0u16; len as usize];
        let cr = CM_Get_Device_ID_ListW(
            PCWSTR(wide.as_ptr()),
            &mut buf,
            CM_GETIDLIST_FILTER_PRESENT | CM_GETIDLIST_FILTER_ENUMERATOR,
        );
        if cr != CR_SUCCESS {
            return Vec::new();
        }

        // The result is a double-null-terminated multi-string.
        buf.split(|&c| c == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf16_lossy(s))
            .collect()
    }
}

const CM_GETIDLIST_FILTER_ENUMERATOR: u32 = 0x00000001;

/// Enumerates Bluetooth devices (Classic and LE) with their readable properties.
///
/// Returns one entry per physical device. Windows creates a child node per
/// Bluetooth profile, and properties are scattered across them rather than
/// collected on the parent — a pair of headphones observed during development
/// reported its battery only on the Hands-Free (HFP) child node, while the
/// top-level node reported none at all. Reading only top-level nodes therefore
/// misses battery entirely. Properties are gathered across every node sharing
/// a device address and attached to that device's top-level entry.
pub fn enumerate_bluetooth_devices() -> Vec<PnpDevice> {
    struct Node {
        instance_id: String,
        friendly_name: Option<String>,
        is_connected: Option<bool>,
        battery_percent: Option<u8>,
        vendor_id: Option<u16>,
        product_id: Option<u16>,
    }

    let mut nodes: Vec<Node> = Vec::new();

    for enumerator in ["BTHENUM", "BTHLE", "BTHLEDEVICE"] {
        for instance_id in enumerate_instance_ids(enumerator) {
            let friendly_name = get_property(&instance_id, &DEVPKEY_DEVICE_FRIENDLY_NAME)
                .and_then(|v| v.as_text().map(str::to_string))
                .or_else(|| {
                    get_property(&instance_id, &DEVPKEY_DEVICE_DESC)
                        .and_then(|v| v.as_text().map(str::to_string))
                });

            nodes.push(Node {
                is_connected: get_property(&instance_id, &DEVPKEY_DEVICE_IS_CONNECTED)
                    .and_then(|v| v.as_bool()),
                battery_percent: get_property(&instance_id, &DEVPKEY_BLUETOOTH_BATTERY)
                    .and_then(|v| v.as_percent()),
                vendor_id: vendor_id(&instance_id),
                product_id: product_id(&instance_id),
                friendly_name,
                instance_id,
            });
        }
    }

    // Fold each device address into a single set of best-known values.
    let mut battery_by_address: BTreeMap<String, u8> = BTreeMap::new();
    let mut vendor_by_address: BTreeMap<String, u16> = BTreeMap::new();
    let mut product_by_address: BTreeMap<String, u16> = BTreeMap::new();
    let mut connected_by_address: BTreeMap<String, bool> = BTreeMap::new();
    let mut services_by_address: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for n in &nodes {
        let Some(addr) = device_address(&n.instance_id) else {
            continue;
        };
        if let Some(uuid) = service_uuid(&n.instance_id) {
            let list = services_by_address.entry(addr.clone()).or_default();
            if !list.contains(&uuid) {
                list.push(uuid);
            }
        }
        if let Some(b) = n.battery_percent {
            battery_by_address.entry(addr.clone()).or_insert(b);
        }
        if let Some(v) = n.vendor_id {
            vendor_by_address.entry(addr.clone()).or_insert(v);
        }
        if let Some(p) = n.product_id {
            product_by_address.entry(addr.clone()).or_insert(p);
        }
        // Any node reporting connected means the device is connected.
        if n.is_connected == Some(true) {
            connected_by_address.insert(addr, true);
        }
    }

    let mut out: BTreeMap<String, PnpDevice> = BTreeMap::new();
    for n in nodes {
        let dev = PnpDevice {
            friendly_name: n.friendly_name,
            is_connected: n.is_connected,
            battery_percent: n.battery_percent,
            vendor_id: n.vendor_id,
            product_id: n.product_id,
            instance_id: n.instance_id,
            service_uuids: Vec::new(),
        };
        if !dev.is_top_level() {
            continue;
        }
        let enriched = match device_address(&dev.instance_id) {
            Some(addr) => PnpDevice {
                battery_percent: dev.battery_percent.or_else(|| battery_by_address.get(&addr).copied()),
                vendor_id: dev.vendor_id.or_else(|| vendor_by_address.get(&addr).copied()),
                product_id: dev
                    .product_id
                    .or_else(|| product_by_address.get(&addr).copied()),
                is_connected: match connected_by_address.get(&addr) {
                    Some(true) => Some(true),
                    _ => dev.is_connected,
                },
                service_uuids: services_by_address.remove(&addr).unwrap_or_default(),
                ..dev
            },
            None => dev,
        };
        out.insert(enriched.instance_id.clone(), enriched);
    }

    out.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(instance_id: &str) -> PnpDevice {
        PnpDevice {
            instance_id: instance_id.to_string(),
            friendly_name: None,
            is_connected: None,
            battery_percent: None,
            vendor_id: None,
            product_id: None,
            service_uuids: Vec::new(),
        }
    }

    #[test]
    fn extracts_the_service_uuid_of_a_profile_node() {
        assert_eq!(
            service_uuid(
                r"BTHENUM\{0000111E-0000-1000-8000-00805F9B34FB}_VID&0001009E_PID&4075\7&78167D1&0&E458BCF9F02E_C00000000"
            )
            .as_deref(),
            Some("0000111E-0000-1000-8000-00805F9B34FB")
        );
        // Case is normalised: Windows is not consistent about it.
        assert_eq!(
            service_uuid(r"bthenum\{00000000-deca-fade-deca-deafdecacaff}_VID&0001009E_PID&4075\x")
                .as_deref(),
            Some("00000000-DECA-FADE-DECA-DEAFDECACAFF")
        );
    }

    #[test]
    fn nodes_without_a_service_have_no_service_uuid() {
        assert_eq!(
            service_uuid(r"BTHENUM\DEV_E458BCF9F02E\7&78167D1&0&BLUETOOTHDEVICE_E458BCF9F02E"),
            None
        );
        // LE service nodes are a different shape and a different namespace.
        assert_eq!(
            service_uuid(r"BTHLEDEVICE\{0000180F-0000-1000-8000-00805F9B34FB}_DEV_VID&0017EF"),
            None
        );
        assert_eq!(service_uuid(r"BTHENUM\{NOT-A-UUID}_VID&0001009E"), None);
    }

    /// A GUID's first three fields are stored little-endian and the rest in
    /// order, so the bytes do not read left to right.
    #[test]
    fn a_guid_property_is_decoded_field_by_field() {
        let bytes = [
            0x06, 0xD2, 0x7E, 0x8C, // 8C7ED206
            0x8A, 0x3F, // 3F8A
            0x27, 0x48, // 4827
            0xB3, 0xAB, 0xAE, 0x9E, 0x1F, 0xAE, 0xFC, 0x6C,
        ];
        assert_eq!(
            guid_from_bytes(&bytes).as_deref(),
            Some("8C7ED206-3F8A-4827-B3AB-AE9E1FAEFC6C")
        );
        assert_eq!(guid_from_bytes(&bytes[..15]), None, "too short to be a GUID");
    }

    #[test]
    fn uuids_are_formatted_the_way_windows_writes_them() {
        assert_eq!(
            format_uuid(0x00000000_deca_fade_deca_deafdecacaff),
            "00000000-DECA-FADE-DECA-DEAFDECACAFF"
        );
        assert_eq!(
            format_uuid(0x00001101_0000_1000_8000_00805F9B34FB),
            "00001101-0000-1000-8000-00805F9B34FB"
        );
    }

    #[test]
    fn advertised_services_are_matched_by_uuid() {
        let mut d = dev(r"BTHENUM\DEV_E458BCF9F02E\7&78167D1&0&BLUETOOTHDEVICE_E458BCF9F02E");
        d.service_uuids = vec![
            "0000111E-0000-1000-8000-00805F9B34FB".into(),
            "00000000-DECA-FADE-DECA-DEAFDECACAFF".into(),
        ];
        assert!(d.advertises_service(0x00000000_deca_fade_deca_deafdecacaff));
        assert!(!d.advertises_service(0x00001101_0000_1000_8000_00805F9B34FB));
    }

    #[test]
    fn parses_the_product_id_of_the_test_headphones() {
        // 0x4075 is "prince", QuietComfort Headphones, in the BMAP catalog.
        assert_eq!(
            product_id(
                r"BTHENUM\{0000110B-0000-1000-8000-00805F9B34FB}_VID&0001009E_PID&4075\7&78167D1&0&E458BCF9F02E_C00000000"
            ),
            Some(0x4075)
        );
    }

    /// The top-level device node carries no VID/PID at all; the value has to
    /// be folded in from the profile child nodes.
    #[test]
    fn top_level_node_has_no_product_id_of_its_own() {
        assert_eq!(
            product_id(r"BTHENUM\DEV_E458BCF9F02E\7&78167D1&0&BLUETOOTHDEVICE_E458BCF9F02E"),
            None
        );
    }

    #[test]
    fn product_id_is_ignored_without_a_sig_vendor_id() {
        assert_eq!(product_id(r"BTHLEDEVICE\{0000180F-...}_DEV_VID&0002009E_PID&6134"), None);
    }

    // Instance ids below were captured from a real Bose QuietComfort
    // (user-renamed to "Aurora") connected to the development machine.

    #[test]
    fn extracts_address_from_every_node_shape() {
        assert_eq!(
            device_address("BTHENUM\\DEV_E458BCF9F02E\\7&78167D1&0&BLUETOOTHDEVICE_E458BCF9F02E")
                .as_deref(),
            Some("E458BCF9F02E")
        );
        // The HFP child node — the one that actually carries the battery.
        assert_eq!(
            device_address(
                "BTHENUM\\{0000111E-0000-1000-8000-00805F9B34FB}_VID&0001009E_PID&4075\\7&78167D1&0&E458BCF9F02E_C00000000"
            )
            .as_deref(),
            Some("E458BCF9F02E")
        );
        assert_eq!(
            device_address("BTHLE\\DEV_79C657FDB4BC\\7&1E36B139&0&79C657FDB4BC").as_deref(),
            Some("79C657FDB4BC")
        );
    }

    /// Service UUIDs contain 12-hex runs of their own, and they appear before
    /// the address. The SIG base UUID ends `-00805F9B34FB`; taking the first
    /// run rather than the last returns that instead of the device.
    #[test]
    fn sig_base_uuid_is_not_mistaken_for_an_address() {
        let id = "BTHENUM\\{0000110B-0000-1000-8000-00805F9B34FB}_VID&0001009E_PID&4075\\7&78167D1&0&E458BCF9F02E_C00000000";
        assert_eq!(device_address(id).as_deref(), Some("E458BCF9F02E"));
    }

    /// The vendor RFCOMM service observed on the test headphones ends
    /// `-C4714A518BCC`, which is also a 12-hex run.
    #[test]
    fn vendor_uuid_is_not_mistaken_for_an_address() {
        let id = "BTHENUM\\{9B26D8C0-A8ED-440B-95B0-C4714A518BCC}_VID&0001009E_PID&4075\\7&78167D1&0&E458BCF9F02E_C00000000";
        assert_eq!(device_address(id).as_deref(), Some("E458BCF9F02E"));
    }

    /// On BLE service nodes the address sits mid-string, not in the final
    /// path segment, so segment-based extraction would miss it.
    #[test]
    fn extracts_address_from_mid_string_ble_service_node() {
        let id = "BTHLEDEVICE\\{0000180F-0000-1000-8000-00805F9B34FB}_DEV_VID&0017EF_PID&6134_REV&0026_79C657FDB4BC\\8&30736FC1&0&0020";
        assert_eq!(device_address(id).as_deref(), Some("79C657FDB4BC"));
    }

    #[test]
    fn parses_bluetooth_sig_vendor_id() {
        // 0x009E is Bose Corporation in the Bluetooth SIG company list.
        assert_eq!(
            vendor_id(
                "BTHENUM\\{0000110B-0000-1000-8000-00805F9B34FB}_VID&0001009E_PID&4075\\7&78167D1&0&E458BCF9F02E_C00000000"
            ),
            Some(0x009E)
        );
    }

    /// A USB-IF vendor id lives in a different namespace and must not be
    /// compared against SIG company identifiers.
    #[test]
    fn usb_namespace_vendor_ids_are_ignored() {
        assert_eq!(vendor_id("BTHLEDEVICE\\{0000180F-...}_DEV_VID&0002009E_PID&6134"), None);
        assert_eq!(vendor_id("BTHENUM\\DEV_E458BCF9F02E\\7&78167D1&0"), None);
    }

    #[test]
    fn top_level_classic_and_le_nodes_are_recognised() {
        assert!(dev("BTHENUM\\DEV_E458BCF9F02E\\7&78167D1&0&BLUETOOTHDEVICE_E458BCF9F02E")
            .is_top_level());
        assert!(dev("BTHLE\\DEV_79C657FDB4BC\\7&1E36B139&0&79C657FDB4BC").is_top_level());
    }

    /// Per-profile child nodes must be filtered out, otherwise a single pair of
    /// headphones appears as half a dozen separate devices.
    #[test]
    fn per_profile_child_nodes_are_excluded() {
        assert!(!dev(
            "BTHENUM\\{0000110C-0000-1000-8000-00805F9B34FB}_VID&00010075_PID&0100\\7&78167D1&0&34F043C9E0F6_C00000000"
        )
        .is_top_level());
        assert!(!dev(
            "BTHLEDEVICE\\{0000180F-0000-1000-8000-00805F9B34FB}_DEV_VID&0017EF_PID&6134_REV&0026_79C657FDB4BC\\8&30736FC1&0&0020"
        )
        .is_top_level());
    }

    #[test]
    fn percent_accepts_byte_and_bounded_u32() {
        assert_eq!(PropValue::Byte(60).as_percent(), Some(60));
        assert_eq!(PropValue::Number(60).as_percent(), Some(60));
        // A UINT32 outside 0-100 is not a percentage; refuse rather than clamp.
        assert_eq!(PropValue::Number(4294967295).as_percent(), None);
        assert_eq!(PropValue::Text("60".into()).as_percent(), None);
    }
}
