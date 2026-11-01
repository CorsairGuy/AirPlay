//! Карточка MRP станции (`src/airplay_mrp` репозитория yandex_station_airplay,
//! main 8edcadb), перенесённая как есть: `wire.rs`, `schema.rs`, `model.rs`,
//! `server.rs`, `stamp.rs`. Отличия от оригинала только на границе:
//! пути модулей, имя устройства вместо константы «Станция Мини», поток
//! канала 130 вместо TCP от shairport и `control.rs` поверх [`Remote`].
#![allow(dead_code, clippy::all)]

pub mod control;
pub mod model;
#[macro_use]
pub mod stamp;
pub mod schema;
pub mod server;
pub mod wire;
