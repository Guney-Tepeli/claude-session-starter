//! Parse `claude -p "/usage"` output to extract session reset time.
//!
//! Expected output format:
//! ```text
//! Current session: 68% used · resets Jul 11, 12:30pm (Europe/Istanbul)
//! Current week (all models): 12% used · resets Jul 11, 4pm (Europe/Istanbul)
//! ```

use chrono::{DateTime, Datelike, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone};
use regex::Regex;

#[derive(Debug, Clone)]
pub struct UsageInfo {
    pub session_percent: u32,
    pub reset_time: Option<DateTime<Local>>,
    pub week_percent: Option<u32>,
}

/// Parse the /usage command output and extract session info.
pub fn parse_usage(output: &str) -> Option<UsageInfo> {
    // Match: "Current session: 68% used · resets Jul 11, 12:30pm (..."
    // The · is a middle-dot; use .*? to be flexible
    // Minutes are omitted for on-the-hour resets: "5pm" instead of "5:00pm"
    let session_re = Regex::new(
        r"Current session:\s+(\d+)%\s+used\s+.*?resets\s+(\w+)\s+(\d+),\s*(\d+)(?::(\d+))?(am|pm)",
    )
    .ok()?;

    let caps = session_re.captures(output)?;

    let percent: u32 = caps.get(1)?.as_str().parse().ok()?;
    let month_str = caps.get(2)?.as_str();
    let day: u32 = caps.get(3)?.as_str().parse().ok()?;
    let hour: u32 = caps.get(4)?.as_str().parse().ok()?;
    let minute: u32 = caps
        .get(5)
        .map_or(Some(0), |m| m.as_str().parse().ok())?;
    let ampm = caps.get(6)?.as_str();

    let reset_time = build_datetime(month_str, day, hour, minute, ampm);

    // Week percent (optional)
    let week_re = Regex::new(r"Current week \(all models\):\s+(\d+)%").ok()?;
    let week_percent = week_re
        .captures(output)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse().ok());

    Some(UsageInfo {
        session_percent: percent,
        reset_time,
        week_percent,
    })
}

/// Convert parsed components into a `DateTime<Local>`.
fn build_datetime(
    month_str: &str,
    day: u32,
    hour12: u32,
    minute: u32,
    ampm: &str,
) -> Option<DateTime<Local>> {
    let now = Local::now();
    let year = now.year();

    let month = match month_str.to_lowercase().as_str() {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    };

    // 12-hour → 24-hour conversion
    let hour24 = match (ampm, hour12) {
        ("am", 12) => 0,
        ("am", h) => h,
        ("pm", 12) => 12,
        ("pm", h) => h + 12,
        _ => return None,
    };

    let date = NaiveDate::from_ymd_opt(year, month, day)?;
    let time = NaiveTime::from_hms_opt(hour24, minute, 0)?;
    let naive = NaiveDateTime::new(date, time);

    let result = Local.from_local_datetime(&naive).single()?;

    // Handle year rollover (Dec → Jan)
    if result < now - chrono::Duration::days(180) {
        let date_next = NaiveDate::from_ymd_opt(year + 1, month, day)?;
        let naive_next = NaiveDateTime::new(date_next, time);
        Local.from_local_datetime(&naive_next).single()
    } else {
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_usage_output() {
        let output = r#"You are currently using your subscription to power your Claude Code usage

Current session: 68% used · resets Jul 11, 12:30pm (Europe/Istanbul)
Current week (all models): 12% used · resets Jul 11, 4pm (Europe/Istanbul)
Current week (Fable): 22% used · resets Jul 11, 4pm (Europe/Istanbul)"#;

        let info = parse_usage(output).expect("Should parse");
        assert_eq!(info.session_percent, 68);
        assert!(info.reset_time.is_some());
        assert_eq!(info.week_percent, Some(12));
    }

    #[test]
    fn test_parse_on_the_hour_reset() {
        // CLI omits minutes when the reset is on the hour
        let output = "Current session: 22% used · resets Jul 12, 5pm (Europe/Istanbul)";
        let info = parse_usage(output).expect("Should parse minute-less time");
        assert_eq!(info.session_percent, 22);
        let reset = info.reset_time.expect("Should have reset time");
        assert_eq!(reset.format("%H:%M").to_string(), "17:00");
    }

    #[test]
    fn test_parse_zero_percent() {
        let output = "Current session: 0% used · resets Jul 12, 1:00am (Europe/Istanbul)";
        let info = parse_usage(output).expect("Should parse 0%");
        assert_eq!(info.session_percent, 0);
    }
}
