#[derive(Debug, Clone)]
pub struct ParsedQuery {
    pub semantic: String,
    pub temporal_after: Option<i64>,
    pub temporal_before: Option<i64>,
    pub content_type: Option<String>,
    pub source_app: Option<String>,
    pub has_temporal: bool,
}

pub fn parse_query(raw: &str) -> ParsedQuery {
    let trimmed = raw.trim();

    // Extract components in order: temporal → source_app → type → semantic
    let (temporal_after, temporal_before, remaining) = extract_temporal(trimmed);
    let (source_app, remaining) = extract_source_app(&remaining);
    let (content_type, semantic) = extract_type_hint(&remaining);

    ParsedQuery {
        semantic: semantic.trim().to_string(),
        temporal_after,
        temporal_before,
        content_type,
        source_app,
        has_temporal: temporal_after.is_some() || temporal_before.is_some(),
    }
}

fn naive_ts(ndt: chrono::NaiveDateTime) -> i64 {
    chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(ndt, chrono::Utc).timestamp()
}

// ─── Temporal Extraction ──────────────────────────────────────────

fn extract_temporal(query: &str) -> (Option<i64>, Option<i64>, String) {
    use chrono::{Datelike, Duration, Local, NaiveTime, TimeZone};

    let now = Local::now();
    let today = |h: u32, m: u32, s: u32| {
        chrono::NaiveDate::from_ymd_opt(now.year(), now.month(), now.day())
            .unwrap()
            .and_time(NaiveTime::from_hms_opt(h, m, s).unwrap())
    };
    let day_range = |dt: chrono::DateTime<Local>| {
        let start = chrono::NaiveDate::from_ymd_opt(dt.year(), dt.month(), dt.day())
            .unwrap()
            .and_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap());
        let end = chrono::NaiveDate::from_ymd_opt(dt.year(), dt.month(), dt.day())
            .unwrap()
            .and_time(NaiveTime::from_hms_opt(23, 59, 59).unwrap());
        (naive_ts(start), naive_ts(end))
    };

    let patterns: Vec<(&str, Box<dyn Fn() -> (Option<i64>, Option<i64>)>)> = vec![
        // Yesterday
        (
            r"(?i)\b(?:from\s+)?yesterday\b",
            Box::new(|| {
                let (s, e) = day_range(now - Duration::days(1));
                (Some(s), Some(e))
            }),
        ),
        // Today
        (
            r"(?i)\btoday\b",
            Box::new(|| {
                let (s, e) = day_range(now);
                (Some(s), Some(e))
            }),
        ),
        // Day names: "monday", "tuesday", etc.
        (
            r"(?i)\b(last\s+)?(monday|tuesday|wednesday|thursday|friday|saturday|sunday)\b",
            Box::new(|| {
                // Handled specially below
                (None, None)
            }),
        ),
        // N hours ago (±30min window)
        (r"(?i)\b(\d+)\s+hours?\s+ago\b", Box::new(|| (None, None))),
        // Around N hours ago
        (
            r"(?i)\baround\s+(\d+)\s+hours?\s+ago\b",
            Box::new(|| (None, None)),
        ),
        // Last hour / 1 hour ago
        (
            r"(?i)\b(?:last\s+hour|1\s+hour\s+ago|an?\s+hour\s+ago)\b",
            Box::new(|| {
                let start = now - Duration::hours(1) - Duration::minutes(30);
                let end = now - Duration::hours(1) + Duration::minutes(30);
                (Some(start.timestamp()), Some(end.timestamp()))
            }),
        ),
        // N days ago
        (r"(?i)\b(\d+)\s+days?\s+ago\b", Box::new(|| (None, None))),
        // Last/past week
        (
            r"(?i)\b(?:last|past)\s+week\b",
            Box::new(|| {
                (
                    Some((now - Duration::days(7)).timestamp()),
                    Some(now.timestamp()),
                )
            }),
        ),
        // Last/past month
        (
            r"(?i)\b(?:last|past)\s+month\b",
            Box::new(|| {
                (
                    Some((now - Duration::days(30)).timestamp()),
                    Some(now.timestamp()),
                )
            }),
        ),
        // Last/past N days
        (
            r"(?i)\b(?:last|past)\s+(\d+)\s+days?\b",
            Box::new(|| (None, None)),
        ),
        // Last/past N hours
        (
            r"(?i)\b(?:last|past)\s+(\d+)\s+hours?\b",
            Box::new(|| (None, None)),
        ),
        // This morning
        (
            r"(?i)\bthis\s+morning\b",
            Box::new(|| {
                (
                    Some(naive_ts(today(6, 0, 0))),
                    Some(naive_ts(today(12, 0, 0))),
                )
            }),
        ),
        // This afternoon
        (
            r"(?i)\bthis\s+afternoon\b",
            Box::new(|| {
                (
                    Some(naive_ts(today(12, 0, 0))),
                    Some(naive_ts(today(18, 0, 0))),
                )
            }),
        ),
        // This evening / tonight
        (
            r"(?i)\b(?:this\s+evening|tonight)\b",
            Box::new(|| {
                (
                    Some(naive_ts(today(18, 0, 0))),
                    Some(naive_ts(today(23, 59, 59))),
                )
            }),
        ),
        // Recently (24 hours)
        (
            r"(?i)\brecently\b",
            Box::new(|| {
                (
                    Some((now - Duration::hours(24)).timestamp()),
                    Some(now.timestamp()),
                )
            }),
        ),
    ];

    for (pattern, range_fn) in &patterns {
        if let Some(mat) = find_pattern(query, pattern) {
            // For patterns with capture groups, extract the number
            let remaining = query[..mat.0].to_string() + &query[mat.1..];

            // Handle "N hours ago" / "around N hours ago"
            if pattern.contains("hours?\\s+ago") {
                if let Some(num) = extract_number(query, mat.0, mat.1) {
                    let around = pattern.contains("around");
                    let tolerance = if around {
                        Duration::minutes(30)
                    } else {
                        Duration::minutes(15)
                    };
                    let target = now - Duration::hours(num);
                    return (
                        Some((target - tolerance).timestamp()),
                        Some((target + tolerance).timestamp()),
                        remaining,
                    );
                }
            }

            // Handle "N days ago"
            if pattern.contains("days?\\s+ago") {
                if let Some(num) = extract_number(query, mat.0, mat.1) {
                    let target = now - Duration::days(num);
                    let (s, e) = day_range(target);
                    return (Some(s), Some(e), remaining);
                }
            }

            // Handle "last/past N days"
            if pattern.contains("last|past") && pattern.contains("days?\\b") {
                if let Some(num) = extract_number(query, mat.0, mat.1) {
                    return (
                        Some((now - Duration::days(num)).timestamp()),
                        Some(now.timestamp()),
                        remaining,
                    );
                }
            }

            // Handle "last/past N hours"
            if pattern.contains("last|past") && pattern.contains("hours?\\b") {
                if let Some(num) = extract_number(query, mat.0, mat.1) {
                    return (
                        Some((now - Duration::hours(num)).timestamp()),
                        Some(now.timestamp()),
                        remaining,
                    );
                }
            }

            // Handle day names
            if pattern.contains("monday|tuesday") {
                if let Some(day_name) = extract_day_name(query, mat.0, mat.1) {
                    let target = find_last_weekday(&now, &day_name);
                    let (s, e) = day_range(target);
                    return (Some(s), Some(e), remaining);
                }
            }

            let (after, before) = range_fn();
            if after.is_some() || before.is_some() {
                return (after, before, remaining);
            }
        }
    }

    // Try chrono-english for flexible parsing
    for prefix in &["from ", "in ", "on ", "during ", "since "] {
        if let Some(idx) = query.to_lowercase().find(prefix) {
            let after_phrase = &query[idx + prefix.len()..];
            if let Some(dt) = chrono_english::parse_date_string(
                after_phrase,
                Local::now(),
                chrono_english::Dialect::Uk,
            )
            .ok()
            {
                let start = chrono::NaiveDate::from_ymd_opt(dt.year(), dt.month(), dt.day())
                    .unwrap()
                    .and_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap());
                let end = chrono::NaiveDate::from_ymd_opt(dt.year(), dt.month(), dt.day())
                    .unwrap()
                    .and_time(NaiveTime::from_hms_opt(23, 59, 59).unwrap());
                let remaining = query[..idx].to_string();
                return (Some(naive_ts(start)), Some(naive_ts(end)), remaining);
            }
        }
    }

    (None, None, query.to_string())
}

fn find_last_weekday(
    now: &chrono::DateTime<chrono::Local>,
    day_name: &str,
) -> chrono::DateTime<chrono::Local> {
    use chrono::{Datelike, Duration};
    let target_dow = match day_name.to_lowercase().as_str() {
        "monday" => chrono::Weekday::Mon,
        "tuesday" => chrono::Weekday::Tue,
        "wednesday" => chrono::Weekday::Wed,
        "thursday" => chrono::Weekday::Thu,
        "friday" => chrono::Weekday::Fri,
        "saturday" => chrono::Weekday::Sat,
        "sunday" => chrono::Weekday::Sun,
        _ => return *now - Duration::days(1),
    };
    let mut d = *now - Duration::days(1);
    for _ in 0..7 {
        if d.weekday() == target_dow {
            return d;
        }
        d = d - Duration::days(1);
    }
    *now - Duration::days(7)
}

// ─── Source App Extraction ────────────────────────────────────────

fn extract_source_app(query: &str) -> (Option<String>, String) {
    // Match patterns like "from Slack", "in Chrome", "from VSCode", "in Finder"
    let re = regex::Regex::new(r"(?i)\b(?:from|in|via)\s+([A-Za-z][A-Za-z0-9. ]*?)(?:\s+(?:yesterday|today|last|this|from|in|\d+|[a-z]+\s+ago|recently|$))")
        .unwrap();

    if let Some(cap) = re.captures(query) {
        let app_name = cap[1].trim().to_string();
        if app_name.len() >= 2 && app_name.len() <= 50 {
            let remaining = query[..cap.get(1).unwrap().start() - 4].to_string()
                + &query[cap.get(1).unwrap().end()..];
            return (Some(app_name), remaining);
        }
    }

    // Also try: "url from Slack" or "code from VSCode" — app name after type
    let re2 = regex::Regex::new(r"(?i)\b(?:url|code|text|image|link)s?\s+(?:from|in|via)\s+([A-Za-z][A-Za-z0-9. ]*?)(?:\s+(?:yesterday|today|last|this|\d+|$))")
        .unwrap();

    if let Some(cap) = re2.captures(query) {
        let app_name = cap[1].trim().to_string();
        if app_name.len() >= 2 {
            let remaining = query[..cap.get(1).unwrap().start() - 4].to_string()
                + &query[cap.get(1).unwrap().end()..];
            return (Some(app_name), remaining);
        }
    }

    (None, query.to_string())
}

// ─── Type Hint Extraction ─────────────────────────────────────────

fn extract_type_hint(query: &str) -> (Option<String>, String) {
    let lower = query.to_lowercase();

    // Match "urls", "code", "images", "links", "text" anywhere as standalone words
    let type_patterns = [
        (r"(?i)\burls?\b", "url"),
        (r"(?i)\blinks?\b", "url"),
        (r"(?i)\bcode\b", "code"),
        (r"(?i)\bimages?\b", "image"),
        (r"(?i)\bphotos?\b", "image"),
        (r"(?i)\btext\b", "text"),
    ];

    for (pattern, type_name) in &type_patterns {
        if let Some(mat) = find_pattern(query, pattern) {
            let remaining = query[..mat.0].to_string() + &query[mat.1..];
            let cleaned = remaining.trim();
            if !cleaned.is_empty() || query.len() > mat.1 - mat.0 {
                return (Some(type_name.to_string()), remaining);
            }
        }
    }

    (None, query.to_string())
}

// ─── Helpers ──────────────────────────────────────────────────────

fn find_pattern(text: &str, pattern: &str) -> Option<(usize, usize)> {
    let re = regex::Regex::new(pattern).ok()?;
    let mat = re.find(text)?;
    Some((mat.start(), mat.end()))
}

fn extract_number(text: &str, start: usize, end: usize) -> Option<i64> {
    let re = regex::Regex::new(r"(\d+)").ok()?;
    let cap = re.captures(&text[start..end])?;
    cap[1].parse().ok()
}

fn extract_day_name(text: &str, start: usize, end: usize) -> Option<String> {
    let re = regex::Regex::new(r"(?i)(monday|tuesday|wednesday|thursday|friday|saturday|sunday)")
        .ok()?;
    let cap = re.captures(&text[start..end])?;
    Some(cap[1].to_lowercase())
}
