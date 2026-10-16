//! RFC 3339 UTC timestamps for journal settlement times.
//!
//! Writing uses `YYYY-MM-DDTHH:MM:SS[.fraction]Z` with nanosecond precision and no
//! trailing fractional zeros, so every written value restores the same instant.
//! Reading also accepts numeric offsets and rejects leap seconds and years
//! outside `0000`–`9999`.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::JsonError;

const SECONDS_PER_DAY: i64 = 86_400;

#[review_kit_observation::operations]
#[helper]
pub(super) fn format(time: SystemTime) -> String {
    let (seconds, nanoseconds) = match time.duration_since(UNIX_EPOCH) {
        Ok(after) => (
            i64::try_from(after.as_secs()).unwrap_or(i64::MAX),
            after.subsec_nanos(),
        ),
        Err(before) => {
            let before = before.duration();
            let whole = i64::try_from(before.as_secs()).unwrap_or(i64::MAX);
            match before.subsec_nanos() {
                0 => (-whole, 0),
                fraction => (-whole - 1, 1_000_000_000 - fraction),
            }
        }
    };
    let days = seconds.div_euclid(SECONDS_PER_DAY);
    let second_of_day = seconds.rem_euclid(SECONDS_PER_DAY);
    let (year, month, day) = civil_from_days(days);
    let mut text = format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
        second_of_day / 3600,
        second_of_day % 3600 / 60,
        second_of_day % 60
    );
    if nanoseconds != 0 {
        let fraction = format!("{nanoseconds:09}");
        text.push('.');
        text.push_str(fraction.trim_end_matches('0'));
    }
    text.push('Z');
    text
}

#[review_kit_observation::operations]
#[helper]
pub(super) fn parse(text: &str) -> Result<SystemTime, JsonError> {
    let bytes = text.as_bytes();
    if !text.is_ascii()
        || bytes.len() < 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return Err(JsonError::InvalidValue);
    }
    let year = digits(&text[0..4])?;
    let month = digits(&text[5..7])?;
    let day = digits(&text[8..10])?;
    let hour = digits(&text[11..13])?;
    let minute = digits(&text[14..16])?;
    let second = digits(&text[17..19])?;
    if !(1..=12).contains(&month)
        || day < 1
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return Err(JsonError::InvalidValue);
    }
    let mut rest = &text[19..];
    let mut nanoseconds = 0;
    if let Some(fraction) = rest.strip_prefix('.') {
        let length = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if length == 0 || length > 9 {
            return Err(JsonError::InvalidValue);
        }
        nanoseconds = digits(&fraction[..length])? * 10_i64.pow(9 - length as u32);
        rest = &fraction[length..];
    }
    let offset = match rest.as_bytes() {
        [b'Z'] => 0,
        [sign @ (b'+' | b'-'), _, _, b':', _, _] => {
            let hours = digits(&rest[1..3])?;
            let minutes = digits(&rest[4..6])?;
            if hours > 23 || minutes > 59 {
                return Err(JsonError::InvalidValue);
            }
            let magnitude = hours * 3600 + minutes * 60;
            if *sign == b'+' { magnitude } else { -magnitude }
        }
        _ => return Err(JsonError::InvalidValue),
    };
    let seconds =
        days_from_civil(year, month, day) * SECONDS_PER_DAY + hour * 3600 + minute * 60 + second
            - offset;
    let nanoseconds = u32::try_from(nanoseconds).map_err(|_| JsonError::InvalidValue)?;
    let instant = if seconds >= 0 {
        UNIX_EPOCH.checked_add(Duration::new(seconds.unsigned_abs(), nanoseconds))
    } else {
        UNIX_EPOCH
            .checked_sub(Duration::from_secs(seconds.unsigned_abs()))
            .and_then(|time| time.checked_add(Duration::from_nanos(u64::from(nanoseconds))))
    };
    instant.ok_or(JsonError::InvalidValue)
}

#[review_kit_observation::operations]
#[helper]
fn digits(text: &str) -> Result<i64, JsonError> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(JsonError::InvalidValue);
    }
    text.parse().map_err(|_| JsonError::InvalidValue)
}

#[review_kit_observation::operations]
#[helper]
fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 in the proleptic Gregorian calendar.
#[review_kit_observation::operations]
#[helper]
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[review_kit_observation::operations]
#[helper]
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::{format, parse};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn written_instants_restore_exactly() -> Result<(), crate::JsonError> {
        for instant in [
            UNIX_EPOCH,
            UNIX_EPOCH + Duration::new(1_791_893_400, 0),
            UNIX_EPOCH + Duration::new(951_782_400, 123_000_000),
            UNIX_EPOCH + Duration::new(4_102_444_799, 999_999_999),
            UNIX_EPOCH - Duration::new(86_401, 500),
        ] {
            assert_eq!(parse(&format(instant))?, instant, "{}", format(instant));
        }
        assert_eq!(
            format(UNIX_EPOCH + Duration::new(1_791_893_400, 0)),
            "2026-10-13T12:10:00Z"
        );
        assert_eq!(
            format(UNIX_EPOCH + Duration::new(951_782_400, 123_000_000)),
            "2000-02-29T00:00:00.123Z"
        );
        Ok(())
    }

    #[test]
    fn offsets_convert_and_invalid_calendar_values_reject() -> Result<(), crate::JsonError> {
        let utc: SystemTime = parse("2026-10-13T12:10:00Z")?;
        assert_eq!(parse("2026-10-13T14:10:00+02:00")?, utc);
        assert_eq!(parse("2026-10-13T11:40:00-00:30")?, utc);
        for text in [
            "2026-02-29T00:00:00Z",
            "2026-13-01T00:00:00Z",
            "2026-10-13T24:00:00Z",
            "2026-10-13T12:10:60Z",
            "2026-10-13T12:10:00",
            "2026-10-13 12:10:00Z",
            "2026-10-13T12:10:00.Z",
            "2026-10-13T12:10:00.1234567890Z",
            "2026-10-13T12:10:00+2:00",
            "+2026-10-13T12:10:00Z",
            "2026-10-1\u{e9}T12:10:00Z",
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
        Ok(())
    }
}
