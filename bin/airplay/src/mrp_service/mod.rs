//! Generic in-process MediaRemote card service. Wire/server foundations were
//! ported from yandex_station_airplay; names and client identifiers now belong
//! to a per-card profile. Device routing/lights/Bluetooth stay in adapters.
#![allow(dead_code, clippy::all)]

pub mod control;
pub mod model;
#[macro_use]
pub mod stamp;
pub mod schema;
pub mod server;
pub mod wire;
