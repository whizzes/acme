//! Redaction rules (spec §21.5), applied before anything is written, never
//! after. Structural where the body parses as JSON, falling back to a
//! digit-run scan on raw text otherwise.

use std::sync::LazyLock;

use regex::Regex;

const SENSITIVE_HEADERS: &[&str] = &[
    "authorization",
    "tbk-api-key-secret",
    "x-veloz-signature",
    "proxy-authorization",
    "cookie",
];

const OVERSIZED_KEYS: &[&str] = &["qr_code_base64", "etiqueta", "label"];
const OVERSIZED_LIMIT: usize = 4096;

static SENSITIVE_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(card[_.]?number|^number$|pan|cvv|cvc|security_code|password|client_secret|private_key)")
        .expect("static redaction regex is valid")
});

fn last_n_chars(s: &str, n: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    let start = chars.len().saturating_sub(n);
    chars[start..].iter().collect()
}

/// `true` if `header_name` (any case) should have its value masked.
pub fn is_sensitive_header(header_name: &str) -> bool {
    SENSITIVE_HEADERS.contains(&header_name.to_ascii_lowercase().as_str())
}

/// Keeps the scheme word and the value's last 4 characters: `Bearer …demo`.
pub fn redact_header_value(value: &str) -> String {
    match value.split_once(' ') {
        Some((scheme, token)) => format!("{scheme} …{}", last_n_chars(token, 4)),
        None => format!("…{}", last_n_chars(value, 4)),
    }
}

fn luhn_valid(digits: &str) -> bool {
    let mut sum = 0u32;
    let mut double = false;
    for c in digits.chars().rev() {
        let mut d = c.to_digit(10).expect("caller guarantees ASCII digits");
        if double {
            d *= 2;
            if d > 9 {
                d -= 9;
            }
        }
        sum += d;
        double = !double;
    }
    sum.is_multiple_of(10)
}

/// `411111••••••1111`: keeps the first 6 and last 4 digits.
fn mask_pan(run: &str) -> String {
    let chars: Vec<char> = run.chars().collect();
    let len = chars.len();
    let head: String = chars[..6].iter().collect();
    let tail: String = chars[len - 4..].iter().collect();
    format!("{head}{}{tail}", "•".repeat(len - 10))
}

/// Masks every 13-19 digit run that passes the Luhn check, anywhere in the
/// string. Returns the (possibly unchanged) string and how many runs were
/// masked.
fn mask_luhn_runs(s: &str) -> (String, u32) {
    let chars: Vec<char> = s.chars().collect();
    let mut result = String::with_capacity(s.len());
    let mut count = 0u32;
    let mut i = 0;

    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            let run: String = chars[start..i].iter().collect();
            if (13..=19).contains(&run.len()) && luhn_valid(&run) {
                result.push_str(&mask_pan(&run));
                count += 1;
            } else {
                result.push_str(&run);
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    (result, count)
}

fn redact_json(value: &mut serde_json::Value, count: &mut u32) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, v) in map.iter_mut() {
                if OVERSIZED_KEYS.iter().any(|k| k.eq_ignore_ascii_case(key))
                    && let serde_json::Value::String(s) = v
                    && s.len() > OVERSIZED_LIMIT
                {
                    let kb = s.len() as f64 / 1024.0;
                    *v = serde_json::Value::String(format!("«{kb:.1} KB image, not stored»"));
                    *count += 1;
                    continue;
                }

                if SENSITIVE_KEY.is_match(key) && v.is_string() {
                    *v = serde_json::Value::String("«redacted»".to_string());
                    *count += 1;
                    continue;
                }

                redact_json(v, count);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items.iter_mut() {
                redact_json(item, count);
            }
        }
        serde_json::Value::String(s) => {
            let (masked, n) = mask_luhn_runs(s);
            if n > 0 {
                *s = masked;
                *count += n;
            }
        }
        _ => {}
    }
}

/// Redacts a request/response body. Structural JSON redaction when the
/// body parses as JSON; a Luhn digit-run scan over the raw bytes otherwise.
/// Returns the (possibly rewritten) bytes and how many values were masked.
pub fn redact_body(bytes: &[u8]) -> (Vec<u8>, u32) {
    if let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(bytes) {
        let mut count = 0;
        redact_json(&mut value, &mut count);
        if let Ok(out) = serde_json::to_vec(&value) {
            return (out, count);
        }
    }

    match std::str::from_utf8(bytes) {
        Ok(text) => {
            let (masked, count) = mask_luhn_runs(text);
            (masked.into_bytes(), count)
        }
        Err(_) => (bytes.to_vec(), 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_last_four_masking() {
        assert!(is_sensitive_header("Authorization"));
        assert!(!is_sensitive_header("Content-Type"));
        assert_eq!(
            redact_header_value("Bearer sk_test_acmepay_demo"),
            "Bearer …demo"
        );
    }

    #[test]
    fn json_key_regex_masking() {
        let body = br#"{"card_number":"4111111111111111","note":"password reset email"}"#;
        let (out, count) = redact_body(body);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("«redacted»"));
        assert!(
            text.contains("password reset email"),
            "unrelated key untouched"
        );
        assert!(count >= 1);
    }

    #[test]
    fn luhn_passing_digit_run_is_masked() {
        let body = br#"{"note":"card 4111111111111111 on file"}"#;
        let (out, _count) = redact_body(body);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("411111••••••1111"));
    }

    #[test]
    fn non_luhn_digit_run_is_untouched() {
        let body = br#"{"note":"order 1234567890123"}"#;
        let (out, count) = redact_body(body);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("1234567890123"));
        assert_eq!(count, 0);
    }

    #[test]
    fn oversized_label_field_is_replaced() {
        let big = "A".repeat(5000);
        let body = format!(r#"{{"label":"{big}"}}"#);
        let (out, count) = redact_body(body.as_bytes());
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("not stored"));
        assert!(!text.contains(&big));
        assert_eq!(count, 1);
    }

    #[test]
    fn untouched_body_is_unchanged() {
        let body = br#"{"amount":1000,"currency":"USD"}"#;
        let (out, count) = redact_body(body);
        assert_eq!(count, 0);
        let original: serde_json::Value = serde_json::from_slice(body).unwrap();
        let redacted: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(original, redacted);
    }
}
