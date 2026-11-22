//! Display formatting: ISK, SP, numbers, durations and dates (local time).

use chrono::{DateTime, Duration, Local, Utc};

/// 1234567 -> "1,234,567"
pub fn int(n: i64) -> String {
    let s = n.unsigned_abs().to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}

/// 1234.5 -> "1,234.50"
pub fn decimal(n: f64, places: usize) -> String {
    let neg = n < 0.0;
    let s = format!("{:.*}", places, n.abs());
    let (whole, frac) = s.split_once('.').unwrap_or((&s, ""));
    let w = int(whole.parse().unwrap_or(0));
    let body = if frac.is_empty() { w } else { format!("{w}.{frac}") };
    if neg {
        format!("-{body}")
    } else {
        body
    }
}

/// Full ISK amount: "1,234,567.89 ISK"
pub fn isk(n: f64) -> String {
    format!("{} ISK", decimal(n, 2))
}

/// Compact ISK: "1.23b", "456.7m", "12.3k"
pub fn isk_short(n: f64) -> String {
    let a = n.abs();
    let (v, suffix) = if a >= 1e12 {
        (n / 1e12, "t")
    } else if a >= 1e9 {
        (n / 1e9, "b")
    } else if a >= 1e6 {
        (n / 1e6, "m")
    } else if a >= 1e3 {
        (n / 1e3, "k")
    } else {
        return format!("{n:.0}");
    };
    format!("{v:.2}{suffix}")
}

/// SP with thousands separators: "12,345,678 SP"
pub fn sp(n: i64) -> String {
    format!("{} SP", int(n))
}

/// Compact SP: "12.3m SP"
pub fn sp_short(n: i64) -> String {
    format!("{} SP", isk_short(n as f64))
}

/// "3d 4h 12m", "4h 12m", "12m 30s"; `parts` caps how many units are shown.
pub fn duration(d: Duration, parts: usize) -> String {
    let mut secs = d.num_seconds().max(0);
    if secs == 0 {
        return "0s".into();
    }
    let units = [(86400 * 365, "y"), (86400, "d"), (3600, "h"), (60, "m"), (1, "s")];
    let mut out = Vec::new();
    for (size, name) in units {
        if secs >= size {
            out.push(format!("{}{}", secs / size, name));
            secs %= size;
        }
        if out.len() >= parts {
            break;
        }
    }
    out.join(" ")
}

pub fn duration_short(d: Duration) -> String {
    duration(d, 2)
}

/// "in 3d 4h" / "2h ago"
pub fn relative(t: DateTime<Utc>) -> String {
    let now = Utc::now();
    if t > now {
        format!("in {}", duration(t - now, 2))
    } else if now - t < Duration::seconds(30) {
        "just now".into()
    } else {
        format!("{} ago", duration(now - t, 2))
    }
}

pub fn time_until(t: DateTime<Utc>) -> String {
    duration(t - Utc::now(), 3)
}

pub fn time_since(t: DateTime<Utc>) -> String {
    duration(Utc::now() - t, 2)
}

/// "2026-11-21 19:42" in local time
pub fn datetime(t: DateTime<Utc>) -> String {
    t.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string()
}

pub fn date(t: DateTime<Utc>) -> String {
    t.with_timezone(&Local).format("%Y-%m-%d").to_string()
}

/// EVE time (UTC): "2026.11.21 18:42"
pub fn eve_time(t: DateTime<Utc>) -> String {
    t.format("%Y.%m.%d %H:%M").to_string()
}

pub fn security(s: f64) -> String {
    format!("{s:.1}")
}

/// Pango-safe text.
pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        assert_eq!(int(1234567), "1,234,567");
        assert_eq!(int(-1000), "-1,000");
        assert_eq!(int(999), "999");
        assert_eq!(decimal(1234.5, 2), "1,234.50");
        assert_eq!(isk_short(1_234_000_000.0), "1.23b");
    }

    #[test]
    fn durations() {
        assert_eq!(duration(Duration::seconds(90061), 3), "1d 1h 1m");
        assert_eq!(duration(Duration::seconds(59), 2), "59s");
    }
}
