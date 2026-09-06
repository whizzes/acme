//! `cargo xtask spec-lint` (spec §22.1/§22.3/§22.14): walks each provider's
//! generated `utoipa::openapi::OpenApi` — serialized to plain JSON, so this
//! doesn't need to track `utoipa`'s internal Rust types, just the stable
//! OpenAPI document shape — and fails with a line-numbered-by-route report
//! if any operation lacks an `operation_id`, any request/response schema is
//! an inline anonymous object instead of a named component, or any
//! operation/parameter/schema field lacks a `description`.

const HTTP_METHODS: &[&str] = &[
    "get", "post", "put", "delete", "patch", "options", "head", "trace",
];

pub fn run() -> bool {
    let mut errors = Vec::new();
    errors.extend(lint_provider(
        "acmepay",
        acme_server::providers::payments::acmepay::openapi(),
    ));
    errors.extend(lint_provider(
        "acmeship",
        acme_server::providers::shipping::acmeship::openapi(),
    ));
    errors.extend(lint_provider(
        "webpay",
        acme_server::providers::payments::webpay::openapi(),
    ));
    errors.extend(lint_provider(
        "iberex",
        acme_server::providers::shipping::iberex::openapi(),
    ));
    errors.extend(lint_source_operation_ids(
        "acmepay",
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../acme-server/src/providers/payments/acmepay/routes.rs"
        ),
    ));
    errors.extend(lint_source_operation_ids(
        "acmeship",
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../acme-server/src/providers/shipping/acmeship/routes.rs"
        ),
    ));
    errors.extend(lint_source_operation_ids(
        "webpay",
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../acme-server/src/providers/payments/webpay/routes.rs"
        ),
    ));
    errors.extend(lint_source_operation_ids(
        "iberex",
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../acme-server/src/providers/shipping/iberex/routes.rs"
        ),
    ));

    if errors.is_empty() {
        println!(
            "spec-lint: ok — every route has an operation_id, every schema is named, every field is described"
        );
        true
    } else {
        eprintln!("spec-lint: {} problem(s) found:", errors.len());
        for error in &errors {
            eprintln!("  {error}");
        }
        false
    }
}

/// `utoipa` defaults a missing `operation_id` to the handler's function
/// name, which is already snake_case — so a *dropped* `operation_id`
/// produces a generated `OpenApi` document indistinguishable from an
/// explicit one (spec §22.3 rule 1 is precisely about this trap: "never
/// the defaulted function name"). Catching a drop needs a source-level
/// check, not a JSON one: every `#[utoipa::path(...)]` block in the given
/// file must contain a literal `operation_id = "..."`.
fn lint_source_operation_ids(name: &str, path: &str) -> Vec<String> {
    let mut errors = Vec::new();
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(error) => {
            return vec![format!(
                "{name}: could not read {path} to check for explicit operation_ids: {error}"
            )];
        }
    };

    let mut search_from = 0;
    while let Some(start) = source[search_from..].find("#[utoipa::path(") {
        let block_start = search_from + start;
        let paren_start = block_start + "#[utoipa::path".len();
        let mut depth = 0i32;
        let mut end = None;
        for (offset, ch) in source[paren_start..].char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(paren_start + offset + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(block_end) = end else { break };
        let block = &source[block_start..block_end];
        let line = source[..block_start].matches('\n').count() + 1;

        if !block.contains("operation_id") {
            errors.push(format!(
                "{name} routes.rs:{line}: #[utoipa::path] has no explicit operation_id"
            ));
        }

        search_from = block_end;
    }

    errors
}

fn lint_provider(name: &str, openapi: utoipa::openapi::OpenApi) -> Vec<String> {
    let json = serde_json::to_value(&openapi).expect("OpenApi always serializes to JSON");
    let mut errors = Vec::new();

    let empty = serde_json::Map::new();
    let paths = json
        .get("paths")
        .and_then(|v| v.as_object())
        .unwrap_or(&empty);

    for (path, item) in paths {
        let item = item.as_object().cloned().unwrap_or_default();
        for (method, op) in &item {
            if !HTTP_METHODS.contains(&method.as_str()) {
                continue;
            }
            let route = format!("{name} {} {path}", method.to_uppercase());
            lint_operation(&route, op, &mut errors);
        }
    }

    let schemas = json
        .get("components")
        .and_then(|c| c.get("schemas"))
        .and_then(|s| s.as_object())
        .cloned()
        .unwrap_or_default();
    for (schema_name, schema) in &schemas {
        lint_schema(
            &format!("{name} schema `{schema_name}`"),
            schema,
            &mut errors,
        );
    }

    errors
}

fn is_present(value: Option<&serde_json::Value>) -> bool {
    value
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty())
}

fn is_snake_case(id: &str) -> bool {
    !id.is_empty()
        && id.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn lint_operation(route: &str, op: &serde_json::Value, errors: &mut Vec<String>) {
    match op.get("operationId").and_then(|v| v.as_str()) {
        Some(id) if is_snake_case(id) => {}
        Some(id) => errors.push(format!("{route}: operation_id `{id}` is not snake_case")),
        None => errors.push(format!("{route}: missing operation_id")),
    }

    if !is_present(op.get("description")) && !is_present(op.get("summary")) {
        errors.push(format!("{route}: operation has no description"));
    }

    for param in op
        .get("parameters")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        let param_name = param
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("<unnamed>");
        if !is_present(param.get("description")) {
            errors.push(format!(
                "{route}: parameter `{param_name}` has no description"
            ));
        }
    }

    if let Some(request_body) = op.get("requestBody") {
        lint_body_schema(&format!("{route} requestBody"), request_body, errors);
    }

    for (status, response) in op
        .get("responses")
        .and_then(|v| v.as_object())
        .into_iter()
        .flatten()
    {
        if !is_present(response.get("description")) {
            errors.push(format!("{route} response {status}: has no description"));
        }
        lint_body_schema(&format!("{route} response {status}"), response, errors);
    }
}

/// A request/response body's schema must be a named component (`$ref`), a
/// bare free-form object (no declared properties — used for opaque
/// metadata maps, not a structured type a generator would need to name),
/// or an array of one of those. Anything else is the inline anonymous
/// object §22.3 rule 2 exists to catch.
fn lint_body_schema(
    location: &str,
    body_or_response: &serde_json::Value,
    errors: &mut Vec<String>,
) {
    let Some(content) = body_or_response.get("content").and_then(|v| v.as_object()) else {
        return;
    };

    for (media_type, media) in content {
        let Some(schema) = media.get("schema") else {
            continue;
        };
        if !schema_is_named_or_bare(schema) {
            errors.push(format!("{location} ({media_type}): schema is an inline anonymous object, not a named component"));
        }
    }
}

fn schema_is_named_or_bare(schema: &serde_json::Value) -> bool {
    if schema.get("$ref").is_some() {
        return true;
    }
    let is_object_type = matches!(
        schema.get("type").and_then(|v| v.as_str()),
        Some("object") | None
    );
    if is_object_type && schema.get("properties").is_none() {
        return true; // bare `Object` schema (opaque metadata map)
    }
    if schema.get("type").and_then(|v| v.as_str()) == Some("array") {
        return schema.get("items").is_none_or(schema_is_named_or_bare);
    }
    false
}

fn lint_schema(label: &str, schema: &serde_json::Value, errors: &mut Vec<String>) {
    if !is_present(schema.get("description")) {
        errors.push(format!("{label}: schema has no description"));
    }

    for (field_name, field_schema) in schema
        .get("properties")
        .and_then(|v| v.as_object())
        .into_iter()
        .flatten()
    {
        if is_ref_like(field_schema) {
            continue; // the referenced schema carries its own description
        }
        if !is_present(field_schema.get("description")) {
            errors.push(format!("{label}.{field_name}: field has no description"));
        }
    }
}

/// True for a bare `$ref`, or an optional (nullable) `$ref` — OpenAPI 3.1's
/// `Option<NamedType>` renders as `{"anyOf": [{"$ref": ...}, {"type":
/// "null"}]}` rather than a bare `$ref`, but the description still lives
/// on the referenced schema either way.
fn is_ref_like(schema: &serde_json::Value) -> bool {
    if schema.get("$ref").is_some() {
        return true;
    }
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(variants) = schema.get(key).and_then(|v| v.as_array())
            && variants.iter().any(|v| v.get("$ref").is_some())
        {
            return true;
        }
    }
    false
}
