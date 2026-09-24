use std::cell::Cell;
use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the epoch, injected so freshness and expiry are testable.
pub trait Clock {
    fn now(&self) -> u64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default()
    }
}

/// An ISO-8601 instant the server sent, as seconds since the epoch. Only the
/// RFC 3339 shape the API promises is accepted; anything else is `None`.
pub fn epoch_from_rfc3339(text: &str) -> Option<u64> {
    let text = text.trim();
    let (date, rest) = text.split_once(['T', 't', ' '])?;
    let mut date = date.split('-');
    let year: i64 = date.next()?.parse().ok()?;
    let month: i64 = two(date.next()?)?;
    let day: i64 = two(date.next()?)?;
    // Bounded so a hostile year cannot overflow the day count.
    if date.next().is_some()
        || !(1970..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
    {
        return None;
    }

    let (clock, offset) = split_offset(rest)?;
    let mut clock = clock.split(':');
    let hour: i64 = two(clock.next()?)?;
    let minute: i64 = two(clock.next()?)?;
    let seconds = clock.next().unwrap_or("0");
    let second: i64 = two(seconds.split(['.', ',']).next()?)?;
    if clock.next().is_some() || hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    let stamp = days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second;
    u64::try_from(stamp - offset).ok()
}

/// The trailing zone: `Z`, or `+HH:MM` / `-HH:MM`, as seconds to subtract.
fn split_offset(rest: &str) -> Option<(&str, i64)> {
    if let Some(clock) = rest.strip_suffix(['Z', 'z']) {
        return Some((clock, 0));
    }
    let at = rest.rfind(['+', '-'])?;
    let (clock, zone) = rest.split_at(at);
    let sign = if zone.starts_with('-') { -1 } else { 1 };
    let mut parts = zone[1..].split(':');
    let hours: i64 = two(parts.next()?)?;
    let minutes: i64 = two(parts.next().unwrap_or("0"))?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some((clock, sign * (hours * 3_600 + minutes * 60)))
}

fn two(text: &str) -> Option<i64> {
    let text = text.trim();
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// Days between the epoch and a civil date, by the shift-the-year-to-March rule.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// A clock a test moves by hand.
pub struct Fixed(Cell<u64>);

impl Fixed {
    pub fn new(at: u64) -> Fixed {
        Fixed(Cell::new(at))
    }

    pub fn advance(&self, seconds: u64) {
        self.0.set(self.0.get() + seconds);
    }
}

impl Clock for Fixed {
    fn now(&self) -> u64 {
        self.0.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_iso_instant_is_the_seconds_since_the_epoch() {
        assert_eq!(epoch_from_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            epoch_from_rfc3339("2026-09-08T12:34:56Z"),
            Some(1_788_870_896)
        );
        assert_eq!(
            epoch_from_rfc3339("2026-09-08T12:34:56.512Z"),
            Some(1_788_870_896),
            "fractional seconds are not a second"
        );
        assert_eq!(
            epoch_from_rfc3339("2026-09-08T14:34:56+02:00"),
            Some(1_788_870_896),
            "an offset is taken off"
        );
        assert_eq!(
            epoch_from_rfc3339("2026-09-08T10:34:56-02:00"),
            Some(1_788_870_896)
        );
        assert_eq!(
            epoch_from_rfc3339("2024-02-29T00:00:00Z"),
            Some(1_709_164_800),
            "a leap day counts"
        );
    }

    #[test]
    fn anything_that_is_not_an_instant_is_no_instant() {
        for text in [
            "",
            "tomorrow",
            "2026-09-08",
            "2026-13-08T00:00:00Z",
            "2026-09-08T25:00:00Z",
            "1969-12-31T23:59:59Z",
            "2026-09-08T12:34:56",
            "99999999999999-01-01T00:00:00Z",
            "9223372036854775807-01-01T00:00:00Z",
            "10000-01-01T00:00:00Z",
            "2026-09-08T12:34:56+9999999999999999:00",
        ] {
            assert_eq!(epoch_from_rfc3339(text), None, "{text} parsed");
        }
    }
}
