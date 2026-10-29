//! Type coercion for tool arguments — the one place that decides how
//! forgiving parsing is about the *shape* of a value the model sent.
//!
//! The failure this exists for is measured, not hypothetical: a model calls
//! `readFile` with `{"path": "…", "startLine": "1", "endLine": "90"}` — every
//! scalar quoted. The call is semantically perfect and the strings are
//! unambiguous, but a plain `Option<u32>` rejects it with `invalid type:
//! string "1", expected u32`, the whole call is lost, and the model's usual
//! recovery is to *drop the parameter* rather than fix its type: retry
//! verbatim once, then re-send with no line range at all and read a 400-line
//! file in full. One transcript spent 6 of its 10 tool errors on this alone.
//!
//! Quoted scalars are not a defect worth punishing. Tool schemas go out
//! without provider-side constrained decoding, so a JSON-Schema type union is
//! advisory to something that is generating tokens. These deserializers accept
//! the unambiguous spellings and reject only what is genuinely undecidable:
//!
//! - `12`, `"12"`, `" 12 "`, `12.0`, `"12.0"` → `Some(12)`
//! - `null`, omitted, `""`, `"null"`, `"none"`, `"undefined"` → `None`
//! - `true`, `"true"`, `"yes"`, `"1"`, `1` → `Some(true)`, and the falsey
//!   spellings correspondingly
//! - `"abc"`, `12.5`, `-3`, `[]`, `{}` → an error naming the value and the
//!   expected spelling, because guessing would silently answer a different
//!   question than the one asked
//!
//! Coercion happens at the edge, so everything downstream still works with
//! honest `u32`/`bool` — no `StringOr<T>` leaks into tool implementations.

use serde::de::{Deserializer, Error as DeError};
use serde::Deserialize;
use serde_json::Value;

/// Spellings of "no value" a model uses interchangeably with `null`. An empty
/// string in particular is how one says "I am not setting this optional
/// parameter" while emitting every scalar as a string.
const NULLISH: [&str; 4] = ["", "null", "none", "undefined"];

const TRUTHY: [&str; 4] = ["true", "yes", "1", "y"];
const FALSY: [&str; 4] = ["false", "no", "0", "n"];

/// `Option<u32>` that also accepts a quoted number. Pair with
/// `#[serde(default)]` — `deserialize_with` runs only when the field is
/// *present*, so the default is what covers omission.
pub fn opt_u32<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: Deserializer<'de>,
{
    opt_uint(deserializer, u32::MAX as u64).map(|v| v.map(|n| n as u32))
}

/// `Option<usize>` counterpart of [`opt_u32`].
pub fn opt_usize<'de, D>(deserializer: D) -> Result<Option<usize>, D::Error>
where
    D: Deserializer<'de>,
{
    opt_uint(deserializer, usize::MAX as u64).map(|v| v.map(|n| n as usize))
}

/// `Option<bool>` that also accepts `"true"`/`"false"`/`"yes"`/`"no"`/`1`/`0`,
/// in any case.
pub fn opt_bool<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: Deserializer<'de>,
{
    let Some(value) = Option::<Value>::deserialize(deserializer)? else {
        return Ok(None);
    };
    match value {
        Value::Null => Ok(None),
        Value::Bool(b) => Ok(Some(b)),
        Value::Number(n) => match n.as_u64() {
            Some(0) => Ok(Some(false)),
            Some(1) => Ok(Some(true)),
            _ => Err(D::Error::custom(format!(
                "expected true or false, got the number {n} (only 0 and 1 are accepted as numbers)"
            ))),
        },
        Value::String(s) => {
            let lowered = s.trim().to_ascii_lowercase();
            if NULLISH.contains(&lowered.as_str()) {
                return Ok(None);
            }
            if TRUTHY.contains(&lowered.as_str()) {
                return Ok(Some(true));
            }
            if FALSY.contains(&lowered.as_str()) {
                return Ok(Some(false));
            }
            Err(D::Error::custom(format!(
                "expected a boolean, got the string \"{s}\" — send it unquoted as true or false"
            )))
        }
        other => Err(D::Error::custom(format!(
            "expected a boolean, got {}",
            describe(&other)
        ))),
    }
}

/// `Option<String>` where `""` and `"null"` mean not set, as a model sends
/// them for an optional name. For a query written as text, the object or
/// array it describes — `"filter": {"age": 3}` for `"{ age: 3 }"` — is taken
/// as its JSON text, which the query syntax reads too.
pub fn opt_text<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match Option::<Value>::deserialize(deserializer)? {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if NULLISH.contains(&s.trim().to_ascii_lowercase().as_str()) => None,
        Some(Value::String(s)) => Some(s),
        Some(other) => Some(other.to_string()),
    })
}

/// `Option<Vec<String>>` that also accepts one string for a one-item list —
/// `"fts": "read_source"` is a list of one term, not a type error. An empty
/// list, a nullish string and `null` all mean "not set".
pub fn opt_string_list<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    let Some(value) = Option::<Value>::deserialize(deserializer)? else {
        return Ok(None);
    };
    let list = match value {
        Value::Null => return Ok(None),
        Value::String(s) if NULLISH.contains(&s.trim().to_ascii_lowercase().as_str()) => return Ok(None),
        Value::String(s) => vec![s],
        Value::Array(items) => items
            .into_iter()
            .map(|item| match item {
                Value::String(s) => Ok(s),
                other => Err(D::Error::custom(format!("expected a list of strings, found {}", describe(&other)))),
            })
            .collect::<Result<Vec<_>, _>>()?,
        other => return Err(D::Error::custom(format!("expected a list of strings, got {}", describe(&other)))),
    };
    Ok((!list.is_empty()).then_some(list))
}

fn opt_uint<'de, D>(deserializer: D, max: u64) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let Some(value) = Option::<Value>::deserialize(deserializer)? else {
        return Ok(None);
    };
    let n = match value {
        Value::Null => return Ok(None),
        Value::Number(ref n) => number_to_uint(&n.to_string()).map_err(|issue| match issue {
            // A JSON number that will not parse as one cannot happen, but
            // saying so beats an `unreachable!`.
            NumberIssue::NotANumber => D::Error::custom("expected a number"),
            NumberIssue::Unusable(msg) => D::Error::custom(msg),
        })?,
        Value::String(ref s) => {
            let trimmed = s.trim();
            if NULLISH.contains(&trimmed.to_ascii_lowercase().as_str()) {
                return Ok(None);
            }
            number_to_uint(trimmed).map_err(|issue| match issue {
                NumberIssue::NotANumber => D::Error::custom(format!(
                    "expected a number, got the string \"{s}\" — send it unquoted, e.g. 12"
                )),
                NumberIssue::Unusable(msg) => D::Error::custom(msg),
            })?
        }
        ref other => {
            return Err(D::Error::custom(format!(
                "expected a number, got {}",
                describe(other)
            )))
        }
    };
    if n > max {
        return Err(D::Error::custom(format!(
            "the number {n} is too large for this parameter (maximum {max})"
        )));
    }
    Ok(Some(n))
}

/// Why a literal could not become a count. `NotANumber` is worded by the
/// caller — a quoted value gets "send it unquoted", a bare one does not —
/// while `Unusable` is a real number the parameter cannot take and carries
/// its own explanation.
enum NumberIssue {
    NotANumber,
    Unusable(String),
}

/// Parses one already-trimmed numeric literal. The same routine runs for a
/// JSON number's own text and for a quoted one, so `12.0` and `"12.0"` cannot
/// disagree. A fractional part is accepted only when zero: `12.0` is
/// unambiguously twelve, while `12.5` is a real mistake about what the
/// parameter means, and rounding it would quietly answer a different question.
fn number_to_uint(text: &str) -> Result<u64, NumberIssue> {
    if let Ok(n) = text.parse::<u64>() {
        return Ok(n);
    }
    let Ok(f) = text.parse::<f64>() else {
        return Err(NumberIssue::NotANumber);
    };
    if f.is_nan() || f.is_infinite() {
        return Err(NumberIssue::Unusable(format!(
            "expected a whole number, got {text}"
        )));
    }
    if f < 0.0 {
        return Err(NumberIssue::Unusable(format!(
            "expected a non-negative whole number, got {text}"
        )));
    }
    if f.fract() != 0.0 {
        return Err(NumberIssue::Unusable(format!(
            "expected a whole number, got {text} — round it to an integer"
        )));
    }
    if f > u64::MAX as f64 {
        return Err(NumberIssue::Unusable(format!("the number {text} is too large")));
    }
    Ok(f as u64)
}

/// The JSON kind of a value, worded for a model rather than for a Rust reader
/// — serde spells these `map`/`seq`.
fn describe(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Probe {
        #[serde(default, deserialize_with = "super::opt_u32")]
        n: Option<u32>,
        #[serde(default, deserialize_with = "super::opt_bool")]
        b: Option<bool>,
    }

    fn n(json: &str) -> Result<Option<u32>, String> {
        serde_json::from_str::<Probe>(json)
            .map(|p| p.n)
            .map_err(|e| e.to_string())
    }

    fn b(json: &str) -> Result<Option<bool>, String> {
        serde_json::from_str::<Probe>(json)
            .map(|p| p.b)
            .map_err(|e| e.to_string())
    }

    /// The exact call that motivated this module.
    #[test]
    fn a_quoted_line_range_is_accepted() {
        assert_eq!(n(r#"{"n": "1"}"#), Ok(Some(1)));
        assert_eq!(n(r#"{"n": " 90 "}"#), Ok(Some(90)));
    }

    #[test]
    fn a_bare_number_still_works() {
        assert_eq!(n(r#"{"n": 12}"#), Ok(Some(12)));
    }

    /// `12.0` and `"12.0"` must not disagree — both go through one parser.
    #[test]
    fn a_zero_fraction_is_the_whole_number() {
        assert_eq!(n(r#"{"n": 12.0}"#), Ok(Some(12)));
        assert_eq!(n(r#"{"n": "12.0"}"#), Ok(Some(12)));
    }

    #[test]
    fn every_spelling_of_absent_reads_as_none() {
        for json in [
            "{}",
            r#"{"n": null}"#,
            r#"{"n": ""}"#,
            r#"{"n": "null"}"#,
            r#"{"n": "none"}"#,
            r#"{"n": "undefined"}"#,
            r#"{"n": "NONE"}"#,
        ] {
            assert_eq!(n(json), Ok(None), "{json}");
        }
    }

    /// Rounding would silently answer a different question than the one asked,
    /// so a real fraction is refused — and the message says what to do.
    #[test]
    fn a_real_fraction_is_refused_with_advice() {
        let err = n(r#"{"n": 12.5}"#).expect_err("12.5 is not a line number");
        assert!(err.contains("whole number"), "{err}");
        assert!(err.contains("round it"), "{err}");
    }

    #[test]
    fn a_negative_number_is_refused() {
        let err = n(r#"{"n": -3}"#).expect_err("-3 is not a line number");
        assert!(err.contains("non-negative"), "{err}");
    }

    /// The message has to name the value and the fix, not just the type — this
    /// text is the model's only feedback.
    #[test]
    fn a_non_numeric_string_names_itself_in_the_error() {
        let err = n(r#"{"n": "abc"}"#).expect_err("abc is not a number");
        assert!(err.contains("abc"), "{err}");
        assert!(err.contains("unquoted"), "{err}");
    }

    #[test]
    fn a_number_too_large_for_the_field_is_refused() {
        let err = n(r#"{"n": 99999999999}"#).expect_err("larger than u32");
        assert!(err.contains("too large"), "{err}");
    }

    #[test]
    fn booleans_accept_the_spellings_models_actually_send() {
        for (json, want) in [
            (r#"{"b": true}"#, Some(true)),
            (r#"{"b": "true"}"#, Some(true)),
            (r#"{"b": "TRUE"}"#, Some(true)),
            (r#"{"b": "yes"}"#, Some(true)),
            (r#"{"b": 1}"#, Some(true)),
            (r#"{"b": false}"#, Some(false)),
            (r#"{"b": "no"}"#, Some(false)),
            (r#"{"b": 0}"#, Some(false)),
            (r#"{"b": null}"#, None),
            (r#"{"b": ""}"#, None),
            ("{}", None),
        ] {
            assert_eq!(b(json), Ok(want), "{json}");
        }
    }

    /// `2` as a boolean is not a spelling, it is a mistake — and one that
    /// guessing would hide.
    #[test]
    fn an_ambiguous_boolean_is_refused() {
        assert!(b(r#"{"b": 2}"#).is_err());
        assert!(b(r#"{"b": "maybe"}"#).is_err());
        assert!(b(r#"{"b": []}"#).is_err());
    }

    #[derive(Debug, Deserialize)]
    struct Terms {
        #[serde(default, deserialize_with = "super::opt_string_list")]
        fts: Option<Vec<String>>,
    }

    fn terms(json: &str) -> Result<Option<Vec<String>>, String> {
        serde_json::from_str::<Terms>(json).map(|t| t.fts).map_err(|e| e.to_string())
    }

    #[test]
    fn a_string_list_takes_one_string_as_a_list_of_one() {
        assert_eq!(terms(r#"{"fts":["a","b"]}"#), Ok(Some(vec!["a".into(), "b".into()])));
        assert_eq!(terms(r#"{"fts":"read_source"}"#), Ok(Some(vec!["read_source".into()])));
        for unset in [r#"{}"#, r#"{"fts":null}"#, r#"{"fts":[]}"#, r#"{"fts":"none"}"#] {
            assert_eq!(terms(unset), Ok(None), "{unset}");
        }
        assert!(terms(r#"{"fts":[1]}"#).unwrap_err().contains("list of strings"));
        assert!(terms(r#"{"fts":3}"#).is_err());
    }
}
