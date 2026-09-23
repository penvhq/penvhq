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
        (span != Span::default()).then_some(span)
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
    let due = (shifted as u64) * 86_400 + clock + span.seconds;
    Rotation::Due {
        due,
        left: due as i64 - now as i64,
    }
}

/// `YYYY-MM-DD` (midnight UTC) or `YYYY-MM-DDTHH:MM:SSZ`, as epoch seconds.
pub fn parse_instant(text: &str) -> Option<u64> {
    let text = text.trim();
    let (date, clock) = match text.split_once(['T', 't']) {
        Some((date, clock)) => (date, Some(clock.strip_suffix(['Z', 'z'])?)),
        None => (text, None),
    };
    let mut parts = date.split('-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let d: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&m) || d < 1 || d > month_length(y, m) {
        return None;
    }
    let mut seconds = 0i64;
    if let Some(clock) = clock {
        let mut c = clock.split(':');
        let h: i64 = c.next()?.parse().ok()?;
        let mi: i64 = c.next()?.parse().ok()?;
        let s: i64 = c.next().unwrap_or("0").split('.').next()?.parse().ok()?;
        if c.next().is_some() || h > 23 || mi > 59 || s > 60 {
            return None;
        }
        seconds = h * 3_600 + mi * 60 + s;
    }
    u64::try_from(days_from_civil(y, m, d) * 86_400 + seconds).ok()
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
    instant(epoch)[..10].to_string()
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
}
