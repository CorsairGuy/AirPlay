//! Время UTC для строк журнала: в RAM-журналах станции иначе нет меток времени.
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_utc(secs)
}

/// `2026-10-29 07:40:55Z`; дата по алгоритму days-from-civil (Howard Hinnant).
pub fn format_utc(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

/// `println!` с меткой времени.
#[macro_export]
macro_rules! station_log {
    ($($t:tt)*) => { println!("{} {}", $crate::mrp_service::stamp::now(), format_args!($($t)*)) };
}

/// `eprintln!` с меткой времени.
#[macro_export]
macro_rules! mrp_log {
    ($($t:tt)*) => { eprintln!("{} {}", $crate::mrp_service::stamp::now(), format_args!($($t)*)) };
}

#[cfg(test)]
mod tests {
    #[test]
    fn utc_dates() {
        assert_eq!(super::format_utc(0), "1970-01-01 00:00:00Z");
        assert_eq!(super::format_utc(951782400), "2000-02-29 00:00:00Z");
        assert_eq!(super::format_utc(1793259655), "2026-10-29 07:40:55Z");
    }
}
