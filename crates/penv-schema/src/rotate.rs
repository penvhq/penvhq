//! `@rotate=<duration>`: a reminder, never an enforcement. The clock is the last
//! write of the value, which the binary finds in the cloud or `.penv/config.toml`.

/// A span: `1y6m`, `90d`, `12h`, `30min`, `5s`. Units run largest first, each
/// once: y, m (months), w, d, h, min, s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub years: u32,
    pub months: u32,
    /// Weeks, days, hours, minutes and seconds, folded into seconds.
    pub seconds: u64,
}

/// The longest span a reminder may name: a thousand Gregorian years, in seconds.
const MAX_SECONDS: u64 = 1_000 * YEAR;
const YEAR: u64 = 31_556_952;
const MONTH: u64 = YEAR / 12;

const UNITS: [(&str, u64); 7] = [
    ("y", 0),
    ("m", 0),
    ("w", 604_800),
    ("d", 86_400),
    ("h", 3_600),
    ("min", 60),
    ("s", 1),
];

impl Span {
    pub fn parse(text: &str) -> Option<Span> {
        let mut span = Span::default();
        let mut next = 0usize;
        let mut rest = text;
        if rest.is_empty() {
            return None;
        }
        while !rest.is_empty() {
            let digits = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            if digits == 0 {
                return None;
            }
            let n: u64 = rest[..digits].parse().ok()?;
            rest = &rest[digits..];
            let letters = rest
                .find(|c: char| !c.is_ascii_alphabetic())
                .unwrap_or(rest.len());
            let unit = &rest[..letters];
            rest = &rest[letters..];
            let at = UNITS.iter().position(|(u, _)| *u == unit)?;
            if at < next {
                return None;
            }
            next = at + 1;
            match at {
                0 => span.years = u32::try_from(n).ok()?,
                1 => span.months = u32::try_from(n).ok()?,
                _ => span.seconds = span.seconds.checked_add(n.checked_mul(UNITS[at].1)?)?,
            }
        }
        let total = u64::from(span.years)
            .checked_mul(YEAR)?
            .checked_add(u64::from(span.months).checked_mul(MONTH)?)?
            .checked_add(span.seconds)?;
        (span != Span::default() && total <= MAX_SECONDS).then_some(span)
    }

    /// True when the span is finer than a day, so dates alone cannot hold it.
    pub fn is_sub_day(&self) -> bool {
        !self.seconds.is_multiple_of(86_400)
    }
}

/// Where one key stands against its `@rotate`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rotation {
    /// No write has been recorded, so there is no clock to read.
    Unrecorded,
    /// Rotate by `due` (epoch seconds); `left` is negative once it has passed.
    Due { due: u64, left: i64 },
}

/// The status of a value last written at `written` (epoch seconds) at `now`.
pub fn rotation(span: Span, written: Option<u64>, now: u64) -> Rotation {
    let Some(written) = written else {
        return Rotation::Unrecorded;
    };
    let day = (written / 86_400) as i64;
    let clock = written % 86_400;
    let (y, m, d) = civil_from_days(day);
    let months = y * 12 + (m - 1) + i64::from(span.years) * 12 + i64::from(span.months);
    let (ny, nm) = (months.div_euclid(12), months.rem_euclid(12) + 1);
    let shifted = days_from_civil(ny, nm, d.min(month_length(ny, nm)));
    let due = i128::from(shifted) * 86_400 + i128::from(clock) + i128::from(span.seconds);
    let due = u64::try_from(due.max(0)).unwrap_or(u64::MAX);
    let left = (i128::from(due) - i128::from(now)).clamp(i64::MIN.into(), i64::MAX.into());
    Rotation::Due {
        due,
        left: left as i64,
    }
}

/// `YYYY-MM-DD` (midnight UTC) or an RFC 3339 instant ending in `Z` or an
/// offset such as `+02:00`, as epoch seconds.
pub fn parse_instant(text: &str) -> Option<u64> {
    let text = text.trim();
    let (date, clock) = match text.split_once(['T', 't']) {
        Some((date, clock)) => (date, Some(clock)),
        None => (text, None),
    };
    let mut parts = date.split('-');
    let y = digits(parts.next()?, 4, 4)?;
    let m = digits(parts.next()?, 1, 2)?;
    let d = digits(parts.next()?, 1, 2)?;
    if parts.next().is_some() || y < 1 || !(1..=12).contains(&m) || d < 1 || d > month_length(y, m)
    {
        return None;
    }
    let mut seconds = 0i64;
    if let Some(clock) = clock {
        let (clock, offset) = split_offset(clock)?;
        let mut c = clock.split(':');
        let h = digits(c.next()?, 1, 2)?;
        let mi = digits(c.next()?, 1, 2)?;
        let s = match c.next() {
            Some(s) => {
                let (whole, fraction) = s.split_once('.').unwrap_or((s, "0"));
                digits(fraction, 1, 9)?;
                digits(whole, 1, 2)?
            }
            None => 0,
        };
        if c.next().is_some() || h > 23 || mi > 59 || s > 60 {
            return None;
        }
        seconds = h * 3_600 + mi * 60 + s - offset;
    }
    u64::try_from(days_from_civil(y, m, d) * 86_400 + seconds).ok()
}

/// A clock and its `Z` or `+HH:MM`/`-HH:MM`, as seconds east of UTC.
fn split_offset(clock: &str) -> Option<(&str, i64)> {
    if let Some(clock) = clock.strip_suffix(['Z', 'z']) {
        return Some((clock, 0));
    }
    let (clock, offset) = clock.split_at(clock.rfind(['+', '-'])?);
    let sign = if offset.starts_with('-') { -1 } else { 1 };
    let (h, m) = offset[1..].split_once(':')?;
    let (h, m) = (digits(h, 2, 2)?, digits(m, 2, 2)?);
    if h > 23 || m > 59 {
        return None;
    }
    Some((clock, sign * (h * 3_600 + m * 60)))
}

/// Unsigned decimal digits, `min` to `max` of them.
fn digits(text: &str, min: usize, max: usize) -> Option<i64> {
    if !(min..=max).contains(&text.len()) || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// Epoch seconds as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn instant(epoch: u64) -> String {
    let (y, m, d) = civil_from_days((epoch / 86_400) as i64);
    let s = epoch % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        s / 3_600,
        s % 3_600 / 60,
        s % 60
    )
}

/// Epoch seconds as `YYYY-MM-DD`.
pub fn day_of(epoch: u64) -> String {
    let (y, m, d) = civil_from_days((epoch / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

fn month_length(y: i64, m: i64) -> i64 {
    match m {
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (
        if m <= 2 {
            yoe + era * 400 + 1
        } else {
            yoe + era * 400
        },
        m,
        d,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_run_largest_unit_first() {
        assert_eq!(Span::parse("90d").unwrap().seconds, 90 * 86_400);
        let ym = Span::parse("1y6m").unwrap();
        assert_eq!((ym.years, ym.months, ym.seconds), (1, 6, 0));
        assert_eq!(Span::parse("1h30min").unwrap().seconds, 5_400);
        assert_eq!(Span::parse("5s").unwrap().seconds, 5);
        assert!(Span::parse("12h").unwrap().is_sub_day());
        for bad in [
            "", "d", "6m1y", "1y1y", "90", "3hr", "0d", "1y 6m", "30mins",
        ] {
            assert_eq!(Span::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_month_lands_on_the_last_day_when_the_day_does_not_exist() {
        let jan31 = parse_instant("2026-01-31").unwrap();
        let Rotation::Due { due, left } = rotation(Span::parse("1m").unwrap(), Some(jan31), jan31)
        else {
            panic!()
        };
        assert_eq!(day_of(due), "2026-02-28");
        assert_eq!(left, 28 * 86_400);
    }

    #[test]
    fn left_goes_negative_once_the_moment_passes() {
        let written = parse_instant("2026-09-22T10:00:00Z").unwrap();
        let now = parse_instant("2026-09-22T12:00:00Z").unwrap();
        let Rotation::Due { due, left } = rotation(Span::parse("1h").unwrap(), Some(written), now)
        else {
            panic!()
        };
        assert_eq!(instant(due), "2026-09-22T11:00:00Z");
        assert_eq!(left, -3_600);
        assert_eq!(
            rotation(Span::parse("1y").unwrap(), None, now),
            Rotation::Unrecorded
        );
    }

    #[test]
    fn instants_round_trip() {
        for t in [
            "1970-01-01T00:00:00Z",
            "2024-02-29T23:59:59Z",
            "2026-09-22T14:05:00Z",
        ] {
            assert_eq!(instant(parse_instant(t).unwrap()), t);
        }
        assert_eq!(day_of(parse_instant("2026-09-22").unwrap()), "2026-09-22");
        assert_eq!(parse_instant("2026-02-30"), None);
    }

    #[test]
    fn a_span_past_a_thousand_years_is_refused_and_rotation_never_overflows() {
        assert_eq!(Span::parse("18446744073709551615s"), None);
        assert_eq!(Span::parse("1001y"), None);
        assert_eq!(Span::parse("12001m"), None);
        assert_eq!(Span::parse("365243d"), None);
        assert_eq!(Span::parse("1000y1s"), None);
        let most = Span::parse("1000y").unwrap();
        let written = parse_instant("9999-12-31T23:59:59Z").unwrap();
        assert!(matches!(
            rotation(most, Some(written), 0),
            Rotation::Due { left, .. } if left > 0
        ));
        let late = Span::parse("365000d").unwrap();
        assert!(matches!(
            rotation(late, Some(u64::MAX), u64::MAX),
            Rotation::Due { .. }
        ));
    }

    #[test]
    fn instants_take_offsets_and_refuse_signs_and_years_out_of_range() {
        let utc = parse_instant("2026-09-22T14:05:00Z").unwrap();
        assert_eq!(parse_instant("2026-09-22T16:05:00+02:00"), Some(utc));
        assert_eq!(parse_instant("2026-09-22T09:05:00.25-05:00"), Some(utc));
        for bad in [
            "2026-+9-22",
            "2026-09--2",
            "-2026-09-22",
            "0000-01-01",
            "10000-01-01",
            "99999999999999999-01-01",
            "2026-09-22T14:05:00+2",
            "2026-09-22T14:05:00+24:00",
            "2026-09-22T-1:05:00Z",
        ] {
            assert_eq!(parse_instant(bad), None, "{bad}");
        }
        let last = parse_instant("9999-12-31").unwrap();
        assert_eq!(day_of(last + 86_400), "10000-01-01");
    }
}
