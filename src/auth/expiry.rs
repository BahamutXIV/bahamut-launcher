//! Parse auth `expires_at` RFC 3339 UTC strings into epoch milliseconds.
//! Only `Z`/`z` suffixes are accepted; fractional seconds are truncated.

/// Parse an RFC 3339 UTC timestamp into epoch milliseconds; the auth contract is UTC-only and fractional seconds are truncated.
pub fn parse_rfc3339_utc_ms(input: &str) -> Result<u64, ExpiryParseError> {
    let trimmed = input.trim();
    let stripped = trimmed
        .strip_suffix('Z')
        .or_else(|| trimmed.strip_suffix('z'))
        .ok_or(ExpiryParseError::ExpectedUtcSuffix)?;
    let (date, time) = stripped
        .split_once('T')
        .or_else(|| stripped.split_once('t'))
        .ok_or(ExpiryParseError::MissingDateTimeSeparator)?;

    let mut date_parts = date.split('-');
    let year = parse_fixed_component(date_parts.next(), ExpiryComponent::Year, 4)? as i32;
    let month = parse_fixed_component(date_parts.next(), ExpiryComponent::Month, 2)?;
    let day = parse_fixed_component(date_parts.next(), ExpiryComponent::Day, 2)?;
    if date_parts.next().is_some() {
        return Err(ExpiryParseError::UnexpectedTrailing("date"));
    }

    let (hms, fractional) = time.split_once('.').unwrap_or((time, ""));
    if time.contains('.')
        && (fractional.is_empty() || !fractional.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(ExpiryParseError::InvalidFraction);
    }
    let mut hms_parts = hms.split(':');
    let hour = parse_fixed_component(hms_parts.next(), ExpiryComponent::Hour, 2)?;
    let minute = parse_fixed_component(hms_parts.next(), ExpiryComponent::Minute, 2)?;
    let second = parse_fixed_component(hms_parts.next(), ExpiryComponent::Second, 2)?;
    if hms_parts.next().is_some() {
        return Err(ExpiryParseError::UnexpectedTrailing("time"));
    }

    if hour > 23 || minute > 59 || second > 60 {
        return Err(ExpiryParseError::OutOfRange);
    }

    let days = days_since_epoch(year, month, day)?;
    // Fold the permitted leap second `:60` into `:59` for coarse expiry.
    let total_seconds =
        days * 86_400 + u64::from(hour) * 3600 + u64::from(minute) * 60 + u64::from(second.min(59));
    Ok(total_seconds * 1000)
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ExpiryParseError {
    #[error("expected RFC 3339 timestamp to end with Z (UTC)")]
    ExpectedUtcSuffix,
    #[error("expected a T separating date and time")]
    MissingDateTimeSeparator,
    #[error("{0:?} must contain the required number of ASCII digits")]
    InvalidComponent(ExpiryComponent),
    #[error("fractional seconds must contain only ASCII digits")]
    InvalidFraction,
    #[error("missing required component {0:?}")]
    MissingComponent(ExpiryComponent),
    #[error("unexpected trailing data in {0}")]
    UnexpectedTrailing(&'static str),
    #[error("timestamp component out of range")]
    OutOfRange,
    #[error("date before the unix epoch (1970-01-01) is not supported")]
    BeforeEpoch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpiryComponent {
    Year,
    Month,
    Day,
    Hour,
    Minute,
    Second,
}

fn parse_fixed_component(
    part: Option<&str>,
    which: ExpiryComponent,
    width: usize,
) -> Result<u32, ExpiryParseError> {
    let raw = part.ok_or(ExpiryParseError::MissingComponent(which))?;
    if raw.len() != width || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ExpiryParseError::InvalidComponent(which));
    }
    raw.parse::<u32>()
        .map_err(|_| ExpiryParseError::InvalidComponent(which))
}

fn days_since_epoch(year: i32, month: u32, day: u32) -> Result<u64, ExpiryParseError> {
    if year < 1970 {
        return Err(ExpiryParseError::BeforeEpoch);
    }
    if !(1..=12).contains(&month) || day == 0 {
        return Err(ExpiryParseError::OutOfRange);
    }
    const DAYS_IN_MONTH: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let leap = is_leap_year(year);
    let max_day = if month == 2 && leap {
        29
    } else {
        DAYS_IN_MONTH[(month - 1) as usize]
    };
    if day > max_day {
        return Err(ExpiryParseError::OutOfRange);
    }

    let mut days: u64 = 0;
    for y in 1970..year {
        days += if is_leap_year(y) { 366 } else { 365 };
    }
    for m in 1..month {
        let dim = if m == 2 && leap {
            29
        } else {
            DAYS_IN_MONTH[(m - 1) as usize]
        };
        days += u64::from(dim);
    }
    days += u64::from(day - 1);
    Ok(days)
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_timestamp() {
        let ms = parse_rfc3339_utc_ms("2000-01-03T17:35:24Z").unwrap();
        assert_eq!(ms, 946_920_924_000);
    }

    #[test]
    fn unix_epoch_is_zero() {
        let ms = parse_rfc3339_utc_ms("1970-01-01T00:00:00Z").unwrap();
        assert_eq!(ms, 0);
    }

    #[test]
    fn fractional_seconds_are_truncated() {
        let ms = parse_rfc3339_utc_ms("2000-01-03T17:35:24.789Z").unwrap();
        assert_eq!(ms, 946_920_924_000);
    }

    #[test]
    fn leap_second_is_folded_into_fifty_nine() {
        let leap = parse_rfc3339_utc_ms("2000-01-03T17:35:60Z").unwrap();
        let normal = parse_rfc3339_utc_ms("2000-01-03T17:35:59Z").unwrap();
        assert_eq!(leap, normal);
    }

    #[test]
    fn lowercase_t_and_z_are_accepted() {
        let ms = parse_rfc3339_utc_ms("2000-01-03t17:35:24z").unwrap();
        assert_eq!(ms, 946_920_924_000);
    }

    #[test]
    fn leap_day_works() {
        let ms = parse_rfc3339_utc_ms("2000-02-29T00:00:00Z").unwrap();
        assert_eq!(ms, 951_782_400_000);
    }

    #[test]
    fn non_leap_year_feb_29_is_rejected() {
        let err = parse_rfc3339_utc_ms("2001-02-29T00:00:00Z").unwrap_err();
        assert_eq!(err, ExpiryParseError::OutOfRange);
    }

    #[test]
    fn missing_z_is_rejected() {
        let err = parse_rfc3339_utc_ms("2000-01-03T17:35:24").unwrap_err();
        assert_eq!(err, ExpiryParseError::ExpectedUtcSuffix);
    }

    #[test]
    fn explicit_offset_is_rejected() {
        let err = parse_rfc3339_utc_ms("2000-01-03T17:35:24+02:00").unwrap_err();
        assert_eq!(err, ExpiryParseError::ExpectedUtcSuffix);
    }

    #[test]
    fn before_epoch_is_rejected() {
        let err = parse_rfc3339_utc_ms("1969-12-31T23:59:59Z").unwrap_err();
        assert_eq!(err, ExpiryParseError::BeforeEpoch);
    }

    #[test]
    fn out_of_range_month_is_rejected() {
        let err = parse_rfc3339_utc_ms("2000-13-01T00:00:00Z").unwrap_err();
        assert_eq!(err, ExpiryParseError::OutOfRange);
    }

    #[test]
    fn rejects_years_wider_than_rfc3339_four_digits() {
        let err = parse_rfc3339_utc_ms("10000-01-01T00:00:00Z").unwrap_err();
        assert_eq!(
            err,
            ExpiryParseError::InvalidComponent(ExpiryComponent::Year)
        );
    }

    #[test]
    fn rejects_malformed_fractional_seconds() {
        for input in [
            "2000-01-03T17:35:24.Z",
            "2000-01-03T17:35:24.abcZ",
            "2000-01-03T17:35:24.1.2Z",
        ] {
            assert_eq!(
                parse_rfc3339_utc_ms(input).unwrap_err(),
                ExpiryParseError::InvalidFraction,
                "{input}"
            );
        }
    }

    #[test]
    fn garbage_input_is_rejected() {
        assert!(parse_rfc3339_utc_ms("not a date").is_err());
        assert!(parse_rfc3339_utc_ms("").is_err());
    }
}
