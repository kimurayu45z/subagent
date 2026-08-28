//! Deterministic, model-free credential redaction: `docs/design.md` section
//! 9's "redact common API keys, authorization headers ... before persistence
//! and before injection" and section 15's "record the number and classes of
//! redactions without recording removed values."
//!
//! This module is pure: it accepts arbitrary bytes and returns a new byte
//! sequence plus provenance. It never touches the filesystem or any other
//! persistent state; callers such as [`super::capsule`] decide when and
//! where to apply it.
//!
//! Non-UTF-8 input cannot be scanned for the textual patterns below without
//! risking corruption of the raw bytes, so it is preserved exactly and
//! tagged with [`CLASS_UNSCANNABLE_NON_UTF8`] instead of being silently
//! treated as clean.

use std::collections::BTreeSet;

use serde_json::Value;

/// The class recorded when input is not valid UTF-8 and therefore was not
/// scanned for credential patterns at all -- the bytes are preserved
/// losslessly, not "inspected and found clean".
pub(crate) const CLASS_UNSCANNABLE_NON_UTF8: &str = "unscannable_non_utf8";

/// Placeholder text substituted for every redacted value. Deliberately
/// content-free: the whole point of a redaction is to avoid ever writing the
/// removed value anywhere, including here.
const REDACTED_PLACEHOLDER: &str = "[REDACTED]";

/// Standalone token prefixes redacted wherever they appear as a whole word,
/// independent of any `key=value` framing.
const KNOWN_PREFIXES: [&str; 5] = ["sk-", "ghp_", "github_pat_", "xoxb-", "xoxp-"];

/// The result of [`redact`]: the (possibly redacted and truncated) bytes,
/// plus provenance that deliberately never includes a removed value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RedactionResult {
    pub redacted_bytes: Vec<u8>,
    pub truncated: bool,
    pub redaction_count: u32,
    /// Sorted, de-duplicated class names, e.g. `["api_key", "bearer_token"]`.
    pub redaction_classes: Vec<String>,
}

/// Redacts common credential forms from `input` and truncates the result to
/// at most `max_bytes`.
///
/// Non-UTF-8 input is preserved verbatim (see the module documentation) and
/// only ever truncated, never scanned. UTF-8 input is redacted first and
/// truncated afterward, at a valid UTF-8 character boundary, so truncation
/// never depends on where a secret happened to be and never splits a
/// multi-byte character.
pub(crate) fn redact(input: &[u8], max_bytes: usize) -> RedactionResult {
    match std::str::from_utf8(input) {
        Ok(text) => {
            let (final_text, truncated, count, classes): (
                String,
                bool,
                u32,
                BTreeSet<&'static str>,
            ) = match serde_json::from_str::<Value>(text) {
                Ok(mut value) => {
                    let mut count: u32 = 0;
                    let mut classes: BTreeSet<&'static str> = BTreeSet::new();
                    redact_json_value(&mut value, &mut count, &mut classes);
                    let serialized: String = serde_json::to_string(&value)
                        .expect("a parsed JSON value always serializes");
                    let (final_text, truncated): (String, bool) =
                        truncate_json_to_bytes(&value, serialized, max_bytes);
                    (final_text, truncated, count, classes)
                }
                Err(_) => {
                    let (redacted, count, classes): (String, u32, BTreeSet<&'static str>) =
                        redact_text(text);
                    let (final_text, truncated): (String, bool) =
                        truncate_str_to_bytes(redacted, max_bytes);
                    (final_text, truncated, count, classes)
                }
            };
            RedactionResult {
                redacted_bytes: final_text.into_bytes(),
                truncated,
                redaction_count: count,
                redaction_classes: classes.into_iter().map(str::to_string).collect(),
            }
        }
        Err(_) => {
            let truncated: bool = input.len() > max_bytes;
            let redacted_bytes: Vec<u8> = if truncated {
                input[..max_bytes].to_vec()
            } else {
                input.to_vec()
            };
            RedactionResult {
                redacted_bytes,
                truncated,
                redaction_count: 0,
                redaction_classes: vec![CLASS_UNSCANNABLE_NON_UTF8.to_string()],
            }
        }
    }
}

/// Keeps a redacted JSON payload syntactically valid even when the caller's
/// byte cap cannot hold it. A small provenance-free sentinel is preferable to
/// persisting an invalid prefix that downstream readers may mistake for the
/// complete provider response. For caps below the object sentinel's size,
/// `null` or `0` is used; no non-empty valid JSON representation fits at cap
/// zero.
fn truncate_json_to_bytes(value: &Value, text: String, max_bytes: usize) -> (String, bool) {
    if text.len() <= max_bytes {
        return (text, false);
    }
    if let Value::Object(object) = value {
        let mut truncated_object: serde_json::Map<String, Value> = serde_json::Map::new();
        truncated_object.insert("_subagent_truncated".to_string(), Value::Bool(true));
        for key in ["result", "structured_output", "message"] {
            let Some(outcome) = object.get(key) else {
                continue;
            };
            truncated_object.insert(key.to_string(), outcome.clone());
            let outcome_text: String =
                serde_json::to_string(&Value::Object(truncated_object.clone()))
                    .expect("a truncated JSON outcome always serializes");
            if outcome_text.len() <= max_bytes {
                return (outcome_text, true);
            }
            truncated_object.remove(key);
        }
    }
    const TRUNCATED_SENTINEL: &str = r#"{"_subagent_truncated":true}"#;
    if TRUNCATED_SENTINEL.len() <= max_bytes {
        return (TRUNCATED_SENTINEL.to_string(), true);
    }
    if max_bytes >= 4 {
        return ("null".to_string(), true);
    }
    if max_bytes >= 1 {
        return ("0".to_string(), true);
    }
    (String::new(), true)
}

fn truncate_str_to_bytes(text: String, max_bytes: usize) -> (String, bool) {
    if text.len() <= max_bytes {
        return (text, false);
    }
    let mut end: usize = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut truncated: String = text;
    truncated.truncate(end);
    (truncated, true)
}

fn char_at(text: &str, index: usize) -> Option<char> {
    text[index..].chars().next()
}

fn is_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '+' | '/' | '=')
}

fn is_unquoted_value_char(c: char) -> bool {
    !(c.is_whitespace() || matches!(c, ',' | ';' | ')' | ']' | '}' | '"' | '\''))
}

/// True when `index` is not preceded by a key-shaped character, so a
/// standalone-prefix or `Bearer` match at `index` is not actually the tail
/// of a longer, unrelated word (for example the `sk-` inside `desk-top`).
fn is_word_boundary_before(text: &str, index: usize) -> bool {
    match text[..index].chars().next_back() {
        Some(c) => !is_key_char(c),
        None => true,
    }
}

fn compact_key(key: &str) -> String {
    key.chars()
        .filter(|character: &char| character.is_ascii_alphanumeric())
        .map(|character: char| character.to_ascii_lowercase())
        .collect()
}

fn is_known_usage_token_key(compact: &str) -> bool {
    matches!(
        compact,
        "inputtokens"
            | "outputtokens"
            | "inputtokensdetails"
            | "outputtokensdetails"
            | "thinkingtokens"
            | "prompttokens"
            | "completiontokens"
            | "reasoningoutputtokens"
            | "cachedtokens"
            | "cachecreationinputtokens"
            | "cachereadinputtokens"
            | "ephemeral1hinputtokens"
            | "ephemeral5minputtokens"
            | "maxoutputtokens"
            | "acceptedpredictiontokens"
            | "rejectedpredictiontokens"
            | "audiotokens"
            | "totaltokens"
            | "tokencount"
            | "tokenlimit"
            | "tokenbudget"
            | "tokenusage"
            | "tokenlength"
    )
}

fn is_allowed_usage_token_value(compact: &str, value: &Value) -> bool {
    match compact {
        "inputtokensdetails" | "outputtokensdetails" | "tokenusage" => {
            matches!(value, Value::Object(_))
        }
        _ => matches!(value, Value::Number(_)),
    }
}

fn classify_key(key: &str) -> Option<&'static str> {
    let compact: String = compact_key(key);
    if compact.contains("apikey") {
        Some("api_key")
    } else if compact.contains("token") {
        Some("token")
    } else if compact.contains("secret") {
        Some("secret")
    } else if compact.contains("password") {
        Some("password")
    } else if compact.contains("authorization") {
        Some("authorization")
    } else if compact.contains("cookie") {
        Some("cookie")
    } else {
        None
    }
}

fn redact_json_value(value: &mut Value, count: &mut u32, classes: &mut BTreeSet<&'static str>) {
    match value {
        Value::Object(entries) => {
            for (key, child) in entries.iter_mut() {
                let compact: String = compact_key(key);
                if is_known_usage_token_key(&compact)
                    && is_allowed_usage_token_value(&compact, child)
                {
                    redact_json_value(child, count, classes);
                } else if let Some(class) = classify_key(key) {
                    if matches!(child, Value::String(text) if text == REDACTED_PLACEHOLDER) {
                        continue;
                    }
                    *child = Value::String(REDACTED_PLACEHOLDER.to_string());
                    *count = count.saturating_add(1);
                    classes.insert(class);
                } else {
                    redact_json_value(child, count, classes);
                }
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                redact_json_value(item, count, classes);
            }
        }
        Value::String(text) => {
            let (redacted, string_count, string_classes): (String, u32, BTreeSet<&'static str>) =
                redact_text(text);
            *text = redacted;
            *count = count.saturating_add(string_count);
            classes.extend(string_classes);
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

/// Scans `text` for credential-shaped substrings and returns the redacted
/// text alongside how many redactions were made and which classes were
/// involved. See the module documentation for the exact forms matched.
fn redact_text(text: &str) -> (String, u32, BTreeSet<&'static str>) {
    let mut output: String = String::with_capacity(text.len());
    let mut count: u32 = 0;
    let mut classes: BTreeSet<&'static str> = BTreeSet::new();
    let len: usize = text.len();
    let mut i: usize = 0;

    while i < len {
        let c: char = char_at(text, i).expect("i is always a valid char boundary");

        if is_word_boundary_before(text, i) {
            if let Some(new_i) = try_match_bearer(text, i) {
                output.push_str(&text[i..new_i.value_start]);
                output.push_str(REDACTED_PLACEHOLDER);
                count += 1;
                classes.insert("bearer_token");
                i = new_i.end;
                continue;
            }
            if let Some(end) = try_match_known_prefix(text, i) {
                output.push_str(REDACTED_PLACEHOLDER);
                count += 1;
                classes.insert("known_prefix");
                i = end;
                continue;
            }
        }

        if is_key_char(c) {
            let run_start: usize = i;
            let mut j: usize = i;
            while let Some(cc) = char_at(text, j) {
                if is_key_char(cc) {
                    j += cc.len_utf8();
                } else {
                    break;
                }
            }
            let key: &str = &text[run_start..j];
            if let Some(class) = classify_key(key)
                && let Some(matched) = try_match_key_value(text, j)
            {
                output.push_str(&text[run_start..matched.value_start]);
                output.push_str(REDACTED_PLACEHOLDER);
                if let Some(quote) = matched.quote {
                    output.push(quote);
                }
                count += 1;
                classes.insert(class);
                i = matched.end;
                continue;
            }
            output.push_str(key);
            i = j;
            continue;
        }

        output.push(c);
        i += c.len_utf8();
    }

    (output, count, classes)
}

struct BearerMatch {
    value_start: usize,
    end: usize,
}

fn existing_placeholder_end(text: &str, start: usize) -> Option<usize> {
    let remaining: &str = &text[start..];
    if !remaining.starts_with(REDACTED_PLACEHOLDER) {
        return None;
    }
    let end: usize = start + REDACTED_PLACEHOLDER.len();
    match char_at(text, end) {
        Some(character) if is_token_char(character) => None,
        _ => Some(end),
    }
}

/// Matches a case-insensitive `Bearer` keyword at `start`, followed by
/// whitespace and a non-empty token, returning the span of just the token
/// (so the `Bearer ` keyword and its whitespace are preserved verbatim by
/// the caller).
fn try_match_bearer(text: &str, start: usize) -> Option<BearerMatch> {
    let remaining: &str = &text[start..];
    if remaining.len() < 6 || !remaining.as_bytes()[..6].eq_ignore_ascii_case(b"bearer") {
        return None;
    }
    let after_keyword: usize = start + 6;
    match char_at(text, after_keyword) {
        Some(' ') | Some('\t') => {}
        _ => return None,
    }
    let mut value_start: usize = after_keyword;
    while let Some(cc) = char_at(text, value_start) {
        if cc == ' ' || cc == '\t' {
            value_start += cc.len_utf8();
        } else {
            break;
        }
    }
    if existing_placeholder_end(text, value_start).is_some() {
        return None;
    }
    let mut end: usize = value_start;
    while let Some(cc) = char_at(text, end) {
        if cc.is_whitespace() {
            break;
        }
        end += cc.len_utf8();
    }
    if end == value_start {
        return None;
    }
    Some(BearerMatch { value_start, end })
}

/// Matches a known standalone credential prefix at `start`, requiring at
/// least one additional token character after the prefix itself so a bare
/// prefix substring is not treated as a full secret.
fn try_match_known_prefix(text: &str, start: usize) -> Option<usize> {
    let remaining: &str = &text[start..];
    let prefix: &str = KNOWN_PREFIXES
        .iter()
        .find(|prefix| remaining.starts_with(**prefix))?;
    let mut end: usize = start + prefix.len();
    while let Some(cc) = char_at(text, end) {
        if is_token_char(cc) {
            end += cc.len_utf8();
        } else {
            break;
        }
    }
    if end == start + prefix.len() {
        return None;
    }
    Some(end)
}

struct KeyValueMatch {
    value_start: usize,
    end: usize,
    quote: Option<char>,
}

/// Matches a `= value` or `: value` following a credential-shaped key that
/// already ends at `after_key`, returning the span of just the value (with
/// the separator and any surrounding whitespace preserved verbatim by the
/// caller).
fn try_match_key_value(text: &str, after_key: usize) -> Option<KeyValueMatch> {
    let mut k: usize = after_key;
    // A JSON-style quoted key (`"password": ...`) has its own closing quote
    // immediately after the key run and before the separator; skip at most
    // one such quote so quoted keys are recognized like unquoted ones.
    if let Some(cc @ ('"' | '\'')) = char_at(text, k) {
        k += cc.len_utf8();
    }
    while let Some(cc) = char_at(text, k) {
        if cc == ' ' || cc == '\t' {
            k += cc.len_utf8();
        } else {
            break;
        }
    }
    let sep: char = char_at(text, k)?;
    if sep != '=' && sep != ':' {
        return None;
    }
    let mut v: usize = k + sep.len_utf8();
    while let Some(cc) = char_at(text, v) {
        if cc == ' ' || cc == '\t' {
            v += cc.len_utf8();
        } else {
            break;
        }
    }

    match char_at(text, v) {
        Some(quote @ ('"' | '\'')) => {
            let value_start: usize = v + quote.len_utf8();
            if let Some(placeholder_end) = existing_placeholder_end(text, value_start)
                && char_at(text, placeholder_end) == Some(quote)
            {
                return None;
            }
            let mut w: usize = value_start;
            loop {
                match char_at(text, w) {
                    Some(cc) if cc == quote => break,
                    Some('\n') | None => return None,
                    Some(cc) => w += cc.len_utf8(),
                }
            }
            if w == value_start {
                return None;
            }
            Some(KeyValueMatch {
                value_start,
                end: w + quote.len_utf8(),
                quote: Some(quote),
            })
        }
        Some(_) => {
            let value_start: usize = v;
            if existing_placeholder_end(text, value_start).is_some() {
                return None;
            }
            if let Some(bearer) = try_match_bearer(text, value_start) {
                return Some(KeyValueMatch {
                    value_start,
                    end: bearer.end,
                    quote: None,
                });
            }
            let mut w: usize = value_start;
            while let Some(cc) = char_at(text, w) {
                if is_unquoted_value_char(cc) {
                    w += cc.len_utf8();
                } else {
                    break;
                }
            }
            if w == value_start {
                return None;
            }
            Some(KeyValueMatch {
                value_start,
                end: w,
                quote: None,
            })
        }
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn redact_str(text: &str, max_bytes: usize) -> RedactionResult {
        redact(text.as_bytes(), max_bytes)
    }

    #[test]
    fn redacts_api_key_assignment_and_hides_the_raw_secret() {
        let result = redact_str("API_KEY=sk-abcdef1234567890 rest of line", 1024);
        let text = String::from_utf8(result.redacted_bytes).unwrap();
        assert!(!text.contains("abcdef1234567890"));
        assert!(text.contains("API_KEY="));
        assert!(text.contains("rest of line"));
        assert_eq!(result.redaction_count, 1);
        assert_eq!(result.redaction_classes, vec!["api_key".to_string()]);
    }

    #[test]
    fn redacts_token_colon_value_case_insensitively() {
        let result = redact_str("Some-Token: verysecrettokenvalue123", 1024);
        let text = String::from_utf8(result.redacted_bytes).unwrap();
        assert!(!text.contains("verysecrettokenvalue123"));
        assert!(text.contains("Some-Token:"));
        assert_eq!(result.redaction_classes, vec!["token".to_string()]);
    }

    #[test]
    fn redacts_quoted_json_style_secret_value() {
        let result = redact_str(r#"{"password": "hunter2example", "ok": true}"#, 1024);
        let text = String::from_utf8(result.redacted_bytes).unwrap();
        assert!(!text.contains("hunter2example"));
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["password"], REDACTED_PLACEHOLDER);
        assert_eq!(value["ok"], true);
        assert_eq!(result.redaction_classes, vec!["password".to_string()]);
    }

    #[test]
    fn preserves_provider_usage_counters_as_valid_json() {
        let input: &str = r#"{
            "usage": {
                "input_tokens": 17,
                "cache_creation_input_tokens": 1978,
                "cache_read_input_tokens": 9168,
                "output_tokens": 406,
                "output_tokens_details": {"thinking_tokens": 280}
            },
            "modelUsage": {
                "claude-haiku": {
                    "inputTokens": 17,
                    "maxOutputTokens": 64000
                }
            },
            "result": "ContextLease1234"
        }"#;
        let result: RedactionResult = redact_str(input, 4096);
        let text: String = String::from_utf8(result.redacted_bytes).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();

        assert_eq!(value["usage"]["input_tokens"], 17);
        assert_eq!(value["usage"]["cache_read_input_tokens"], 9168);
        assert_eq!(value["modelUsage"]["claude-haiku"]["inputTokens"], 17);
        assert_eq!(
            value["modelUsage"]["claude-haiku"]["maxOutputTokens"],
            64000
        );
        assert_eq!(value["result"], "ContextLease1234");
        assert_eq!(result.redaction_count, 0);
        assert!(result.redaction_classes.is_empty());
    }

    #[test]
    fn redacts_ambiguous_plural_token_fields_but_preserves_known_usage_keys() {
        let input: &str = r#"{
            "access_tokens": ["opaque-a", "opaque-b"],
            "tokens": ["opaque-c"],
            "usage": {
                "input_tokens": 17,
                "outputTokens": 23,
                "total_tokens": 40
            }
        }"#;
        let result: RedactionResult = redact_str(input, 4096);
        let text: String = String::from_utf8(result.redacted_bytes).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();

        assert_eq!(value["access_tokens"], REDACTED_PLACEHOLDER);
        assert_eq!(value["tokens"], REDACTED_PLACEHOLDER);
        assert_eq!(value["usage"]["input_tokens"], 17);
        assert_eq!(value["usage"]["outputTokens"], 23);
        assert_eq!(value["usage"]["total_tokens"], 40);
        assert_eq!(result.redaction_count, 2);
        assert_eq!(result.redaction_classes, vec!["token".to_string()]);
    }

    #[test]
    fn redacts_known_usage_keys_when_their_json_value_has_an_unexpected_type() {
        let input: &str = r#"{
            "input_tokens": "opaque-a",
            "outputTokens": ["opaque-b"],
            "total_tokens": {"value": "opaque-c"},
            "output_tokens_details": {
                "thinking_tokens": 11,
                "access_token": "opaque-d"
            }
        }"#;
        let result: RedactionResult = redact_str(input, 4096);
        let text: String = String::from_utf8(result.redacted_bytes).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();

        assert_eq!(value["input_tokens"], REDACTED_PLACEHOLDER);
        assert_eq!(value["outputTokens"], REDACTED_PLACEHOLDER);
        assert_eq!(value["total_tokens"], REDACTED_PLACEHOLDER);
        assert_eq!(value["output_tokens_details"]["thinking_tokens"], 11);
        assert_eq!(
            value["output_tokens_details"]["access_token"],
            REDACTED_PLACEHOLDER
        );
        assert_eq!(result.redaction_count, 4);
        assert_eq!(result.redaction_classes, vec!["token".to_string()]);
    }

    #[test]
    fn redacts_structured_secret_fields_without_breaking_json() {
        let input: &str = r#"{
            "access_token": 123456,
            "clientSecret": {"nested": true},
            "metadata": {
                "apiKey": "sk-example-secret",
                "token_count": 9,
                "token_value": "opaque-value",
                "password_hash": "opaque-hash"
            },
            "message": "Authorization: Bearer abcdefghijklmnop"
        }"#;
        let result: RedactionResult = redact_str(input, 4096);
        let text: String = String::from_utf8(result.redacted_bytes).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();

        assert_eq!(value["access_token"], REDACTED_PLACEHOLDER);
        assert_eq!(value["clientSecret"], REDACTED_PLACEHOLDER);
        assert_eq!(value["metadata"]["apiKey"], REDACTED_PLACEHOLDER);
        assert_eq!(value["metadata"]["token_count"], 9);
        assert_eq!(value["metadata"]["token_value"], REDACTED_PLACEHOLDER);
        assert_eq!(value["metadata"]["password_hash"], REDACTED_PLACEHOLDER);
        assert_eq!(value["message"], "Authorization: [REDACTED]");
        assert_eq!(result.redaction_count, 6);
        assert_eq!(
            result.redaction_classes,
            vec![
                "api_key".to_string(),
                "authorization".to_string(),
                "password".to_string(),
                "secret".to_string(),
                "token".to_string(),
            ]
        );
    }

    #[test]
    fn truncated_json_is_replaced_by_a_valid_secret_free_sentinel() {
        let input: &str = r#"{"result":"safe but much too long","api_key":"sk-secret-value"}"#;
        let result: RedactionResult = redact_str(input, 32);
        let text: String = String::from_utf8(result.redacted_bytes).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();

        assert_eq!(value["_subagent_truncated"], true);
        assert!(!text.contains("sk-secret-value"));
        assert!(result.truncated);
        assert_eq!(result.redaction_count, 1);
    }

    #[test]
    fn truncated_json_preserves_a_small_provider_outcome_when_it_fits() {
        let input: String = serde_json::to_string(&serde_json::json!({
            "transport_padding": "x".repeat(8_000),
            "api_key": "sk-secret-value",
            "result": "OutcomeMarker"
        }))
        .unwrap();
        let result: RedactionResult = redact_str(&input, 128);
        let text: String = String::from_utf8(result.redacted_bytes).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();

        assert_eq!(value["_subagent_truncated"], true);
        assert_eq!(value["result"], "OutcomeMarker");
        assert!(!text.contains("transport_padding"));
        assert!(!text.contains("sk-secret-value"));
        assert!(result.truncated);
        assert_eq!(result.redaction_count, 1);
    }

    #[test]
    fn structured_redaction_is_idempotent_for_existing_placeholders() {
        let first: RedactionResult =
            redact_str(r#"{"api_key":"sk-secret-value","input_tokens":17}"#, 1024);
        let second: RedactionResult = redact(&first.redacted_bytes, 1024);

        assert_eq!(first.redaction_count, 1);
        assert_eq!(second.redaction_count, 0);
        assert_eq!(second.redacted_bytes, first.redacted_bytes);
        assert!(second.redaction_classes.is_empty());
    }

    #[test]
    fn text_redaction_is_idempotent_for_existing_placeholders() {
        let original: &str = "Authorization: Bearer abcdefghijklmnop API_KEY=sk-secret-value";
        let first: RedactionResult = redact_str(original, 1024);
        let second: RedactionResult = redact(&first.redacted_bytes, 1024);

        assert_eq!(first.redaction_count, 2);
        assert_eq!(second.redaction_count, 0);
        assert_eq!(second.redacted_bytes, first.redacted_bytes);
        assert!(second.redaction_classes.is_empty());
    }

    #[test]
    fn redacts_bearer_token_but_keeps_the_bearer_keyword() {
        let result = redact_str(
            "Authorization header: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6",
            1024,
        );
        let text = String::from_utf8(result.redacted_bytes).unwrap();
        assert!(!text.contains("eyJhbGciOiJIUzI1NiIsInR5cCI6"));
        assert!(text.contains("Bearer [REDACTED]"));
    }

    #[test]
    fn redacts_standalone_known_prefixes() {
        for (prefix, sample) in [
            ("sk-", "sk-1234567890abcdef"),
            ("ghp_", "ghp_1234567890abcdefghij"),
            ("github_pat_", "github_pat_11ABCDEFG0123456789"),
            ("xoxb-", "xoxb-1234-5678-abcdefgh"),
            ("xoxp-", "xoxp-1234-5678-abcdefgh"),
        ] {
            let input = format!("token value: {sample} end");
            let result = redact_str(&input, 1024);
            let text = String::from_utf8(result.redacted_bytes.clone()).unwrap();
            assert!(!text.contains(sample), "prefix {prefix} leaked raw secret");
            assert!(
                result.redaction_count >= 1,
                "prefix {prefix} was not redacted"
            );
        }
    }

    #[test]
    fn does_not_redact_a_prefix_embedded_inside_an_unrelated_word() {
        let result = redact_str("the desk-top computer needs a token upgrade", 1024);
        let text = String::from_utf8(result.redacted_bytes).unwrap();
        assert_eq!(text, "the desk-top computer needs a token upgrade");
        assert_eq!(result.redaction_count, 0);
        assert!(result.redaction_classes.is_empty());
    }

    #[test]
    fn preserves_ordinary_prose_without_credential_shapes() {
        let prose = "Please review the tokenizer output and the API documentation for typos.";
        let result = redact_str(prose, 1024);
        let text = String::from_utf8(result.redacted_bytes).unwrap();
        assert_eq!(text, prose);
        assert_eq!(result.redaction_count, 0);
        assert!(result.redaction_classes.is_empty());
    }

    #[test]
    fn preserves_surrounding_whitespace_around_a_redacted_value() {
        let result = redact_str("  API_KEY = sk-XXXXXXXXXXXX  \n", 1024);
        let text = String::from_utf8(result.redacted_bytes).unwrap();
        assert_eq!(text, "  API_KEY = [REDACTED]  \n");
    }

    #[test]
    fn count_and_classes_cover_multiple_distinct_matches() {
        let input = "API_KEY=sk-aaaaaaaaaaaa\nCookie: session=zzz\nBearer bbbbbbbbbbbbbbbb";
        let result = redact_str(input, 1024);
        assert_eq!(result.redaction_count, 3);
        assert_eq!(
            result.redaction_classes,
            vec![
                "api_key".to_string(),
                "bearer_token".to_string(),
                "cookie".to_string(),
            ]
        );
    }

    #[test]
    fn non_utf8_bytes_are_preserved_losslessly_and_marked_unscannable() {
        let input: Vec<u8> = vec![0x41, 0xff, 0x42, b'=', 0xfe];
        let result = redact(&input, 1024);
        assert_eq!(result.redacted_bytes, input);
        assert!(!result.truncated);
        assert_eq!(result.redaction_count, 0);
        assert_eq!(
            result.redaction_classes,
            vec![CLASS_UNSCANNABLE_NON_UTF8.to_string()]
        );
    }

    #[test]
    fn non_utf8_bytes_beyond_the_cap_are_truncated_without_scanning() {
        let input: Vec<u8> = vec![0x41, 0xff, 0x42, 0xfe, 0x43];
        let result = redact(&input, 3);
        assert_eq!(result.redacted_bytes, input[..3]);
        assert!(result.truncated);
    }

    #[test]
    fn truncates_after_redaction_at_a_utf8_char_boundary() {
        let input =
            "API_KEY=sk-aaaa \u{00e9}\u{00e9}\u{00e9}\u{00e9}\u{00e9}\u{00e9}\u{00e9}\u{00e9}";
        let full = redact_str(input, 4096);
        let full_text = String::from_utf8(full.redacted_bytes).unwrap();
        assert!(!full_text.contains("sk-aaaa"));

        // Cap in the middle of a two-byte UTF-8 character to prove the
        // boundary search actually moves the cut point.
        let cap = full_text.len() - 1;
        let result = redact_str(input, cap);
        assert!(result.truncated);
        let text = String::from_utf8(result.redacted_bytes.clone()).unwrap();
        assert!(text.len() <= cap);
        assert!(std::str::from_utf8(&result.redacted_bytes).is_ok());
    }

    #[test]
    fn truncation_flag_is_false_when_content_fits() {
        let result = redact_str("short and clean", 1024);
        assert!(!result.truncated);
    }

    #[test]
    fn empty_input_produces_no_redactions() {
        let result = redact_str("", 1024);
        assert_eq!(result.redacted_bytes, Vec::<u8>::new());
        assert!(!result.truncated);
        assert_eq!(result.redaction_count, 0);
        assert!(result.redaction_classes.is_empty());
    }
}
