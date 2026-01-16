// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! DateTime agents for workflow execution
//!
//! This module provides date and time manipulation operations:
//! - Get current date/time (get-current-date)
//! - Format date (format-date)
//! - Add to date (add-to-date)
//! - Subtract from date (subtract-from-date)
//! - Get time between dates (get-time-between)
//! - Extract date part (extract-date-part)
//! - Round date (round-date)
//! - Date to Unix timestamp (date-to-unix)
//! - Unix timestamp to date (unix-to-date)
//!
//! All dates are handled in UTC by default with optional timezone support.
//! Format strings use Luxon-style tokens (yyyy, MM, dd, HH, mm, ss).

use chrono::{
    DateTime, Datelike, Duration, FixedOffset, NaiveDateTime, Offset, TimeZone, Timelike, Utc,
};
use chrono_tz::Tz;
use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use runtara_dsl::agent_meta::EnumVariants;
use serde::{Deserialize, Serialize};
use strum::VariantNames;

// ============================================================================
// Enums
// ============================================================================

/// Time unit for date arithmetic and rounding operations
#[derive(Debug, Default, Deserialize, Clone, Copy, VariantNames)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum TimeUnit {
    /// Years
    Years,
    /// Months
    Months,
    /// Weeks
    Weeks,
    /// Days
    #[default]
    Days,
    /// Hours
    Hours,
    /// Minutes
    Minutes,
    /// Seconds
    Seconds,
}

impl EnumVariants for TimeUnit {
    fn variant_names() -> &'static [&'static str] {
        Self::VARIANTS
    }
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

/// Date component to extract
#[derive(Debug, Default, Deserialize, Clone, Copy, VariantNames)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum DatePart {
    /// Year (e.g., 2024)
    Year,
    /// Month (1-12)
    Month,
    /// ISO week number (1-53)
    Week,
    /// Day of month (1-31)
    #[default]
    Day,
    /// Day of week (1=Monday, 7=Sunday)
    DayOfWeek,
    /// Day of year (1-366)
    DayOfYear,
    /// Hour (0-23)
    Hour,
    /// Minute (0-59)
    Minute,
    /// Second (0-59)
    Second,
    /// Millisecond (0-999)
    Millisecond,
    /// Quarter (1-4)
    Quarter,
}

impl EnumVariants for DatePart {
    fn variant_names() -> &'static [&'static str] {
        Self::VARIANTS
    }
}

/// Rounding mode for round-date operation
#[derive(Debug, Default, Deserialize, Clone, Copy, VariantNames)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum RoundMode {
    /// Round down (floor)
    Floor,
    /// Round up (ceil)
    Ceil,
    /// Round to nearest
    #[default]
    Round,
}

impl EnumVariants for RoundMode {
    fn variant_names() -> &'static [&'static str] {
        Self::VARIANTS
    }
}

/// Preset date formats
#[derive(Debug, Default, Deserialize, Clone, Copy, VariantNames)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum DateFormat {
    /// ISO 8601: 2024-01-15T14:30:00Z
    #[default]
    Iso8601,
    /// RFC 2822: Mon, 15 Jan 2024 14:30:00 +0000
    Rfc2822,
    /// Date only: 2024-01-15
    DateOnly,
    /// Time only: 14:30:00
    TimeOnly,
    /// US short date: 01/15/2024
    UsShortDate,
    /// EU short date: 15/01/2024
    EuShortDate,
    /// Long date: January 15, 2024
    LongDate,
    /// Date and time: 2024-01-15 14:30:00
    DateTime,
    /// Unix timestamp (seconds)
    Unix,
    /// Unix timestamp (milliseconds)
    UnixMs,
    /// Custom format (uses customFormat field)
    Custom,
}

impl EnumVariants for DateFormat {
    fn variant_names() -> &'static [&'static str] {
        Self::VARIANTS
    }
}

// ============================================================================
// Input/Output Types
// ============================================================================

/// Input for getting current date/time
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Current Date Input")]
pub struct GetCurrentDateInput {
    /// Whether to include time component (default: true)
    #[field(
        display_name = "Include Time",
        description = "Whether to include time in the output (default: true)",
        example = "true",
        default = "true"
    )]
    #[serde(default = "default_true")]
    #[serde(rename = "includeTime")]
    pub include_time: bool,

    /// Timezone for the output (IANA name or offset)
    #[field(
        display_name = "Timezone",
        description = "Timezone (e.g., 'America/New_York', '+05:30', 'UTC'). Default: UTC",
        example = "America/New_York"
    )]
    #[serde(default)]
    pub timezone: Option<String>,
}

impl Default for GetCurrentDateInput {
    fn default() -> Self {
        Self {
            include_time: true,
            timezone: None,
        }
    }
}

/// Input for formatting a date
#[derive(Debug, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "Format Date Input")]
pub struct FormatDateInput {
    /// The date to format (ISO 8601 string or Unix timestamp)
    #[field(
        display_name = "Date",
        description = "Date to format (ISO 8601, Unix timestamp, or common formats)",
        example = "2024-01-15T14:30:00Z"
    )]
    #[serde(default)]
    pub date: Option<String>,

    /// Preset format or custom
    #[field(
        display_name = "Format",
        description = "Preset format type",
        example = "iso8601",
        default = "iso8601",
        enum_type = "DateFormat"
    )]
    #[serde(default)]
    pub format: DateFormat,

    /// Custom format string using Luxon-style tokens
    #[field(
        display_name = "Custom Format",
        description = "Custom format using Luxon tokens (yyyy, MM, dd, HH, mm, ss)",
        example = "yyyy-MM-dd HH:mm:ss"
    )]
    #[serde(default)]
    #[serde(rename = "customFormat")]
    pub custom_format: Option<String>,

    /// Timezone for output
    #[field(
        display_name = "Timezone",
        description = "Output timezone (e.g., 'America/New_York', '+05:30'). Default: UTC",
        example = "Europe/London"
    )]
    #[serde(default)]
    pub timezone: Option<String>,
}

/// Input for adding duration to a date
#[derive(Debug, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "Add to Date Input")]
pub struct AddToDateInput {
    /// The base date
    #[field(
        display_name = "Date",
        description = "The date to add to (ISO 8601, Unix timestamp, or common formats)",
        example = "2024-01-15T14:30:00Z"
    )]
    #[serde(default)]
    pub date: Option<String>,

    /// Amount to add (can be negative)
    #[field(
        display_name = "Amount",
        description = "Amount to add (positive) or subtract (negative)",
        example = "7"
    )]
    #[serde(default)]
    pub amount: i64,

    /// Unit of time to add
    #[field(
        display_name = "Unit",
        description = "Time unit (years, months, weeks, days, hours, minutes, seconds)",
        example = "days",
        default = "days",
        enum_type = "TimeUnit"
    )]
    #[serde(default)]
    pub unit: TimeUnit,

    /// Timezone for the operation
    #[field(
        display_name = "Timezone",
        description = "Timezone for the operation. Default: UTC",
        example = "UTC"
    )]
    #[serde(default)]
    pub timezone: Option<String>,
}

/// Input for subtracting duration from a date
#[derive(Debug, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "Subtract from Date Input")]
pub struct SubtractFromDateInput {
    /// The base date
    #[field(
        display_name = "Date",
        description = "The date to subtract from (ISO 8601, Unix timestamp, or common formats)",
        example = "2024-01-15T14:30:00Z"
    )]
    #[serde(default)]
    pub date: Option<String>,

    /// Amount to subtract
    #[field(
        display_name = "Amount",
        description = "Amount to subtract",
        example = "3"
    )]
    #[serde(default)]
    pub amount: i64,

    /// Unit of time to subtract
    #[field(
        display_name = "Unit",
        description = "Time unit (years, months, weeks, days, hours, minutes, seconds)",
        example = "months",
        default = "days",
        enum_type = "TimeUnit"
    )]
    #[serde(default)]
    pub unit: TimeUnit,

    /// Timezone for the operation
    #[field(
        display_name = "Timezone",
        description = "Timezone for the operation. Default: UTC",
        example = "UTC"
    )]
    #[serde(default)]
    pub timezone: Option<String>,
}

/// Input for calculating time between two dates
#[derive(Debug, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "Get Time Between Input")]
pub struct GetTimeBetweenInput {
    /// Start date
    #[field(
        display_name = "Start Date",
        description = "The start date (ISO 8601, Unix timestamp, or common formats)",
        example = "2024-01-01T00:00:00Z"
    )]
    #[serde(default)]
    #[serde(rename = "startDate")]
    pub start_date: Option<String>,

    /// End date
    #[field(
        display_name = "End Date",
        description = "The end date (ISO 8601, Unix timestamp, or common formats)",
        example = "2024-01-15T00:00:00Z"
    )]
    #[serde(default)]
    #[serde(rename = "endDate")]
    pub end_date: Option<String>,

    /// Unit to return the difference in
    #[field(
        display_name = "Unit",
        description = "Unit for the result (years, months, weeks, days, hours, minutes, seconds)",
        example = "days",
        default = "days",
        enum_type = "TimeUnit"
    )]
    #[serde(default)]
    pub unit: TimeUnit,
}

/// Output for time between calculation
#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Time Between Result")]
#[serde(rename_all = "camelCase")]
pub struct TimeBetweenResult {
    /// The numeric difference in the specified unit
    #[field(
        display_name = "Difference",
        description = "The difference in the specified unit",
        example = "14"
    )]
    pub difference: i64,

    /// The unit of the difference
    #[field(
        display_name = "Unit",
        description = "The unit of the difference",
        example = "days"
    )]
    pub unit: String,

    /// Exact difference in milliseconds for precision
    #[field(
        display_name = "Exact Milliseconds",
        description = "Exact difference in milliseconds",
        example = "1209600000"
    )]
    pub exact_ms: i64,
}

/// Input for extracting a part of a date
#[derive(Debug, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "Extract Date Part Input")]
pub struct ExtractDatePartInput {
    /// The date to extract from
    #[field(
        display_name = "Date",
        description = "The date to extract from (ISO 8601, Unix timestamp, or common formats)",
        example = "2024-01-15T14:30:00Z"
    )]
    #[serde(default)]
    pub date: Option<String>,

    /// Part to extract
    #[field(
        display_name = "Part",
        description = "Date part to extract (year, month, week, day, hour, minute, second, etc.)",
        example = "year",
        default = "day",
        enum_type = "DatePart"
    )]
    #[serde(default)]
    pub part: DatePart,

    /// Timezone for the extraction
    #[field(
        display_name = "Timezone",
        description = "Timezone for extraction. Default: UTC",
        example = "America/New_York"
    )]
    #[serde(default)]
    pub timezone: Option<String>,
}

/// Input for converting date to Unix timestamp
#[derive(Debug, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "Date to Unix Input")]
pub struct DateToUnixInput {
    /// The date to convert
    #[field(
        display_name = "Date",
        description = "The date to convert (ISO 8601, Unix timestamp, or common formats)",
        example = "2024-01-15T14:30:00Z"
    )]
    #[serde(default)]
    pub date: Option<String>,

    /// Whether to output milliseconds instead of seconds
    #[field(
        display_name = "Milliseconds",
        description = "If true, returns Unix timestamp in milliseconds instead of seconds (default: false)",
        example = "false",
        default = "false"
    )]
    #[serde(default)]
    pub milliseconds: bool,
}

/// Output for Unix timestamp conversion
#[derive(Debug, Clone, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Unix Timestamp Result")]
#[serde(rename_all = "camelCase")]
pub struct UnixTimestampResult {
    /// The Unix timestamp
    #[field(
        display_name = "Timestamp",
        description = "Unix timestamp (seconds or milliseconds based on input)",
        example = "1705329000"
    )]
    pub timestamp: i64,

    /// Whether this is in milliseconds
    #[field(
        display_name = "Is Milliseconds",
        description = "True if timestamp is in milliseconds, false if seconds",
        example = "false"
    )]
    pub is_milliseconds: bool,
}

/// Input for converting Unix timestamp to date
#[derive(Debug, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "Unix to Date Input")]
pub struct UnixToDateInput {
    /// The Unix timestamp
    #[field(
        display_name = "Timestamp",
        description = "Unix timestamp in seconds or milliseconds",
        example = "1705329000"
    )]
    #[serde(default)]
    pub timestamp: Option<i64>,

    /// Whether the input is in milliseconds
    #[field(
        display_name = "Is Milliseconds",
        description = "If true, timestamp is in milliseconds; if false, in seconds (default: auto-detect)",
        example = "false"
    )]
    #[serde(default)]
    #[serde(rename = "isMilliseconds")]
    pub is_milliseconds: Option<bool>,

    /// Timezone for output
    #[field(
        display_name = "Timezone",
        description = "Output timezone (e.g., 'America/New_York', '+05:30'). Default: UTC",
        example = "UTC"
    )]
    #[serde(default)]
    pub timezone: Option<String>,
}

/// Input for rounding a date
#[derive(Debug, Deserialize, Default, CapabilityInput)]
#[capability_input(display_name = "Round Date Input")]
pub struct RoundDateInput {
    /// The date to round
    #[field(
        display_name = "Date",
        description = "The date to round (ISO 8601, Unix timestamp, or common formats)",
        example = "2024-01-15T14:37:42Z"
    )]
    #[serde(default)]
    pub date: Option<String>,

    /// Unit to round to
    #[field(
        display_name = "Unit",
        description = "Time unit to round to (years, months, weeks, days, hours, minutes, seconds)",
        example = "hours",
        default = "days",
        enum_type = "TimeUnit"
    )]
    #[serde(default)]
    pub unit: TimeUnit,

    /// Rounding mode
    #[field(
        display_name = "Mode",
        description = "Rounding mode (floor, ceil, round)",
        example = "floor",
        default = "round",
        enum_type = "RoundMode"
    )]
    #[serde(default)]
    pub mode: RoundMode,

    /// Timezone for the operation
    #[field(
        display_name = "Timezone",
        description = "Timezone for rounding. Default: UTC",
        example = "UTC"
    )]
    #[serde(default)]
    pub timezone: Option<String>,
}

// ============================================================================
// Default Value Helpers
// ============================================================================

fn default_true() -> bool {
    true
}

// ============================================================================
// Luxon to Chrono Format Conversion
// ============================================================================

/// Converts a Luxon-style format string to chrono strftime format
/// Uses a character-by-character approach to avoid partial replacement issues
fn luxon_to_chrono_format(luxon_format: &str) -> String {
    let mut result = String::with_capacity(luxon_format.len() * 2);
    let chars: Vec<char> = luxon_format.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let remaining = &luxon_format[luxon_format.char_indices().nth(i).unwrap().0..];

        // Try to match tokens (longest first)
        let matched = try_match_token(remaining);

        if let Some((luxon_len, chrono_token)) = matched {
            result.push_str(chrono_token);
            i += luxon_len;
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    result
}

/// Try to match a Luxon token at the start of the string
/// Returns (token_char_length, chrono_format) if matched
fn try_match_token(s: &str) -> Option<(usize, &'static str)> {
    // Tokens ordered by length (longest first) and by specificity
    static TOKENS: &[(&str, &str)] = &[
        // 4-char tokens
        ("yyyy", "%Y"), // 4-digit year: 2024
        ("MMMM", "%B"), // Full month name: January
        ("EEEE", "%A"), // Full weekday: Monday
        // 3-char tokens
        ("MMM", "%b"),  // Abbreviated month: Jan
        ("EEE", "%a"),  // Abbreviated weekday: Mon
        ("SSS", "%3f"), // Milliseconds: 123
        ("ZZZ", "%:z"), // Timezone offset: +05:30
        // 2-char tokens
        ("yy", "%y"), // 2-digit year: 24
        ("MM", "%m"), // 2-digit month: 01
        ("dd", "%d"), // 2-digit day: 01
        ("HH", "%H"), // 24-hour with padding: 09
        ("hh", "%I"), // 12-hour with padding: 09
        ("mm", "%M"), // Minutes with padding: 05
        ("ss", "%S"), // Seconds with padding: 05
        ("ZZ", "%z"), // Timezone offset: +0530
        // 1-char tokens (only match specific single chars that are unambiguous)
        ("a", "%p"), // AM/PM
        ("W", "%W"), // Week of year
        ("o", "%j"), // Day of year: 001-366
        ("Z", "%Z"), // Timezone abbreviation: EST
        ("E", "%a"), // Same as EEE
    ];

    for (luxon, chrono) in TOKENS {
        if s.starts_with(luxon) {
            return Some((luxon.chars().count(), *chrono));
        }
    }

    None
}

// ============================================================================
// Date Parsing Helpers
// ============================================================================

/// Common date formats to try when parsing
const PARSE_FORMATS: &[&str] = &[
    // ISO 8601 variants (always 4-digit year, safe to try first)
    "%Y-%m-%dT%H:%M:%S%.fZ",
    "%Y-%m-%dT%H:%M:%SZ",
    "%Y-%m-%dT%H:%M:%S%.f%:z",
    "%Y-%m-%dT%H:%M:%S%:z",
    "%Y-%m-%dT%H:%M:%S%.f",
    "%Y-%m-%dT%H:%M:%S",
    "%Y-%m-%d %H:%M:%S%.f",
    "%Y-%m-%d %H:%M:%S",
    "%Y-%m-%d",
    // US formats (2-digit year FIRST - chrono's %Y is lenient and would match 2-digit years as year 25 AD)
    "%m/%d/%y %H:%M:%S",
    "%m/%d/%y",
    // US formats (4-digit year)
    "%m/%d/%Y %H:%M:%S",
    "%m/%d/%Y",
    // European formats (2-digit year FIRST)
    "%d/%m/%y %H:%M:%S",
    "%d/%m/%y",
    "%d.%m.%y %H:%M:%S",
    "%d.%m.%y",
    // European formats (4-digit year)
    "%d/%m/%Y %H:%M:%S",
    "%d/%m/%Y",
    "%d.%m.%Y %H:%M:%S",
    "%d.%m.%Y",
];

/// Parse a date string with flexible format detection
fn parse_flexible_date(
    date_str: &str,
    timezone: Option<&str>,
) -> Result<DateTime<FixedOffset>, String> {
    let trimmed = date_str.trim();

    // Try Unix timestamp first (seconds)
    if let Ok(ts) = trimmed.parse::<i64>() {
        // Check if it looks like milliseconds (> year 2001 in seconds)
        let (secs, nanos) = if ts > 1_000_000_000_000 {
            (ts / 1000, ((ts % 1000) * 1_000_000) as u32)
        } else {
            (ts, 0)
        };

        let utc = DateTime::from_timestamp(secs, nanos)
            .ok_or_else(|| format!("Invalid Unix timestamp: {}", ts))?;
        return apply_timezone(utc, timezone);
    }

    // Try ISO 8601 parsing with chrono's built-in parser (RFC 3339)
    if let Ok(dt) = DateTime::parse_from_rfc3339(trimmed) {
        return apply_timezone(dt.with_timezone(&Utc), timezone);
    }

    // Try RFC 2822
    if let Ok(dt) = DateTime::parse_from_rfc2822(trimmed) {
        return apply_timezone(dt.with_timezone(&Utc), timezone);
    }

    // Try each format as datetime
    for fmt in PARSE_FORMATS {
        if let Ok(naive) = NaiveDateTime::parse_from_str(trimmed, fmt) {
            let utc = Utc.from_utc_datetime(&naive);
            return apply_timezone(utc, timezone);
        }
    }

    // Try date-only formats (2-digit year FIRST - chrono's %Y is lenient)
    const DATE_ONLY_FORMATS: &[&str] = &[
        "%Y-%m-%d", // ISO format is safe (always 4-digit year with dashes)
        // 2-digit year variants first
        "%m/%d/%y", "%d/%m/%y", "%d.%m.%y", // 4-digit year variants
        "%m/%d/%Y", "%d/%m/%Y", "%d.%m.%Y",
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

// ============================================================================
// Timezone Helpers
// ============================================================================

/// Parse a timezone string (IANA name or offset)
fn parse_timezone(tz_str: &str) -> Result<FixedOffset, String> {
    let trimmed = tz_str.trim();

    // Handle UTC explicitly
    if trimmed.eq_ignore_ascii_case("utc") || trimmed == "Z" {
        return Ok(FixedOffset::east_opt(0).unwrap());
    }

    // Try parsing as offset (+05:30, -08:00, +0530)
    if trimmed.starts_with('+') || trimmed.starts_with('-') {
        return parse_offset(trimmed);
    }

    // Try parsing as IANA timezone name
    if let Ok(tz) = trimmed.parse::<Tz>() {
        // Get current offset for this timezone
        let now = Utc::now().with_timezone(&tz);
        let fixed = now.offset().fix();
        return Ok(fixed);
    }

    Err(format!(
        "Unknown timezone: '{}'. Use IANA names (e.g., 'America/New_York') or offsets (e.g., '+05:30')",
        trimmed
    ))
}

/// Parse timezone offset string (+05:30, -08:00, +0530)
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

/// Apply timezone to a UTC datetime
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

/// Format a datetime to ISO 8601 string
fn format_iso8601(dt: &DateTime<FixedOffset>) -> String {
    if dt.offset().local_minus_utc() == 0 {
        dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()
    } else {
        dt.format("%Y-%m-%dT%H:%M:%S%:z").to_string()
    }
}

// ============================================================================
// Date Arithmetic Helpers
// ============================================================================

/// Add months to a date, handling month-end edge cases
fn add_months(dt: DateTime<FixedOffset>, months: i32) -> DateTime<FixedOffset> {
    let naive = dt.naive_local();
    let year = naive.year();
    let month = naive.month() as i32; // 1-12
    let day = naive.day();

    // Calculate new year and month
    // month is 1-based, so we subtract 1 to make it 0-based for arithmetic
    let total_months = month - 1 + months; // 0-based month + months to add
    let years_to_add = total_months.div_euclid(12);
    let new_month = (total_months.rem_euclid(12) + 1) as u32; // back to 1-based
    let new_year = year + years_to_add;

    // Cap day to max days in target month
    let max_day = days_in_month(new_year, new_month);
    let new_day = day.min(max_day);

    let new_naive = chrono::NaiveDate::from_ymd_opt(new_year, new_month, new_day)
        .and_then(|d| d.and_hms_opt(naive.hour(), naive.minute(), naive.second()))
        .unwrap_or(naive);

    dt.offset().from_local_datetime(&new_naive).unwrap()
}

/// Get the number of days in a month
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

/// Check if a year is a leap year
fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

// ============================================================================
// Capabilities
// ============================================================================

/// Get the current date and optionally time
#[capability(
    module = "datetime",
    display_name = "Get Current Date",
    description = "Get the current date and optionally time in the specified timezone",
    module_display_name = "DateTime",
    module_description = "Date and time capabilities for parsing, formatting, calculating, and manipulating dates"
)]
pub fn get_current_date(input: GetCurrentDateInput) -> Result<String, String> {
    let now = Utc::now();
    let dt = apply_timezone(now, input.timezone.as_deref())?;

    if input.include_time {
        Ok(format_iso8601(&dt))
    } else {
        Ok(dt.format("%Y-%m-%d").to_string())
    }
}

/// Format a date using preset formats or custom Luxon-style tokens
#[capability(
    module = "datetime",
    display_name = "Format Date",
    description = "Format a date using preset formats or custom Luxon-style tokens (yyyy, MM, dd, HH, mm, ss)"
)]
pub fn format_date(input: FormatDateInput) -> Result<String, String> {
    let date_str = input.date.as_ref().ok_or("Date is required")?;

    let dt = parse_flexible_date(date_str, input.timezone.as_deref())?;

    match input.format {
        DateFormat::Iso8601 => Ok(format_iso8601(&dt)),
        DateFormat::Rfc2822 => Ok(dt.format("%a, %d %b %Y %H:%M:%S %z").to_string()),
        DateFormat::DateOnly => Ok(dt.format("%Y-%m-%d").to_string()),
        DateFormat::TimeOnly => Ok(dt.format("%H:%M:%S").to_string()),
        DateFormat::UsShortDate => Ok(dt.format("%m/%d/%Y").to_string()),
        DateFormat::EuShortDate => Ok(dt.format("%d/%m/%Y").to_string()),
        DateFormat::LongDate => Ok(dt.format("%B %d, %Y").to_string()),
        DateFormat::DateTime => Ok(dt.format("%Y-%m-%d %H:%M:%S").to_string()),
        DateFormat::Unix => Ok(dt.timestamp().to_string()),
        DateFormat::UnixMs => {
            Ok((dt.timestamp() * 1000 + dt.timestamp_subsec_millis() as i64).to_string())
        }
        DateFormat::Custom => {
            let custom = input
                .custom_format
                .as_ref()
                .ok_or("Custom format is required when format is 'custom'")?;
            let chrono_fmt = luxon_to_chrono_format(custom);
            Ok(dt.format(&chrono_fmt).to_string())
        }
    }
}

/// Add a duration to a date
#[capability(
    module = "datetime",
    display_name = "Add to Date",
    description = "Add a duration (years, months, weeks, days, hours, minutes, seconds) to a date"
)]
pub fn add_to_date(input: AddToDateInput) -> Result<String, String> {
    let date_str = input.date.as_ref().ok_or("Date is required")?;

    let dt = parse_flexible_date(date_str, input.timezone.as_deref())?;
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

    Ok(format_iso8601(&result))
}

/// Subtract a duration from a date
#[capability(
    module = "datetime",
    display_name = "Subtract from Date",
    description = "Subtract a duration (years, months, weeks, days, hours, minutes, seconds) from a date"
)]
pub fn subtract_from_date(input: SubtractFromDateInput) -> Result<String, String> {
    // Reuse add_to_date with negated amount
    let add_input = AddToDateInput {
        date: input.date,
        amount: -input.amount,
        unit: input.unit,
        timezone: input.timezone,
    };
    add_to_date(add_input)
}

/// Calculate the difference between two dates
#[capability(
    module = "datetime",
    display_name = "Get Time Between Dates",
    description = "Calculate the difference between two dates in the specified unit"
)]
pub fn get_time_between(input: GetTimeBetweenInput) -> Result<TimeBetweenResult, String> {
    let start_str = input.start_date.as_ref().ok_or("Start date is required")?;
    let end_str = input.end_date.as_ref().ok_or("End date is required")?;

    let start = parse_flexible_date(start_str, None)?;
    let end = parse_flexible_date(end_str, None)?;

    let duration = end.signed_duration_since(start);
    let exact_ms = duration.num_milliseconds();

    let difference = match input.unit {
        TimeUnit::Years => {
            // Approximate years (365.25 days)
            duration.num_days() / 365
        }
        TimeUnit::Months => {
            // Approximate months (30.44 days)
            duration.num_days() / 30
        }
        TimeUnit::Weeks => duration.num_weeks(),
        TimeUnit::Days => duration.num_days(),
        TimeUnit::Hours => duration.num_hours(),
        TimeUnit::Minutes => duration.num_minutes(),
        TimeUnit::Seconds => duration.num_seconds(),
    };

    Ok(TimeBetweenResult {
        difference,
        unit: input.unit.to_string(),
        exact_ms,
    })
}

/// Extract a specific component from a date
#[capability(
    module = "datetime",
    display_name = "Extract Part of Date",
    description = "Extract a specific component (year, month, day, hour, etc.) from a date"
)]
pub fn extract_date_part(input: ExtractDatePartInput) -> Result<i32, String> {
    let date_str = input.date.as_ref().ok_or("Date is required")?;

    let dt = parse_flexible_date(date_str, input.timezone.as_deref())?;

    let value = match input.part {
        DatePart::Year => dt.year(),
        DatePart::Month => dt.month() as i32,
        DatePart::Week => dt.iso_week().week() as i32,
        DatePart::Day => dt.day() as i32,
        DatePart::DayOfWeek => dt.weekday().num_days_from_monday() as i32 + 1, // 1=Monday, 7=Sunday
        DatePart::DayOfYear => dt.ordinal() as i32,
        DatePart::Hour => dt.hour() as i32,
        DatePart::Minute => dt.minute() as i32,
        DatePart::Second => dt.second() as i32,
        DatePart::Millisecond => (dt.nanosecond() / 1_000_000) as i32,
        DatePart::Quarter => ((dt.month() - 1) / 3 + 1) as i32,
    };

    Ok(value)
}

/// Round a date to the nearest unit
#[capability(
    module = "datetime",
    display_name = "Round Date",
    description = "Round a date to the nearest unit (floor, ceil, or round)"
)]
pub fn round_date(input: RoundDateInput) -> Result<String, String> {
    let date_str = input.date.as_ref().ok_or("Date is required")?;

    let dt = parse_flexible_date(date_str, input.timezone.as_deref())?;
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
            // Get the Monday of the current week
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
        TimeUnit::Seconds => {
            // Seconds are already the smallest unit we handle
            naive
        }
    };

    let result = dt.offset().from_local_datetime(&rounded_naive).unwrap();
    Ok(format_iso8601(&result))
}

/// Convert a date to Unix timestamp
#[capability(
    module = "datetime",
    display_name = "Date to Unix Timestamp",
    description = "Convert a date to Unix timestamp (seconds or milliseconds)"
)]
pub fn date_to_unix(input: DateToUnixInput) -> Result<UnixTimestampResult, String> {
    let date_str = input.date.as_ref().ok_or("Date is required")?;

    let dt = parse_flexible_date(date_str, None)?;

    let timestamp = if input.milliseconds {
        dt.timestamp() * 1000 + dt.timestamp_subsec_millis() as i64
    } else {
        dt.timestamp()
    };

    Ok(UnixTimestampResult {
        timestamp,
        is_milliseconds: input.milliseconds,
    })
}

/// Convert a Unix timestamp to a date string
#[capability(
    module = "datetime",
    display_name = "Unix Timestamp to Date",
    description = "Convert a Unix timestamp (seconds or milliseconds) to an ISO 8601 date string"
)]
pub fn unix_to_date(input: UnixToDateInput) -> Result<String, String> {
    let ts = input.timestamp.ok_or("Timestamp is required")?;

    // Auto-detect if milliseconds based on magnitude, or use explicit flag
    // If timestamp is larger than year 2001 in seconds (~1 billion),
    // and looks like milliseconds (> 10 billion), assume milliseconds
    let is_ms = input.is_milliseconds.unwrap_or(ts > 1_000_000_000_000);

    let (secs, nanos) = if is_ms {
        (ts / 1000, ((ts % 1000) * 1_000_000) as u32)
    } else {
        (ts, 0)
    };

    let utc = DateTime::from_timestamp(secs, nanos)
        .ok_or_else(|| format!("Invalid Unix timestamp: {}", ts))?;

    let dt = apply_timezone(utc, input.timezone.as_deref())?;
    Ok(format_iso8601(&dt))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // Format conversion tests
    #[test]
    fn test_luxon_format_year() {
        assert_eq!(luxon_to_chrono_format("yyyy"), "%Y");
        assert_eq!(luxon_to_chrono_format("yy"), "%y");
    }

    #[test]
    fn test_luxon_format_complex() {
        assert_eq!(
            luxon_to_chrono_format("yyyy-MM-dd HH:mm:ss"),
            "%Y-%m-%d %H:%M:%S"
        );
    }

    #[test]
    fn test_luxon_format_12hour() {
        assert_eq!(luxon_to_chrono_format("hh:mm:ss a"), "%I:%M:%S %p");
    }

    // Date parsing tests
    #[test]
    fn test_parse_iso8601() {
        let result = parse_flexible_date("2024-01-15T14:30:00Z", None);
        assert!(result.is_ok());
        let dt = result.unwrap();
        assert_eq!(dt.year(), 2024);
        assert_eq!(dt.month(), 1);
        assert_eq!(dt.day(), 15);
    }

    #[test]
    fn test_parse_unix_timestamp() {
        let result = parse_flexible_date("1705329000", None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_unix_ms_timestamp() {
        let result = parse_flexible_date("1705329000000", None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_date_only() {
        let result = parse_flexible_date("2024-01-15", None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_two_digit_year() {
        // US format MM/DD/YY - year 25 should be 2025, not 0025
        let result = parse_flexible_date("10/22/25", None);
        assert!(result.is_ok());
        let dt = result.unwrap();
        assert_eq!(dt.year(), 2025);
        assert_eq!(dt.month(), 10);
        assert_eq!(dt.day(), 22);

        // European format DD.MM.YY
        let result = parse_flexible_date("22.10.25", None);
        assert!(result.is_ok());
        let dt = result.unwrap();
        assert_eq!(dt.year(), 2025);

        // Year 99 should be 1999 (chrono pivot: 70-99 -> 1970-1999)
        let result = parse_flexible_date("01/15/99", None);
        assert!(result.is_ok());
        let dt = result.unwrap();
        assert_eq!(dt.year(), 1999);

        // Year 00 should be 2000 (chrono pivot: 00-69 -> 2000-2069)
        let result = parse_flexible_date("01/15/00", None);
        assert!(result.is_ok());
        let dt = result.unwrap();
        assert_eq!(dt.year(), 2000);
    }

    // Timezone tests
    #[test]
    fn test_timezone_utc() {
        let offset = parse_timezone("UTC");
        assert!(offset.is_ok());
        assert_eq!(offset.unwrap().local_minus_utc(), 0);
    }

    #[test]
    fn test_timezone_offset() {
        let offset = parse_timezone("+05:30");
        assert!(offset.is_ok());
        assert_eq!(offset.unwrap().local_minus_utc(), 5 * 3600 + 30 * 60);
    }

    #[test]
    fn test_timezone_negative_offset() {
        let offset = parse_timezone("-08:00");
        assert!(offset.is_ok());
        assert_eq!(offset.unwrap().local_minus_utc(), -8 * 3600);
    }

    #[test]
    fn test_timezone_iana() {
        let offset = parse_timezone("America/New_York");
        assert!(offset.is_ok());
    }

    // Capability tests
    #[test]
    fn test_get_current_date_utc() {
        let input = GetCurrentDateInput::default();
        let result = get_current_date(input);
        assert!(result.is_ok());
        let date = result.unwrap();
        assert!(date.ends_with('Z'));
    }

    #[test]
    fn test_get_current_date_no_time() {
        let input = GetCurrentDateInput {
            include_time: false,
            timezone: None,
        };
        let result = get_current_date(input);
        assert!(result.is_ok());
        let date = result.unwrap();
        assert!(!date.contains('T'));
        assert!(date.contains('-'));
    }

    #[test]
    fn test_format_date_iso8601() {
        let input = FormatDateInput {
            date: Some("2024-01-15T14:30:00Z".to_string()),
            format: DateFormat::Iso8601,
            ..Default::default()
        };
        let result = format_date(input);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "2024-01-15T14:30:00Z");
    }

    #[test]
    fn test_format_date_custom() {
        let input = FormatDateInput {
            date: Some("2024-01-15T14:30:00Z".to_string()),
            format: DateFormat::Custom,
            custom_format: Some("yyyy/MM/dd".to_string()),
            ..Default::default()
        };
        let result = format_date(input);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "2024/01/15");
    }

    #[test]
    fn test_format_date_unix() {
        let input = FormatDateInput {
            date: Some("2024-01-15T14:30:00Z".to_string()),
            format: DateFormat::Unix,
            ..Default::default()
        };
        let result = format_date(input);
        assert!(result.is_ok());
        // Just verify it's a valid number
        assert!(result.unwrap().parse::<i64>().is_ok());
    }

    #[test]
    fn test_add_days() {
        let input = AddToDateInput {
            date: Some("2024-01-15T00:00:00Z".to_string()),
            amount: 7,
            unit: TimeUnit::Days,
            ..Default::default()
        };
        let result = add_to_date(input);
        assert!(result.is_ok());
        assert!(result.unwrap().contains("2024-01-22"));
    }

    #[test]
    fn test_add_months() {
        let input = AddToDateInput {
            date: Some("2024-01-31T00:00:00Z".to_string()),
            amount: 1,
            unit: TimeUnit::Months,
            ..Default::default()
        };
        let result = add_to_date(input);
        assert!(result.is_ok());
        // January 31 + 1 month = February 29 (2024 is a leap year)
        assert!(result.unwrap().contains("2024-02-29"));
    }

    #[test]
    fn test_add_negative() {
        let input = AddToDateInput {
            date: Some("2024-01-15T00:00:00Z".to_string()),
            amount: -5,
            unit: TimeUnit::Days,
            ..Default::default()
        };
        let result = add_to_date(input);
        assert!(result.is_ok());
        assert!(result.unwrap().contains("2024-01-10"));
    }

    #[test]
    fn test_subtract_days() {
        let input = SubtractFromDateInput {
            date: Some("2024-01-15T00:00:00Z".to_string()),
            amount: 5,
            unit: TimeUnit::Days,
            ..Default::default()
        };
        let result = subtract_from_date(input);
        assert!(result.is_ok());
        assert!(result.unwrap().contains("2024-01-10"));
    }

    #[test]
    fn test_time_between_days() {
        let input = GetTimeBetweenInput {
            start_date: Some("2024-01-01T00:00:00Z".to_string()),
            end_date: Some("2024-01-15T00:00:00Z".to_string()),
            unit: TimeUnit::Days,
        };
        let result = get_time_between(input).unwrap();
        assert_eq!(result.difference, 14);
        assert_eq!(result.unit, "days");
    }

    #[test]
    fn test_time_between_hours() {
        let input = GetTimeBetweenInput {
            start_date: Some("2024-01-01T00:00:00Z".to_string()),
            end_date: Some("2024-01-01T12:00:00Z".to_string()),
            unit: TimeUnit::Hours,
        };
        let result = get_time_between(input).unwrap();
        assert_eq!(result.difference, 12);
    }

    #[test]
    fn test_extract_year() {
        let input = ExtractDatePartInput {
            date: Some("2024-01-15T14:30:00Z".to_string()),
            part: DatePart::Year,
            ..Default::default()
        };
        let result = extract_date_part(input);
        assert_eq!(result.unwrap(), 2024);
    }

    #[test]
    fn test_extract_month() {
        let input = ExtractDatePartInput {
            date: Some("2024-01-15T14:30:00Z".to_string()),
            part: DatePart::Month,
            ..Default::default()
        };
        let result = extract_date_part(input);
        assert_eq!(result.unwrap(), 1);
    }

    #[test]
    fn test_extract_quarter() {
        let input = ExtractDatePartInput {
            date: Some("2024-07-15T14:30:00Z".to_string()),
            part: DatePart::Quarter,
            ..Default::default()
        };
        let result = extract_date_part(input);
        assert_eq!(result.unwrap(), 3);
    }

    #[test]
    fn test_round_to_hour_floor() {
        let input = RoundDateInput {
            date: Some("2024-01-15T14:37:42Z".to_string()),
            unit: TimeUnit::Hours,
            mode: RoundMode::Floor,
            ..Default::default()
        };
        let result = round_date(input);
        assert!(result.is_ok());
        assert!(result.unwrap().contains("14:00:00"));
    }

    #[test]
    fn test_round_to_hour_ceil() {
        let input = RoundDateInput {
            date: Some("2024-01-15T14:37:42Z".to_string()),
            unit: TimeUnit::Hours,
            mode: RoundMode::Ceil,
            ..Default::default()
        };
        let result = round_date(input);
        assert!(result.is_ok());
        assert!(result.unwrap().contains("15:00:00"));
    }

    #[test]
    fn test_round_to_day() {
        let input = RoundDateInput {
            date: Some("2024-01-15T14:37:42Z".to_string()),
            unit: TimeUnit::Days,
            mode: RoundMode::Round,
            ..Default::default()
        };
        let result = round_date(input);
        assert!(result.is_ok());
        // 14:37 is past noon, so should round up to the 16th
        assert!(result.unwrap().contains("2024-01-16"));
    }

    // Edge case tests
    #[test]
    fn test_leap_year() {
        let input = AddToDateInput {
            date: Some("2024-02-28T00:00:00Z".to_string()),
            amount: 1,
            unit: TimeUnit::Days,
            ..Default::default()
        };
        let result = add_to_date(input);
        assert!(result.is_ok());
        assert!(result.unwrap().contains("2024-02-29"));
    }

    #[test]
    fn test_non_leap_year() {
        let input = AddToDateInput {
            date: Some("2023-02-28T00:00:00Z".to_string()),
            amount: 1,
            unit: TimeUnit::Days,
            ..Default::default()
        };
        let result = add_to_date(input);
        assert!(result.is_ok());
        assert!(result.unwrap().contains("2023-03-01"));
    }

    #[test]
    fn test_empty_date_error() {
        let input = FormatDateInput::default();
        let result = format_date(input);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Date is required"));
    }

    #[test]
    fn test_invalid_date_error() {
        let input = FormatDateInput {
            date: Some("not-a-date".to_string()),
            ..Default::default()
        };
        let result = format_date(input);
        assert!(result.is_err());
    }

    // Unix timestamp capability tests
    #[test]
    fn test_date_to_unix_seconds() {
        let input = DateToUnixInput {
            date: Some("2024-01-15T14:30:00Z".to_string()),
            milliseconds: false,
        };
        let result = date_to_unix(input).unwrap();
        assert_eq!(result.timestamp, 1705329000);
        assert!(!result.is_milliseconds);
    }

    #[test]
    fn test_date_to_unix_milliseconds() {
        let input = DateToUnixInput {
            date: Some("2024-01-15T14:30:00Z".to_string()),
            milliseconds: true,
        };
        let result = date_to_unix(input).unwrap();
        assert_eq!(result.timestamp, 1705329000000);
        assert!(result.is_milliseconds);
    }

    #[test]
    fn test_unix_to_date_seconds() {
        let input = UnixToDateInput {
            timestamp: Some(1705329000),
            is_milliseconds: Some(false),
            timezone: None,
        };
        let result = unix_to_date(input).unwrap();
        assert_eq!(result, "2024-01-15T14:30:00Z");
    }

    #[test]
    fn test_unix_to_date_milliseconds() {
        let input = UnixToDateInput {
            timestamp: Some(1705329000000),
            is_milliseconds: Some(true),
            timezone: None,
        };
        let result = unix_to_date(input).unwrap();
        assert_eq!(result, "2024-01-15T14:30:00Z");
    }

    #[test]
    fn test_unix_to_date_auto_detect_seconds() {
        let input = UnixToDateInput {
            timestamp: Some(1705329000),
            is_milliseconds: None, // auto-detect
            timezone: None,
        };
        let result = unix_to_date(input).unwrap();
        assert_eq!(result, "2024-01-15T14:30:00Z");
    }

    #[test]
    fn test_unix_to_date_auto_detect_milliseconds() {
        let input = UnixToDateInput {
            timestamp: Some(1705329000000),
            is_milliseconds: None, // auto-detect
            timezone: None,
        };
        let result = unix_to_date(input).unwrap();
        assert_eq!(result, "2024-01-15T14:30:00Z");
    }

    #[test]
    fn test_unix_to_date_with_timezone() {
        let input = UnixToDateInput {
            timestamp: Some(1705329000),
            is_milliseconds: Some(false),
            timezone: Some("+05:30".to_string()),
        };
        let result = unix_to_date(input).unwrap();
        assert!(result.contains("20:00:00"));
        assert!(result.contains("+05:30"));
    }

    #[test]
    fn test_date_to_unix_roundtrip() {
        // Convert date to unix
        let to_unix = DateToUnixInput {
            date: Some("2024-01-15T14:30:00Z".to_string()),
            milliseconds: false,
        };
        let unix_result = date_to_unix(to_unix).unwrap();

        // Convert back to date
        let to_date = UnixToDateInput {
            timestamp: Some(unix_result.timestamp),
            is_milliseconds: Some(false),
            timezone: None,
        };
        let date_result = unix_to_date(to_date).unwrap();
        assert_eq!(date_result, "2024-01-15T14:30:00Z");
    }
}
