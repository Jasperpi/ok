//! Calendar arithmetic and the clock, the same on every system. Times on
//! disk are kept as Windows FILETIME ticks (100 ns since 1601) everywhere,
//! so a folder's state reads the same whichever system wrote it.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 100 ns ticks between 1601 and 1970.
pub(crate) const FILETIME_EPOCH: u64 = 116_444_736_000_000_000;

/// Milliseconds since 1970, UTC.
pub(crate) fn unix_ms_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// FILETIME ticks to milliseconds since 1970.
pub(crate) fn filetime_to_unix_ms(ticks: u64) -> u64 {
    ticks.saturating_sub(FILETIME_EPOCH) / 10_000
}

/// FILETIME ticks as a `SystemTime`, for handing a creation time to std.
#[cfg_attr(unix, allow(dead_code))]
pub(crate) fn filetime_to_system(ticks: u64) -> Option<SystemTime> {
    let since = ticks.checked_sub(FILETIME_EPOCH)?;
    UNIX_EPOCH.checked_add(Duration::from_nanos(since.checked_mul(100)?))
}

/// The UTC calendar fields of an instant, by arithmetic alone.
pub(crate) fn utc_fields(unix_ms: u64) -> (u16, u8, u8, u8, u8, u8) {
    let secs = unix_ms / 1000;
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Civil-from-days (Howard Hinnant's algorithm), valid far beyond any date a file can carry.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let year = (yoe + era * 400 + if month <= 2 { 1 } else { 0 }) as u16;
    (year, month, day, (rem / 3600) as u8, (rem % 3600 / 60) as u8, (rem % 60) as u8)
}

/// The inverse of [`utc_fields`]: milliseconds since 1970 for a UTC calendar
/// time, or `None` for fields that are not a date.
pub(crate) fn unix_ms_from_utc(year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> Option<u64> {
    if !(1970..=9999).contains(&year) || !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let y = year as i64 - if month <= 2 { 1 } else { 0 };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (month as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + hour as i64 * 3600 + minute as i64 * 60 + second as i64;
    u64::try_from(secs).ok().map(|s| s * 1000)
}

/// FILETIME ticks for a `SystemTime`, or 0 before 1601.
#[cfg_attr(windows, allow(dead_code))]
pub(crate) fn system_to_filetime(time: SystemTime) -> u64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(since) => FILETIME_EPOCH.saturating_add((since.as_nanos() / 100) as u64),
        Err(before) => FILETIME_EPOCH.saturating_sub((before.duration().as_nanos() / 100) as u64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_fields_match_known_dates() {
        assert_eq!(utc_fields(0), (1970, 1, 1, 0, 0, 0));
        // 2026-10-11 14:32:05 UTC
        assert_eq!(utc_fields(1_791_729_125_000), (2026, 10, 11, 14, 32, 5));
        // A leap day.
        assert_eq!(utc_fields(1_709_210_096_000), (2024, 2, 29, 12, 34, 56));
    }

    #[test]
    fn filetime_conversions_agree() {
        let ms = 1_791_729_125_123u64;
        let ticks = ms * 10_000 + FILETIME_EPOCH;
        assert_eq!(filetime_to_unix_ms(ticks), ms);
        let system = filetime_to_system(ticks).unwrap();
        assert_eq!(system.duration_since(UNIX_EPOCH).unwrap().as_millis() as u64, ms);
        assert!(filetime_to_system(5).is_none());
    }

    #[test]
    fn calendar_fields_convert_both_ways() {
        for ms in [0u64, 1_791_729_125_000, 1_709_210_096_000, 951_782_400_000, 4_102_444_799_000] {
            let (y, mo, d, h, mi, s) = utc_fields(ms);
            assert_eq!(unix_ms_from_utc(y, mo, d, h, mi, s), Some(ms), "{ms}");
        }
        assert_eq!(unix_ms_from_utc(2026, 13, 1, 0, 0, 0), None);
        assert_eq!(unix_ms_from_utc(1969, 12, 31, 23, 59, 59), None);
        assert_eq!(unix_ms_from_utc(2026, 10, 11, 24, 0, 0), None);
    }

    #[test]
    fn system_times_round_trip_through_filetime() {
        let ms = 1_791_729_125_123u64;
        let system = UNIX_EPOCH + Duration::from_millis(ms);
        assert_eq!(filetime_to_unix_ms(system_to_filetime(system)), ms);
    }
}
