//! RFC 3339 timestamps (`2026-11-08T12:34:56.789Z`, `+09:00` offsets) to
//! unix milliseconds, without a date library.

/// Unix milliseconds of an RFC 3339 / ISO 8601 date-time, `None` if it does
/// not parse. Fractions beyond milliseconds are ignored; no offset = UTC.
pub fn parse_rfc3339_ms(s: &str) -> Option<u64> {
    let b = s.trim().as_bytes();
    if b.len() < 19 {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let part = b.get(r)?;
        if part.is_empty() || !part.iter().all(u8::is_ascii_digit) {
            return None;
        }
        std::str::from_utf8(part).ok()?.parse().ok()
    };
    if b[4] != b'-'
        || b[7] != b'-'
        || !matches!(b[10], b'T' | b't' | b' ')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let mut i = 19;
    let mut ms = 0i64;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            if i - start < 3 {
                ms = ms * 10 + (b[i] - b'0') as i64;
            }
            i += 1;
        }
        let digits = (i - start).min(3);
        if digits == 0 {
            return None;
        }
        for _ in digits..3 {
            ms *= 10;
        }
    }
    let offset_min = match b.get(i) {
        None | Some(b'Z') | Some(b'z') => 0,
        Some(sign @ (b'+' | b'-')) => {
            let oh = num(i + 1..i + 3)?;
            let om = if b.get(i + 3) == Some(&b':') {
                num(i + 4..i + 6)?
            } else {
                num(i + 3..i + 5)?
            };
            let m = oh * 60 + om;
            if *sign == b'+' { m } else { -m }
        }
        _ => return None,
    };
    let days = days_from_civil(y, mo, d);
    let secs = days * 86_400 + h * 3600 + mi * 60 + sec - offset_min * 60;
    let total = secs.checked_mul(1000)?.checked_add(ms)?;
    u64::try_from(total).ok()
}

/// Days since 1970-01-01 (proleptic Gregorian), H. Hinnant's algorithm.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_shapes() {
        assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:01.5Z"), Some(1500));
        assert_eq!(
            parse_rfc3339_ms("2026-11-08T12:00:00.123Z"),
            Some(1_794_139_200_123)
        );
        assert_eq!(
            parse_rfc3339_ms("2026-11-08T21:00:00.123456+09:00"),
            Some(1_794_139_200_123)
        );
        assert_eq!(
            parse_rfc3339_ms("2026-11-08T12:00:00"),
            Some(1_794_139_200_000)
        );
        assert_eq!(
            parse_rfc3339_ms("2000-02-29T00:00:00Z"),
            Some(951_782_400_000)
        );
    }

    #[test]
    fn rejects_garbage() {
        for s in [
            "",
            "yesterday",
            "2026-13-01T00:00:00Z",
            "2026-11-08",
            "2026-11-08T12:00:00.Z",
            "2026-11-08T12:00:00Q",
            "1960-01-01T00:00:00Z",
            "２０２６-11-08T12:00:00Z",
        ] {
            assert_eq!(parse_rfc3339_ms(s), None, "{s}");
        }
    }
}
