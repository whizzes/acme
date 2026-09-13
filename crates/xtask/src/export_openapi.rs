//! `cargo xtask export-openapi` (spec `specs/1-Acme-Client.md`): writes
//! Acme Pay's and Acme Ship's OpenAPI documents to `assets/` as plain
//! JSON, using the same state-free `providers::*::openapi()`
//! constructors `spec_lint` already calls — no DB pool, no running
//! server. `crates/acme-client` generates its client from these files, so
//! they must stay a faithful, reproducible export of what `acme-server`
//! actually serves at `/openapi/{acmepay,acmeship}.json` — never hand
//! edited.

use std::path::Path;

use serde_json::Value;

pub fn run(pay_out: &str, ship_out: &str) {
    write_spec(
        pay_out,
        acme_server::providers::payments::acmepay::openapi(),
    );
    write_spec(
        ship_out,
        acme_server::providers::shipping::acmeship::openapi(),
    );
}

fn write_spec(out: &str, openapi: utoipa::openapi::OpenApi) {
    let mut json = serde_json::to_value(&openapi).expect("OpenApi always serializes to JSON");
    downgrade_to_openapi_3_0(&mut json);
    let json = serde_json::to_string_pretty(&json).expect("Value always serializes to JSON");
    let path = Path::new(out);
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|error| panic!("could not create {}: {error}", parent.display()));
    }
    std::fs::write(path, json + "\n")
        .unwrap_or_else(|error| panic!("could not write {out}: {error}"));
    println!("wrote {out}");
}

/// `utoipa` 5 only emits OpenAPI 3.1 (`utoipa::openapi::OpenApiVersion` has
/// a single `Version31` variant); `progenitor` 0.15 only accepts 3.0.x
/// (`progenitor_impl::validate_openapi_spec_version`). The two documents
/// differ in two ways this rewrite fixes:
///
/// - a nullable plain-typed field is `"type": ["string", "null"]` in 3.1
///   vs `"type": "string", "nullable": true` in 3.0;
/// - a nullable named-schema field (e.g. `customer: Option<CustomerInput>`)
///   is `"oneOf": [{"type": "null"}, {"$ref": "...", "description": "..."}]`
///   in 3.1 vs a 3.0 document can't put siblings next to a bare `$ref` (the
///   spec says they're ignored), so it becomes `"allOf": [{"$ref": "..."}],
///   "description": "...", "nullable": true` instead.
///
/// Both shapes panic on anything unexpected (a `oneOf`/`anyOf` with other
/// than exactly one `{"type": "null"}` branch, or a `type` array not of
/// the form `[T, "null"]`) so a future schema shape `progenitor` can't
/// read fails loudly at export time instead of silently producing a spec
/// it misreads.
fn downgrade_to_openapi_3_0(value: &mut Value) {
    if let Some(root) = value.as_object_mut() {
        root.insert("openapi".to_string(), Value::String("3.0.3".to_string()));
        if let Some(license) = root
            .get_mut("info")
            .and_then(|i| i.as_object_mut())
            .and_then(|i| i.get_mut("license"))
            .and_then(|l| l.as_object_mut())
        {
            license.remove("identifier");
        }
    }
    denullify(value);
}

fn is_null_schema(value: &Value) -> bool {
    value
        .as_object()
        .and_then(|o| o.get("type"))
        .and_then(|t| t.as_str())
        == Some("null")
}

/// Merges the non-null branch of a `oneOf`/`anyOf` nullable pair into
/// `map` (the schema object that held that `oneOf`/`anyOf`), 3.0-style.
fn apply_nullable_variant(map: &mut serde_json::Map<String, Value>, other: Value) {
    map.insert("nullable".to_string(), Value::Bool(true));
    let Value::Object(mut other_map) = other else {
        return;
    };
    if let Some(reference) = other_map.remove("$ref") {
        map.insert(
            "allOf".to_string(),
            Value::Array(vec![serde_json::json!({ "$ref": reference })]),
        );
    }
    for (k, v) in other_map {
        map.entry(k).or_insert(v);
    }
}

fn denullify(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if let Some(Value::Array(types)) = map.get("type").cloned() {
                match types.as_slice() {
                    [Value::String(single)] => {
                        map.insert("type".to_string(), Value::String(single.clone()));
                    }
                    [a, b] if a == "null" || b == "null" => {
                        let kept = if a == "null" { b } else { a };
                        map.insert("type".to_string(), kept.clone());
                        map.insert("nullable".to_string(), Value::Bool(true));
                    }
                    other => panic!(
                        "downgrade_to_openapi_3_0: unsupported OpenAPI 3.1 `type` array {other:?} — extend denullify() to handle it before exporting"
                    ),
                }
            }

            for key in ["oneOf", "anyOf"] {
                let Some(Value::Array(variants)) = map.get(key).cloned() else {
                    continue;
                };
                let null_positions: Vec<usize> = variants
                    .iter()
                    .enumerate()
                    .filter(|(_, v)| is_null_schema(v))
                    .map(|(i, _)| i)
                    .collect();
                match (variants.len(), null_positions.as_slice()) {
                    (_, []) => {} // a real union, unrelated to nullability — leave as-is
                    (2, [null_idx]) => {
                        let other = variants[1 - null_idx].clone();
                        map.remove(key);
                        apply_nullable_variant(map, other);
                    }
                    _ => panic!(
                        "downgrade_to_openapi_3_0: unsupported `{key}` shape {variants:?} — extend denullify() to handle it before exporting"
                    ),
                }
            }

            for v in map.values_mut() {
                denullify(v);
            }
        }
        Value::Array(items) => {
            for v in items {
                denullify(v);
            }
        }
        _ => {}
    }
}
