//! On-disk encoding rules. Ids and datetimes keep the forms the v2
//! (Python/SQLAlchemy) backend wrote, so existing databases stay readable:
//!
//! | Column type (SQLAlchemy)   | On-disk form                          |
//! |----------------------------|---------------------------------------|
//! | `sa.Uuid()`                | 32-char lowercase hex, no hyphens     |
//! | `TZDateTime` / `DateTime`  | naive UTC `YYYY-MM-DD HH:MM:SS.ffffff` |
//! | `JSON()`                   | compact UTF-8 JSON; `None` → `'null'`. Older rows hold Python `json.dumps` text, which reads the same. |
//!
//! The API layer, on the other hand, speaks Pydantic: hyphenated UUIDs and
//! ISO-8601 datetimes with a `Z` suffix (fraction omitted when zero).

use chrono::{DateTime, NaiveDateTime, SecondsFormat, TimeZone, Utc};
use serde_json::Value;
use uuid::Uuid;

/// Encode a UUID the way `sa.Uuid()` stores it on SQLite.
pub fn uuid_db(id: &Uuid) -> String {
    id.simple().to_string()
}

/// Parse a UUID from either the hex or the hyphenated form.
pub fn parse_uuid(raw: &str) -> Option<Uuid> {
    Uuid::parse_str(raw.trim()).ok()
}

/// Normalise any UUID spelling to the on-disk hex form.
///
/// Non-UUID strings (e.g. `scheduled_task.session_id`, which is a free-form
/// `VARCHAR(200)`) are returned unchanged.
pub fn db_id(raw: &str) -> String {
    parse_uuid(raw).map(|u| uuid_db(&u)).unwrap_or_else(|| raw.to_string())
}

/// Render a stored UUID in the API (Pydantic) form — hyphenated.
pub fn api_uuid(db: &str) -> String {
    parse_uuid(db).map(|u| u.hyphenated().to_string()).unwrap_or_else(|| db.to_string())
}

/// Generate a fresh UUIDv7 in its on-disk form.
pub fn new_id() -> String {
    uuid_db(&Uuid::now_v7())
}

const DB_DT_FORMAT: &str = "%Y-%m-%d %H:%M:%S%.6f";

/// Encode a UTC datetime exactly as SQLAlchemy's SQLite dialect does.
pub fn dt_db(dt: &DateTime<Utc>) -> String {
    dt.naive_utc().format(DB_DT_FORMAT).to_string()
}

/// Current time in the on-disk datetime form.
pub fn now_db() -> String {
    dt_db(&Utc::now())
}

/// Parse a stored or client-supplied datetime.
///
/// Accepts the SQLAlchemy form, ISO-8601 with `T`, with or without a
/// fraction, with `Z`/offset or naive (naive is treated as UTC, matching
/// `TZDateTime.process_result_value`).
pub fn parse_dt(raw: &str) -> Option<DateTime<Utc>> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    // Offset forms that are not strict RFC 3339 (space separator).
    for fmt in ["%Y-%m-%d %H:%M:%S%.f%:z", "%Y-%m-%dT%H:%M:%S%.f%:z"] {
        if let Ok(dt) = DateTime::parse_from_str(s, fmt) {
            return Some(dt.with_timezone(&Utc));
        }
    }
    for fmt in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M", "%Y-%m-%dT%H:%M"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(s, fmt) {
            return Some(Utc.from_utc_datetime(&naive));
        }
    }
    None
}

/// Normalise any accepted datetime spelling to the on-disk form.
pub fn db_dt(raw: &str) -> Option<String> {
    parse_dt(raw).map(|dt| dt_db(&dt))
}

/// Pydantic v2 JSON rendering of an aware UTC datetime:
/// `2026-09-23T06:56:28.225815Z`, or `2026-09-23T06:56:28Z` when the
/// microsecond component is zero.
pub fn api_dt_from(dt: &DateTime<Utc>) -> String {
    if dt.timestamp_subsec_micros() == 0 {
        dt.to_rfc3339_opts(SecondsFormat::Secs, true)
    } else {
        dt.to_rfc3339_opts(SecondsFormat::Micros, true)
    }
}

/// Render a stored datetime in API form; unparseable input passes through.
pub fn api_dt(db: &str) -> String {
    parse_dt(db).map(|dt| api_dt_from(&dt)).unwrap_or_else(|| db.to_string())
}

/// Python `datetime.isoformat()` of an aware UTC value (`+00:00` suffix).
///
/// v2 embeds this form inside JSON blobs (e.g. `chat_sessions.revert`).
pub fn py_isoformat(dt: &DateTime<Utc>) -> String {
    let base = if dt.timestamp_subsec_micros() == 0 { dt.naive_utc().format("%Y-%m-%dT%H:%M:%S").to_string() } else { dt.naive_utc().format("%Y-%m-%dT%H:%M:%S%.6f").to_string() };
    format!("{base}+00:00")
}

/// Decode a `JSON()` column. SQL `NULL` and JSON `null` both map to `None`.
pub fn json_col(raw: Option<&str>) -> Option<Value> {
    let text = raw?.trim();
    if text.is_empty() {
        return None;
    }
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Null) | Err(_) => None,
        Ok(v) => Some(v),
    }
}

/// Encode a value for a `JSON()` column: compact, non-ASCII verbatim, and
/// `None` → `'null'` like SQLAlchemy.
pub fn json_db(value: Option<&Value>) -> String {
    match value {
        None => "null".to_string(),
        Some(v) => serde_json::to_string(v).expect("serde_json::Value always serializes"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn uuid_round_trips_between_forms() {
        let hex = "01a0cd0d2e41761388e265d70f885069";
        let hyph = "01a0cd0d-2e41-7613-88e2-65d70f885069";
        assert_eq!(db_id(hyph), hex);
        assert_eq!(db_id(hex), hex);
        assert_eq!(api_uuid(hex), hyph);
        assert_eq!(db_id("scheduled:nightly"), "scheduled:nightly");
    }

    #[test]
    fn datetime_matches_sqlalchemy_and_pydantic() {
        let stored = "2026-09-23 06:56:28.225815";
        let dt = parse_dt(stored).unwrap();
        assert_eq!(dt_db(&dt), stored);
        assert_eq!(api_dt(stored), "2026-09-23T06:56:28.225815Z");
        assert_eq!(api_dt("2026-09-23 06:56:28.000000"), "2026-09-23T06:56:28Z");
        assert_eq!(py_isoformat(&dt), "2026-09-23T06:56:28.225815+00:00");
        assert_eq!(db_dt("2026-09-23T13:56:28.225815+07:00").unwrap(), "2026-09-23 06:56:28.225815");
        assert_eq!(db_dt("2026-09-23T06:56:28Z").unwrap(), "2026-09-23 06:56:28.000000");
    }

    #[test]
    fn json_columns_are_compact_utf8_and_old_rows_still_read() {
        let v = json!({"model": "codex:gpt-5.5", "n": [1, 2], "vi": "Tiếng Việt 😀"});
        assert_eq!(json_db(Some(&v)), r#"{"model":"codex:gpt-5.5","n":[1,2],"vi":"Tiếng Việt 😀"}"#);
        assert_eq!(json_db(None), "null");
        // Rows written by v2 / earlier v3 builds use Python's json.dumps style.
        assert_eq!(json_col(Some(r#"{"model": "codex:gpt-5.5", "n": [1, 2], "vi": "Ti\u1ebfng Vi\u1ec7t \ud83d\ude00"}"#)), Some(v));
        assert_eq!(json_col(Some("null")), None);
        assert_eq!(json_col(None), None);
        assert_eq!(json_col(Some("{\"a\": 1}")), Some(json!({"a": 1})));
    }
}
