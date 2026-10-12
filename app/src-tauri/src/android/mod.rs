//! Android: the same application, with Android standing where Windows does.
//!
//! Nothing above this module changes. The control link still validates its
//! endpoint, still reads back every write, and still decides everything; what
//! differs is who carries the bytes and who answers "is it paired, is it
//! connected, where does its audio go". On Windows those are Winsock, PnP and
//! Core Audio. Here they are one small Kotlin class
//! (`gen/android/.../AuroraPlugin.kt`), reached through [`bridge`].
//!
//! | Module | Stands in for |
//! | --- | --- |
//! | [`bridge`] | — the calls into Kotlin |
//! | [`rfcomm`] | `bose::bmap::winsock` |
//! | [`directory`] | `bluetooth::pnp` and `bluetooth::radio` |
//! | [`sound`] | `audio::playback` |
//!
//! The module is also compiled for tests on other systems, so that the parts
//! that are plain logic — parsing an address, recognising the control
//! service, reading a product id out of a reply — are tested where the rest
//! of the suite runs. There the bridge has nothing behind it and says so.
#![cfg(any(target_os = "android", test))]
#![cfg_attr(not(target_os = "android"), allow(dead_code))]

pub mod bridge;
pub mod directory;
pub mod rfcomm;
pub mod sound;
