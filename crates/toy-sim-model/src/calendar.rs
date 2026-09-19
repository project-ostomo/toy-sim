use std::time::{SystemTime, UNIX_EPOCH};

pub const FOUR_HUNDRED_YEARS_MS: i64 = 146_097 * 86_400_000;

pub fn from_real_unix_ms(real_unix_ms: i64) -> i64 {
    real_unix_ms.saturating_add(FOUR_HUNDRED_YEARS_MS)
}

pub fn now_unix_ms() -> i64 {
    let real = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_millis().min(i64::MAX as u128) as i64,
        Err(error) => -(error.duration().as_millis().min(i64::MAX as u128) as i64),
    };
    from_real_unix_ms(real)
}

pub fn format_utc(unix_ms: i64) -> String {
    let seconds = unix_ms.div_euclid(1000);
    let days = seconds.div_euclid(86_400);
    let within_day = seconds.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;
    let month = month_from_march + if month_from_march < 10 { 3 } else { -9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    let weekday = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][days.rem_euclid(7) as usize];
    format!(
        "{weekday} {year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        within_day / 3600,
        within_day / 60 % 60,
        within_day % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_centuries_preserve_date_and_weekday_across_leap_days() {
        assert_eq!(format_utc(0), "Thu 1970-01-01 00:00:00 UTC");
        assert_eq!(
            format_utc(FOUR_HUNDRED_YEARS_MS),
            "Thu 2370-01-01 00:00:00 UTC"
        );
        let leap = 951_782_400_000;
        assert_eq!(format_utc(leap), "Tue 2000-02-29 00:00:00 UTC");
        assert_eq!(
            format_utc(leap + FOUR_HUNDRED_YEARS_MS),
            "Tue 2400-02-29 00:00:00 UTC"
        );
        assert_eq!(format_utc(-1000), "Wed 1969-12-31 23:59:59 UTC");
    }
    #[test]
    fn real_calendar_in_2026_becomes_2426_across_midnight_and_year_boundaries() {
        assert_eq!(
            format_utc(from_real_unix_ms(1_789_689_600_000)),
            "Fri 2426-09-18 00:00:00 UTC"
        );
        assert_eq!(
            format_utc(from_real_unix_ms(1_798_761_599_999)),
            "Thu 2426-12-31 23:59:59 UTC"
        );
        assert_eq!(
            format_utc(from_real_unix_ms(1_798_761_600_000)),
            "Fri 2427-01-01 00:00:00 UTC"
        );
        assert_eq!(FOUR_HUNDRED_YEARS_MS / 86_400_000 % 7, 0);
    }

    #[test]
    fn century_exceptions_and_negative_milliseconds_are_formatted_correctly() {
        assert_eq!(
            format_utc(from_real_unix_ms(4_107_542_399_999)),
            "Sun 2500-02-28 23:59:59 UTC"
        );
        assert_eq!(
            format_utc(from_real_unix_ms(4_107_542_400_000)),
            "Mon 2500-03-01 00:00:00 UTC"
        );
        assert_eq!(format_utc(-1), "Wed 1969-12-31 23:59:59 UTC");
    }
}
