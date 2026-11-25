//! Совместимость: пульт, MRP и карточка переехали в `media::control` и
//! сервис `remote`.
//!
//! Команды отправителю уходят через канал событий звукового соединения,
//! поэтому владелец у них — медиаслой. Карточка, MRP-служба и очередь
//! вывода живут в `remote`; этот модуль держит прежние имена для адаптеров.

pub use ::media::control::{BPLIST, Command, HostCommands, MrpHandler, MrpService, OCTETS, Remote};
pub use ::remote::{mrp_service, output_control, output_selection};
