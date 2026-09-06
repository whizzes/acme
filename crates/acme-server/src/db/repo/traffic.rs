//! Read side of the traffic inspector (spec §21.8): the first reader of
//! `http_exchanges`/`http_bodies`/`exchange_resources`, which
//! `capture::recorder` has been writing since M1.

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::domain::ids::{ExchangeId, TraceId};

fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .expect("stored timestamps are valid rfc3339")
        .with_timezone(&Utc)
}

pub struct ExchangeRow {
    pub id: ExchangeId,
    pub trace_id: TraceId,
    pub parent_id: Option<ExchangeId>,
    pub direction: String,
    pub channel: String,
    pub started_at: DateTime<Utc>,
    pub sim_at: DateTime<Utc>,
    pub duration_ms: Option<i64>,
    pub provider_slug: Option<String>,
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    pub route_pattern: Option<String>,
    pub status_code: Option<i32>,
    pub outcome: String,
    pub idempotency_key: Option<String>,
    pub idempotent_replay: bool,
    /// Set only on `Channel::Webhook` exchanges (spec §21.9/§12.1's
    /// `Acme-Attempt` header) — the delivery attempt number this exchange
    /// recorded, and the exact signed string/signature sent with it.
    pub attempt: Option<i32>,
    pub signed_payload: Option<String>,
    pub signature: Option<String>,
}

#[derive(sqlx::FromRow)]
struct ExchangeSqlRow {
    id: String,
    trace_id: String,
    parent_id: Option<String>,
    direction: String,
    channel: String,
    started_at: String,
    sim_at: String,
    duration_ms: Option<i64>,
    provider_slug: Option<String>,
    method: String,
    path: String,
    query: Option<String>,
    route_pattern: Option<String>,
    status_code: Option<i64>,
    outcome: String,
    idempotency_key: Option<String>,
    idempotent_replay: i64,
    attempt: Option<i64>,
    signed_payload: Option<String>,
    signature: Option<String>,
}

impl ExchangeSqlRow {
    fn into_row(self) -> ExchangeRow {
        ExchangeRow {
            id: self.id.parse().expect("stored exchange id is valid"),
            trace_id: self.trace_id.parse().expect("stored trace id is valid"),
            parent_id: self.parent_id.and_then(|p| p.parse().ok()),
            direction: self.direction,
            channel: self.channel,
            started_at: parse_dt(&self.started_at),
            sim_at: parse_dt(&self.sim_at),
            duration_ms: self.duration_ms,
            provider_slug: self.provider_slug,
            method: self.method,
            path: self.path,
            query: self.query,
            route_pattern: self.route_pattern,
            status_code: self.status_code.map(|s| s as i32),
            outcome: self.outcome,
            idempotency_key: self.idempotency_key,
            idempotent_replay: self.idempotent_replay != 0,
            attempt: self.attempt.map(|a| a as i32),
            signed_payload: self.signed_payload,
            signature: self.signature,
        }
    }
}

const EXCHANGE_COLUMNS: &str = "id, trace_id, parent_id, direction, channel, started_at, sim_at, duration_ms,
     provider_slug, method, path, query, route_pattern, status_code, outcome, idempotency_key, idempotent_replay,
     attempt, signed_payload, signature";

/// The body/header detail behind one exchange, joined in by `get`.
pub struct ExchangeDetail {
    pub exchange: ExchangeRow,
    pub url: String,
    pub http_version: String,
    pub request_headers: String,
    pub request_body: Option<BodyRow>,
    pub request_bytes: i64,
    pub status_text: Option<String>,
    pub response_headers: Option<String>,
    pub response_body: Option<BodyRow>,
    pub response_bytes: i64,
    pub transport_error: Option<String>,
    pub error_code: Option<String>,
    pub merchant_id: Option<String>,
    pub auth_subject: Option<String>,
    pub replay_of: Option<ExchangeId>,
    pub search_key: Option<String>,
}

pub struct BodyRow {
    pub content_type: Option<String>,
    pub encoding: String,
    pub size_bytes: i64,
    pub truncated: bool,
    pub redactions: i64,
    pub body: Option<Vec<u8>>,
}

pub struct ResourceRef {
    pub resource_type: String,
    pub resource_id: String,
    pub role: String,
}

#[derive(Default)]
pub struct ListFilter {
    pub direction: Option<String>,
    pub provider_slug: Option<String>,
    pub method: Option<String>,
    pub status_class: Option<String>,
    pub path_contains: Option<String>,
    pub idempotent_replay_only: bool,
    pub limit: i64,
    pub starting_after: Option<ExchangeId>,
}

/// Cursor-paginated, newest first — `starting_after` is the previous
/// page's last id (exchange ids are ULIDs, so lexical order is creation
/// order, same idiom as `db::repo::payments::list`).
pub async fn list(pool: &SqlitePool, filter: &ListFilter) -> anyhow::Result<Vec<ExchangeRow>> {
    let mut sql = format!("SELECT {EXCHANGE_COLUMNS} FROM http_exchanges WHERE 1=1");
    let mut binds: Vec<String> = Vec::new();

    if let Some(direction) = &filter.direction {
        sql.push_str(" AND direction = ?");
        binds.push(direction.clone());
    }
    if let Some(provider_slug) = &filter.provider_slug {
        sql.push_str(" AND provider_slug = ?");
        binds.push(provider_slug.clone());
    }
    if let Some(method) = &filter.method {
        sql.push_str(" AND method = ?");
        binds.push(method.clone());
    }
    if let Some(class) = &filter.status_class {
        // e.g. "4" for 4xx — one leading digit is enough to bucket a class.
        sql.push_str(" AND CAST(status_code AS TEXT) LIKE ?");
        binds.push(format!("{class}%"));
    }
    if let Some(path) = &filter.path_contains {
        sql.push_str(" AND path LIKE ?");
        binds.push(format!("%{path}%"));
    }
    if filter.idempotent_replay_only {
        sql.push_str(" AND idempotent_replay = 1");
    }
    if let Some(starting_after) = filter.starting_after {
        sql.push_str(" AND id < ?");
        binds.push(starting_after.to_string());
    }
    sql.push_str(" ORDER BY id DESC LIMIT ?");

    let mut query = sqlx::query_as::<_, ExchangeSqlRow>(&sql);
    for bind in &binds {
        query = query.bind(bind);
    }
    query = query.bind(filter.limit);

    let rows = query.fetch_all(pool).await?;
    Ok(rows.into_iter().map(ExchangeSqlRow::into_row).collect())
}

pub async fn get(pool: &SqlitePool, id: ExchangeId) -> anyhow::Result<Option<ExchangeDetail>> {
    #[derive(sqlx::FromRow)]
    struct DetailSqlRow {
        id: String,
        trace_id: String,
        parent_id: Option<String>,
        direction: String,
        channel: String,
        started_at: String,
        sim_at: String,
        duration_ms: Option<i64>,
        provider_slug: Option<String>,
        method: String,
        path: String,
        query: Option<String>,
        route_pattern: Option<String>,
        status_code: Option<i64>,
        outcome: String,
        idempotency_key: Option<String>,
        idempotent_replay: i64,
        url: String,
        http_version: String,
        request_headers: String,
        request_bytes: i64,
        request_body_content_type: Option<String>,
        request_body_encoding: Option<String>,
        request_body_size_bytes: Option<i64>,
        request_body_truncated: Option<i64>,
        request_body_redactions: Option<i64>,
        request_body: Option<Vec<u8>>,
        status_text: Option<String>,
        response_headers: Option<String>,
        response_bytes: i64,
        response_body_content_type: Option<String>,
        response_body_encoding: Option<String>,
        response_body_size_bytes: Option<i64>,
        response_body_truncated: Option<i64>,
        response_body_redactions: Option<i64>,
        response_body: Option<Vec<u8>>,
        transport_error: Option<String>,
        error_code: Option<String>,
        merchant_id: Option<String>,
        auth_subject: Option<String>,
        replay_of: Option<String>,
        search_key: Option<String>,
        attempt: Option<i64>,
        signed_payload: Option<String>,
        signature: Option<String>,
    }

    let row: Option<DetailSqlRow> = sqlx::query_as(
        "SELECT ex.id, ex.trace_id, ex.parent_id, ex.direction, ex.channel, ex.started_at, ex.sim_at,
                ex.duration_ms, ex.provider_slug, ex.method, ex.path, ex.query, ex.route_pattern,
                ex.status_code, ex.outcome, ex.idempotency_key, ex.idempotent_replay,
                ex.attempt, ex.signed_payload, ex.signature,
                ex.url, ex.http_version, ex.request_headers, ex.request_bytes,
                rb.content_type AS request_body_content_type, rb.encoding AS request_body_encoding,
                rb.size_bytes AS request_body_size_bytes, rb.truncated AS request_body_truncated,
                rb.redactions AS request_body_redactions, rb.body AS request_body,
                ex.status_text, ex.response_headers, ex.response_bytes,
                sb.content_type AS response_body_content_type, sb.encoding AS response_body_encoding,
                sb.size_bytes AS response_body_size_bytes, sb.truncated AS response_body_truncated,
                sb.redactions AS response_body_redactions, sb.body AS response_body,
                ex.transport_error, ex.error_code, ex.merchant_id, ex.auth_subject, ex.replay_of, ex.search_key
         FROM http_exchanges ex
         LEFT JOIN http_bodies rb ON rb.id = ex.request_body_id
         LEFT JOIN http_bodies sb ON sb.id = ex.response_body_id
         WHERE ex.id = ?1",
    )
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| ExchangeDetail {
        request_body: r.request_body_encoding.map(|encoding| BodyRow {
            content_type: r.request_body_content_type,
            encoding,
            size_bytes: r.request_body_size_bytes.unwrap_or(0),
            truncated: r.request_body_truncated.unwrap_or(0) != 0,
            redactions: r.request_body_redactions.unwrap_or(0),
            body: r.request_body,
        }),
        response_body: r.response_body_encoding.map(|encoding| BodyRow {
            content_type: r.response_body_content_type,
            encoding,
            size_bytes: r.response_body_size_bytes.unwrap_or(0),
            truncated: r.response_body_truncated.unwrap_or(0) != 0,
            redactions: r.response_body_redactions.unwrap_or(0),
            body: r.response_body,
        }),
        url: r.url,
        http_version: r.http_version,
        request_headers: r.request_headers,
        request_bytes: r.request_bytes,
        status_text: r.status_text,
        response_headers: r.response_headers,
        response_bytes: r.response_bytes,
        transport_error: r.transport_error,
        error_code: r.error_code,
        merchant_id: r.merchant_id,
        auth_subject: r.auth_subject,
        replay_of: r.replay_of.and_then(|s| s.parse().ok()),
        search_key: r.search_key,
        exchange: ExchangeRow {
            id: r.id.parse().expect("stored exchange id is valid"),
            trace_id: r.trace_id.parse().expect("stored trace id is valid"),
            parent_id: r.parent_id.and_then(|p| p.parse().ok()),
            direction: r.direction,
            channel: r.channel,
            started_at: parse_dt(&r.started_at),
            sim_at: parse_dt(&r.sim_at),
            duration_ms: r.duration_ms,
            provider_slug: r.provider_slug,
            method: r.method,
            path: r.path,
            query: r.query,
            route_pattern: r.route_pattern,
            status_code: r.status_code.map(|s| s as i32),
            outcome: r.outcome,
            idempotency_key: r.idempotency_key,
            idempotent_replay: r.idempotent_replay != 0,
            attempt: r.attempt.map(|a| a as i32),
            signed_payload: r.signed_payload,
            signature: r.signature,
        },
    }))
}

/// Every resource an exchange touched (spec §21.8's "Related" pane).
pub async fn resources_for(
    pool: &SqlitePool,
    exchange_id: ExchangeId,
) -> anyhow::Result<Vec<ResourceRef>> {
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT resource_type, resource_id, role FROM exchange_resources WHERE exchange_id = ?1",
    )
    .bind(exchange_id.to_string())
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(resource_type, resource_id, role)| ResourceRef {
            resource_type,
            resource_id,
            role,
        })
        .collect())
}

/// Every exchange sharing `trace_id`, oldest first — the waterfall view
/// (spec §21.8's Trace view).
pub async fn trace(pool: &SqlitePool, trace_id: TraceId) -> anyhow::Result<Vec<ExchangeRow>> {
    let rows: Vec<ExchangeSqlRow> = sqlx::query_as(&format!(
        "SELECT {EXCHANGE_COLUMNS} FROM http_exchanges WHERE trace_id = ?1 ORDER BY started_at ASC"
    ))
    .bind(trace_id.to_string())
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(ExchangeSqlRow::into_row).collect())
}

/// Every attempt recorded for one webhook delivery, oldest first (spec
/// §21.9's attempt timeline) — `dashboard::dispatcher` stamps `delivery_id`
/// on each outbound `Exchange` it records, so this is a plain filter over
/// the same table `/traffic` already reads.
pub async fn list_by_delivery(
    pool: &SqlitePool,
    delivery_id: crate::domain::ids::WebhookDeliveryId,
) -> anyhow::Result<Vec<ExchangeRow>> {
    let rows: Vec<ExchangeSqlRow> = sqlx::query_as(&format!(
        "SELECT {EXCHANGE_COLUMNS} FROM http_exchanges WHERE delivery_id = ?1 ORDER BY attempt ASC"
    ))
    .bind(delivery_id.to_string())
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(ExchangeSqlRow::into_row).collect())
}

/// Total exchange count — the Overview KPI row and `/traffic`'s empty
/// state both need this cheaply.
pub async fn count(pool: &SqlitePool) -> anyhow::Result<i64> {
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM http_exchanges")
        .fetch_one(pool)
        .await?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::recorder::{
        BodyCapture, Channel, Exchange, ResourceRef as WriteResourceRef,
    };
    use bytes::Bytes;

    async fn insert_exchange(
        pool: &SqlitePool,
        method: &str,
        path: &str,
        status_code: u16,
        trace_id: TraceId,
    ) -> ExchangeId {
        let exchange = Exchange {
            id: ExchangeId::new(),
            trace_id,
            parent_id: None,
            direction: "inbound",
            channel: Channel::Api,
            started_at: Utc::now(),
            sim_at: Utc::now(),
            duration_ms: 12,
            method: method.into(),
            url: format!("http://localhost{path}"),
            path: path.into(),
            query: None,
            route_pattern: Some(path.into()),
            request_headers: vec![("content-type".into(), "application/json".into())],
            request_body: Some(BodyCapture {
                content_type: Some("application/json".into()),
                bytes: Bytes::from_static(b"{}"),
                encoding: "utf8",
                redactions: 0,
            }),
            status_code: Some(status_code),
            response_headers: vec![],
            response_body: None,
            outcome: "ok",
            search_key: None,
            resources: vec![WriteResourceRef {
                resource_type: "payment".into(),
                resource_id: "pay_test".into(),
                role: "created".into(),
            }],
            delivery_id: None,
            event_id: None,
            attempt: None,
            signed_payload: None,
            signature: None,
        };
        let id = exchange.id;
        crate::capture::recorder::write_exchange(pool, exchange)
            .await
            .unwrap();
        id
    }

    #[sqlx::test]
    async fn list_filters_by_method_and_status_class(pool: SqlitePool) {
        let trace_id = TraceId::new();
        insert_exchange(&pool, "GET", "/acmepay/v1/payments", 200, trace_id).await;
        insert_exchange(&pool, "POST", "/acmepay/v1/payments", 422, trace_id).await;

        let all = list(
            &pool,
            &ListFilter {
                limit: 10,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(all.len(), 2);

        let posts = list(
            &pool,
            &ListFilter {
                method: Some("POST".into()),
                limit: 10,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(posts.len(), 1);

        let errors = list(
            &pool,
            &ListFilter {
                status_class: Some("4".into()),
                limit: 10,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].status_code, Some(422));
    }

    #[sqlx::test]
    async fn get_joins_bodies_and_resources(pool: SqlitePool) {
        let id = insert_exchange(&pool, "GET", "/x", 200, TraceId::new()).await;

        let detail = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(detail.exchange.id, id);
        let body = detail.request_body.unwrap();
        assert_eq!(body.body.as_deref(), Some(&b"{}"[..]));

        let resources = resources_for(&pool, id).await.unwrap();
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].resource_id, "pay_test");
    }

    #[sqlx::test]
    async fn trace_returns_every_exchange_sharing_the_trace_id(pool: SqlitePool) {
        let trace_id = TraceId::new();
        insert_exchange(&pool, "GET", "/a", 200, trace_id).await;
        insert_exchange(&pool, "GET", "/b", 200, trace_id).await;
        insert_exchange(&pool, "GET", "/c", 200, TraceId::new()).await;

        let chain = trace(&pool, trace_id).await.unwrap();
        assert_eq!(chain.len(), 2);
    }
}
