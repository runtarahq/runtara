//! DateTime agent — date/time manipulation — as a WebAssembly component.
//!
//! Schema matches the legacy `runtara-agents/src/agents/datetime.rs` agent so
//! A/B parity tests can compare results byte-for-byte.
//!
//! Capabilities:
//! - `get-current-date`      – current UTC date/time with optional timezone
//! - `format-date`           – preset or custom Luxon-style format tokens
//! - `add-to-date`           – add years/months/weeks/days/hours/minutes/seconds
//! - `subtract-from-date`    – subtract (delegates to add-to-date with negated amount)
//! - `get-time-between`      – difference in a chosen unit
//! - `extract-date-part`     – extract year/month/week/day/hour/minute/second/…
//! - `round-date`            – floor/ceil/round to a time unit
//! - `date-to-unix`          – convert date string → Unix timestamp
//! - `unix-to-date`          – convert Unix timestamp → ISO 8601 string

#![cfg(target_arch = "wasm32")]

#[allow(warnings)]
mod bindings;

use bindings::exports::runtara::agent::capabilities::{
    CapabilityInfo, ConnectionInfo, ErrorInfo, Guest, ModuleInfo,
};
use chrono::{
    DateTime, Datelike, Duration, FixedOffset, NaiveDateTime, Offset, TimeZone, Timelike, Utc,
};
use chrono_tz::Tz;

// ============================================================================
// Input/output types — mirror the legacy agent structs
// ============================================================================

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetCurrentDateInput {
    #[serde(default = "default_true")]
    include_time: bool,
    #[serde(default)]
    timezone: Option<String>,
}

impl Default for GetCurrentDateInput {
    fn default() -> Self {
        Self {
            include_time: true,
            timezone: None,
        }
    }
}

#[derive(serde::Deserialize, Default)]
struct FormatDateInput {
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    format: DateFormat,
    #[serde(rename = "customFormat", default)]
    custom_format: Option<String>,
    #[serde(default)]
    timezone: Option<String>,
}

#[derive(serde::Deserialize, Default)]
struct AddToDateInput {
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    amount: i64,
    #[serde(default)]
    unit: TimeUnit,
    #[serde(default)]
    timezone: Option<String>,
}

#[derive(serde::Deserialize, Default)]
struct SubtractFromDateInput {
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    amount: i64,
    #[serde(default)]
    unit: TimeUnit,
    #[serde(default)]
    timezone: Option<String>,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct GetTimeBetweenInput {
    #[serde(default)]
    start_date: Option<String>,
    #[serde(default)]
    end_date: Option<String>,
    #[serde(default)]
    unit: TimeUnit,
}

#[derive(serde::Deserialize, Default)]
struct ExtractDatePartInput {
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    part: DatePart,
    #[serde(default)]
    timezone: Option<String>,
}

#[derive(serde::Deserialize, Default)]
struct DateToUnixInput {
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    milliseconds: bool,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct UnixToDateInput {
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    is_milliseconds: Option<bool>,
    #[serde(default)]
    timezone: Option<String>,
}

#[derive(serde::Deserialize, Default)]
struct RoundDateInput {
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    unit: TimeUnit,
    #[serde(default)]
    mode: RoundMode,
    #[serde(default)]
    timezone: Option<String>,
}

// ---- output structs ----

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TimeBetweenResult {
    difference: i64,
    unit: String,
    exact_ms: i64,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct UnixTimestampResult {
    timestamp: i64,
    is_milliseconds: bool,
}

// ============================================================================
// Enums
// ============================================================================

#[derive(serde::Deserialize, Clone, Copy, Default)]
#[serde(rename_all = "kebab-case")]
enum TimeUnit {
    Years,
    Months,
    Weeks,
    #[default]
    Days,
    Hours,
    Minutes,
    Seconds,
}

impl std::fmt::Display for TimeUnit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TimeUnit::Years => write!(f, "years"),
            TimeUnit::Months => write!(f, "months"),
            TimeUnit::Weeks => write!(f, "weeks"),
            TimeUnit::Days => write!(f, "days"),
            TimeUnit::Hours => write!(f, "hours"),
            TimeUnit::Minutes => write!(f, "minutes"),
            TimeUnit::Seconds => write!(f, "seconds"),
        }
    }
}

#[derive(serde::Deserialize, Clone, Copy, Default)]
#[serde(rename_all = "kebab-case")]
enum DatePart {
    Year,
    Month,
    Week,
    #[default]
    Day,
    DayOfWeek,
    DayOfYear,
    Hour,
    Minute,
    Second,
    Millisecond,
    Quarter,
}

#[derive(serde::Deserialize, Clone, Copy, Default)]
#[serde(rename_all = "kebab-case")]
enum RoundMode {
    Floor,
    Ceil,
    #[default]
    Round,
}

#[derive(serde::Deserialize, Clone, Copy, Default)]
#[serde(rename_all = "kebab-case")]
enum DateFormat {
    #[default]
    Iso8601,
    Rfc2822,
    DateOnly,
    TimeOnly,
    UsShortDate,
    EuShortDate,
    LongDate,
    DateTime,
    Unix,
    UnixMs,
    Custom,
}

// ============================================================================
// Helpers
// ============================================================================

fn default_true() -> bool {
    true
}

fn bad_json(e: serde_json::Error) -> ErrorInfo {
    ErrorInfo {
        code: "INPUT_DESERIALIZATION_ERROR".into(),
        message: e.to_string(),
        category: "permanent".into(),
        severity: "error".into(),
        retryable: false,
        retry_after_ms: None,
        attributes: None,
    }
}

fn date_err(msg: impl Into<String>) -> ErrorInfo {
    ErrorInfo {
        code: "DATE_ERROR".into(),
        message: msg.into(),
        category: "permanent".into(),
        severity: "error".into(),
        retryable: false,
        retry_after_ms: None,
        attributes: None,
    }
}

// ---- Luxon → chrono format conversion ----

fn luxon_to_chrono_format(luxon_format: &str) -> String {
    let mut result = String::with_capacity(luxon_format.len() * 2);
    let chars: Vec<char> = luxon_format.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let remaining = &luxon_format[luxon_format.char_indices().nth(i).unwrap().0..];
        if let Some((luxon_len, chrono_token)) = try_match_token(remaining) {
            result.push_str(chrono_token);
            i += luxon_len;
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }
    result
}

fn try_match_token(s: &str) -> Option<(usize, &'static str)> {
    static TOKENS: &[(&str, &str)] = &[
        ("yyyy", "%Y"),
        ("MMMM", "%B"),
        ("EEEE", "%A"),
        ("MMM", "%b"),
        ("EEE", "%a"),
        ("SSS", "%3f"),
        ("ZZZ", "%:z"),
        ("yy", "%y"),
        ("MM", "%m"),
        ("dd", "%d"),
        ("HH", "%H"),
        ("hh", "%I"),
        ("mm", "%M"),
        ("ss", "%S"),
        ("ZZ", "%z"),
        ("a", "%p"),
        ("W", "%W"),
        ("o", "%j"),
        ("Z", "%Z"),
        ("E", "%a"),
    ];
    for (luxon, chrono) in TOKENS {
        if s.starts_with(luxon) {
            return Some((luxon.chars().count(), *chrono));
        }
    }
    None
}

// ---- Date parsing ----

const PARSE_FORMATS: &[&str] = &[
    "%Y-%m-%dT%H:%M:%S%.fZ",
    "%Y-%m-%dT%H:%M:%SZ",
    "%Y-%m-%dT%H:%M:%S%.f%:z",
    "%Y-%m-%dT%H:%M:%S%:z",
    "%Y-%m-%dT%H:%M:%S%.f",
    "%Y-%m-%dT%H:%M:%S",
    "%Y-%m-%d %H:%M:%S%.f",
    "%Y-%m-%d %H:%M:%S",
    "%Y-%m-%d",
    "%m/%d/%y %H:%M:%S",
    "%m/%d/%y",
    "%m/%d/%Y %H:%M:%S",
    "%m/%d/%Y",
    "%d/%m/%y %H:%M:%S",
    "%d/%m/%y",
    "%d.%m.%y %H:%M:%S",
    "%d.%m.%y",
    "%d/%m/%Y %H:%M:%S",
    "%d/%m/%Y",
    "%d.%m.%Y %H:%M:%S",
    "%d.%m.%Y",
];

fn parse_flexible_date(
    date_str: &str,
    timezone: Option<&str>,
) -> Result<DateTime<FixedOffset>, String> {
    let trimmed = date_str.trim();

    // Try Unix timestamp first
    if let Ok(ts) = trimmed.parse::<i64>() {
        let (secs, nanos) = if ts > 1_000_000_000_000 {
            (ts / 1000, ((ts % 1000) * 1_000_000) as u32)
        } else {
            (ts, 0)
        };
        let utc = DateTime::from_timestamp(secs, nanos)
            .ok_or_else(|| format!("Invalid Unix timestamp: {}", ts))?;
        return apply_timezone(utc, timezone);
    }

    if let Ok(dt) = DateTime::parse_from_rfc3339(trimmed) {
        return apply_timezone(dt.with_timezone(&Utc), timezone);
    }

    if let Ok(dt) = DateTime::parse_from_rfc2822(trimmed) {
        return apply_timezone(dt.with_timezone(&Utc), timezone);
    }

    for fmt in PARSE_FORMATS {
        if let Ok(naive) = NaiveDateTime::parse_from_str(trimmed, fmt) {
            let utc = Utc.from_utc_datetime(&naive);
            return apply_timezone(utc, timezone);
        }
    }

    const DATE_ONLY_FORMATS: &[&str] = &[
        "%Y-%m-%d", "%m/%d/%y", "%d/%m/%y", "%d.%m.%y", "%m/%d/%Y", "%d/%m/%Y", "%d.%m.%Y",
    ];
    for fmt in DATE_ONLY_FORMATS {
        if let Ok(naive_date) = chrono::NaiveDate::parse_from_str(trimmed, fmt) {
            let naive = naive_date.and_hms_opt(0, 0, 0).unwrap();
            let utc = Utc.from_utc_datetime(&naive);
            return apply_timezone(utc, timezone);
        }
    }

    Err(format!(
        "Unable to parse date: '{}'. Supported formats: ISO 8601, RFC 2822, Unix timestamp, or common date formats",
        date_str
    ))
}

// ---- Timezone helpers ----

fn parse_timezone(tz_str: &str) -> Result<FixedOffset, String> {
    let trimmed = tz_str.trim();
    if trimmed.eq_ignore_ascii_case("utc") || trimmed == "Z" {
        return Ok(FixedOffset::east_opt(0).unwrap());
    }
    if trimmed.starts_with('+') || trimmed.starts_with('-') {
        return parse_offset(trimmed);
    }
    if let Ok(tz) = trimmed.parse::<Tz>() {
        let now = Utc::now().with_timezone(&tz);
        let fixed = now.offset().fix();
        return Ok(fixed);
    }
    Err(format!(
        "Unknown timezone: '{}'. Use IANA names (e.g., 'America/New_York') or offsets (e.g., '+05:30')",
        trimmed
    ))
}

fn parse_offset(offset_str: &str) -> Result<FixedOffset, String> {
    let sign = if offset_str.starts_with('-') { -1 } else { 1 };
    let without_sign = offset_str.trim_start_matches(['+', '-']);
    let (hours, minutes) = if without_sign.contains(':') {
        let parts: Vec<&str> = without_sign.split(':').collect();
        if parts.len() != 2 {
            return Err(format!("Invalid offset format: {}", offset_str));
        }
        (
            parts[0]
                .parse::<i32>()
                .map_err(|_| format!("Invalid hours in offset: {}", offset_str))?,
            parts[1]
                .parse::<i32>()
                .map_err(|_| format!("Invalid minutes in offset: {}", offset_str))?,
        )
    } else if without_sign.len() == 4 {
        (
            without_sign[0..2]
                .parse::<i32>()
                .map_err(|_| format!("Invalid offset: {}", offset_str))?,
            without_sign[2..4]
                .parse::<i32>()
                .map_err(|_| format!("Invalid offset: {}", offset_str))?,
        )
    } else {
        return Err(format!(
            "Invalid offset format: {}. Use +HH:MM or +HHMM",
            offset_str
        ));
    };
    let total_seconds = sign * (hours * 3600 + minutes * 60);
    FixedOffset::east_opt(total_seconds)
        .ok_or_else(|| format!("Offset out of range: {}", offset_str))
}

fn apply_timezone(
    utc: DateTime<Utc>,
    timezone: Option<&str>,
) -> Result<DateTime<FixedOffset>, String> {
    match timezone {
        Some(tz) if !tz.is_empty() => {
            let offset = parse_timezone(tz)?;
            Ok(utc.with_timezone(&offset))
        }
        _ => Ok(utc.with_timezone(&FixedOffset::east_opt(0).unwrap())),
    }
}

fn format_iso8601(dt: &DateTime<FixedOffset>) -> String {
    if dt.offset().local_minus_utc() == 0 {
        dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()
    } else {
        dt.format("%Y-%m-%dT%H:%M:%S%:z").to_string()
    }
}

// ---- Date arithmetic helpers ----

fn add_months(dt: DateTime<FixedOffset>, months: i32) -> DateTime<FixedOffset> {
    let naive = dt.naive_local();
    let year = naive.year();
    let month = naive.month() as i32;
    let day = naive.day();
    let total_months = month - 1 + months;
    let years_to_add = total_months.div_euclid(12);
    let new_month = (total_months.rem_euclid(12) + 1) as u32;
    let new_year = year + years_to_add;
    let max_day = days_in_month(new_year, new_month);
    let new_day = day.min(max_day);
    let new_naive = chrono::NaiveDate::from_ymd_opt(new_year, new_month, new_day)
        .and_then(|d| d.and_hms_opt(naive.hour(), naive.minute(), naive.second()))
        .unwrap_or(naive);
    dt.offset().from_local_datetime(&new_naive).unwrap()
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

// ============================================================================
// Capability implementations
// ============================================================================

fn get_current_date(input_json: &str) -> Result<String, ErrorInfo> {
    let input: GetCurrentDateInput = if input_json.trim().is_empty() || input_json.trim() == "null"
    {
        GetCurrentDateInput::default()
    } else {
        serde_json::from_str(input_json).map_err(bad_json)?
    };

    let now = Utc::now();
    let dt = apply_timezone(now, input.timezone.as_deref()).map_err(date_err)?;

    let result = if input.include_time {
        format_iso8601(&dt)
    } else {
        dt.format("%Y-%m-%d").to_string()
    };
    serde_json::to_string(&result).map_err(bad_json)
}

fn format_date(input_json: &str) -> Result<String, ErrorInfo> {
    let input: FormatDateInput = serde_json::from_str(input_json).map_err(bad_json)?;
    let date_str = input
        .date
        .as_ref()
        .ok_or_else(|| date_err("Date is required"))?;
    let dt = parse_flexible_date(date_str, input.timezone.as_deref()).map_err(date_err)?;

    let formatted = match input.format {
        DateFormat::Iso8601 => format_iso8601(&dt),
        DateFormat::Rfc2822 => dt.format("%a, %d %b %Y %H:%M:%S %z").to_string(),
        DateFormat::DateOnly => dt.format("%Y-%m-%d").to_string(),
        DateFormat::TimeOnly => dt.format("%H:%M:%S").to_string(),
        DateFormat::UsShortDate => dt.format("%m/%d/%Y").to_string(),
        DateFormat::EuShortDate => dt.format("%d/%m/%Y").to_string(),
        DateFormat::LongDate => dt.format("%B %d, %Y").to_string(),
        DateFormat::DateTime => dt.format("%Y-%m-%d %H:%M:%S").to_string(),
        DateFormat::Unix => dt.timestamp().to_string(),
        DateFormat::UnixMs => {
            (dt.timestamp() * 1000 + dt.timestamp_subsec_millis() as i64).to_string()
        }
        DateFormat::Custom => {
            let custom = input
                .custom_format
                .as_ref()
                .ok_or_else(|| date_err("Custom format is required when format is 'custom'"))?;
            let chrono_fmt = luxon_to_chrono_format(custom);
            dt.format(&chrono_fmt).to_string()
        }
    };
    serde_json::to_string(&formatted).map_err(bad_json)
}

fn add_to_date(input_json: &str) -> Result<String, ErrorInfo> {
    let input: AddToDateInput = serde_json::from_str(input_json).map_err(bad_json)?;
    let date_str = input
        .date
        .as_ref()
        .ok_or_else(|| date_err("Date is required"))?;
    let dt = parse_flexible_date(date_str, input.timezone.as_deref()).map_err(date_err)?;
    let amount = input.amount;

    let result = match input.unit {
        TimeUnit::Years => add_months(dt, (amount * 12) as i32),
        TimeUnit::Months => add_months(dt, amount as i32),
        TimeUnit::Weeks => dt + Duration::weeks(amount),
        TimeUnit::Days => dt + Duration::days(amount),
        TimeUnit::Hours => dt + Duration::hours(amount),
        TimeUnit::Minutes => dt + Duration::minutes(amount),
        TimeUnit::Seconds => dt + Duration::seconds(amount),
    };

    serde_json::to_string(&format_iso8601(&result)).map_err(bad_json)
}

fn subtract_from_date(input_json: &str) -> Result<String, ErrorInfo> {
    let input: SubtractFromDateInput = serde_json::from_str(input_json).map_err(bad_json)?;
    // Delegate to add_to_date with negated amount
    let add_input = AddToDateInput {
        date: input.date,
        amount: -input.amount,
        unit: input.unit,
        timezone: input.timezone,
    };
    let add_json = serde_json::to_string(&serde_json::json!({
        "date": add_input.date,
        "amount": add_input.amount,
        "unit": format!("{}", add_input.unit),
        "timezone": add_input.timezone,
    }))
    .map_err(bad_json)?;
    add_to_date(&add_json)
}

fn get_time_between(input_json: &str) -> Result<String, ErrorInfo> {
    let input: GetTimeBetweenInput = serde_json::from_str(input_json).map_err(bad_json)?;
    let start_str = input
        .start_date
        .as_ref()
        .ok_or_else(|| date_err("Start date is required"))?;
    let end_str = input
        .end_date
        .as_ref()
        .ok_or_else(|| date_err("End date is required"))?;
    let start = parse_flexible_date(start_str, None).map_err(date_err)?;
    let end = parse_flexible_date(end_str, None).map_err(date_err)?;

    let duration = end.signed_duration_since(start);
    let exact_ms = duration.num_milliseconds();

    let difference = match input.unit {
        TimeUnit::Years => duration.num_days() / 365,
        TimeUnit::Months => duration.num_days() / 30,
        TimeUnit::Weeks => duration.num_weeks(),
        TimeUnit::Days => duration.num_days(),
        TimeUnit::Hours => duration.num_hours(),
        TimeUnit::Minutes => duration.num_minutes(),
        TimeUnit::Seconds => duration.num_seconds(),
    };

    let result = TimeBetweenResult {
        difference,
        unit: input.unit.to_string(),
        exact_ms,
    };
    serde_json::to_string(&result).map_err(bad_json)
}

fn extract_date_part(input_json: &str) -> Result<String, ErrorInfo> {
    let input: ExtractDatePartInput = serde_json::from_str(input_json).map_err(bad_json)?;
    let date_str = input
        .date
        .as_ref()
        .ok_or_else(|| date_err("Date is required"))?;
    let dt = parse_flexible_date(date_str, input.timezone.as_deref()).map_err(date_err)?;

    let value: i32 = match input.part {
        DatePart::Year => dt.year(),
        DatePart::Month => dt.month() as i32,
        DatePart::Week => dt.iso_week().week() as i32,
        DatePart::Day => dt.day() as i32,
        DatePart::DayOfWeek => dt.weekday().num_days_from_monday() as i32 + 1,
        DatePart::DayOfYear => dt.ordinal() as i32,
        DatePart::Hour => dt.hour() as i32,
        DatePart::Minute => dt.minute() as i32,
        DatePart::Second => dt.second() as i32,
        DatePart::Millisecond => (dt.nanosecond() / 1_000_000) as i32,
        DatePart::Quarter => ((dt.month() - 1) / 3 + 1) as i32,
    };
    serde_json::to_string(&value).map_err(bad_json)
}

fn round_date(input_json: &str) -> Result<String, ErrorInfo> {
    let input: RoundDateInput = serde_json::from_str(input_json).map_err(bad_json)?;
    let date_str = input
        .date
        .as_ref()
        .ok_or_else(|| date_err("Date is required"))?;
    let dt = parse_flexible_date(date_str, input.timezone.as_deref()).map_err(date_err)?;
    let naive = dt.naive_local();

    let rounded_naive = match input.unit {
        TimeUnit::Years => {
            let year = match input.mode {
                RoundMode::Floor => naive.year(),
                RoundMode::Ceil => {
                    if naive.month() > 1
                        || naive.day() > 1
                        || naive.hour() > 0
                        || naive.minute() > 0
                        || naive.second() > 0
                    {
                        naive.year() + 1
                    } else {
                        naive.year()
                    }
                }
                RoundMode::Round => {
                    if naive.month() >= 7 {
                        naive.year() + 1
                    } else {
                        naive.year()
                    }
                }
            };
            NaiveDateTime::new(
                chrono::NaiveDate::from_ymd_opt(year, 1, 1).unwrap(),
                chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap(),
            )
        }
        TimeUnit::Months => {
            let (year, month) = match input.mode {
                RoundMode::Floor => (naive.year(), naive.month()),
                RoundMode::Ceil => {
                    if naive.day() > 1
                        || naive.hour() > 0
                        || naive.minute() > 0
                        || naive.second() > 0
                    {
                        if naive.month() == 12 {
                            (naive.year() + 1, 1)
                        } else {
                            (naive.year(), naive.month() + 1)
                        }
                    } else {
                        (naive.year(), naive.month())
                    }
                }
                RoundMode::Round => {
                    if naive.day() >= 16 {
                        if naive.month() == 12 {
                            (naive.year() + 1, 1)
                        } else {
                            (naive.year(), naive.month() + 1)
                        }
                    } else {
                        (naive.year(), naive.month())
                    }
                }
            };
            NaiveDateTime::new(
                chrono::NaiveDate::from_ymd_opt(year, month, 1).unwrap(),
                chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap(),
            )
        }
        TimeUnit::Weeks => {
            let weekday = naive.weekday().num_days_from_monday();
            let monday = naive.date() - Duration::days(weekday as i64);
            let next_monday = monday + Duration::days(7);
            let target_date = match input.mode {
                RoundMode::Floor => monday,
                RoundMode::Ceil => {
                    if weekday > 0 || naive.hour() > 0 || naive.minute() > 0 || naive.second() > 0 {
                        next_monday
                    } else {
                        monday
                    }
                }
                RoundMode::Round => {
                    if weekday >= 4 {
                        next_monday
                    } else {
                        monday
                    }
                }
            };
            NaiveDateTime::new(
                target_date,
                chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap(),
            )
        }
        TimeUnit::Days => {
            let date = match input.mode {
                RoundMode::Floor => naive.date(),
                RoundMode::Ceil => {
                    if naive.hour() > 0 || naive.minute() > 0 || naive.second() > 0 {
                        naive.date() + Duration::days(1)
                    } else {
                        naive.date()
                    }
                }
                RoundMode::Round => {
                    if naive.hour() >= 12 {
                        naive.date() + Duration::days(1)
                    } else {
                        naive.date()
                    }
                }
            };
            NaiveDateTime::new(date, chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap())
        }
        TimeUnit::Hours => {
            let (date, hour) = match input.mode {
                RoundMode::Floor => (naive.date(), naive.hour()),
                RoundMode::Ceil => {
                    if naive.minute() > 0 || naive.second() > 0 {
                        if naive.hour() == 23 {
                            (naive.date() + Duration::days(1), 0)
                        } else {
                            (naive.date(), naive.hour() + 1)
                        }
                    } else {
                        (naive.date(), naive.hour())
                    }
                }
                RoundMode::Round => {
                    if naive.minute() >= 30 {
                        if naive.hour() == 23 {
                            (naive.date() + Duration::days(1), 0)
                        } else {
                            (naive.date(), naive.hour() + 1)
                        }
                    } else {
                        (naive.date(), naive.hour())
                    }
                }
            };
            NaiveDateTime::new(date, chrono::NaiveTime::from_hms_opt(hour, 0, 0).unwrap())
        }
        TimeUnit::Minutes => {
            let total_mins = naive.hour() * 60 + naive.minute();
            let new_mins = match input.mode {
                RoundMode::Floor => total_mins,
                RoundMode::Ceil => {
                    if naive.second() > 0 {
                        total_mins + 1
                    } else {
                        total_mins
                    }
                }
                RoundMode::Round => {
                    if naive.second() >= 30 {
                        total_mins + 1
                    } else {
                        total_mins
                    }
                }
            };
            let (extra_days, final_mins) = if new_mins >= 24 * 60 {
                (1, new_mins - 24 * 60)
            } else {
                (0, new_mins)
            };
            let new_hour = final_mins / 60;
            let new_min = final_mins % 60;
            NaiveDateTime::new(
                naive.date() + Duration::days(extra_days),
                chrono::NaiveTime::from_hms_opt(new_hour, new_min, 0).unwrap(),
            )
        }
        TimeUnit::Seconds => naive,
    };

    let result = dt.offset().from_local_datetime(&rounded_naive).unwrap();
    serde_json::to_string(&format_iso8601(&result)).map_err(bad_json)
}

fn date_to_unix(input_json: &str) -> Result<String, ErrorInfo> {
    let input: DateToUnixInput = serde_json::from_str(input_json).map_err(bad_json)?;
    let date_str = input
        .date
        .as_ref()
        .ok_or_else(|| date_err("Date is required"))?;
    let dt = parse_flexible_date(date_str, None).map_err(date_err)?;

    let timestamp = if input.milliseconds {
        dt.timestamp() * 1000 + dt.timestamp_subsec_millis() as i64
    } else {
        dt.timestamp()
    };

    let result = UnixTimestampResult {
        timestamp,
        is_milliseconds: input.milliseconds,
    };
    serde_json::to_string(&result).map_err(bad_json)
}

fn unix_to_date(input_json: &str) -> Result<String, ErrorInfo> {
    let input: UnixToDateInput = serde_json::from_str(input_json).map_err(bad_json)?;
    let ts = input
        .timestamp
        .ok_or_else(|| date_err("Timestamp is required"))?;
    let is_ms = input.is_milliseconds.unwrap_or(ts > 1_000_000_000_000);
    let (secs, nanos) = if is_ms {
        (ts / 1000, ((ts % 1000) * 1_000_000) as u32)
    } else {
        (ts, 0)
    };
    let utc = DateTime::from_timestamp(secs, nanos)
        .ok_or_else(|| date_err(format!("Invalid Unix timestamp: {}", ts)))?;
    let dt = apply_timezone(utc, input.timezone.as_deref()).map_err(date_err)?;
    serde_json::to_string(&format_iso8601(&dt)).map_err(bad_json)
}

// ============================================================================
// JSON Schemas
// ============================================================================

const GET_CURRENT_DATE_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "includeTime": {
      "type": "boolean",
      "description": "Whether to include time in the output (default: true)",
      "default": true
    },
    "timezone": {
      "type": "string",
      "description": "Timezone (e.g., 'America/New_York', '+05:30', 'UTC'). Default: UTC"
    }
  }
}"#;

const GET_CURRENT_DATE_OUTPUT_SCHEMA: &str = r#"{
  "type": "string",
  "description": "Current date/time as ISO 8601 string or date-only string"
}"#;

const FORMAT_DATE_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "required": ["date"],
  "properties": {
    "date": {
      "type": "string",
      "description": "Date to format (ISO 8601, Unix timestamp, or common formats)"
    },
    "format": {
      "type": "string",
      "enum": ["iso8601","rfc2822","date-only","time-only","us-short-date","eu-short-date","long-date","date-time","unix","unix-ms","custom"],
      "default": "iso8601"
    },
    "customFormat": {
      "type": "string",
      "description": "Custom format using Luxon tokens (yyyy, MM, dd, HH, mm, ss)"
    },
    "timezone": {
      "type": "string",
      "description": "Output timezone. Default: UTC"
    }
  }
}"#;

const FORMAT_DATE_OUTPUT_SCHEMA: &str = r#"{
  "type": "string",
  "description": "Formatted date string"
}"#;

const ADD_TO_DATE_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "required": ["date"],
  "properties": {
    "date": { "type": "string", "description": "The date to add to" },
    "amount": { "type": "integer", "description": "Amount to add (positive) or subtract (negative)", "default": 0 },
    "unit": {
      "type": "string",
      "enum": ["years","months","weeks","days","hours","minutes","seconds"],
      "default": "days"
    },
    "timezone": { "type": "string", "description": "Timezone for the operation. Default: UTC" }
  }
}"#;

const DATE_OUTPUT_SCHEMA: &str = r#"{
  "type": "string",
  "description": "ISO 8601 date/time string"
}"#;

const SUBTRACT_FROM_DATE_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "required": ["date"],
  "properties": {
    "date": { "type": "string", "description": "The date to subtract from" },
    "amount": { "type": "integer", "description": "Amount to subtract", "default": 0 },
    "unit": {
      "type": "string",
      "enum": ["years","months","weeks","days","hours","minutes","seconds"],
      "default": "days"
    },
    "timezone": { "type": "string", "description": "Timezone for the operation. Default: UTC" }
  }
}"#;

const GET_TIME_BETWEEN_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "required": ["startDate","endDate"],
  "properties": {
    "startDate": { "type": "string", "description": "The start date" },
    "endDate": { "type": "string", "description": "The end date" },
    "unit": {
      "type": "string",
      "enum": ["years","months","weeks","days","hours","minutes","seconds"],
      "default": "days"
    }
  }
}"#;

const GET_TIME_BETWEEN_OUTPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "difference": { "type": "integer", "description": "The difference in the specified unit" },
    "unit": { "type": "string", "description": "The unit of the difference" },
    "exactMs": { "type": "integer", "description": "Exact difference in milliseconds" }
  }
}"#;

const EXTRACT_DATE_PART_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "required": ["date"],
  "properties": {
    "date": { "type": "string", "description": "The date to extract from" },
    "part": {
      "type": "string",
      "enum": ["year","month","week","day","day-of-week","day-of-year","hour","minute","second","millisecond","quarter"],
      "default": "day"
    },
    "timezone": { "type": "string", "description": "Timezone for extraction. Default: UTC" }
  }
}"#;

const EXTRACT_DATE_PART_OUTPUT_SCHEMA: &str = r#"{
  "type": "integer",
  "description": "Extracted date part value"
}"#;

const ROUND_DATE_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "required": ["date"],
  "properties": {
    "date": { "type": "string", "description": "The date to round" },
    "unit": {
      "type": "string",
      "enum": ["years","months","weeks","days","hours","minutes","seconds"],
      "default": "days"
    },
    "mode": {
      "type": "string",
      "enum": ["floor","ceil","round"],
      "default": "round"
    },
    "timezone": { "type": "string", "description": "Timezone for rounding. Default: UTC" }
  }
}"#;

const DATE_TO_UNIX_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "required": ["date"],
  "properties": {
    "date": { "type": "string", "description": "The date to convert" },
    "milliseconds": {
      "type": "boolean",
      "description": "If true, returns Unix timestamp in milliseconds instead of seconds (default: false)",
      "default": false
    }
  }
}"#;

const DATE_TO_UNIX_OUTPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "timestamp": { "type": "integer", "description": "Unix timestamp (seconds or milliseconds)" },
    "isMilliseconds": { "type": "boolean", "description": "True if timestamp is in milliseconds" }
  }
}"#;

const UNIX_TO_DATE_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "required": ["timestamp"],
  "properties": {
    "timestamp": { "type": "integer", "description": "Unix timestamp in seconds or milliseconds" },
    "isMilliseconds": {
      "type": "boolean",
      "description": "If true, timestamp is in milliseconds; if false, in seconds (default: auto-detect)"
    },
    "timezone": { "type": "string", "description": "Output timezone. Default: UTC" }
  }
}"#;

// ============================================================================
// Component plumbing
// ============================================================================

struct Component;

impl Guest for Component {
    fn get_module_info() -> ModuleInfo {
        ModuleInfo {
            id: "datetime".into(),
            display_name: "DateTime".into(),
            description: "Date and time capabilities for parsing, formatting, calculating, and manipulating dates.".into(),
            has_side_effects: false,
            supports_connections: false,
            integration_ids: vec![],
            secure: false,
        }
    }

    fn list_capabilities() -> Vec<CapabilityInfo> {
        vec![
            CapabilityInfo {
                id: "get-current-date".into(),
                function_name: "get-current-date".into(),
                display_name: Some("Get Current Date".into()),
                description: Some("Get the current date and optionally time in the specified timezone.".into()),
                has_side_effects: false,
                is_idempotent: false,
                rate_limited: false,
                tags: vec!["datetime".into()],
                input_schema: GET_CURRENT_DATE_INPUT_SCHEMA.into(),
                output_schema: GET_CURRENT_DATE_OUTPUT_SCHEMA.into(),
                known_errors: vec![],
                compensation_hint: None,
            },
            CapabilityInfo {
                id: "format-date".into(),
                function_name: "format-date".into(),
                display_name: Some("Format Date".into()),
                description: Some("Format a date using preset formats or custom Luxon-style tokens (yyyy, MM, dd, HH, mm, ss).".into()),
                has_side_effects: false,
                is_idempotent: true,
                rate_limited: false,
                tags: vec!["datetime".into()],
                input_schema: FORMAT_DATE_INPUT_SCHEMA.into(),
                output_schema: FORMAT_DATE_OUTPUT_SCHEMA.into(),
                known_errors: vec![],
                compensation_hint: None,
            },
            CapabilityInfo {
                id: "add-to-date".into(),
                function_name: "add-to-date".into(),
                display_name: Some("Add to Date".into()),
                description: Some("Add a duration (years, months, weeks, days, hours, minutes, seconds) to a date.".into()),
                has_side_effects: false,
                is_idempotent: true,
                rate_limited: false,
                tags: vec!["datetime".into()],
                input_schema: ADD_TO_DATE_INPUT_SCHEMA.into(),
                output_schema: DATE_OUTPUT_SCHEMA.into(),
                known_errors: vec![],
                compensation_hint: None,
            },
            CapabilityInfo {
                id: "subtract-from-date".into(),
                function_name: "subtract-from-date".into(),
                display_name: Some("Subtract from Date".into()),
                description: Some("Subtract a duration (years, months, weeks, days, hours, minutes, seconds) from a date.".into()),
                has_side_effects: false,
                is_idempotent: true,
                rate_limited: false,
                tags: vec!["datetime".into()],
                input_schema: SUBTRACT_FROM_DATE_INPUT_SCHEMA.into(),
                output_schema: DATE_OUTPUT_SCHEMA.into(),
                known_errors: vec![],
                compensation_hint: None,
            },
            CapabilityInfo {
                id: "get-time-between".into(),
                function_name: "get-time-between".into(),
                display_name: Some("Get Time Between Dates".into()),
                description: Some("Calculate the difference between two dates in the specified unit.".into()),
                has_side_effects: false,
                is_idempotent: true,
                rate_limited: false,
                tags: vec!["datetime".into()],
                input_schema: GET_TIME_BETWEEN_INPUT_SCHEMA.into(),
                output_schema: GET_TIME_BETWEEN_OUTPUT_SCHEMA.into(),
                known_errors: vec![],
                compensation_hint: None,
            },
            CapabilityInfo {
                id: "extract-date-part".into(),
                function_name: "extract-date-part".into(),
                display_name: Some("Extract Part of Date".into()),
                description: Some("Extract a specific component (year, month, day, hour, etc.) from a date.".into()),
                has_side_effects: false,
                is_idempotent: true,
                rate_limited: false,
                tags: vec!["datetime".into()],
                input_schema: EXTRACT_DATE_PART_INPUT_SCHEMA.into(),
                output_schema: EXTRACT_DATE_PART_OUTPUT_SCHEMA.into(),
                known_errors: vec![],
                compensation_hint: None,
            },
            CapabilityInfo {
                id: "round-date".into(),
                function_name: "round-date".into(),
                display_name: Some("Round Date".into()),
                description: Some("Round a date to the nearest unit (floor, ceil, or round).".into()),
                has_side_effects: false,
                is_idempotent: true,
                rate_limited: false,
                tags: vec!["datetime".into()],
                input_schema: ROUND_DATE_INPUT_SCHEMA.into(),
                output_schema: DATE_OUTPUT_SCHEMA.into(),
                known_errors: vec![],
                compensation_hint: None,
            },
            CapabilityInfo {
                id: "date-to-unix".into(),
                function_name: "date-to-unix".into(),
                display_name: Some("Date to Unix Timestamp".into()),
                description: Some("Convert a date to Unix timestamp (seconds or milliseconds).".into()),
                has_side_effects: false,
                is_idempotent: true,
                rate_limited: false,
                tags: vec!["datetime".into()],
                input_schema: DATE_TO_UNIX_INPUT_SCHEMA.into(),
                output_schema: DATE_TO_UNIX_OUTPUT_SCHEMA.into(),
                known_errors: vec![],
                compensation_hint: None,
            },
            CapabilityInfo {
                id: "unix-to-date".into(),
                function_name: "unix-to-date".into(),
                display_name: Some("Unix Timestamp to Date".into()),
                description: Some("Convert a Unix timestamp (seconds or milliseconds) to an ISO 8601 date string.".into()),
                has_side_effects: false,
                is_idempotent: true,
                rate_limited: false,
                tags: vec!["datetime".into()],
                input_schema: UNIX_TO_DATE_INPUT_SCHEMA.into(),
                output_schema: DATE_OUTPUT_SCHEMA.into(),
                known_errors: vec![],
                compensation_hint: None,
            },
        ]
    }

    fn invoke(
        capability_id: String,
        input: String,
        _connection: Option<ConnectionInfo>,
    ) -> Result<String, ErrorInfo> {
        match capability_id.as_str() {
            "get-current-date" => get_current_date(&input),
            "format-date" => format_date(&input),
            "add-to-date" => add_to_date(&input),
            "subtract-from-date" => subtract_from_date(&input),
            "get-time-between" => get_time_between(&input),
            "extract-date-part" => extract_date_part(&input),
            "round-date" => round_date(&input),
            "date-to-unix" => date_to_unix(&input),
            "unix-to-date" => unix_to_date(&input),
            other => Err(ErrorInfo {
                code: "UNKNOWN_CAPABILITY".into(),
                message: format!("datetime agent has no capability `{other}`"),
                category: "permanent".into(),
                severity: "error".into(),
                retryable: false,
                retry_after_ms: None,
                attributes: None,
            }),
        }
    }
}

bindings::export!(Component with_types_in bindings);
