//! The Bose headphones and earbuds Void knows of, and how much is known about
//! each.
//!
//! The list is upstream's catalogue of Bose products, keyed by the product id
//! a device reports. Speakers are left out: Void is for things you wear.
//!
//! What can be done with a model depends on what is known about its control
//! protocol, and that comes in three degrees:
//!
//! * **Tested** — run against the physical model by this project, every
//!   feature proven on it. One model: the QuietComfort Headphones.
//! * **Documented** — upstream describes its protocol layout, from people who
//!   have the model. Void uses that description; it has never been run here.
//! * **Recognized** — known to be a Bose model of this kind, with nothing
//!   written down about its controls. Void asks it the questions every
//!   documented model of its generation answers the same way — its name, its
//!   equalizer, multipoint, its list of noise modes — and offers what it
//!   answers. Nothing is written to it whose layout is not known.
//!
//! In every case a change counts only when the device reports it, so a model
//! that does not behave as described shows a change as unconfirmed rather
//! than as made. The degree is shown to the user.

use bmap::catalog::{self, Category as Kind};
use bmap::device::{
    parse_mode_config_44, parse_mode_config_prince, parse_mode_config_qc_ultra2, Addr,
    DeviceConfig, DeviceInfo, ModeConfig,
};
use serde::{Deserialize, Serialize};

/// What kind of thing a model is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    Headphones,
    Earbuds,
}

/// How much is known about a model's controls. See the module notes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Support {
    Tested,
    Documented,
    Recognized,
}

/// One model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Model {
    pub product_id: u16,
    /// Bose's name for it, from the catalogue.
    pub name: &'static str,
    pub category: Category,
    pub support: Support,
}

/// Product ids this project has run against the physical device.
const TESTED: &[u16] = &[
    0x4075, // QuietComfort Headphones ("prince")
];

/// The model a product id belongs to, if it is Bose headphones or earbuds.
/// Whether the catalogue has the product as a speaker: not something worn,
/// and not something Void lists.
pub fn is_speaker(product_id: u16) -> bool {
    catalog::lookup_device(product_id).is_some_and(|entry| entry.category == Kind::Speaker)
}

pub fn model_for(product_id: u16) -> Option<Model> {
    let entry = catalog::lookup_device(product_id)?;
    let category = match entry.category {
        Kind::Headphones => Category::Headphones,
        Kind::Earbuds => Category::Earbuds,
        Kind::Speaker => return None,
    };
    let support = if TESTED.contains(&product_id) {
        Support::Tested
    } else if entry.config.is_some() {
        Support::Documented
    } else {
        Support::Recognized
    };
    Some(Model {
        product_id,
        name: entry.name,
        category,
        support,
    })
}

/// Every Bose headphone and earbud model Void knows of, in catalogue order.
pub fn all_models() -> Vec<Model> {
    catalog::CATALOG
        .iter()
        .filter_map(|entry| model_for(entry.product_id))
        .collect()
}

/// The control-protocol description Void uses for a model, if it can drive
/// one at all.
///
/// A documented model gets upstream's description, under the catalogue's
/// name for the product (two products share one description). A recognized
/// one gets [`generic_config`]. A model needs a way to read its noise
/// cancellation: a current-mode function, or — the QuietComfort 35
/// generation — a noise-cancellation level (see [`uses_levels`]).
pub fn config_for(product_id: u16) -> Option<DeviceConfig> {
    if is_speaker(product_id) {
        return None;
    }
    let (Some(model), Some(entry)) = (model_for(product_id), catalog::lookup_device(product_id))
    else {
        // A Bose product the catalogue does not have — newer than it, most
        // likely. It is only asked when it speaks the protocol, so it is
        // driven the cautious way: what it answers is read and offered, and
        // nothing is written into its modes.
        return Some(generic_config(UNLISTED, "unlisted"));
    };
    let mut config = match entry.config {
        Some(key) => bmap::devices::get_device(key)?,
        None => generic_config(entry.name, entry.codename),
    };
    config.info.name = model.name;
    // A model given another's description keeps its own codename, so that
    // what was checked on that other model is not taken as checked on it.
    config.info.codename = entry.codename;
    if config.current_mode.is_none() && config.anr.is_none() {
        return None;
    }
    Some(config)
}

/// Whether the model's noise cancellation is a level — High, Low, Off —
/// rather than a choice of modes: the QuietComfort 35 generation. See
/// [`super::anr`].
pub fn uses_levels(config: &DeviceConfig) -> bool {
    config.current_mode.is_none() && config.anr.is_some()
}

/// What a Bose product the catalogue does not have is called, until it says
/// its own name.
pub const UNLISTED: &str = "Bose device";

/// The codename a generic description carries, so that the rest of the
/// control link can tell it from a documented one.
pub const GENERIC_PLATFORM: &str = "generic";

/// Whether a description is the generic one.
pub fn is_generic(config: &DeviceConfig) -> bool {
    config.info.platform == GENERIC_PLATFORM
}

/// The description used for a recognized model: the functions every
/// documented model of the current generation has at the same addresses,
/// and nothing else.
///
/// * Firmware `[0.5]`, name `[1.2]`, equalizer `[1.7]`, multipoint `[1.10]`,
///   battery `[2.2]`, the mode list `[31.1]`, the current mode `[31.3]` and a
///   mode's configuration `[31.6]`.
/// * No presets: which mode is which is read from the device's own list,
///   by name, rather than assumed from another model.
/// * No editable slots and no builder: nothing in a mode's configuration is
///   written without its layout.
/// * The mode list is read with whichever documented layout matches the
///   length the device answers with ([`parse_mode_config_any`]).
pub fn generic_config(name: &'static str, codename: &'static str) -> DeviceConfig {
    DeviceConfig {
        info: DeviceInfo {
            name,
            codename,
            platform: GENERIC_PLATFORM,
        },
        rfcomm_channel: 8,
        init_packet: None,
        battery: Some(Addr(2, 2)),
        battery_components: &[],
        battery_aggregate_id: None,
        battery_aggregate_sources: &[],
        firmware: Some(Addr(0, 5)),
        product_name: Some(Addr(1, 2)),
        voice_prompts: None,
        cnc: None,
        eq: Some(Addr(1, 7)),
        buttons: None,
        multipoint: Some(Addr(1, 10)),
        sidetone: None,
        auto_pause: None,
        auto_answer: None,
        anr: None,
        pairing: None,
        routing: None,
        source: None,
        power: None,
        get_all_modes: Some(Addr(31, 1)),
        current_mode: Some(Addr(31, 3)),
        mode_config: Some(Addr(31, 6)),
        favorites: None,
        audio_settings: None,
        preset_modes: &[],
        editable_slots: &[],
        parse_mode_config: Some(parse_mode_config_any),
        build_mode_config: None,
        supports_anc_toggle: false,
        cnc_direct_setget: false,
    }
}

/// Reads a mode configuration in whichever documented layout has its length:
/// 47 bytes (QuietComfort Headphones), 48 (QuietComfort Ultra, 2nd
/// generation) or 44 (QuietComfort Earbuds). Any other length is not read.
pub fn parse_mode_config_any(payload: &[u8]) -> Option<ModeConfig> {
    match payload.len() {
        47 => parse_mode_config_prince(payload),
        48 => parse_mode_config_qc_ultra2(payload),
        44 => parse_mode_config_44(payload),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tested_model_is_the_quietcomfort_headphones() {
        let model = model_for(0x4075).unwrap();
        assert_eq!(model.name, "QuietComfort Headphones");
        assert_eq!(model.category, Category::Headphones);
        assert_eq!(model.support, Support::Tested);
        let config = config_for(0x4075).unwrap();
        assert_eq!(config.info.codename, "prince");
        assert!(!is_generic(&config));
    }

    /// The models the owner asked for by name.
    #[test]
    fn the_quietcomfort_and_ultra_generations_are_all_known() {
        for (id, name, support) in [
            (0x4075, "QuietComfort Headphones", Support::Tested),
            (0x4083, "QuietComfort Headphones (2nd Gen)", Support::Documented),
            (0x4066, "QuietComfort Ultra Headphones", Support::Recognized),
            (0x4082, "QuietComfort Ultra Headphones (2nd Gen)", Support::Documented),
        ] {
            let model = model_for(id).unwrap_or_else(|| panic!("{id:#06x}"));
            assert_eq!((model.name, model.category, model.support), (name, Category::Headphones, support));
            assert!(config_for(id).is_some(), "{name} can be driven");
        }
        for (id, name, support) in [
            (0x402F, "QuietComfort Earbuds", Support::Documented),
            (0x4064, "QuietComfort Earbuds II", Support::Recognized),
            (0x4072, "QuietComfort Ultra Earbuds", Support::Recognized),
            (0x4062, "QuietComfort Ultra Earbuds (2nd Gen)", Support::Documented),
            (0x4068, "Ultra Open Earbuds", Support::Documented),
        ] {
            let model = model_for(id).unwrap_or_else(|| panic!("{id:#06x}"));
            assert_eq!((model.name, model.category, model.support), (name, Category::Earbuds, support));
            assert!(config_for(id).is_some(), "{name} can be driven");
        }
    }

    /// Two products share upstream's QC Ultra description; each is called
    /// what it is.
    #[test]
    fn a_shared_description_goes_by_the_products_own_name() {
        assert_eq!(config_for(0x4083).unwrap().info.name, "QuietComfort Headphones (2nd Gen)");
        assert_eq!(config_for(0x4082).unwrap().info.name, "QuietComfort Ultra Headphones (2nd Gen)");
    }

    #[test]
    fn speakers_are_not_models_here() {
        assert!(model_for(0xBC59).is_none(), "SoundLink Flex");
        assert!(all_models().iter().all(|m| matches!(m.category, Category::Headphones | Category::Earbuds)));
        assert!(model_for(0x1234).is_none(), "not a Bose product id");
    }

    /// The QC35 generation has no mode function: its noise cancellation is a
    /// level, and it is driven by that.
    #[test]
    fn the_qc35_generation_is_driven_by_its_noise_cancellation_level() {
        let qc35 = model_for(0x4020).unwrap();
        assert_eq!(qc35.name, "QuietComfort 35 II");
        assert_eq!(qc35.support, Support::Documented);
        let config = config_for(0x4020).unwrap();
        assert!(uses_levels(&config));
        assert_eq!(config.info.name, "QuietComfort 35 II");
        assert!(matches!(config.init_packet, Some(Addr(0, 1))), "asked for its version first");
        assert!(uses_levels(&config_for(0x400C).unwrap()), "the first QC35 too");
        assert!(!uses_levels(&config_for(0x4075).unwrap()));
    }

    #[test]
    fn a_recognized_model_is_asked_only_what_every_documented_model_answers_alike() {
        let config = config_for(0x4066).unwrap();
        assert!(is_generic(&config));
        assert_eq!(config.info.name, "QuietComfort Ultra Headphones");
        assert!(config.preset_modes.is_empty(), "which mode is which comes from the device");
        assert!(config.editable_slots.is_empty() && config.build_mode_config.is_none(), "nothing written into a mode");
        assert!(matches!(config.current_mode, Some(Addr(31, 3))));
        assert!(matches!(config.eq, Some(Addr(1, 7))));
    }

    #[test]
    fn a_mode_configuration_is_read_only_in_a_documented_length() {
        let mut prince = vec![0u8; 47];
        prince[0] = 2;
        prince[3] = 1;
        prince[4] = 1;
        prince[6..11].copy_from_slice(b"Focus");
        let parsed = parse_mode_config_any(&prince).unwrap();
        assert_eq!((parsed.mode_idx, parsed.name.as_str(), parsed.editable), (2, "Focus", true));

        let mut ultra = vec![0u8; 48];
        ultra[0] = 2;
        ultra[6..15].copy_from_slice(b"Immersion");
        assert_eq!(parse_mode_config_any(&ultra).unwrap().name, "Immersion");

        for len in [0, 6, 39, 40, 45, 46, 49, 60] {
            assert!(parse_mode_config_any(&vec![0u8; len]).is_none(), "{len}");
        }
    }

    /// Prints the table of what each model is offered, from the same rules
    /// the application uses, for the README.
    #[test]
    #[ignore = "prints the per-model table for the README"]
    fn print_model_table() {
        println!("| Model | Product id | Kind | Tried | Noise control | Custom mode's level | Equalizer | Battery |");
        println!("| --- | --- | --- | --- | --- | --- | --- | --- |");
        let mut models = all_models();
        models.sort_by_key(|m| (m.category as u8, m.name));
        for m in models {
            let config = config_for(m.product_id).expect("every model is driven");
            let generic = is_generic(&config);
            let noise = if uses_levels(&config) {
                "Levels: High, Low, Off".to_string()
            } else if generic {
                "Asked on connection: modes, levels, or none".to_string()
            } else {
                let mut names: Vec<String> = config
                    .preset_modes
                    .iter()
                    .map(|(k, _)| {
                        let mut c = k.chars();
                        c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
                    })
                    .collect();
                if !config.editable_slots.is_empty() {
                    names.push("your custom modes".to_string());
                }
                if names.is_empty() {
                    "Modes, from its own list".to_string()
                } else {
                    format!("Modes: {}", names.join(", "))
                }
            };
            let level = if super::super::custom::can_adjust(&config) { "Yes" } else { "No" };
            let eq = match (config.eq.is_some(), generic) {
                (true, true) => "If it answers",
                (true, false) => "Yes",
                (false, _) => "No",
            };
            let battery = if config.battery_components.is_empty() {
                match m.category {
                    Category::Earbuds => "Level, and Right, Left, Case if it reports them".to_string(),
                    Category::Headphones => "Level".to_string(),
                }
            } else {
                format!(
                    "Level, and {}",
                    config.battery_components.iter().map(|(_, n)| *n).collect::<Vec<_>>().join(", ")
                )
            };
            let kind = match m.category {
                Category::Headphones => "Headphones",
                Category::Earbuds => "Earbuds",
            };
            let tried = match m.support {
                Support::Tested => "Tested",
                Support::Documented => "Described",
                Support::Recognized => "Recognized",
            };
            println!(
                "| {} | `0x{:04X}` | {kind} | {tried} | {noise} | {level} | {eq} | {battery} |",
                m.name, m.product_id
            );
        }
    }
}
