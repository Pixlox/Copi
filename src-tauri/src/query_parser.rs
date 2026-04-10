use chrono::{Datelike, Duration, Local, NaiveDate, NaiveTime, TimeZone, Weekday};
use std::sync::LazyLock;

#[derive(Debug, Clone)]
pub struct ParsedQuery {
    pub semantic: String,
    pub keywords: Vec<String>,
    pub temporal_after: Option<i64>,
    pub temporal_before: Option<i64>,
    pub temporal_confidence: f32,
    pub content_type: Option<String>,
    pub source_apps: Vec<String>,
    pub source_hints: Vec<String>,
    pub languages: Vec<String>,
    pub is_pinned: Option<bool>,
    pub min_length: Option<usize>,
    pub is_multiline: Option<bool>,
    pub ordering: Option<Ordering>,
    pub has_temporal: bool,
    pub query_is_empty_after_parse: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Ordering {
    Newest,
    Oldest,
    SecondNewest,
}

#[derive(Clone, Copy)]
struct PeriodRange {
    start_hour: u32,
    start_minute: u32,
    end_hour: u32,
    end_minute: u32,
    crosses_midnight: bool,
}

static RE_MINUTES_AGO: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\b(\d+)\s*m(?:in(?:utes?)?)?\s+ago\b").unwrap());
static RE_SECONDS_AGO: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\b(\d+)\s*s(?:ec(?:onds?)?)?\s+ago\b").unwrap());
static RE_JUST_NOW: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(?:just\s+now|a?\s*moment\s+ago|moments?\s+ago)\b").unwrap()
});
static RE_FEW_MINUTES: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(?:a?\s*(?:few|couple)\s+minutes?\s+ago)\b").unwrap()
});
static RE_HOURS_AGO: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\b(\d+)\s*h(?:ours?)?\s+ago\b").unwrap());
static RE_ABOUT_HOURS_AGO: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(?:around|about|roughly)\s+(\d+)\s*h(?:ours?)?\s+ago\b").unwrap()
});
static RE_HALF_HOUR: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\bhalf\s+(?:an?\s+)?hour\s+ago\b").unwrap());
static RE_AN_HOUR: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(?:about\s+)?an?\s+hour(?:\s+or\s+so)?\s+ago\b").unwrap()
});
static RE_FEW_HOURS: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\ba?\s*(?:couple|few)\s+hours?\s+ago\b").unwrap());
static RE_DAY_REF: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)\b(?:(this|last|on)\s+)?(monday|tuesday|wednesday|thursday|friday|saturday|sunday|mon|tue|wed|thu|fri|sat|sun)\b",
    )
    .unwrap()
});
static RE_YESTERDAY: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\byesterday\b").unwrap());
static RE_TODAY: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\btoday\b").unwrap());
static RE_LAST_NIGHT: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\blast\s+night\b").unwrap());
static RE_OTHER_DAY: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\bthe\s+other\s+day\b").unwrap());
static RE_EARLIER: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\b(?:earlier(?:\s+today)?|earlier)\b").unwrap());
static RE_THIS_WEEK: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\bthis\s+week\b").unwrap());
static RE_LAST_WEEK: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\b(?:last|past)\s+week\b").unwrap());
static RE_THIS_MONTH: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\bthis\s+month\b").unwrap());
static RE_LAST_MONTH: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\blast\s+month\b").unwrap());
static RE_WEEKEND: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\b(?:this|last|the)\s+weekend\b").unwrap());
static RE_DAYS_AGO: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\b(\d+)\s+days?\s+ago\b").unwrap());
static RE_WEEKS_AGO: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\b(\d+)\s+weeks?\s+ago\b").unwrap());
static RE_PAST_DAYS: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\b(?:past|last)\s+(\d+)\s+days?\b").unwrap());
static RE_PAST_HOURS: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\b(?:past|last)\s+(\d+)\s+hours?\b").unwrap());
static RE_WHILE_AGO: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\ba\s+while\s+(?:ago|back)\b").unwrap());
static RE_NOT_LONG_AGO: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\bnot\s+(?:too\s+)?long\s+ago\b").unwrap());
static RE_AGES_AGO: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\b(?:ages\s+ago|a\s+long\s+time\s+ago)\b").unwrap());
static RE_RECENTLY: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\brecently\b").unwrap());
static RE_CLOCK: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)\b(?:(around|about|at|before|after)\s+)?(\d{1,2})(?::(\d{2}))?\s*(am|pm)\b",
    )
    .unwrap()
});
static RE_CLOCK_AMBIG: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(?:around|about)\s+(\d{1,2})(?::(\d{2}))?\s*(?:o'?clock)?\b").unwrap()
});
static RE_BEFORE_AFTER_WORD: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\b(before|after)\s+(noon|lunch|work)\b").unwrap());
static RE_BETWEEN_HOURS: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)\bbetween\s+(\d{1,2})(?::(\d{2}))?\s*(am|pm)?\s+and\s+(\d{1,2})(?::(\d{2}))?\s*(am|pm)?\b",
    )
    .unwrap()
});
static RE_SOURCE_STRICT: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)\b(?:from|in|via|on|copied\s+from|pasted\s+from)\s+([A-Za-z][A-Za-z0-9\s]{1,30})\b",
    )
    .unwrap()
});
static RE_SOURCE_FALLBACK: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(?:from|in|via|on)\s+([A-Za-z][A-Za-z0-9]{1,25})\b").unwrap()
});
static RE_MIN_LENGTH: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)\b(?:at\s+least|over|longer\s+than)\s+(\d{2,5})\s*(?:chars?|characters?)\b",
    )
    .unwrap()
});
static RE_MULTILINE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(?:multiline|multi[- ]line|paragraph|multiple\s+lines)\b").unwrap()
});
static RE_CONTENT_TYPE_CODE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)\b(?:snippets?|functions?|json|sql|bash|commands?|python|javascript|typescript|rust|source\s+code|code\s+snippet|code\s+block|endpoints?)\b",
    )
    .unwrap()
});
static RE_CONTENT_TYPE_IMAGE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(?:screenshots?|pictures?|photos?|pics?|images?)\b").unwrap()
});
static RE_CONTENT_TYPE_URL: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\b(?:urls?|links?|websites?|https?)\b").unwrap());
static RE_CONTENT_TYPE_TEXT: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\btext\b").unwrap());
static RE_PINNED: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\b(?:pinned|starred|saved)\b").unwrap());

const APP_MAP: &[(&str, &[&str])] = &[
    ("safari", &["safari"]),
    ("chrome", &["chrome", "google chrome"]),
    ("firefox", &["firefox"]),
    ("arc", &["arc"]),
    ("brave", &["brave"]),
    ("edge", &["edge", "microsoft edge"]),
    (
        "browser",
        &[
            "safari",
            "chrome",
            "google chrome",
            "firefox",
            "arc",
            "brave",
            "edge",
        ],
    ),
    (
        "web",
        &[
            "safari",
            "chrome",
            "google chrome",
            "firefox",
            "arc",
            "brave",
            "edge",
        ],
    ),
    ("notes", &["notes", "apple notes"]),
    ("notion", &["notion"]),
    ("obsidian", &["obsidian"]),
    ("bear", &["bear"]),
    ("messages", &["messages", "imessage"]),
    ("slack", &["slack"]),
    ("discord", &["discord"]),
    ("teams", &["teams", "microsoft teams"]),
    ("telegram", &["telegram"]),
    ("whatsapp", &["whatsapp"]),
    ("mail", &["mail", "apple mail"]),
    ("outlook", &["outlook"]),
    ("vscode", &["visual studio code", "vscode", "code"]),
    ("code", &["visual studio code", "vscode", "code"]),
    ("xcode", &["xcode"]),
    (
        "terminal",
        &["terminal", "iterm", "iterm2", "ghostty", "warp"],
    ),
    ("iterm", &["iterm", "iterm2"]),
    ("warp", &["warp"]),
    ("cursor", &["cursor"]),
    ("figma", &["figma"]),
    ("linear", &["linear"]),
    ("things", &["things"]),
    ("reminders", &["reminders"]),
    ("calendar", &["calendar"]),
    ("finder", &["finder"]),
    ("spotify", &["spotify"]),
    ("photos", &["photos"]),
];

const LANGUAGE_ALIASES: &[(&str, &[&str])] = &[
    ("ar", &["arabic"]),
    ("de", &["german", "deutsch"]),
    ("en", &["english"]),
    ("es", &["spanish", "espanol"]),
    ("fr", &["french", "francais"]),
    ("he", &["hebrew"]),
    ("hi", &["hindi"]),
    ("it", &["italian"]),
    ("ja", &["japanese", "japan", "nihongo"]),
    ("ko", &["korean", "hangul"]),
    ("pt", &["portuguese"]),
    ("ru", &["russian"]),
    ("th", &["thai"]),
    ("uk", &["ukrainian"]),
    ("vi", &["vietnamese"]),
    ("zh", &["chinese", "mandarin", "cantonese"]),
];

const STOPWORDS: &[&str] = &[
    "a", "an", "and", "at", "for", "from", "i", "in", "is", "it", "my", "of", "on", "or", "the",
    "to", "via",
];

const INTENT_EXPANSIONS: &[(&[&str], &[&str])] = &[
    // === Travel & Logistics ===
    (
        &["flight info", "boarding info", "flight details"],
        &[
            "boarding",
            "pass",
            "itinerary",
            "gate",
            "terminal",
            "airline",
            "reservation",
            "ticket",
        ],
    ),
    (
        &["boarding pass"],
        &["flight", "gate", "terminal", "ticket"],
    ),
    (
        &["hotel", "hotel reservation", "booking"],
        &[
            "reservation",
            "confirmation",
            "check-in",
            "check-out",
            "room",
        ],
    ),
    (
        &["car rental", "rental car"],
        &["reservation", "pickup", "return", "vehicle"],
    ),
    (
        &["trip", "travel itinerary"],
        &["flight", "hotel", "booking", "schedule"],
    ),
    (
        &["airport", "boarding", "departure"],
        &["flight", "gate", "terminal", "airline"],
    ),
    // === Auth & Security ===
    (
        &["auth code", "verification code", "login code", "otp", "2fa"],
        &[
            "verification",
            "passcode",
            "one-time",
            "token",
            "signin",
            "pin",
            "security",
        ],
    ),
    (
        &["password", "credentials", "login"],
        &[
            "username",
            "password",
            "email",
            "account",
            "secret",
            "credential",
        ],
    ),
    (
        &["api key", "access token", "bearer token"],
        &["secret", "bearer", "authorization", "token", "key", "auth"],
    ),
    // NEW: Auth/security related searches
    (
        &["that thing", "that code", "that login"],
        &["password", "credential", "auth", "login", "code"],
    ),
    (
        &["secret", "token", "key"],
        &["api", "secret", "key", "token", "password"],
    ),
    (
        &["ssh key", "private key"],
        &["ssh", "key", "pem", "private", "connect"],
    ),
    // === Meetings & Calendar ===
    (
        &["meeting notes", "meeting summary"],
        &["agenda", "notes", "action", "followup"],
    ),
    (
        &["meeting link", "zoom link", "meet link", "video call"],
        &["https", "url", "invite", "zoom", "meet"],
    ),
    (
        &["calendar", "event", "appointment"],
        &["schedule", "date", "time", "location"],
    ),
    (
        &["standup", "stand-up", "daily"],
        &["yesterday", "today", "blockers", "updates"],
    ),
    // === Commerce & Finance ===
    (
        &["receipt", "invoice"],
        &["order", "payment", "total", "subtotal", "tax"],
    ),
    (
        &["tax", "taxes", "tax number"],
        &["ssn", "tin", "invoice", "payment", "amount", "refund"],
    ),
    (
        &["tracking number", "tracking info"],
        &["shipment", "tracking", "delivery", "order"],
    ),
    (
        &["price", "cost", "pricing"],
        &["amount", "total", "dollar", "usd", "eur"],
    ),
    (
        &["bank", "account number"],
        &["routing", "swift", "iban", "transfer"],
    ),
    (
        &["expense", "expenses"],
        &["receipt", "reimbursement", "amount", "category"],
    ),
    (
        &["order", "purchase"],
        &["confirmation", "shipping", "delivery", "item"],
    ),
    (
        &["credit card", "card number"],
        &["card", "payment", "visa", "mastercard", "expiry", "cvv"],
    ),
    // === Contact Info ===
    (
        &["address", "location"],
        &["street", "road", "avenue", "postcode", "zip", "city"],
    ),
    (
        &["phone number", "phone"],
        &["mobile", "cell", "telephone", "contact"],
    ),
    (
        &["email address", "email"],
        &["@", "gmail", "outlook", "inbox"],
    ),
    // === Code & Development ===
    (
        &["code block", "code snippet"],
        &["snippet", "function", "implementation", "source"],
    ),
    (
        &["error", "error message", "stack trace"],
        &["exception", "traceback", "panic", "failed", "bug"],
    ),
    (
        &["pull request", "pr", "merge request"],
        &["review", "branch", "commit", "diff", "github"],
    ),
    (
        &["docker", "container"],
        &["image", "dockerfile", "compose", "container"],
    ),
    (
        &["config", "configuration", "settings"],
        &["yaml", "json", "toml", "env", "environment"],
    ),
    (
        &["endpoint", "api"],
        &["url", "route", "request", "response", "http"],
    ),
    (
        &["database", "query", "sql"],
        &["select", "from", "where", "join", "table"],
    ),
    (
        &["deploy", "deployment"],
        &["build", "release", "staging", "production"],
    ),
    (
        &["ssh", "terminal command"],
        &["connect", "server", "host", "key", "command"],
    ),
    (
        &["staging server", "production server", "ip address"],
        &[
            "ip",
            "host",
            "server",
            "ssh",
            "endpoint",
            "staging",
            "production",
        ],
    ),
    (
        &["git", "commit", "branch"],
        &["repository", "push", "pull", "merge", "diff"],
    ),
    // === Documents & Notes ===
    (
        &["todo", "to-do", "task list"],
        &["task", "checkbox", "item", "list", "done"],
    ),
    (&["notes", "note"], &["text", "memo", "reminder", "jot"]),
    (
        &["draft", "draft email"],
        &["compose", "write", "message", "send"],
    ),
    (
        &["apology email", "sorry email"],
        &["apology", "sorry", "regards", "followup", "email"],
    ),
    (
        &["contract", "agreement"],
        &["terms", "signature", "sign", "clause", "legal"],
    ),
    (
        &["document", "file"],
        &["pdf", "download", "folder", "path"],
    ),
    (
        &["privacy policy", "policy"],
        &["privacy", "policy", "terms", "legal", "compliance"],
    ),
    // === Media ===
    (
        &["screenshot", "screen capture"],
        &["image", "png", "screen", "capture"],
    ),
    (
        &["score screenshot", "maimai score", "game score"],
        &["score", "rating", "result", "screenshot", "image"],
    ),
    (
        &["photo", "picture", "image"],
        &["image", "jpg", "camera", "gallery"],
    ),
    (
        &["link", "url", "website"],
        &["https", "http", "web", "page", "site"],
    ),
    (
        &["google doc", "project doc"],
        &["docs.google.com", "document", "project", "link", "url"],
    ),
    // === Social & Communication ===
    (
        &["message", "text message"],
        &["sms", "chat", "imessage", "conversation"],
    ),
    // NEW: General context searches
    (
        &["from slack", "slack message", "slack"],
        &["slack", "message", "workspace", "channel", "chat"],
    ),
    (
        &["from discord", "discord message"],
        &["discord", "message", "server", "channel", "chat"],
    ),
    (
        &["from teams", "teams message"],
        &["teams", "microsoft", "message", "chat", "meeting"],
    ),
    (
        &["from whatsapp", "whatsapp"],
        &["whatsapp", "message", "chat", "conversation"],
    ),
    (
        &["from email", "gmail", "outlook email"],
        &["email", "gmail", "outlook", "inbox", "message"],
    ),
    (
        &["from notion", "notion page"],
        &["notion", "page", "doc", "workspace"],
    ),
    (
        &["from figma", "figma design"],
        &["figma", "design", "prototype", "link"],
    ),
    (
        &["from vscode", "vscode", "from code"],
        &["vscode", "code", "editor", "terminal", "snippet"],
    ),
    (
        &["from terminal", "from terminal"],
        &["terminal", "command", "bash", "shell", "zsh"],
    ),
    (
        &["deadline", "boss said", "due date"],
        &["deadline", "due", "date", "priority", "urgent", "message"],
    ),
    (
        &["thread", "conversation"],
        &["reply", "message", "chat", "discussion"],
    ),
    (
        &["tweet", "post", "social media"],
        &["twitter", "x.com", "status", "share"],
    ),
    (
        &["chat", "dm", "direct message"],
        &["message", "conversation", "thread"],
    ),
    // === Entertainment ===
    (
        &["playlist", "song", "music"],
        &["spotify", "apple music", "track", "album"],
    ),
    (
        &["video", "youtube"],
        &["watch", "channel", "playlist", "stream"],
    ),
    (
        &["recipe", "cooking"],
        &["ingredients", "steps", "instructions", "cups"],
    ),
    // === Work & Productivity ===
    (
        &["project", "sprint"],
        &["task", "deadline", "milestone", "progress"],
    ),
    (
        &["bug", "issue", "ticket"],
        &["report", "fix", "assign", "priority"],
    ),
    (
        &["document", "doc"],
        &["google docs", "notion", "confluence", "page"],
    ),
    (
        &["spreadsheet", "excel"],
        &["cells", "formula", "rows", "columns", "data"],
    ),
    (
        &["presentation", "slides"],
        &["powerpoint", "keynote", "slide", "deck"],
    ),
];

const NAMED_PERIODS: &[(&[&str], PeriodRange, f32)] = &[
    (
        &["early morning"],
        PeriodRange {
            start_hour: 5,
            start_minute: 0,
            end_hour: 8,
            end_minute: 0,
            crosses_midnight: false,
        },
        0.85,
    ),
    (
        &["mid morning"],
        PeriodRange {
            start_hour: 8,
            start_minute: 0,
            end_hour: 10,
            end_minute: 30,
            crosses_midnight: false,
        },
        0.85,
    ),
    (
        &["late morning"],
        PeriodRange {
            start_hour: 10,
            start_minute: 30,
            end_hour: 12,
            end_minute: 0,
            crosses_midnight: false,
        },
        0.85,
    ),
    (
        &["this morning", "morning"],
        PeriodRange {
            start_hour: 5,
            start_minute: 0,
            end_hour: 12,
            end_minute: 0,
            crosses_midnight: false,
        },
        0.85,
    ),
    (
        &["at lunch", "lunchtime", "at noon", "noon", "lunch"],
        PeriodRange {
            start_hour: 11,
            start_minute: 30,
            end_hour: 13,
            end_minute: 30,
            crosses_midnight: false,
        },
        0.85,
    ),
    (
        &["after lunch"],
        PeriodRange {
            start_hour: 13,
            start_minute: 0,
            end_hour: 15,
            end_minute: 0,
            crosses_midnight: false,
        },
        0.85,
    ),
    (
        &["this afternoon", "afternoon"],
        PeriodRange {
            start_hour: 12,
            start_minute: 0,
            end_hour: 17,
            end_minute: 0,
            crosses_midnight: false,
        },
        0.85,
    ),
    (
        &["late afternoon"],
        PeriodRange {
            start_hour: 15,
            start_minute: 0,
            end_hour: 17,
            end_minute: 30,
            crosses_midnight: false,
        },
        0.85,
    ),
    (
        &["this evening", "evening"],
        PeriodRange {
            start_hour: 17,
            start_minute: 0,
            end_hour: 21,
            end_minute: 0,
            crosses_midnight: false,
        },
        0.85,
    ),
    (
        &["tonight", "night"],
        PeriodRange {
            start_hour: 21,
            start_minute: 0,
            end_hour: 23,
            end_minute: 59,
            crosses_midnight: false,
        },
        0.85,
    ),
    (
        &["midnight"],
        PeriodRange {
            start_hour: 23,
            start_minute: 0,
            end_hour: 1,
            end_minute: 0,
            crosses_midnight: true,
        },
        0.85,
    ),
    (
        &["dawn", "sunrise"],
        PeriodRange {
            start_hour: 5,
            start_minute: 0,
            end_hour: 7,
            end_minute: 30,
            crosses_midnight: false,
        },
        0.85,
    ),
    (
        &["dusk", "sunset"],
        PeriodRange {
            start_hour: 17,
            start_minute: 30,
            end_hour: 20,
            end_minute: 30,
            crosses_midnight: false,
        },
        0.85,
    ),
];

pub fn parse_query(raw: &str) -> ParsedQuery {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return empty_query();
    }

    if let Some(shortcut) = check_shortcircuits(trimmed) {
        return shortcut;
    }

    let mut remaining = trimmed.to_string();
    let mut parsed = empty_query();

    if let Some((after, before, confidence, spans)) = extract_temporal(&remaining) {
        parsed.temporal_after = Some(after);
        parsed.temporal_before = Some(before);
        parsed.temporal_confidence = confidence;
        parsed.has_temporal = true;
        remaining = remove_spans(&remaining, &spans);
    }

    let (languages, language_spans) = extract_languages(&remaining);
    if !language_spans.is_empty() {
        remaining = remove_spans(&remaining, &language_spans);
    }
    parsed.languages = languages;

    let (source_apps, source_hints, source_spans) = extract_source_apps(&remaining);
    if !source_spans.is_empty() {
        remaining = remove_spans(&remaining, &source_spans);
    }
    parsed.source_apps = source_apps;
    parsed.source_hints = source_hints;

    let (content_type, is_pinned, min_length, is_multiline, meta_spans) = extract_meta(&remaining);
    if !meta_spans.is_empty() {
        remaining = remove_spans(&remaining, &meta_spans);
    }
    parsed.content_type = content_type;
    parsed.is_pinned = is_pinned;
    parsed.min_length = min_length;
    parsed.is_multiline = is_multiline;

    let semantic = normalize_whitespace(&remaining);

    // If semantic is only stopwords, clear it so temporal/source filters can drive search
    let semantic_is_only_stopwords = !semantic.is_empty()
        && semantic
            .split_whitespace()
            .all(|w| STOPWORDS.contains(&w.to_lowercase().as_str()));

    if semantic_is_only_stopwords {
        parsed.semantic.clear();
        parsed.keywords.clear();
    } else {
        // Pass original trimmed query for intent phrase detection
        // This ensures "meeting link" triggers expansion even though "link" was extracted as content_type
        let (keywords, enriched) =
            semantic_keywords_with_enrichment(&semantic, parsed.content_type.as_deref(), trimmed);
        parsed.keywords = keywords;
        parsed.semantic = enriched;
    }

    parsed.query_is_empty_after_parse = parsed.semantic.is_empty()
        && !parsed.has_temporal
        && parsed.content_type.is_none()
        && parsed.source_apps.is_empty()
        && parsed.languages.is_empty()
        && parsed.is_pinned.is_none()
        && parsed.min_length.is_none()
        && parsed.is_multiline.is_none();

    parsed
}

fn empty_query() -> ParsedQuery {
    ParsedQuery {
        semantic: String::new(),
        keywords: vec![],
        temporal_after: None,
        temporal_before: None,
        temporal_confidence: 0.0,
        content_type: None,
        source_apps: vec![],
        source_hints: vec![],
        languages: vec![],
        is_pinned: None,
        min_length: None,
        is_multiline: None,
        ordering: None,
        has_temporal: false,
        query_is_empty_after_parse: true,
    }
}

fn check_shortcircuits(q: &str) -> Option<ParsedQuery> {
    let mut parsed = empty_query();
    match q.trim().to_lowercase().as_str() {
        "pinned" | "starred" => {
            parsed.is_pinned = Some(true);
            Some(parsed)
        }
        "last thing" | "most recent" | "latest" => {
            parsed.ordering = Some(Ordering::Newest);
            Some(parsed)
        }
        "oldest" | "the first" => {
            parsed.ordering = Some(Ordering::Oldest);
            Some(parsed)
        }
        "the one before" | "previous one" => {
            parsed.ordering = Some(Ordering::SecondNewest);
            Some(parsed)
        }
        _ => None,
    }
}

fn extract_languages(q: &str) -> (Vec<String>, Vec<(usize, usize)>) {
    let lower = q.to_lowercase();
    let mut langs = Vec::new();
    let mut spans = Vec::new();
    for (code, aliases) in LANGUAGE_ALIASES {
        for alias in *aliases {
            let pattern = format!(r"\b{}\b", regex::escape(alias));
            if let Ok(re) = regex::Regex::new(&pattern) {
                if let Some(m) = re.find(&lower) {
                    let mut start = m.start();
                    if start >= 3 && &lower[start - 3..start] == "in " {
                        start -= 3;
                    }
                    if !langs.iter().any(|existing| existing == code) {
                        langs.push((*code).to_string());
                    }
                    spans.push((start, m.end()));
                    break;
                }
            }
        }
    }
    (langs, spans)
}

fn extract_source_apps(q: &str) -> (Vec<String>, Vec<String>, Vec<(usize, usize)>) {
    let mut apps = Vec::new();
    let mut hints = Vec::new();
    let mut spans = Vec::new();
    for re in [&*RE_SOURCE_STRICT, &*RE_SOURCE_FALLBACK] {
        for captures in re.captures_iter(q) {
            let Some(full_match) = captures.get(0) else {
                continue;
            };
            let captured = captures
                .get(1)
                .map(|m| clean_source_candidate(m.as_str()))
                .unwrap_or_default();
            if captured.is_empty() || is_language_alias(&captured) {
                continue;
            }
            let mapped = map_source_app(&captured);
            if mapped.is_empty() {
                if captured.len() >= 2 && !hints.iter().any(|existing| existing == &captured) {
                    hints.push(captured);
                }
                continue;
            }
            for value in mapped {
                if !apps.iter().any(|existing| existing == &value) {
                    apps.push(value);
                }
            }
            spans.push((full_match.start(), full_match.end()));
        }
        if !apps.is_empty() {
            break;
        }
    }
    (apps, hints, spans)
}

fn map_source_app(captured: &str) -> Vec<String> {
    if let Some((_, mapped)) = APP_MAP
        .iter()
        .find(|(name, mapped)| *name == captured || mapped.iter().any(|alias| *alias == captured))
    {
        return mapped.iter().map(|value| (*value).to_string()).collect();
    }

    for (name, mapped) in APP_MAP {
        if captured.len() >= 3
            && (captured.contains(name)
                || name.contains(captured)
                || mapped
                    .iter()
                    .any(|alias| captured.contains(alias) || alias.contains(&captured)))
        {
            return mapped.iter().map(|value| (*value).to_string()).collect();
        }
    }

    Vec::new()
}

fn extract_meta(
    q: &str,
) -> (
    Option<String>,
    Option<bool>,
    Option<usize>,
    Option<bool>,
    Vec<(usize, usize)>,
) {
    let mut content_type = None;
    let mut is_pinned = None;
    let mut min_length = None;
    let mut is_multiline = None;
    let mut spans = Vec::new();

    if let Some(m) = RE_CONTENT_TYPE_CODE.find(q) {
        content_type = Some("code".to_string());
        spans.push((m.start(), m.end()));
    } else if let Some(m) = RE_CONTENT_TYPE_IMAGE.find(q) {
        content_type = Some("image".to_string());
        spans.push((m.start(), m.end()));
    } else if let Some(m) = RE_CONTENT_TYPE_URL.find(q) {
        content_type = Some("url".to_string());
        spans.push((m.start(), m.end()));
    } else if let Some(m) = RE_CONTENT_TYPE_TEXT.find(q) {
        content_type = Some("text".to_string());
        spans.push((m.start(), m.end()));
    }

    if let Some(m) = RE_PINNED.find(q) {
        is_pinned = Some(true);
        spans.push((m.start(), m.end()));
    }

    if let Some(captures) = RE_MIN_LENGTH.captures(q) {
        if let Some(full_match) = captures.get(0) {
            min_length = captures
                .get(1)
                .and_then(|m| m.as_str().parse::<usize>().ok());
            spans.push((full_match.start(), full_match.end()));
        }
    }

    if let Some(m) = RE_MULTILINE.find(q) {
        is_multiline = Some(true);
        spans.push((m.start(), m.end()));
    }

    (content_type, is_pinned, min_length, is_multiline, spans)
}

fn extract_temporal(q: &str) -> Option<(i64, i64, f32, Vec<(usize, usize)>)> {
    if let Some((anchor_date, day_span, day_conf)) = extract_day_reference(q) {
        if let Some((period, period_span, period_conf)) = extract_named_period(q) {
            let (after, before) = period_bounds(anchor_date, period);
            return Some((
                after,
                before,
                day_conf.min(period_conf),
                vec![day_span, period_span],
            ));
        }

        if let Some((after, before, conf, clock_span)) = extract_clock_time(q, Some(anchor_date)) {
            return Some((
                after,
                before,
                day_conf.min(conf),
                vec![day_span, clock_span],
            ));
        }
    }

    if let Some(found) = extract_relative_temporal(q) {
        return Some(found);
    }

    if let Some(found) = extract_named_relative_temporal(q) {
        return Some(found);
    }

    if let Some(found) = extract_clock_time(q, None) {
        let (after, before, conf, span) = found;
        return Some((after, before, conf, vec![span]));
    }

    if let Some((anchor_date, day_span, conf)) = extract_day_reference(q) {
        let (after, before) = full_day(anchor_date);
        return Some((after, before, conf, vec![day_span]));
    }

    if let Some((period, span, conf)) = extract_named_period(q) {
        let today = Local::now().date_naive();
        let (after, before) = period_bounds(today, period);
        return Some((
            after,
            before.min(Local::now().timestamp()),
            conf,
            vec![span],
        ));
    }

    extract_range_temporal(q)
}

fn extract_day_reference(q: &str) -> Option<(NaiveDate, (usize, usize), f32)> {
    let now = Local::now();

    if let Some(m) = RE_YESTERDAY.find(q) {
        return Some((
            (now - Duration::days(1)).date_naive(),
            (m.start(), m.end()),
            0.98,
        ));
    }
    if let Some(m) = RE_TODAY.find(q) {
        return Some((now.date_naive(), (m.start(), m.end()), 0.98));
    }
    if let Some(captures) = RE_DAY_REF.captures(q) {
        let full_match = captures.get(0)?;
        let modifier = captures.get(1).map(|m| m.as_str().to_lowercase());
        let name = captures.get(2)?.as_str().to_lowercase();
        let weekday = parse_weekday(&name)?;
        let date = resolve_weekday(now.date_naive(), weekday, modifier.as_deref());
        return Some((date, (full_match.start(), full_match.end()), 0.95));
    }
    None
}

fn extract_named_period(q: &str) -> Option<(PeriodRange, (usize, usize), f32)> {
    let lower = q.to_lowercase();
    for (aliases, range, confidence) in NAMED_PERIODS {
        for alias in *aliases {
            let pattern = format!(r"\b{}\b", regex::escape(alias));
            if let Ok(re) = regex::Regex::new(&pattern) {
                if let Some(m) = re.find(&lower) {
                    return Some((*range, (m.start(), m.end()), *confidence));
                }
            }
        }
    }
    None
}

fn extract_relative_temporal(q: &str) -> Option<(i64, i64, f32, Vec<(usize, usize)>)> {
    let now = Local::now();
    if let Some(captures) = RE_MINUTES_AGO.captures(q) {
        let full_match = captures.get(0)?;
        let value = captures.get(1)?.as_str().parse::<i64>().ok().unwrap_or(1);
        let center = now - Duration::minutes(value);
        let start = center - Duration::seconds((value as f64 * 60.0 * 0.3) as i64);
        let end = center + Duration::seconds((value as f64 * 60.0 * 0.3) as i64);
        return Some((
            start.timestamp(),
            end.timestamp(),
            0.97,
            vec![(full_match.start(), full_match.end())],
        ));
    }
    if let Some(captures) = RE_SECONDS_AGO.captures(q) {
        let full_match = captures.get(0)?;
        let value = captures.get(1)?.as_str().parse::<i64>().ok().unwrap_or(1);
        return Some((
            (now - Duration::seconds(value + 15)).timestamp(),
            (now - Duration::seconds(value - 15)).timestamp(),
            0.97,
            vec![(full_match.start(), full_match.end())],
        ));
    }
    if let Some(m) = RE_JUST_NOW.find(q) {
        return Some((
            (now - Duration::minutes(3)).timestamp(),
            now.timestamp(),
            0.95,
            vec![(m.start(), m.end())],
        ));
    }
    if let Some(m) = RE_FEW_MINUTES.find(q) {
        return Some((
            (now - Duration::minutes(10)).timestamp(),
            (now - Duration::minutes(2)).timestamp(),
            0.85,
            vec![(m.start(), m.end())],
        ));
    }
    if let Some(captures) = RE_HOURS_AGO.captures(q) {
        let full_match = captures.get(0)?;
        let value = captures.get(1)?.as_str().parse::<i64>().ok().unwrap_or(1);
        let center = now - Duration::hours(value);
        let half_window_minutes = ((value as f64 * 60.0 * 0.15).max(10.0)) as i64;
        return Some((
            (center - Duration::minutes(half_window_minutes)).timestamp(),
            (center + Duration::minutes(half_window_minutes)).timestamp(),
            0.93,
            vec![(full_match.start(), full_match.end())],
        ));
    }
    if let Some(captures) = RE_ABOUT_HOURS_AGO.captures(q) {
        let full_match = captures.get(0)?;
        let value = captures.get(1)?.as_str().parse::<i64>().ok().unwrap_or(1);
        let center = now - Duration::hours(value);
        return Some((
            (center - Duration::minutes(45)).timestamp(),
            (center + Duration::minutes(45)).timestamp(),
            0.80,
            vec![(full_match.start(), full_match.end())],
        ));
    }
    if let Some(m) = RE_HALF_HOUR.find(q) {
        return Some((
            (now - Duration::minutes(35)).timestamp(),
            (now - Duration::minutes(25)).timestamp(),
            0.93,
            vec![(m.start(), m.end())],
        ));
    }
    if let Some(m) = RE_AN_HOUR.find(q) {
        return Some((
            (now - Duration::minutes(75)).timestamp(),
            (now - Duration::minutes(45)).timestamp(),
            0.90,
            vec![(m.start(), m.end())],
        ));
    }
    if let Some(m) = RE_FEW_HOURS.find(q) {
        return Some((
            (now - Duration::hours(5)).timestamp(),
            (now - Duration::minutes(90)).timestamp(),
            0.70,
            vec![(m.start(), m.end())],
        ));
    }
    None
}

fn extract_named_relative_temporal(q: &str) -> Option<(i64, i64, f32, Vec<(usize, usize)>)> {
    let now = Local::now();
    if let Some(m) = RE_LAST_NIGHT.find(q) {
        let yesterday = (now - Duration::days(1)).date_naive();
        return Some((
            local_timestamp(yesterday, 20, 0, 0),
            local_timestamp(now.date_naive(), 3, 0, 0),
            0.90,
            vec![(m.start(), m.end())],
        ));
    }
    if let Some(m) = RE_OTHER_DAY.find(q) {
        let start = (now - Duration::days(4)).date_naive();
        let end = (now - Duration::days(2)).date_naive();
        return Some((
            full_day(start).0,
            full_day(end).1,
            0.60,
            vec![(m.start(), m.end())],
        ));
    }
    if let Some(m) = RE_EARLIER.find(q) {
        return Some((
            local_timestamp(now.date_naive(), 0, 0, 0),
            (now - Duration::hours(1)).timestamp(),
            0.85,
            vec![(m.start(), m.end())],
        ));
    }
    None
}

fn extract_range_temporal(q: &str) -> Option<(i64, i64, f32, Vec<(usize, usize)>)> {
    let now = Local::now();
    if let Some(m) = RE_THIS_WEEK.find(q) {
        let monday = start_of_week(now.date_naive());
        return Some((
            full_day(monday).0,
            now.timestamp(),
            0.92,
            vec![(m.start(), m.end())],
        ));
    }
    if let Some(m) = RE_LAST_WEEK.find(q) {
        let this_monday = start_of_week(now.date_naive());
        let last_monday = this_monday - Duration::days(7);
        let last_sunday = last_monday + Duration::days(6);
        return Some((
            full_day(last_monday).0,
            full_day(last_sunday).1,
            0.92,
            vec![(m.start(), m.end())],
        ));
    }
    if let Some(m) = RE_THIS_MONTH.find(q) {
        let first = NaiveDate::from_ymd_opt(now.year(), now.month(), 1)?;
        return Some((
            full_day(first).0,
            now.timestamp(),
            0.92,
            vec![(m.start(), m.end())],
        ));
    }
    if let Some(m) = RE_LAST_MONTH.find(q) {
        let (year, month) = if now.month() == 1 {
            (now.year() - 1, 12)
        } else {
            (now.year(), now.month() - 1)
        };
        let first = NaiveDate::from_ymd_opt(year, month, 1)?;
        let last = NaiveDate::from_ymd_opt(year, month, last_day_of_month(year, month))?;
        return Some((
            full_day(first).0,
            full_day(last).1,
            0.92,
            vec![(m.start(), m.end())],
        ));
    }
    if let Some(m) = RE_WEEKEND.find(q) {
        let recent_saturday = most_recent_weekday(now.date_naive(), Weekday::Sat, true);
        let start = if q[m.start()..m.end()]
            .to_lowercase()
            .contains("last weekend")
        {
            recent_saturday - Duration::days(7)
        } else {
            recent_saturday
        };
        return Some((
            full_day(start).0,
            full_day(start + Duration::days(1)).1,
            0.90,
            vec![(m.start(), m.end())],
        ));
    }
    if let Some(captures) = RE_DAYS_AGO.captures(q) {
        let full_match = captures.get(0)?;
        let value = captures.get(1)?.as_str().parse::<i64>().ok().unwrap_or(1);
        let day = (now - Duration::days(value)).date_naive();
        return Some((
            full_day(day).0,
            full_day(day).1,
            0.92,
            vec![(full_match.start(), full_match.end())],
        ));
    }
    if let Some(captures) = RE_WEEKS_AGO.captures(q) {
        let full_match = captures.get(0)?;
        let value = captures.get(1)?.as_str().parse::<i64>().ok().unwrap_or(1);
        let week_start = start_of_week(now.date_naive()) - Duration::days(value * 7);
        return Some((
            full_day(week_start).0,
            full_day(week_start + Duration::days(6)).1,
            0.92,
            vec![(full_match.start(), full_match.end())],
        ));
    }
    if let Some(captures) = RE_PAST_DAYS.captures(q) {
        let full_match = captures.get(0)?;
        let value = captures.get(1)?.as_str().parse::<i64>().ok().unwrap_or(1);
        return Some((
            (now - Duration::days(value)).timestamp(),
            now.timestamp(),
            0.92,
            vec![(full_match.start(), full_match.end())],
        ));
    }
    if let Some(captures) = RE_PAST_HOURS.captures(q) {
        let full_match = captures.get(0)?;
        let value = captures.get(1)?.as_str().parse::<i64>().ok().unwrap_or(1);
        return Some((
            (now - Duration::hours(value)).timestamp(),
            now.timestamp(),
            0.92,
            vec![(full_match.start(), full_match.end())],
        ));
    }
    if let Some(m) = RE_WHILE_AGO.find(q) {
        return Some((
            (now - Duration::days(7)).timestamp(),
            (now - Duration::days(1)).timestamp(),
            0.45,
            vec![(m.start(), m.end())],
        ));
    }
    if let Some(m) = RE_NOT_LONG_AGO.find(q) {
        return Some((
            (now - Duration::hours(6)).timestamp(),
            (now - Duration::hours(1)).timestamp(),
            0.50,
            vec![(m.start(), m.end())],
        ));
    }
    if let Some(m) = RE_AGES_AGO.find(q) {
        return Some((
            (now - Duration::days(60)).timestamp(),
            (now - Duration::days(14)).timestamp(),
            0.35,
            vec![(m.start(), m.end())],
        ));
    }
    if let Some(m) = RE_RECENTLY.find(q) {
        return Some((
            (now - Duration::hours(2)).timestamp(),
            now.timestamp(),
            0.70,
            vec![(m.start(), m.end())],
        ));
    }
    None
}

fn extract_clock_time(
    q: &str,
    anchor_date: Option<NaiveDate>,
) -> Option<(i64, i64, f32, (usize, usize))> {
    if let Some(captures) = RE_BETWEEN_HOURS.captures(q) {
        let full_match = captures.get(0)?;
        let start_hour = captures.get(1)?.as_str().parse::<u32>().ok()?;
        let start_minute = captures
            .get(2)
            .and_then(|m| m.as_str().parse::<u32>().ok())
            .unwrap_or(0);
        let start_meridiem = captures.get(3).map(|m| m.as_str());
        let end_hour = captures.get(4)?.as_str().parse::<u32>().ok()?;
        let end_minute = captures
            .get(5)
            .and_then(|m| m.as_str().parse::<u32>().ok())
            .unwrap_or(0);
        let end_meridiem = captures.get(6).map(|m| m.as_str()).or(start_meridiem);
        let date = anchor_date.unwrap_or_else(|| Local::now().date_naive());
        let start_ts = resolve_clock_on(date, start_hour, start_minute, start_meridiem, false)?;
        let end_ts = resolve_clock_on(date, end_hour, end_minute, end_meridiem, false)? + (59 * 60);
        return Some((
            start_ts,
            end_ts,
            0.82,
            (full_match.start(), full_match.end()),
        ));
    }

    if let Some(captures) = RE_BEFORE_AFTER_WORD.captures(q) {
        let full_match = captures.get(0)?;
        let direction = captures.get(1)?.as_str().to_lowercase();
        let word = captures.get(2)?.as_str().to_lowercase();
        let date = anchor_date.unwrap_or_else(|| Local::now().date_naive());
        let (hour, minute) = match word.as_str() {
            "noon" => (12, 0),
            "lunch" => (13, 0),
            "work" => (17, 0),
            _ => (12, 0),
        };
        let pivot = local_timestamp(date, hour, minute, 0);
        let result = if direction == "before" {
            (local_timestamp(date, 0, 0, 0), pivot)
        } else {
            (pivot, full_day(date).1)
        };
        return Some((
            result.0,
            result.1,
            0.80,
            (full_match.start(), full_match.end()),
        ));
    }

    if let Some(captures) = RE_CLOCK.captures(q) {
        let full_match = captures.get(0)?;
        let prefix = captures
            .get(1)
            .map(|m| m.as_str().to_lowercase())
            .unwrap_or_default();
        let hour = captures.get(2)?.as_str().parse::<u32>().ok()?;
        let minute = captures
            .get(3)
            .and_then(|m| m.as_str().parse::<u32>().ok())
            .unwrap_or(0);
        let meridiem = captures.get(4).map(|m| m.as_str())?;
        let (after, before) = if let Some(date) = anchor_date {
            let pivot = resolve_clock_on(date, hour, minute, Some(meridiem), false)?;
            if prefix == "before" {
                (local_timestamp(date, 0, 0, 0), pivot)
            } else if prefix == "after" {
                (pivot, full_day(date).1)
            } else {
                let tolerance = if prefix == "around" || prefix == "about" {
                    45 * 60
                } else {
                    20 * 60
                };
                (pivot - tolerance, pivot + tolerance)
            }
        } else {
            resolve_clock_without_anchor(hour, minute, meridiem, &prefix)?
        };
        let confidence = if prefix == "around" || prefix == "about" {
            0.82
        } else {
            0.88
        };
        return Some((
            after,
            before,
            confidence,
            (full_match.start(), full_match.end()),
        ));
    }

    if let Some(captures) = RE_CLOCK_AMBIG.captures(q) {
        let full_match = captures.get(0)?;
        let hour = captures.get(1)?.as_str().parse::<u32>().ok()?;
        let minute = captures
            .get(2)
            .and_then(|m| m.as_str().parse::<u32>().ok())
            .unwrap_or(0);
        if !(1..=12).contains(&hour) {
            return None;
        }
        let meridiem = if (7..=11).contains(&hour) { "am" } else { "pm" };
        let date = anchor_date.unwrap_or_else(|| Local::now().date_naive());
        let pivot = resolve_clock_on(date, hour, minute, Some(meridiem), false)?;
        return Some((
            pivot - 3600,
            pivot + 3600,
            0.65,
            (full_match.start(), full_match.end()),
        ));
    }

    None
}

fn resolve_clock_without_anchor(
    hour: u32,
    minute: u32,
    meridiem: &str,
    prefix: &str,
) -> Option<(i64, i64)> {
    let now = Local::now();
    let today = now.date_naive();
    let mut date = today;
    let mut pivot = resolve_clock_on(date, hour, minute, Some(meridiem), false)?;
    if pivot > now.timestamp() {
        date = today - Duration::days(1);
        pivot = resolve_clock_on(date, hour, minute, Some(meridiem), false)?;
    }
    if prefix == "before" {
        Some((local_timestamp(date, 0, 0, 0), pivot))
    } else if prefix == "after" {
        Some((pivot, full_day(date).1))
    } else {
        let tolerance = if prefix == "around" || prefix == "about" {
            45 * 60
        } else {
            20 * 60
        };
        Some((pivot - tolerance, pivot + tolerance))
    }
}

fn resolve_clock_on(
    date: NaiveDate,
    hour: u32,
    minute: u32,
    meridiem: Option<&str>,
    fallback_pm_for_small_hours: bool,
) -> Option<i64> {
    let normalized = if let Some(meridiem) = meridiem {
        let mut hour_24 = hour % 12;
        if meridiem.eq_ignore_ascii_case("pm") {
            hour_24 += 12;
        }
        if meridiem.eq_ignore_ascii_case("am") && hour == 12 {
            hour_24 = 0;
        }
        hour_24
    } else if fallback_pm_for_small_hours && (1..=6).contains(&hour) {
        hour + 12
    } else {
        hour
    };
    Some(local_timestamp(date, normalized, minute, 0))
}

fn period_bounds(date: NaiveDate, period: PeriodRange) -> (i64, i64) {
    let start = local_timestamp(date, period.start_hour, period.start_minute, 0);
    let end_date = if period.crosses_midnight {
        date + Duration::days(1)
    } else {
        date
    };
    let end = local_timestamp(end_date, period.end_hour, period.end_minute, 0);
    (start, end)
}

fn resolve_weekday(today: NaiveDate, weekday: Weekday, modifier: Option<&str>) -> NaiveDate {
    match modifier {
        Some("this") => {
            let this_week =
                start_of_week(today) + Duration::days(weekday.num_days_from_monday() as i64);
            if this_week <= today {
                this_week
            } else {
                this_week - Duration::days(7)
            }
        }
        _ => most_recent_weekday(today, weekday, false),
    }
}

fn most_recent_weekday(today: NaiveDate, weekday: Weekday, allow_today: bool) -> NaiveDate {
    let mut date = if allow_today {
        today
    } else {
        today - Duration::days(1)
    };
    for _ in 0..7 {
        if date.weekday() == weekday {
            return date;
        }
        date -= Duration::days(1);
    }
    today - Duration::days(7)
}

fn start_of_week(date: NaiveDate) -> NaiveDate {
    date - Duration::days(date.weekday().num_days_from_monday() as i64)
}

fn full_day(date: NaiveDate) -> (i64, i64) {
    (
        local_timestamp(date, 0, 0, 0),
        local_timestamp(date, 23, 59, 59),
    )
}

fn local_timestamp(date: NaiveDate, hour: u32, minute: u32, second: u32) -> i64 {
    let naive = date.and_time(NaiveTime::from_hms_opt(hour, minute, second).unwrap());
    match Local.from_local_datetime(&naive) {
        chrono::LocalResult::Single(dt) => dt.timestamp(),
        chrono::LocalResult::Ambiguous(dt, _) => dt.timestamp(),
        chrono::LocalResult::None => Local.from_utc_datetime(&naive).timestamp(),
    }
}

fn last_day_of_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

fn parse_weekday(name: &str) -> Option<Weekday> {
    match name {
        "monday" | "mon" => Some(Weekday::Mon),
        "tuesday" | "tue" => Some(Weekday::Tue),
        "wednesday" | "wed" => Some(Weekday::Wed),
        "thursday" | "thu" => Some(Weekday::Thu),
        "friday" | "fri" => Some(Weekday::Fri),
        "saturday" | "sat" => Some(Weekday::Sat),
        "sunday" | "sun" => Some(Weekday::Sun),
        _ => None,
    }
}

/// Returns (keywords, enriched_semantic) where enriched includes intent expansion terms.
/// The enriched semantic is used for embedding, giving the model better context.
/// original_query is used for intent phrase detection (before content_type extraction).
fn semantic_keywords_with_enrichment(
    text: &str,
    content_type: Option<&str>,
    original_query: &str,
) -> (Vec<String>, String) {
    let mut keywords = text
        .split_whitespace()
        .map(|part| {
            part.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|word| word.len() >= 2 && !STOPWORDS.contains(&word.as_str()))
        .fold(Vec::<String>::new(), |mut acc, word| {
            if !acc.iter().any(|existing| existing == &word) {
                acc.push(word);
            }
            acc
        });

    let mut enriched = text.to_string();
    // Use original query for intent phrase detection (before content_type extraction removed words)
    let original_lower = original_query.to_lowercase();

    for (phrases, expansions) in INTENT_EXPANSIONS {
        if phrases.iter().any(|phrase| original_lower.contains(phrase)) {
            for expansion in *expansions {
                push_keyword(&mut keywords, expansion);
                // Add expansion to enriched semantic for better embedding
                if !enriched.to_lowercase().contains(expansion) {
                    enriched.push(' ');
                    enriched.push_str(expansion);
                }
            }
        }
    }

    if keywords.iter().any(|keyword| keyword == "flight") {
        for extra in ["boarding", "pass", "itinerary", "gate"] {
            push_keyword(&mut keywords, extra);
            if !enriched.to_lowercase().contains(extra) {
                enriched.push(' ');
                enriched.push_str(extra);
            }
        }
    }

    if keywords.iter().any(|keyword| keyword == "link") || content_type == Some("url") {
        for extra in ["url", "https", "website"] {
            push_keyword(&mut keywords, extra);
        }
    }

    if content_type == Some("code") || keywords.iter().any(|keyword| keyword == "snippet") {
        for extra in ["code", "function", "block"] {
            push_keyword(&mut keywords, extra);
        }
    }

    (keywords, enriched)
}

fn push_keyword(keywords: &mut Vec<String>, value: &str) {
    let value = value.to_lowercase();
    if value.len() < 2 || STOPWORDS.contains(&value.as_str()) {
        return;
    }
    if !keywords.iter().any(|existing| existing == &value) {
        keywords.push(value);
    }
}

fn normalize_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn remove_spans(text: &str, spans: &[(usize, usize)]) -> String {
    let mut sorted = spans.to_vec();
    sorted.sort_by(|a, b| b.0.cmp(&a.0));
    let mut output = text.to_string();
    for (start, end) in sorted {
        if start < end && end <= output.len() {
            output.replace_range(start..end, " ");
        }
    }
    normalize_whitespace(&output)
}

fn is_language_alias(candidate: &str) -> bool {
    LANGUAGE_ALIASES
        .iter()
        .any(|(_, aliases)| aliases.iter().any(|alias| *alias == candidate))
}

fn clean_source_candidate(candidate: &str) -> String {
    let cleaned = normalize_whitespace(candidate).to_lowercase();
    let stop_tokens = [
        " yesterday",
        " today",
        " last ",
        " this ",
        " around ",
        " about ",
        " at ",
        " before ",
        " after ",
    ];
    for stop in stop_tokens {
        if let Some(index) = cleaned.find(stop) {
            return cleaned[..index].trim().to_string();
        }
    }
    cleaned
}

/// Detect language from script characters (CJK, Arabic, Cyrillic, etc.)
/// Used by clipboard watcher to tag captured clips with language.
pub fn detect_script_language(text: &str) -> Option<&'static str> {
    for ch in text.chars() {
        match ch as u32 {
            0x3040..=0x309F | 0x30A0..=0x30FF => return Some("ja"),
            0xAC00..=0xD7AF => return Some("ko"),
            0x0600..=0x06FF => return Some("ar"),
            0x0400..=0x04FF => return Some("ru"),
            0x0900..=0x097F => return Some("hi"),
            0x0E00..=0x0E7F => return Some("th"),
            0x0370..=0x03FF => return Some("el"),
            0x0590..=0x05FF => return Some("he"),
            0x4E00..=0x9FFF => return Some("zh"),
            _ => {}
        }
    }
    None
}

pub fn detect_language(text: &str) -> Option<&'static str> {
    detect_script_language(text).or_else(|| detect_latin_language(text))
}

fn detect_latin_language(text: &str) -> Option<&'static str> {
    const LATIN_HINTS: &[(&str, &[&str], &[char])] = &[
        (
            "en",
            &[
                "the", "and", "for", "with", "from", "this", "that", "your", "please", "thanks",
            ],
            &[],
        ),
        (
            "es",
            &[
                "el", "la", "de", "que", "para", "con", "por", "una", "hola", "gracias",
            ],
            &['ñ', 'á', 'é', 'í', 'ó', 'ú'],
        ),
        (
            "fr",
            &[
                "le", "la", "de", "et", "pour", "avec", "une", "bonjour", "merci", "vous",
            ],
            &[
                'à', 'â', 'ç', 'é', 'è', 'ê', 'ë', 'î', 'ï', 'ô', 'ù', 'û', 'ü',
            ],
        ),
        (
            "de",
            &[
                "der", "die", "das", "und", "mit", "für", "nicht", "ein", "eine", "danke",
            ],
            &['ä', 'ö', 'ü', 'ß'],
        ),
        (
            "pt",
            &[
                "de", "que", "para", "com", "uma", "não", "você", "obrigado", "olá",
            ],
            &['ã', 'õ', 'á', 'â', 'ê', 'é', 'í', 'ó', 'ô', 'ú', 'ç'],
        ),
        (
            "it",
            &[
                "di", "che", "per", "con", "una", "non", "sono", "come", "ciao", "grazie",
            ],
            &['à', 'è', 'é', 'ì', 'ò', 'ù'],
        ),
    ];

    let lower = text.to_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return None;
    }

    let tokens = lower
        .split(|c: char| !c.is_alphabetic() && c != '\'' && c != '’')
        .filter(|token| token.len() >= 2)
        .take(80)
        .collect::<Vec<_>>();
    if tokens.len() < 3 {
        return None;
    }

    let mut best: Option<(&str, usize)> = None;
    for (code, stopwords, accents) in LATIN_HINTS {
        let stopword_hits = stopwords
            .iter()
            .filter(|needle| tokens.iter().any(|token| token == *needle))
            .count();
        let accent_hits = accents
            .iter()
            .filter(|needle| lower.contains(**needle))
            .count();
        let score = stopword_hits * 2 + accent_hits;
        match best {
            Some((_, best_score)) if score <= best_score => {}
            _ => best = Some((code, score)),
        }
    }

    match best {
        Some((code, score)) if score >= 3 => Some(code),
        Some(("en", score)) if score >= 2 => Some("en"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_source_app_without_temporal_tail() {
        let parsed = parse_query("from Arc");
        assert!(parsed.source_apps.iter().any(|app| app == "arc"));
        assert!(parsed.semantic.is_empty());
    }

    #[test]
    fn ambiguous_from_phrase_stays_semantic() {
        let parsed = parse_query("from australia");
        assert!(parsed.source_apps.is_empty());
        assert!(parsed.source_hints.iter().any(|hint| hint == "australia"));
        assert!(parsed.semantic.contains("australia"));
        assert!(parsed.keywords.contains(&"australia".to_string()));
    }

    #[test]
    fn unknown_source_app_becomes_hint() {
        let parsed = parse_query("from File Explorer");
        assert!(parsed.source_apps.is_empty());
        assert!(
            parsed
                .source_hints
                .iter()
                .any(|hint| hint == "file explorer")
        );
        assert!(parsed.semantic.to_lowercase().contains("file explorer"));
    }

    #[test]
    fn parses_language_and_semantic() {
        let parsed = parse_query("auth code in japanese");
        assert!(parsed.languages.iter().any(|lang| lang == "ja"));
        // Semantic is enriched with intent expansion terms
        assert!(parsed.semantic.contains("auth code"));
        assert!(parsed.semantic.contains("verification"));
    }

    #[test]
    fn parses_compound_day_period() {
        let parsed = parse_query("monday morning");
        assert!(parsed.temporal_after.is_some());
        assert!(parsed.temporal_before.is_some());
        assert!(parsed.has_temporal);
    }

    #[test]
    fn detects_latin_languages() {
        assert_eq!(
            detect_language("Gracias por tu ayuda con la reserva"),
            Some("es")
        );
        assert_eq!(
            detect_language("Please send the boarding pass for this flight"),
            Some("en")
        );
    }

    #[test]
    fn parses_relative_minutes() {
        let parsed = parse_query("10m ago");
        assert!(parsed.temporal_after.is_some());
        assert!(parsed.temporal_before.is_some());
        assert!(parsed.semantic.is_empty());
    }

    #[test]
    fn temporal_only_query_clears_stopword_semantic() {
        let parsed = parse_query("from yesterday");
        assert!(
            parsed.semantic.is_empty(),
            "semantic should be empty when it's only stopwords after temporal extraction"
        );
        assert!(parsed.has_temporal);
        assert!(parsed.keywords.is_empty());
    }

    #[test]
    fn intent_expansion_enriches_keywords() {
        let parsed = parse_query("flight info");
        assert!(
            parsed.keywords.contains(&"boarding".to_string()),
            "should contain 'boarding'"
        );
        assert!(
            parsed.keywords.contains(&"itinerary".to_string()),
            "should contain 'itinerary'"
        );
        assert!(
            parsed.keywords.contains(&"gate".to_string()),
            "should contain 'gate'"
        );
    }

    #[test]
    fn intent_expansion_enriches_semantic_for_embedding() {
        let parsed = parse_query("flight info");
        assert!(
            parsed.semantic.contains("boarding"),
            "semantic should contain 'boarding'"
        );
        assert!(
            parsed.semantic.contains("itinerary"),
            "semantic should contain 'itinerary'"
        );
    }

    #[test]
    fn temporal_with_semantic_preserves_text() {
        let parsed = parse_query("recipe from yesterday");
        // Semantic is enriched with expansion terms
        assert!(parsed.semantic.contains("recipe"), "should contain recipe");
        assert!(parsed.has_temporal);
        assert!(parsed.keywords.contains(&"recipe".to_string()));
    }

    #[test]
    fn source_app_with_temporal_works() {
        let parsed = parse_query("from slack yesterday");
        assert!(parsed.source_apps.iter().any(|a| a == "slack"));
        assert!(parsed.has_temporal);
    }

    #[test]
    fn auth_code_expansion() {
        let parsed = parse_query("auth code");
        assert!(parsed.keywords.contains(&"verification".to_string()));
        assert!(parsed.keywords.contains(&"token".to_string()));
    }

    #[test]
    fn error_expansion() {
        let parsed = parse_query("error message");
        assert!(parsed.keywords.contains(&"exception".to_string()));
        assert!(parsed.keywords.contains(&"traceback".to_string()));
    }

    #[test]
    fn receipt_expansion() {
        let parsed = parse_query("receipt");
        assert!(parsed.keywords.contains(&"payment".to_string()));
        assert!(parsed.keywords.contains(&"total".to_string()));
    }

    #[test]
    fn code_block_expansion() {
        let parsed = parse_query("code block");
        assert!(parsed.keywords.contains(&"snippet".to_string()));
        assert!(parsed.keywords.contains(&"function".to_string()));
    }

    #[test]
    fn meeting_link_expansion() {
        let parsed = parse_query("meeting link");
        assert!(parsed.keywords.contains(&"zoom".to_string()));
        assert!(parsed.keywords.contains(&"invite".to_string()));
    }

    #[test]
    fn todo_list_expansion() {
        let parsed = parse_query("todo list");
        assert!(parsed.keywords.contains(&"task".to_string()));
        assert!(parsed.keywords.contains(&"checkbox".to_string()));
    }

    #[test]
    fn docker_expansion() {
        let parsed = parse_query("docker");
        assert!(parsed.keywords.contains(&"container".to_string()));
        assert!(parsed.keywords.contains(&"dockerfile".to_string()));
    }

    #[test]
    fn endpoint_is_content_type() {
        let parsed = parse_query("endpoint");
        // "endpoint" is extracted as content_type="code" (matches RE_CONTENT_TYPE_CODE)
        assert_eq!(parsed.content_type, Some("code".to_string()));
    }

    #[test]
    fn address_expansion() {
        let parsed = parse_query("address");
        assert!(parsed.keywords.contains(&"street".to_string()));
        assert!(parsed.keywords.contains(&"zip".to_string()));
    }

    #[test]
    fn tracking_number_expansion() {
        let parsed = parse_query("tracking number");
        assert!(parsed.keywords.contains(&"shipment".to_string()));
        assert!(parsed.keywords.contains(&"delivery".to_string()));
    }

    #[test]
    fn password_expansion() {
        let parsed = parse_query("password");
        assert!(parsed.keywords.contains(&"username".to_string()));
        assert!(parsed.keywords.contains(&"account".to_string()));
    }

    #[test]
    fn calendar_event_expansion() {
        let parsed = parse_query("calendar");
        assert!(parsed.keywords.contains(&"schedule".to_string()));
        assert!(parsed.keywords.contains(&"date".to_string()));
    }

    #[test]
    fn screenshot_is_content_type() {
        let parsed = parse_query("screenshot");
        // "screenshot" is extracted as content_type="image", not as a keyword
        assert_eq!(parsed.content_type, Some("image".to_string()));
    }

    #[test]
    fn git_commit_expansion() {
        let parsed = parse_query("git");
        assert!(parsed.keywords.contains(&"repository".to_string()));
        assert!(parsed.keywords.contains(&"merge".to_string()));
    }

    #[test]
    fn spreadsheet_expansion() {
        let parsed = parse_query("spreadsheet");
        assert!(parsed.keywords.contains(&"cells".to_string()));
        assert!(parsed.keywords.contains(&"formula".to_string()));
    }

    #[test]
    fn presentation_expansion() {
        let parsed = parse_query("presentation");
        assert!(parsed.keywords.contains(&"slide".to_string()));
        assert!(parsed.keywords.contains(&"deck".to_string()));
    }

    #[test]
    fn chat_dm_expansion() {
        let parsed = parse_query("chat");
        assert!(parsed.keywords.contains(&"message".to_string()));
        assert!(parsed.keywords.contains(&"conversation".to_string()));
    }

    #[test]
    fn playlist_music_expansion() {
        let parsed = parse_query("playlist");
        assert!(parsed.keywords.contains(&"spotify".to_string()));
        assert!(parsed.keywords.contains(&"track".to_string()));
    }

    #[test]
    fn privacy_policy_expansion() {
        let parsed = parse_query("privacy policy");
        assert!(parsed.keywords.contains(&"privacy".to_string()));
        assert!(parsed.keywords.contains(&"legal".to_string()));
    }

    #[test]
    fn staging_server_expansion() {
        let parsed = parse_query("staging server ip address");
        assert!(parsed.keywords.contains(&"ip".to_string()));
        assert!(parsed.keywords.contains(&"ssh".to_string()));
    }

    #[test]
    fn apology_email_expansion() {
        let parsed = parse_query("apology email");
        assert!(parsed.keywords.contains(&"sorry".to_string()));
        assert!(parsed.keywords.contains(&"followup".to_string()));
    }

    #[test]
    fn tax_expansion() {
        let parsed = parse_query("the number i copied when i was doing taxes");
        assert!(parsed.keywords.contains(&"ssn".to_string()));
        assert!(parsed.keywords.contains(&"refund".to_string()));
    }

    #[test]
    fn cross_signal_query() {
        let parsed = parse_query("auth code from slack yesterday");
        assert!(parsed.keywords.contains(&"verification".to_string()));
        assert!(parsed.source_apps.iter().any(|a| a == "slack"));
        assert!(parsed.has_temporal);
    }

    #[test]
    fn url_from_source() {
        let parsed = parse_query("url from slack");
        assert_eq!(parsed.content_type, Some("url".to_string()));
        assert!(parsed.source_apps.iter().any(|a| a == "slack"));
    }

    #[test]
    fn code_from_vscode() {
        let parsed = parse_query("code from vscode");
        // "code" matches both content_type and source_app, source_app wins
        // because it's a more specific filter
        assert!(parsed.source_apps.iter().any(|a| a == "visual studio code"));
    }
}
