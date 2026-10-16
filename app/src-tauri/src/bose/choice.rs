//! The Bose headphones and earbuds paired with this machine, to choose from.
//!
//! Built from what the system already knows about its paired devices — names,
//! whether each is connected, the product id each was paired with — so on
//! Windows listing them sends nothing to any of them. Android keeps no product
//! id: there a connected device is asked for it once, read-only, and the
//! answer is remembered. Speakers and other devices that
//! are not worn are left out; a Bose device whose product id is not in the
//! catalogue is listed without a model.
//!
//! The id a device is listed under is a salted hash of the system's id for
//! it, as everywhere else: the system's id carries the Bluetooth address,
//! which stays native.

use super::bmap::models;
use super::real::Sighting;
use crate::device::state::ModelState;
use crate::util::stable_id;
use serde::Serialize;

/// One paired Bose device, as the interface lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairedBose {
    /// What choosing it asks for. A salted hash; never the address.
    pub id: String,
    /// The name it goes by: the one its owner gave it.
    pub name: String,
    /// Which model it is. `None` for a Bose device the catalogue does not have.
    pub model: Option<ModelState>,
    pub connected: bool,
    /// The one this application is looking at now.
    pub current: bool,
}

/// A paired device as the system describes it, before it is listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The system's id. Contains the Bluetooth address; stays native.
    pub instance_id: String,
    pub name: String,
    pub product_id: Option<u16>,
    pub connected: bool,
    pub battery: Option<u8>,
}

impl Candidate {
    fn sighting(&self) -> Sighting {
        Sighting {
            instance_id: self.instance_id.clone(),
            name: self.name.clone(),
            connected: self.connected,
            battery: self.battery,
        }
    }
}

/// The devices to choose from: worn ones only, connected first, then the
/// current one, then by name.
pub fn choices(candidates: Vec<Candidate>, current: Option<&str>) -> Vec<PairedBose> {
    let mut listed: Vec<PairedBose> = candidates
        .into_iter()
        .filter(|c| !c.product_id.is_some_and(models::is_speaker))
        .map(|c| PairedBose {
            id: stable_id(&c.instance_id),
            current: current == Some(c.instance_id.as_str()),
            model: c.product_id.and_then(models::model_for).map(|m| ModelState {
                name: m.name.to_string(),
                category: m.category,
                support: m.support,
            }),
            connected: c.connected,
            name: c.name,
        })
        .collect();
    listed.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then(b.current.cmp(&a.current))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    listed
}

/// The candidate listed under `id`, as a sighting to remember.
pub fn sighting_for(candidates: Vec<Candidate>, id: &str) -> Option<Sighting> {
    candidates
        .into_iter()
        .filter(|c| !c.product_id.is_some_and(models::is_speaker))
        .find(|c| stable_id(&c.instance_id) == id)
        .map(|c| c.sighting())
}

/// Whether paired devices can be listed at all: a Bluetooth radio, switched
/// on, and on Android the permission to use it. Without them the list would
/// be empty for a reason that has nothing to do with the headphones.
pub fn ready_to_list() -> Result<(), crate::device::DeviceError> {
    use crate::device::DeviceError;
    #[cfg(windows)]
    {
        if !crate::bluetooth::radio::availability().radio_present {
            return Err(DeviceError::BluetoothUnavailable);
        }
    }
    #[cfg(target_os = "android")]
    {
        let report = crate::android::directory::report().map_err(DeviceError::Platform)?;
        if !report.radio_present {
            return Err(DeviceError::BluetoothUnavailable);
        }
        // The permission first: without it Android does not say whether
        // Bluetooth is on, and it reads as off.
        if !report.permission {
            return Err(DeviceError::NotPermitted(
                "This application has not been allowed to use Bluetooth. Allow \"Nearby                  devices\" for it, and it will find your headphones and earbuds."
                    .to_string(),
            ));
        }
        if !report.radio_enabled {
            return Err(DeviceError::BluetoothUnavailable);
        }
    }
    Ok(())
}

/// The Bose devices paired with this machine, as the system describes them.
/// Only enumerates; nothing is sent to any device.
pub fn paired() -> Vec<Candidate> {
    #[cfg(windows)]
    {
        crate::bluetooth::pnp::enumerate_bluetooth_devices()
            .into_iter()
            .filter(|d| d.is_top_level())
            .filter(|d| {
                super::is_bose_device(d.vendor_id, d.friendly_name.as_deref().unwrap_or_default())
            })
            .map(|d| Candidate {
                name: d
                    .friendly_name
                    .clone()
                    .unwrap_or_else(|| "Bose device".to_string()),
                product_id: d.product_id,
                connected: d.is_connected.unwrap_or(false),
                battery: d.battery_percent,
                instance_id: d.instance_id,
            })
            .collect()
    }
    #[cfg(target_os = "android")]
    {
        use crate::android::directory;
        directory::report()
            .map(|report| report.devices)
            .unwrap_or_default()
            .into_iter()
            .filter(|d| {
                d.advertises_bmap || (!d.low_energy_only && super::looks_like_bose(&d.name))
            })
            .map(|d| Candidate {
                // Android keeps no product id, so a connected device is asked
                // for its model: one read-only request, made once and
                // remembered, as for the device in use. One that is not
                // connected cannot be asked, and is listed without a model.
                product_id: match d.address {
                    Some(address) if d.connected => directory::product_id(address),
                    _ => None,
                },
                instance_id: d.instance_id,
                name: d.name,
                connected: d.connected,
                battery: d.battery,
            })
            .collect()
    }
    #[cfg(not(any(windows, target_os = "android")))]
    {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use models::{Category, Support};

    fn candidate(instance: &str, name: &str, product_id: Option<u16>, connected: bool) -> Candidate {
        Candidate {
            instance_id: instance.to_string(),
            name: name.to_string(),
            product_id,
            connected,
            battery: None,
        }
    }

    fn paired() -> Vec<Candidate> {
        vec![
            candidate("BTHENUM\\DEV_A", "Living room", Some(0x4085), true),
            candidate("BTHENUM\\DEV_B", "Zed's buds", Some(0x4062), false),
            candidate("BTHENUM\\DEV_C", "Aurora", Some(0x4075), true),
            candidate("BTHENUM\\DEV_D", "Old pair", None, false),
            candidate("BTHENUM\\DEV_E", "Commute", Some(0x4082), false),
        ]
    }

    #[test]
    fn worn_devices_are_listed_connected_first_with_their_models() {
        let listed = choices(paired(), Some("BTHENUM\\DEV_E"));
        let names: Vec<&str> = listed.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["Aurora", "Commute", "Old pair", "Zed's buds"], "no speaker");

        let aurora = &listed[0];
        assert!(aurora.connected && !aurora.current);
        let model = aurora.model.as_ref().unwrap();
        assert_eq!(model.name, "QuietComfort Headphones");
        assert_eq!(model.category, Category::Headphones);
        assert_eq!(model.support, Support::Tested);

        assert!(listed[1].current);
        assert!(listed[2].model.is_none(), "a Bose device the catalogue does not have");
        let buds = listed[3].model.as_ref().unwrap();
        assert_eq!(buds.category, Category::Earbuds);
        assert_eq!(buds.support, Support::Documented);
    }

    /// What is listed carries no address, and choosing goes by the same id.
    #[test]
    fn a_device_is_chosen_by_the_id_it_was_listed_under() {
        let listed = choices(paired(), None);
        for device in &listed {
            assert!(!device.id.contains("DEV_"), "{}", device.id);
            assert!(crate::device::state::is_paired_device_id(&device.id), "{}", device.id);
        }
        let commute = listed.iter().find(|d| d.name == "Commute").unwrap();
        let seen = sighting_for(paired(), &commute.id).unwrap();
        assert_eq!(seen.instance_id, "BTHENUM\\DEV_E");
        assert_eq!(seen.name, "Commute");

        let speaker_id = stable_id("BTHENUM\\DEV_A");
        assert!(sighting_for(paired(), &speaker_id).is_none(), "a speaker is not chosen");
        assert!(sighting_for(paired(), "00000000000000000000").is_none());
    }
}
