//! Unix seconds to and from the forms people type and feeds require.
//!
//! Hand-rolled rather than a date crate: the only calendar arithmetic needed is UTC days to a
//! civil date and back, which is a dozen lines (Howard Hinnant's `days_from_civil` and its
//! inverse) and tested against known dates below.

/// `2026-09-29T01:01:43Z`.
pub fn iso8601(unix: i64) -> String {
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

/// `2026-09-29 01:01Z`, for people.
pub fn short(unix: i64) -> String {
    let full = iso8601(unix);
    format!("{} {}Z", &full[..10], &full[11..16])
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = i64::from(if m > 2 { m - 3 } else { m + 9 });
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// When `since` means, relative to `now`: `90m`, `24h`, `7d`, `2026-09-29`,
/// `2026-09-29T01:00:00Z`, or Unix seconds. `None` for anything else, rather than a guess.
pub fn parse_since(since: &str, now: i64) -> Option<i64> {
    let s = since.trim();
    if let Some((n, unit)) = s
        .strip_suffix('m')
        .map(|n| (n, 60))
        .or_else(|| s.strip_suffix('h').map(|n| (n, 3600)))
        .or_else(|| s.strip_suffix('d').map(|n| (n, 86_400)))
    {
        return n.parse::<i64>().ok().map(|n| now - n * unit);
    }
    if let Ok(unix) = s.parse::<i64>() {
        return Some(unix);
    }
    parse_date(s)
}

fn parse_date(s: &str) -> Option<i64> {
    let (date, time) = match s.split_once('T') {
        Some((d, t)) => (d, Some(t.strip_suffix('Z')?)),
        None => (s, None),
    };
    let mut parts = date.split('-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let secs = match time {
        None => 0,
        Some(t) => {
            let mut hms = t.split(':').map(|p| p.parse::<i64>().ok());
            let h = hms.next()??;
            let mi = hms.next().unwrap_or(Some(0))?;
            let se = hms.next().unwrap_or(Some(0))?;
            if !(0..24).contains(&h) || !(0..60).contains(&mi) || !(0..61).contains(&se) {
                return None;
            }
            h * 3600 + mi * 60 + se
        }
    };
    Some(days_from_civil(y, m, d) * 86_400 + secs)
}

/// `48h`, `2h`, `90m`, `3d 1h`: a delay as a person reads it. Zero is `none`, which is what a
/// timelock of zero seconds is.
pub fn duration(secs: u64) -> String {
    if secs == 0 {
        return "none".into();
    }
    let (d, h, m, s) = (
        secs / 86_400,
        (secs % 86_400) / 3600,
        (secs % 3600) / 60,
        secs % 60,
    );
    if d > 0 && h == 0 && m == 0 && s == 0 && d <= 3 {
        return format!("{}h", d * 24);
    }
    let parts: Vec<String> = [(d, "d"), (h, "h"), (m, "m"), (s, "s")]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, u)| format!("{n}{u}"))
        .collect();
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Block 51,927,178's timestamp, read from the chain.
    const SEP29_TRANSFER: i64 = 1_790_643_703;

    #[test]
    fn a_known_block_time_formats_to_its_known_date() {
        assert_eq!(iso8601(SEP29_TRANSFER), "2026-09-29T01:01:43Z");
        assert_eq!(iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(951_782_400), "2000-02-29T00:00:00Z", "a leap day");
        assert_eq!(short(SEP29_TRANSFER), "2026-09-29 01:01Z");
    }

    #[test]
    fn a_date_parses_back_to_the_second_it_names() {
        assert_eq!(parse_since("2026-09-29T01:01:43Z", 0), Some(SEP29_TRANSFER));
        assert_eq!(
            parse_since("2026-09-29", 0),
            Some(SEP29_TRANSFER - 3703),
            "midnight UTC"
        );
        for t in [0, SEP29_TRANSFER, 4_102_444_800] {
            assert_eq!(parse_since(&iso8601(t), 0), Some(t));
        }
    }

    #[test]
    fn a_relative_since_counts_back_from_now() {
        assert_eq!(parse_since("24h", 100_000), Some(100_000 - 86_400));
        assert_eq!(parse_since("7d", 1_000_000), Some(1_000_000 - 604_800));
        assert_eq!(parse_since("90m", 10_000), Some(10_000 - 5_400));
        assert_eq!(parse_since("1790643703", 0), Some(SEP29_TRANSFER));
    }

    #[test]
    fn nonsense_is_refused_rather_than_guessed() {
        for s in [
            "",
            "yesterday",
            "24x",
            "2026-13-01",
            "2026-09-29T25:00Z",
            "2026-09",
        ] {
            assert_eq!(parse_since(s, 0), None, "{s}");
        }
    }

    #[test]
    fn a_delay_reads_the_way_people_say_it() {
        assert_eq!(duration(0), "none");
        assert_eq!(duration(172_800), "48h");
        assert_eq!(duration(7_200), "2h");
        assert_eq!(duration(259_200), "72h");
        assert_eq!(duration(90_061), "1d 1h 1m 1s");
    }
}
