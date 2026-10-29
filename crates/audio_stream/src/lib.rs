//! Аудиопотоки AirPlay 2: буферизованный (тип 103, AAC по TCP) и realtime
//! (тип 96, ALAC по UDP). Приём, расшифровка, буфер по RTP-меткам и вывод
//! точно ко времени якоря с подстройкой под часы отправителя.

mod net;
mod playout;

pub use net::{Stream, StreamKey, StreamKind, bind_tcp};
pub use playout::{Config, Playout};
