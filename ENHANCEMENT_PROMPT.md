We are improving the search pipeline in Copi — a clipboard manager built with Tauri 2, Rust, SQLite (FTS5 + sqlite-vec/vec0), and GTE-multilingual-base ONNX (768d embeddings). No LLM. No external APIs. Everything is deterministic Rust.

The goal: make search feel genuinely magical. When a user types "around 7am", "japanese from notion last week", "that auth code thing", "10m ago", or "from Arc yesterday" — Copi must understand it completely and return the right clips. Currently it fails most of these. We are fixing that now.

=============================================================
EXISTING FILES TO UNDERSTAND BEFORE TOUCHING ANYTHING
=============================================================

src-tauri/src/query_parser.rs
  — Current ParsedQuery struct has: semantic, temporal_after, temporal_before,
    content_type, source_app, has_temporal
  — MANY temporal patterns currently return (None, None) and do nothing.
    Day names, "X minutes ago", clock times ("around 7am"), "10m ago"
    are ALL broken or missing.
  — extract_source_app regex requires the app name to be followed by a
    temporal keyword — so "from Arc" alone never matches. Broken.
  — No language detection. No semantic expansion. No fuzzy app matching.

src-tauri/src/search.rs
  — search_clips takes query: String, filter: String
  — Calls query_parser::parse_query, then runs FTS5, vec0, LIKE fallback
  — Uses simple RRF (Reciprocal Rank Fusion) with k=60
  — Temporal filter applied as raw SQL WHERE clause injection
  — Weights are not dynamic — FTS, vec, LIKE all treated equally

src-tauri/src/embed.rs
  — embed_text() and embed_query() are the same function
  — Model is GTE-multilingual-base, 768 dimensions
  — Already normalises embeddings (L2 norm)
  — embed_query is what search.rs calls for the semantic query

src-tauri/src/db.rs
  — clips table: id, content, content_hash, content_type, source_app,
    source_app_icon, content_highlighted, ocr_text, image_data,
    image_thumbnail, image_width, image_height, created_at, pinned,
    collection_id
  — clips_fts: FTS5 virtual table over content + ocr_text
  — clip_embeddings: vec0 virtual table, float[768]
  — content_type is constrained: 'text' | 'url' | 'code' | 'image'

src-tauri/src/macos.rs
  — get_frontmost_app_info() returns FrontmostApp { name, bundle_id, path }
  — Uses localizedName() — already correct

=============================================================
TASK 1 — REWRITE query_parser.rs COMPLETELY
=============================================================

Replace the entire file. The new ParsedQuery must be:

```rust
#[derive(Debug, Clone)]
pub struct ParsedQuery {
    pub semantic: String,           // expanded string for embedding
    pub keywords: Vec,      // exact terms for FTS boost
    pub temporal_after: Option,
    pub temporal_before: Option,
    pub temporal_confidence: f32,   // 0.0–1.0, used for score weighting
    pub content_type: Option,
    pub source_apps: Vec,   // multiple candidates now
    pub languages: Vec,     // ISO 639-1 codes e.g. ["ja", "zh"]
    pub is_pinned: Option,
    pub min_length: Option,
    pub is_multiline: Option,
    pub ordering: Option,
    pub has_temporal: bool,
    pub query_is_empty_after_parse: bool, // true if only filters, no semantic
}

#[derive(Debug, Clone)]
pub enum Ordering {
    Newest,
    Oldest,
    SecondNewest,
}
```

------------------------------------------------------------
SECTION A: TEMPORAL PARSING — implement ALL of these patterns
------------------------------------------------------------

Use chrono::Local::now() for all relative calculations.
Each pattern returns (from: i64, to: i64, confidence: f32, consumed_span: (usize, usize)).

IMPORTANT: After matching a temporal token, REMOVE it from the
remaining string before passing to subsequent stages. Currently
the code removes it correctly — keep that behaviour.

ALSO IMPORTANT: When combining a day name + time-of-day
(e.g. "monday morning"), both tokens must be consumed and
combined into a single range. Handle this BEFORE individual
token matching.

--- Sub-minute patterns (confidence 0.97) ---
  "Xm ago" | "X min ago" | "X mins ago" | "X minute ago" | "X minutes ago"
    → now minus X minutes, window = now-(X*1.3)min to now-(X*0.7)min
    Note: "Xm ago" with no space (e.g. "10m ago") must work — regex:
    r"(?i)\b(\d+)\s*m(?:in(?:utes?)?)?\s+ago\b"

  "Xs ago" | "X sec ago" | "X seconds ago"
    → now minus X seconds, ±15 seconds

  "just now" | "a moment ago" | "moments ago"
    → now-3min to now, confidence 0.95

  "a few minutes ago" | "couple minutes ago"
    → now-10min to now-2min, confidence 0.85

--- Hour patterns (confidence 0.93) ---
  "Xhr ago" | "X hour ago" | "X hours ago" | "Xh ago"
    → now minus X hours, window ±(X * 0.15) hours minimum ±10min
    regex: r"(?i)\b(\d+)\s*h(?:ours?)?\s+ago\b"

  "around X hours ago" | "about X hours ago" | "roughly X hours ago"
    → now minus X hours, window ±45min, confidence 0.80

  "half an hour ago" | "half hour ago"
    → now-35min to now-25min

  "an hour ago" | "about an hour ago" | "an hour or so ago"
    → now-75min to now-45min

  "a couple hours ago" | "a few hours ago"
    → now-5hr to now-1.5hr, confidence 0.70

--- Clock time patterns (confidence 0.88) ---
  These must handle BOTH today and yesterday intelligently.
  If the resolved time is in the future, use yesterday.

  "at Xam" | "at X am" | "at X:YYam"
    → resolve to that clock time today or yesterday, ±20min

  "at Xpm" | "at X pm" | "at X:YYpm"
    → resolve to that clock time, ±20min

  "around Xam" | "around X am"
    → that clock time ±45min, confidence 0.82

  "around Xpm" | "around X:YYpm"
    → that clock time ±45min, confidence 0.82

  "about X" (with no am/pm, X is 1-12)
    → ambiguous: if 7-11 treat as AM, if 1-6 treat as PM,
      window ±1hr, confidence 0.65

  "before Xpm" | "before noon" | "before lunch"
    → today/yesterday 00:00 to that time

  "after Xpm" | "after lunch" | "after work"
    → that time to 23:59

  "between X and Y" (e.g. "between 2 and 4")
    → X:00 to Y:59, resolve am/pm by context

  regex for clock time: r"(?i)\b(?:around\s+|about\s+|at\s+)?(\d{1,2})(?::(\d{2}))?\s*(am|pm)\b"
  Also handle: r"(?i)\b(?:around\s+|about\s+)?(\d{1,2})(?::(\d{2}))?\s*(?:o'?clock)?\b"

--- Named day patterns (confidence 0.95) ---
  CURRENTLY BROKEN — these return (None, None). Fix this.

  "monday" | "on monday" | "last monday"
    → find the most recent past Monday, full day 00:00–23:59
    Use find_last_weekday() which already exists — just wire it up correctly.

  "this monday" → if today is after Monday this week, use this week's Monday.
                  if today IS Monday, use today.

  Abbreviations: "mon", "tue", "wed", "thu", "fri", "sat", "sun"
    → same logic as full names

  Combined with time-of-day (handle FIRST before individual patterns):
    "monday morning" → last Monday 05:00–12:00
    "tuesday afternoon" → last Tuesday 12:00–17:00
    "friday evening" → last Friday 17:00–21:00
    "saturday night" → last Saturday 21:00–23:59

--- Named relative day patterns (confidence 0.98) ---
  "yesterday" → yesterday 00:00–23:59  (ALREADY WORKS)
  "today" → today 00:00–now  (ALREADY WORKS)
  "last night" → yesterday 20:00 to today 03:00
  "the other day" → 2–4 days ago, full days, confidence 0.60
  "earlier today" | "earlier" (standalone) → today 00:00 to now-1hr

--- Time-of-day named periods (confidence 0.85) ---
  "this morning" → today 05:00–12:00  (ALREADY WORKS)
  "this afternoon" → today 12:00–17:00  (ALREADY WORKS)
  "this evening" | "tonight" → today 17:00–23:59  (ALREADY WORKS)
  "early morning" → 05:00–08:00
  "mid morning" → 08:00–10:30
  "late morning" → 10:30–12:00
  "at lunch" | "lunchtime" | "at noon" → 11:30–13:30
  "after lunch" → 13:00–15:00
  "late afternoon" → 15:00–17:30
  "evening" (standalone) → 17:00–21:00
  "night" (standalone) → 21:00–23:59
  "midnight" → 23:00 to 01:00 spanning the day boundary
  "dawn" | "sunrise" → 05:00–07:30
  "dusk" | "sunset" → 17:30–20:30

--- Week/month/range patterns (confidence 0.92) ---
  "this week" → this Monday 00:00 to now
  "last week" → last Mon 00:00 to last Sun 23:59  (ALREADY WORKS)
  "this month" → 1st of current month 00:00 to now
  "last month" → 1st of previous month to last day of previous month 23:59
  "the weekend" → most recent Saturday 00:00 to Sunday 23:59
  "this weekend" → same
  "last weekend" → the Saturday before most recent Saturday
  "X days ago" → that calendar day 00:00–23:59  (ALREADY WORKS — verify)
  "X weeks ago" → that Mon 00:00 to that Sun 23:59
  "past X days" | "last X days" → now-X*24hr to now
  "past X hours" | "last X hours" → now-X*hr to now

--- Vague ordering (confidence 0.40–0.55) ---
  "a while ago" | "a while back" → 1–7 days ago, confidence 0.45
  "not long ago" | "not too long ago" → 1hr–6hr ago, confidence 0.50
  "ages ago" | "a long time ago" → 14–60 days ago, confidence 0.35
  "recently" → last 2hr, confidence 0.70  (CURRENTLY last 24hr — tighten this)

--- Compound temporal (handle BEFORE individual patterns) ---
  If query contains BOTH a day name AND a time-of-day:
  "yesterday morning", "tuesday around 3pm", "last monday evening"
  → combine: the day's date + the time-of-day range

Implementation note: Run compound patterns first, then individual.
Use a priority list. Once a span is consumed, mark those char
indices as used so later patterns don't double-match.

------------------------------------------------------------
SECTION B: SOURCE APP DETECTION — complete rewrite
------------------------------------------------------------

The current regex only matches "from X" when X is followed by
a temporal word. This is WRONG. "from Arc" alone must work.

New approach — two-phase:

Phase 1: Pattern match for preposition + identifier
  regex: r"(?i)\b(?:from|in|via|on|copied\s+from|pasted\s+from)\s+([A-Za-z][A-Za-z0-9\s]{1,30}?)(?=\s+(?:yesterday|today|last|this|around|at|\d)|$)"

  This is STILL too strict. Also try:
  regex: r"(?i)\b(?:from|in|via)\s+([A-Za-z][A-Za-z0-9]{1,25})\b"
  and validate the captured name against the APP_MAP below.

Phase 2: Look up the captured name in this static map.
Return Vec of bundle ID fragments (not just one).

Build this map as a static slice in the file:

const APP_MAP: &[(&str, &[&str])] = &[
  // Browsers
  ("safari",      &["com.apple.Safari"]),
  ("chrome",      &["com.google.Chrome"]),
  ("firefox",     &["org.mozilla.firefox"]),
  ("arc",         &["company.thebrowser.Browser"]),
  ("brave",       &["com.brave.Browser"]),
  ("edge",        &["com.microsoft.edgemac"]),
  ("browser",     &["Safari", "Chrome", "firefox", "thebrowser", "brave", "edgemac"]),
  ("web",         &["Safari", "Chrome", "firefox", "thebrowser", "brave", "edgemac"]),
  // Notes
  ("notes",       &["com.apple.Notes"]),
  ("note",        &["com.apple.Notes", "notion.id", "md.obsidian", "net.shinyfrog"]),
  ("notion",      &["notion.id"]),
  ("obsidian",    &["md.obsidian"]),
  ("bear",        &["net.shinyfrog.bear"]),
  ("craft",       &["de.lukasklein.craft"]),
  ("logseq",      &["com.logseq"]),
  // Messaging
  ("messages",    &["com.apple.MobileSMS"]),
  ("imessage",    &["com.apple.MobileSMS"]),
  ("slack",       &["com.tinyspeck.slackmacgap"]),
  ("discord",     &["com.hnc.Discord"]),
  ("teams",       &["com.microsoft.teams"]),
  ("telegram",    &["ru.keepcoder.Telegram"]),
  ("whatsapp",    &["net.whatsapp.WhatsApp"]),
  ("signal",      &["org.whispersystems.signal"]),
  ("chat",        &["com.tinyspeck", "com.hnc.Discord", "com.apple.MobileSMS", "ru.keepcoder", "net.whatsapp"]),
  ("dm",          &["com.tinyspeck", "com.hnc.Discord", "com.apple.MobileSMS"]),
  ("texts",       &["com.apple.MobileSMS", "com.beeper"]),
  // Email
  ("mail",        &["com.apple.mail"]),
  ("outlook",     &["com.microsoft.Outlook"]),
  ("spark",       &["com.readdle.spark"]),
  ("email",       &["com.apple.mail", "com.microsoft.Outlook", "com.readdle.spark"]),
  // Dev tools
  ("vscode",      &["com.microsoft.VSCode"]),
  ("code",        &["com.microsoft.VSCode", "com.apple.dt.Xcode"]),
  ("xcode",       &["com.apple.dt.Xcode"]),
  ("terminal",    &["com.apple.Terminal", "com.googlecode.iterm2", "com.mitchellh.ghostty"]),
  ("iterm",       &["com.googlecode.iterm2"]),
  ("ghostty",     &["com.mitchellh.ghostty"]),
  ("warp",        &["dev.warp.Warp-Stable"]),
  ("cursor",      &["com.todesktop.230313mzl4w4u92"]),
  ("vim",         &["org.vim.MacVim", "com.qvacua.VimR"]),
  ("editor",      &["com.microsoft.VSCode", "com.apple.dt.Xcode", "com.todesktop.230313mzl4w4u92"]),
  // Design
  ("figma",       &["com.figma.Desktop"]),
  ("sketch",      &["com.bohemiancoding.sketch3"]),
  ("framer",      &["com.framer.desktop"]),
  ("canva",       &["com.canva.CanvaDesktop"]),
  // Productivity
  ("linear",      &["com.linear.Linear"]),
  ("jira",        &["com.atlassian"]),
  ("notion",      &["notion.id"]),
  ("todoist",     &["com.todoist"]),
  ("things",      &["com.culturedcode.ThingsMac"]),
  ("reminders",   &["com.apple.reminders"]),
  ("calendar",    &["com.apple.iCal"]),
  // Office
  ("word",        &["com.microsoft.Word"]),
  ("excel",       &["com.microsoft.Excel"]),
  ("powerpoint",  &["com.microsoft.Powerpoint"]),
  ("pages",       &["com.apple.iWork.Pages"]),
  ("numbers",     &["com.apple.iWork.Numbers"]),
  ("keynote",     &["com.apple.iWork.Keynote"]),
  ("spreadsheet", &["com.microsoft.Excel", "com.apple.iWork.Numbers"]),
  ("document",    &["com.microsoft.Word", "com.apple.iWork.Pages"]),
  // System / other
  ("finder",      &["com.apple.finder"]),
  ("spotify",     &["com.spotify.client"]),
  ("music",       &["com.apple.Music", "com.spotify.client"]),
  ("photos",      &["com.apple.Photos"]),
  ("maps",        &["com.apple.Maps"]),
  ("shortcuts",   &["com.apple.shortcuts"]),
  // Category aliases
  ("work",        &["com.tinyspeck", "com.microsoft.teams", "com.linear.Linear", "com.microsoft.Outlook"]),
  ("social",      &["com.twitter", "com.facebook", "com.instagram"]),
];

Phase 3: Fuzzy match if exact lookup fails.
  Implement levenshtein(a: &str, b: &str) -> usize inline.
  If the captured name has levenshtein distance <= 2 from any key
  in APP_MAP, use that key's bundle IDs.
  This catches "slakc" → "slack", "notoin" → "notion", etc.

The returned source_apps field is Vec of bundle ID fragments.
In search.rs, the SQL WHERE clause becomes:
  AND (LOWER(c.source_app) LIKE '%frag1%' OR LOWER(c.source_app) LIKE '%frag2%' ...)

------------------------------------------------------------
SECTION C: LANGUAGE DETECTION
------------------------------------------------------------

Add to ParsedQuery: languages: Vec

Two-pass detection:

Pass 1: Named language in query text
  Build this lookup (check against lowercased query):

  ("japanese", "japan", "日本語", "jp", "in japanese", "kanji",
   "hiragana", "katakana") → "ja"
  ("chinese", "mandarin", "cantonese", "中文", "in chinese",
   "simplified chinese", "traditional chinese") → "zh"
  ("korean", "korea", "한국어", "hangul", "in korean") → "ko"
  ("arabic", "عربي", "in arabic") → "ar"
  ("french", "français", "in french", "en français") → "fr"
  ("spanish", "español", "in spanish", "en español") → "es"
  ("german", "deutsch", "in german", "auf deutsch") → "de"
  ("russian", "русский", "in russian", "cyrillic") → "ru"
  ("italian", "italiano", "in italian") → "it"
  ("portuguese", "português", "in portuguese") → "pt"
  ("dutch", "nederlands", "in dutch") → "nl"
  ("hindi", "हिंदी", "in hindi") → "hi"
  ("thai", "ไทย", "in thai") → "th"
  ("turkish", "türkçe", "in turkish") → "tr"
  ("hebrew", "עברית", "in hebrew") → "he"
  ("greek", "ελληνικά", "in greek") → "el"
  ("polish", "polski", "in polish") → "pl"
  ("swedish", "svenska", "in swedish") → "sv"
  ("vietnamese", "tiếng việt", "in vietnamese") → "vi"
  ("indonesian", "bahasa indonesia", "in indonesian") → "id"

Pass 2: Script detection from the query string itself
  (The user may type in the target language)

  fn detect_script_language(text: &str) -> Option<&'static str> {
    for ch in text.chars() {
      match ch as u32 {
        0x3040..=0x309F | 0x30A0..=0x30FF => return Some("ja"),
        0x4E00..=0x9FFF                   => return Some("zh"),
        0xAC00..=0xD7AF                   => return Some("ko"),
        0x0600..=0x06FF                   => return Some("ar"),
        0x0400..=0x04FF                   => return Some("ru"),
        0x0900..=0x097F                   => return Some("hi"),
        0x0E00..=0x0E7F                   => return Some("th"),
        0x0370..=0x03FF                   => return Some("el"),
        0x0590..=0x05FF                   => return Some("he"),
        _ => {}
      }
    }
    None
  }

If a language is detected from the user's own script,
set query_is_foreign = true (add this field to ParsedQuery).
When query_is_foreign = true, keep the raw query text as a
keyword so FTS can match it verbatim.

After detecting language, consume the language tokens from
the remaining string before semantic extraction.

------------------------------------------------------------
SECTION D: CONTENT TYPE DETECTION — extend existing
------------------------------------------------------------

Keep existing patterns. Add:

  "snippet" | "function" | "script" | "json" | "sql" | "query"
  | "bash" | "command" | "python" | "javascript" | "typescript"
  | "api key" | "endpoint" | "regex" | "yaml" | "config" → "code"

  "screenshot" | "picture" | "photo" | "pic" | "img" → "image"

  "link" | "url" | "website" | "webpage" | "domain"
  | "http" | "https" → "url"

  "pinned" | "starred" | "saved" | "favourite" | "favorited"
  → set is_pinned = Some(true), don't set content_type

  "long" | "lengthy" | "that big" → set min_length = Some(300)

  "multiline" | "multi-line" | "multiple lines" | "paragraph"
  | "block of text" → set is_multiline = Some(true)

------------------------------------------------------------
SECTION E: SPECIAL QUERY SHORTCIRCUITS
------------------------------------------------------------

Check these BEFORE running the full pipeline. If matched,
return a ParsedQuery with only the relevant fields set and
query_is_empty_after_parse = true.

  "pinned" | "starred" | "my pins"
    → is_pinned = Some(true)

  "the last thing" | "most recent" | "latest" | "last copied"
    → ordering = Some(Ordering::Newest)

  "the first" | "oldest"
    → ordering = Some(Ordering::Oldest)

  "the one before" | "previous one" | "before this"
    → ordering = Some(Ordering::SecondNewest)

  "" (empty string after trim)
    → query_is_empty_after_parse = true, return defaults

------------------------------------------------------------
SECTION F: SEMANTIC EXPANSION
------------------------------------------------------------

After all filters are extracted, what remains is the semantic
content the user wants. Before passing it to embed_query(),
expand it using this hardcoded map. This is the single most
impactful thing for "magical" feel — it means "auth thing"
finds clips containing JWT/Bearer/credentials the user never typed.

fn expand_semantic(terms: &[&str]) -> String

Build EXPANSIONS as a static slice:

const EXPANSIONS: &[(&str, &[&str])] = &[
  ("auth",         &["auth", "authentication", "login", "password", "token",
                      "JWT", "OAuth", "session", "cookie", "bearer",
                      "credentials", "secret", "API key", "authorize", "hash"]),
  ("meeting",      &["meeting", "call", "zoom", "calendar", "schedule",
                      "invite", "agenda", "standup", "sync", "catchup",
                      "conference", "attendees", "1:1", "discussion"]),
  ("recipe",       &["recipe", "ingredients", "tablespoon", "cup", "oven",
                      "bake", "cook", "preheat", "mix", "serves", "calories",
                      "prep time", "grams"]),
  ("error",        &["error", "exception", "failed", "undefined", "null",
                      "crash", "stack trace", "TypeError", "SyntaxError",
                      "warning", "fatal", "panic", "traceback", "caused by",
                      "at line"]),
  ("address",      &["address", "street", "avenue", "road", "city", "state",
                      "zip", "postcode", "floor", "suite", "apartment",
                      "building", "country"]),
  ("flight",       &["flight", "airline", "departure", "arrival", "gate",
                      "terminal", "booking", "confirmation", "seat",
                      "boarding", "passport", "baggage"]),
  ("money",        &["price", "cost", "$", "€", "£", "payment", "invoice",
                      "total", "amount", "balance", "transfer", "transaction",
                      "revenue", "fee", "pay"]),
  ("contact",      &["email", "phone", "mobile", "@", "LinkedIn", "twitter",
                      "address", "name", "number", "tel", "cell"]),
  ("todo",         &["task", "TODO", "done", "complete", "action", "follow up",
                      "checkbox", "[ ]", "[x]", "next steps", "action items"]),
  ("password",     &["password", "passwd", "credentials", "login", "secret",
                      "key", "token", "auth", "pin"]),
  ("order",        &["order", "tracking", "confirmation", "shipped", "delivery",
                      "package", "item", "purchase", "dispatch"]),
  ("deploy",       &["deploy", "deployment", "CI", "CD", "pipeline", "build",
                      "release", "production", "staging", "docker", "k8s",
                      "kubernetes", "git push", "merge"]),
  ("config",       &["config", "configuration", "settings", "env", ".env",
                      "environment", "variable", "key", "value", "yaml",
                      "toml", "json"]),
  ("sql",          &["SELECT", "INSERT", "UPDATE", "DELETE", "FROM", "WHERE",
                      "JOIN", "table", "database", "query", "schema",
                      "migration", "index"]),
  ("git",          &["git", "commit", "branch", "merge", "pull", "push",
                      "clone", "rebase", "diff", "stash", "checkout"]),
  ("docker",       &["docker", "container", "image", "run", "build",
                      "compose", "dockerfile", "port", "volume"]),
  ("api",          &["API", "endpoint", "REST", "GET", "POST", "PUT",
                      "DELETE", "request", "response", "header", "body",
                      "JSON", "status", "curl"]),
  ("license",      &["license", "MIT", "Apache", "GPL", "copyright",
                      "permission", "rights", "BSD"]),
  ("link",         &["http", "https", "www", "url", "link", "website",
                      ".com", ".io", ".org", ".dev"]),
  ("color",        &["#", "rgb", "hsl", "hex", "color", "colour",
                      "opacity", "alpha", "palette", "rgba"]),
  ("markdown",     &["#", "##", "**", "- ", "```", "markdown", "heading",
                      "list", "bold", "italic"]),
  ("csv",          &["csv", "comma", "column", "row", "data", "export",
                      "import", "spreadsheet", "separator"]),
  ("key",          &["API key", "secret", "token", "private key", "ssh",
                      "rsa", "pk", "sk-", "key"]),
  ("phone",        &["phone", "mobile", "cell", "tel", "+1", "number",
                      "call", "contact", "area code"]),
  ("code snippet", &["function", "def", "const", "let", "var", "class",
                      "import", "return", "async", "await", "=>", "{}", "[]"]),
  ("translation",  &["translate", "translation", "meaning", "in english",
                      "what does", "how to say"]),
  ("ai",           &["AI", "GPT", "Claude", "LLM", "prompt", "model",
                      "response", "generate", "inference", "context"]),
];

Logic:
  1. For each term in remaining_terms (after all filters extracted),
     check if it appears as a key in EXPANSIONS (case-insensitive,
     partial match allowed — "auth thing" should match "auth").
  2. If matched, REPLACE that term with the expansion array joined by space.
  3. If no match, keep the original term.
  4. Join all terms into a single string and pass to embed_query().

IMPORTANT: The semantic string sent to GTE must be a real sentence
or phrase, not a word salad. Format it as:
  "clipboard content about {expanded terms}"
This gives GTE better context for embedding.

Example:
  input remaining: "auth thing"
  expanded: "auth authentication login password token JWT OAuth session
             cookie bearer credentials secret API key authorize hash"
  final semantic: "clipboard content about auth authentication login
                   password token JWT OAuth"
  (truncate to ~100 tokens, the model has a 512 token limit)

------------------------------------------------------------
SECTION G: parse_query() top-level function
------------------------------------------------------------

The new flow in parse_query(raw: &str) -> ParsedQuery:

  1. Check special shortcircuits → return early if matched
  2. Run compound temporal patterns (day + time-of-day combos)
  3. Run individual temporal patterns
  4. Run language detection (named + script)
  5. Run source app extraction (with new regex + APP_MAP lookup)
  6. Run content type / meta extraction
  7. Remaining tokens → run through EXPANSIONS → build semantic string
  8. If semantic is empty but has_temporal or source_apps is non-empty,
     set query_is_empty_after_parse = true
  9. Return ParsedQuery

=============================================================
TASK 2 — UPDATE search.rs TO USE THE NEW ParsedQuery
=============================================================

The new ParsedQuery has source_apps: Vec (was source_app: Option).
Update all SQL generation accordingly.

--- Source app SQL clause ---
Old: AND LOWER(c.source_app) LIKE '%name%'
New:
  fn build_source_app_clause(apps: &[String]) -> String {
    if apps.is_empty() { return String::new(); }
    let conditions: Vec = apps.iter()
      .map(|a| format!("LOWER(c.source_app) LIKE '%{}%'", a.to_lowercase()))
      .collect();
    format!(" AND ({})", conditions.join(" OR "))
  }

--- Replace RRF with weighted scoring ---

The current rrf() function treats all result lists equally.
Replace it with a weighted version that considers temporal_confidence.

New scoring:

  fn weighted_merge(
    fts: Vec,
    vec: Vec,
    like: Vec,
    parsed: &ParsedQuery,
  ) -> Vec

  Weights (adjust dynamically based on what was parsed):

  Case 1: query_is_empty_after_parse = true (pure filter query, no semantic)
    → vec weight = 0.0, fts weight = 0.0
    → Return clips sorted by created_at DESC, filters applied in SQL
    → Don't run embedding at all — skip the embed_query() call

  Case 2: has_temporal = true AND semantic is non-empty
    → fts_weight = 0.30, vec_weight = 0.45, like_weight = 0.05
    → temporal_bonus = temporal_confidence * 0.20
    → Apply temporal_bonus to clips within the time window AFTER scoring

  Case 3: semantic only (no temporal, no source app)
    → fts_weight = 0.35, vec_weight = 0.55, like_weight = 0.10

  Case 4: source_apps non-empty AND semantic non-empty
    → fts_weight = 0.40, vec_weight = 0.50, like_weight = 0.10

  Case 5: single word query
    → fts_weight = 0.55, vec_weight = 0.35, like_weight = 0.10

  Implementation using RRF base formula with per-list weights:
    score(clip) = Σ (weight_i / (k + rank_i))
    where k = 60, weight_i is the case-based weight

  After merging, apply temporal boost:
    if clip.created_at is within [temporal_after, temporal_before]:
      score += temporal_confidence * 0.15
    else if clip is within 2x the window size of the boundary:
      exponential falloff: score += temporal_confidence * 0.08 *
        exp(-distance_seconds / window_size_seconds)

--- Skip embedding when not needed ---

Currently embed_query is always called if model is loaded.
Add this check in search_clips:

  let should_embed = !parsed.query_is_empty_after_parse
    && !parsed.semantic.is_empty()
    && parsed.semantic.split_whitespace().count() > 0;

  let vec_results = if should_embed {
    if let Some(ref model) = state.model {
      do_search_vec(model, &conn, &parsed.semantic, ef, &tc, &sa)
    } else { vec![] }
  } else { vec![] };

--- Keywords field ---

The new ParsedQuery has keywords: Vec.
Use these for FTS in addition to the semantic string.
If keywords is non-empty, run FTS with each keyword separately
and union the results before passing to weighted_merge.

--- Ordering hint ---

If parsed.ordering is Some(Ordering::Newest):
  return search_empty(conn, filter) — first result only
If parsed.ordering is Some(Ordering::Oldest):
  SELECT from clips ORDER BY created_at ASC LIMIT 1
If parsed.ordering is Some(Ordering::SecondNewest):
  SELECT from clips ORDER BY created_at DESC LIMIT 2 → return index 1

--- Language filter in SQL ---

If parsed.languages is non-empty, the clips table doesn't have
a language column yet. Two options:

Option A (simpler, implement now):
  When languages is non-empty, add the language codes and
  script-characteristic characters as FTS keywords.
  e.g. for "ja": add "日本語" as a keyword hint, and bias
  vector search by prepending "Japanese text: " to the semantic query.

Option B (proper, add migration):
  Add a `language TEXT` column to clips table in db.rs migrations.
  Populate it during clipboard capture using detect_script_language().
  Then filter: AND c.language IN ('ja', 'zh', ...)
  Implement Option B.

=============================================================
TASK 3 — ADD language COLUMN TO db.rs
=============================================================

In run_migrations() in db.rs, add to the `needed` array:
  ("language", "TEXT")

In clipboard.rs, when inserting a new clip, call
detect_script_language() from query_parser (make it pub)
on the content and store it:

  let language = crate::query_parser::detect_script_language(&content)
    .map(|s| s.to_string());

Pass it through to the INSERT statement.
Also run it on OCR text when OCR completes.

=============================================================
TASK 4 — COMPILE AND TEST
=============================================================

After implementing everything:

1. Run: cargo check --manifest-path src-tauri/Cargo.toml
   Fix all compiler errors. There will be type mismatches
   because source_app changed from Option to Vec.
   Grep for all usages of parsed.source_app across search.rs
   and fix them.

2. Test these specific queries manually in the UI:
   - "10m ago"            → should find clips from ~10 minutes ago
   - "around 7am"         → should find clips from 6:15–7:45am today/yesterday
   - "monday"             → should find clips from last Monday
   - "monday morning"     → last Monday 05:00–12:00
   - "from arc yesterday" → Arc browser clips from yesterday
   - "japanese"           → clips with Japanese text
   - "auth thing"         → clips with JWT/OAuth/password/token content
   - "that code thing"    → clips with code content, expanded semantics
   - "from slack"         → Slack clips (no temporal required)
   - "pinned"             → pinned clips only
   - ""  (empty)          → recent 50 clips, no crash

3. The key success criterion: typing "around 7am" must resolve
   to an actual datetime range in the logs. Add an eprintln!
   in parse_query for debug builds:
     eprintln!("[Query] temporal: {:?}–{:?} (conf: {}), apps: {:?}, lang: {:?}, semantic: '{}'",
       parsed.temporal_after, parsed.temporal_before,
       parsed.temporal_confidence, parsed.source_apps,
       parsed.languages, parsed.semantic);

=============================================================
DO NOT:
=============================================================
- Do not add any external LLM API calls (obviously)
- Do not add any new Cargo dependencies beyond what already
  exists (chrono, regex, chrono-english are already there)
- Do not change the search_clips function signature
- Do not change ClipResult struct
- Do not break the existing FTS5 or vec0 query patterns
- Do not change the clips table schema beyond adding `language TEXT`

=============================================================
WHAT ALREADY WORKS (do not regress):
=============================================================
- "yesterday" → correct full day range
- "today" → correct range
- "this morning/afternoon/evening" → correct ranges
- "last week/month" → correct ranges
- "X hours ago" (with full word "hours") → works when number extracted
- RRF merge of fts + vec + like results
- Temporal SQL injection (tc variable in search.rs)
- source_app LIKE query when source_app is Some
- embed_query() call and vec0 knn search
- Fallback to LIKE when few results
