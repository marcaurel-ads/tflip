//! The time, in the local time zone and in UTC, as the original engine
//! reports it: a Julian day and milliseconds since midnight.

use std::time::SystemTime;

use chrono::{DateTime, Datelike, Local, NaiveDateTime, Timelike, Utc};

/// The time now, where the engine runs (`DATE()`, `NOW()`) and in UTC
/// (`NOW_UTC()`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Now {
    pub local: (i32, u32),
    pub utc: (i32, u32),
}

/// The time now.
pub fn now() -> Now {
    let utc = Utc::now();
    Now {
        local: day_and_ms(&utc.with_timezone(&Local).naive_local()),
        utc: day_and_ms(&utc.naive_utc()),
    }
}

/// The local date of `time` (a file's modification time), as a Julian day.
pub fn local_date(time: SystemTime) -> i32 {
    day_and_ms(&DateTime::<Local>::from(time).naive_local()).0
}

/// For tests comparing a DBF file tflip just wrote with one the original
/// engine wrote on the day it was recorded: copies the date of the last
/// update (header bytes 1 to 3) of `ours` over `recorded`'s when `name` is a
/// `.dbf` file, or the `.dbf.bak` a restructure keeps, and `ours` carries today's local date, or yesterday's for a
/// run across midnight. Any other date in `ours` stays a difference.
#[cfg(any(test, feature = "test-hooks"))]
pub fn adopt_todays_dbf_date(name: &str, ours: &[u8], recorded: &mut [u8]) {
    let name = name.to_ascii_lowercase();
    let dbf = name.ends_with(".dbf") || name.ends_with(".dbf.bak");
    if !dbf || ours.len() < 4 || recorded.len() < 4 {
        return;
    }
    let today = now().local.0;
    let recent = [today, today - 1].into_iter().any(|day| {
        let date = tflip_storage::value::julian_to_ymd(day);
        ours[1..4] == [(date.year - 1900) as u8, date.month as u8, date.day as u8]
    });
    if recent {
        recorded[1..4].copy_from_slice(&ours[1..4]);
    }
}

/// 0001-01-01, day 1 of the common era, is Julian day 1721426.
const JULIAN_OF_CE_DAY_0: i32 = 1_721_425;

fn day_and_ms(time: &NaiveDateTime) -> (i32, u32) {
    let day = time.date().num_days_from_ce() + JULIAN_OF_CE_DAY_0;
    let ms = time.num_seconds_from_midnight() * 1000 + time.nanosecond() / 1_000_000 % 1000;
    (day, ms)
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    #[test]
    fn julian_days_and_milliseconds() {
        let time = NaiveDate::from_ymd_opt(2017, 11, 8)
            .unwrap()
            .and_hms_milli_opt(13, 45, 30, 250);
        assert_eq!(day_and_ms(&time.unwrap()), (2_458_066, 49_530_250));
        let first = NaiveDate::from_ymd_opt(1, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        assert_eq!(day_and_ms(&first), (1_721_426, 0));
        let now = now();
        assert!((now.local.0 - now.utc.0).abs() <= 1);
    }

    #[test]
    fn only_todays_dbf_date_is_adopted() {
        let date = tflip_storage::value::julian_to_ymd(now().local.0);
        let today = [
            3,
            (date.year - 1900) as u8,
            date.month as u8,
            date.day as u8,
        ];
        let mut recorded = [3, 117, 11, 8];
        adopt_todays_dbf_date("t.DBF", &today, &mut recorded);
        assert_eq!(recorded, today);
        let mut recorded = [3, 117, 11, 8];
        adopt_todays_dbf_date("t.dbf.BAK", &today, &mut recorded);
        assert_eq!(recorded, today);
        let mut recorded = [3, 117, 11, 8];
        adopt_todays_dbf_date("t.dbf", &[3, 100, 1, 1], &mut recorded);
        assert_eq!(recorded, [3, 117, 11, 8]);
        adopt_todays_dbf_date("t.adt", &today, &mut recorded);
        assert_eq!(recorded, [3, 117, 11, 8]);
    }
}
