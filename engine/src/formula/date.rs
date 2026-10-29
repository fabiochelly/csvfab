//! Dates as JavaScript holds them: milliseconds since 1970 UTC, NaN when
//! invalid. The formulas build them at midnight UTC from the text of a cell, so
//! a date never shifts with the time zone; only today() and the local getters
//! (getFullYear…) read the zone.

use std::fmt::Write as _;
use std::sync::OnceLock;

pub const DAY: f64 = 86_400_000.0;

/// Days from 1970-01-01 to y-m-d (proleptic Gregorian; m 1..=12).
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + (m <= 2) as i64, m, d)
}

/// TimeClip.
pub fn clip(t: f64) -> f64 {
    if !t.is_finite() || t.abs() > 8.64e15 { f64::NAN } else { t.trunc() + 0.0 }
}

/// MakeDate(MakeDay(y, mon, d), MakeTime(…)): a month or a day out of range carries over.
pub fn make(y: f64, mon: f64, d: f64, h: f64, mi: f64, s: f64, ms: f64) -> f64 {
    if ![y, mon, d, h, mi, s, ms].iter().all(|x| x.is_finite()) {
        return f64::NAN;
    }
    let (y, mon) = (y.trunc(), mon.trunc());
    let ym = y + (mon / 12.0).floor();
    if ym.abs() > 400_000.0 {
        return f64::NAN;
    }
    let mn = mon.rem_euclid(12.0);
    let day = days_from_civil(ym as i64, mn as i64 + 1, 1) as f64 + d.trunc() - 1.0;
    clip(day * DAY + h.trunc() * 3.6e6 + mi.trunc() * 6e4 + s.trunc() * 1e3 + ms.trunc())
}

/// Date.UTC: a year 0 to 99 means 1900 to 1999.
pub fn utc(y: f64, mon: f64, d: f64, h: f64, mi: f64, s: f64) -> f64 {
    let y = if y.is_finite() && (0.0..100.0).contains(&y.trunc()) { 1900.0 + y.trunc() } else { y };
    make(y, mon, d, h, mi, s, 0.0)
}

#[derive(Clone, Copy, Debug)]
pub struct Parts {
    pub y: i64,
    /// 1..=12
    pub mo: u32,
    pub d: u32,
    pub h: u32,
    pub mi: u32,
    pub s: u32,
    pub ms: u32,
    /// 0 Sunday … 6 Saturday
    pub wd: u32,
}

/// The UTC fields of a valid time.
pub fn parts(t: f64) -> Parts {
    let days = (t / DAY).floor();
    let rem = (t - days * DAY) as u64;
    let (y, mo, d) = civil_from_days(days as i64);
    Parts {
        y,
        mo,
        d,
        h: (rem / 3_600_000) as u32,
        mi: (rem / 60_000 % 60) as u32,
        s: (rem / 1000 % 60) as u32,
        ms: (rem % 1000) as u32,
        wd: (days as i64 + 4).rem_euclid(7) as u32,
    }
}

/// The local zone's offset at `t` (ms east of UTC) and its abbreviation.
#[cfg(unix)]
pub fn local_offset(t: f64) -> (f64, String) {
    let secs = (t / 1000.0).floor() as libc::time_t;
    // SAFETY: localtime_r writes into the tm we own; tm_zone points to static zone data.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&secs, &mut tm).is_null() {
            return (0.0, "UTC".into());
        }
        let zone = if tm.tm_zone.is_null() { String::new() } else { std::ffi::CStr::from_ptr(tm.tm_zone).to_string_lossy().into_owned() };
        (tm.tm_gmtoff as f64 * 1000.0, zone)
    }
}

#[cfg(not(unix))]
pub fn local_offset(_t: f64) -> (f64, String) {
    (0.0, "UTC".into())
}

pub fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_millis() as f64)
}

/// today(): the local date, at midnight UTC like every date a formula makes.
pub fn today() -> f64 {
    let n = now();
    ((n + local_offset(n).0) / DAY).floor() * DAY
}

/// Up to ten years ahead a two-digit year is this century, else the last one
/// (birth dates). Read once: localtime takes a lock, and the year seldom turns under a running app.
fn pivot() -> i64 {
    static P: OnceLock<i64> = OnceLock::new();
    *P.get_or_init(|| {
        let n = now();
        parts(n + local_offset(n).0).y % 100 + 10
    })
}

fn digits(b: &[u8], i: usize, max: usize) -> usize {
    b.get(i..).map_or(0, |r| r.iter().take(max).take_while(|c| c.is_ascii_digit()).count())
}

fn num(b: &[u8]) -> i64 {
    b.iter().fold(0, |n, c| n * 10 + (c - b'0') as i64)
}

/// A date cell (parseDateCell, day first): yyyy-mm-dd, dd/mm/yyyy, dd.mm.yy,
/// with an optional time "[ T]hh:mm…". (y, month 1-12, day, the rest).
pub fn parse_cell(s: &str) -> Option<(i64, i64, i64, &str)> {
    let b = s.as_bytes();
    let sep = |c: u8| matches!(c, b'-' | b'/' | b'.');
    let la = digits(b, 0, 5);
    if la == 0 || la > 4 || la >= b.len() || !sep(b[la]) {
        return None;
    }
    let i = la + 1;
    let lb = digits(b, i, 3);
    if lb == 0 || lb > 2 || i + lb >= b.len() || !sep(b[i + lb]) {
        return None;
    }
    let j = i + lb + 1;
    let lc = digits(b, j, 5);
    if lc == 0 || lc > 4 {
        return None;
    }
    let rest = &s[j + lc..];
    // ((?:[ T]\d{1,2}:\d{2}.*)?)$ — `.` stops at line terminators
    if !rest.is_empty() {
        let r = rest.as_bytes();
        let h = digits(r, 1, 3);
        if !matches!(r[0], b' ' | b'T') || h == 0 || h > 2 || r.get(1 + h) != Some(&b':') || digits(r, 2 + h, 2) != 2
            || rest.contains(['\n', '\r', '\u{2028}', '\u{2029}']) {
            return None;
        }
    }
    let (a, bb, c) = (num(&b[..la]), num(&b[i..i + lb]), num(&b[j..j + lc]));
    let (mut y, mo, d) = if la == 4 {
        if lc > 2 {
            return None;
        }
        (a, bb, c)
    } else {
        if la > 2 || (lc != 2 && lc != 4) {
            return None;
        }
        (c, bb, a)
    };
    if la != 4 && lc == 2 {
        y += if y <= pivot() { 2000 } else { 1900 };
    }
    if !(1..=12).contains(&mo) || d < 1 {
        return None;
    }
    let last = parts(utc(y as f64, mo as f64, 0.0, 0.0, 0.0, 0.0)).d as i64;
    if d > last {
        return None;
    }
    Some((y, mo, d, rest))
}

/// fxDate on a text: the date, its time kept (the first hh:mm[:ss] after it).
pub fn from_text(s: &str) -> Option<f64> {
    let (y, mo, d, rest) = parse_cell(s)?;
    let (mut h, mut mi, mut se) = (0.0, 0.0, 0.0);
    let r = rest.as_bytes();
    // (\d{1,2}):(\d{2})(?::(\d{2}))? — its first match
    for k in 0..r.len() {
        let lh = digits(r, k, 2);
        if lh == 0 || r.get(k + lh) != Some(&b':') || digits(r, k + lh + 1, 2) != 2 {
            continue;
        }
        h = num(&r[k..k + lh]) as f64;
        mi = num(&r[k + lh + 1..k + lh + 3]) as f64;
        let p = k + lh + 3;
        if r.get(p) == Some(&b':') && digits(r, p + 1, 2) == 2 {
            se = num(&r[p + 1..p + 3]) as f64;
        }
        break;
    }
    let t = utc(y as f64, (mo - 1) as f64, d as f64, h, mi, se);
    if t.is_nan() { None } else { Some(t) }
}

/// toISOString (valid times only).
pub fn iso(t: f64, out: &mut String) {
    let p = parts(t);
    if (0..=9999).contains(&p.y) {
        let _ = write!(out, "{:04}", p.y);
    } else {
        let _ = write!(out, "{}{:06}", if p.y < 0 { '-' } else { '+' }, p.y.abs());
    }
    let _ = write!(out, "-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z", p.mo, p.d, p.h, p.mi, p.s, p.ms);
}

const WD: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MO: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// Date::toString, in the local zone: "Fri Mar 15 2024 01:00:00 GMT+0100 (CET)"
/// (V8 names the zone in full, which needs ICU; the abbreviation stands in).
pub fn to_string(t: f64, out: &mut String) {
    if t.is_nan() {
        return out.push_str("Invalid Date");
    }
    let (off, zone) = local_offset(t);
    let p = parts(t + off);
    let m = (off / 60_000.0) as i64;
    let _ = write!(out, "{} {} {:02} {:04} {:02}:{:02}:{:02} GMT{}{:02}{:02} ({})", WD[p.wd as usize], MO[p.mo as usize - 1], p.d, p.y,
        p.h, p.mi, p.s, if m < 0 { '-' } else { '+' }, m.abs() / 60, m.abs() % 60, zone);
}

/// Whole months from a to b, negative when b is earlier: 31/01 → 28/02 is 0, a birthday not reached.
pub fn months(a: f64, b: f64) -> f64 {
    let (sign, a, b) = if b < a { (-1.0, b, a) } else { (1.0, a, b) };
    let (pa, pb) = (parts(a), parts(b));
    let mut m = (pb.y - pa.y) * 12 + pb.mo as i64 - pa.mo as i64;
    if pb.d < pa.d {
        m -= 1;
    }
    sign * m as f64
}

/// ISO week number.
pub fn week(t: f64) -> f64 {
    let p = parts(t);
    let wd = if p.wd == 0 { 7 } else { p.wd };
    let th = utc(p.y as f64, (p.mo - 1) as f64, (p.d as i64 + 4 - wd as i64) as f64, 0.0, 0.0, 0.0); // the Thursday of its week
    if th.is_nan() {
        return f64::NAN;
    }
    let jan1 = utc(parts(th).y as f64, 0.0, 1.0, 0.0, 0.0, 0.0);
    (((th - jan1) / DAY + 1.0) / 7.0).ceil()
}

/// n months later; the 31st becomes the month's last day when needed (31 January + 1: 28/29 February).
pub fn add_months(t: f64, n: f64) -> f64 {
    let p = parts(t);
    let (y, m) = (p.y as f64, (p.mo - 1) as f64 + n.trunc());
    let last = utc(y, m + 1.0, 0.0, 0.0, 0.0, 0.0);
    if last.is_nan() {
        return f64::NAN;
    }
    utc(y, m, (p.d.min(parts(last).d)) as f64, p.h as f64, p.mi as f64, p.s as f64)
}

/// fmtDate's pattern: yyyy yy mm dd hh mi ss.
pub fn format(t: f64, pattern: &str) -> String {
    let p = parts(t);
    let mut out = String::with_capacity(pattern.len() + 4);
    let mut rest = pattern;
    while let Some(c) = rest.chars().next() {
        let (n, skip) = match rest.get(..2) {
            _ if rest.starts_with("yyyy") => {
                let _ = write!(out, "{}", p.y);
                rest = &rest[4..];
                continue;
            }
            Some("yy") => (p.y % 100, 2),
            Some("mm") => (p.mo as i64, 2),
            Some("dd") => (p.d as i64, 2),
            Some("hh") => (p.h as i64, 2),
            Some("mi") => (p.mi as i64, 2),
            Some("ss") => (p.s as i64, 2),
            _ => {
                out.push(c);
                rest = &rest[c.len_utf8()..];
                continue;
            }
        };
        let _ = write!(out, "{:02}", n);
        rest = &rest[skip..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_round_trip() {
        for z in [-800_000i64, -1, 0, 1, 19_797, 2_932_896] {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m as i64, d as i64), z);
        }
        assert_eq!(utc(2024.0, 0.0, 31.0 + 29.0 + 1.0, 0.0, 0.0, 0.0), utc(2024.0, 2.0, 1.0, 0.0, 0.0, 0.0));
    }

    #[test]
    fn cells() {
        let iso_of = |s: &str| from_text(s).map(|t| { let mut o = String::new(); iso(t, &mut o); o });
        assert_eq!(iso_of("15/03/2024").as_deref(), Some("2024-03-15T00:00:00.000Z"));
        assert_eq!(iso_of("2024-03-15T10:20:30Z").as_deref(), Some("2024-03-15T10:20:30.000Z"));
        assert_eq!(iso_of("15.03.2024 9:05").as_deref(), Some("2024-03-15T09:05:00.000Z"));
        assert_eq!(iso_of("31/02/2024"), None);
        assert_eq!(iso_of("2024-3-5").as_deref(), Some("2024-03-05T00:00:00.000Z"));
        assert_eq!(iso_of("05/03/99").as_deref(), Some("1999-03-05T00:00:00.000Z"));
        assert_eq!(iso_of("05/03/24").as_deref(), Some("2024-03-05T00:00:00.000Z"));
        assert_eq!(iso_of("2024-03-15x"), None);
        assert_eq!(iso_of("2024"), None);
        assert_eq!(format(from_text("05/03/2024 08:07:06").unwrap(), "yyyy-mm-dd hh:mi:ss yy"), "2024-03-05 08:07:06 24");
        assert_eq!(week(from_text("2024-12-30").unwrap()), 1.0);
        assert_eq!(week(from_text("2021-01-03").unwrap()), 53.0);
        let jan31 = from_text("2024-01-31").unwrap();
        assert_eq!(add_months(jan31, 1.0), from_text("2024-02-29").unwrap());
        assert_eq!(months(jan31, from_text("2024-02-29").unwrap()), 0.0);
        assert_eq!(months(from_text("2024-02-29").unwrap(), jan31), 0.0);
        assert_eq!(months(jan31, from_text("2025-03-01").unwrap()), 13.0);
    }
}
