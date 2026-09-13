//! Fault-rule persistence (spec §9.3, specs/008-Simulation.md item 3):
//! `sim_faults` CRUD plus the candidate query `sim::fault::inject` matches
//! against on every request.

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::domain::ids::FaultId;

pub struct NewFault {
    pub provider_slug: Option<String>,
    pub method: Option<String>,
    pub path_glob: String,
    pub mode: String,
    pub http_status: Option<i32>,
    pub error_code: Option<String>,
    pub latency_ms: Option<i64>,
    pub probability: f64,
    pub remaining: Option<i32>,
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

pub struct FaultRow {
    pub id: FaultId,
    pub provider_slug: Option<String>,
    pub method: Option<String>,
    pub path_glob: String,
    pub mode: String,
    pub http_status: Option<i32>,
    pub error_code: Option<String>,
    pub latency_ms: Option<i64>,
    pub probability: f64,
    pub remaining: Option<i32>,
    pub active: bool,
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct FaultSqlRow {
    id: String,
    provider_slug: Option<String>,
    method: Option<String>,
    path_glob: String,
    mode: String,
    http_status: Option<i64>,
    error_code: Option<String>,
    latency_ms: Option<i64>,
    probability: f64,
    remaining: Option<i64>,
    active: i64,
    note: Option<String>,
    created_at: String,
    expires_at: Option<String>,
}

fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .expect("stored timestamps are valid rfc3339")
        .with_timezone(&Utc)
}

impl FaultSqlRow {
    fn into_row(self) -> FaultRow {
        FaultRow {
            id: self.id.parse().expect("stored fault id is valid"),
            provider_slug: self.provider_slug,
            method: self.method,
            path_glob: self.path_glob,
            mode: self.mode,
            http_status: self.http_status.map(|v| v as i32),
            error_code: self.error_code,
            latency_ms: self.latency_ms,
            probability: self.probability,
            remaining: self.remaining.map(|v| v as i32),
            active: self.active != 0,
            note: self.note,
            created_at: parse_dt(&self.created_at),
            expires_at: self.expires_at.as_deref().map(parse_dt),
        }
    }
}

const FAULT_COLUMNS: &str = "id, provider_slug, method, path_glob, mode, http_status, error_code,
     latency_ms, probability, remaining, active, note, created_at, expires_at";

pub async fn create(pool: &SqlitePool, new: &NewFault) -> anyhow::Result<FaultId> {
    let id = FaultId::new();
    sqlx::query(
        "INSERT INTO sim_faults (id, provider_slug, method, path_glob, mode, http_status, error_code,
            latency_ms, probability, remaining, active, note, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 1, ?11, ?12, ?13)",
    )
    .bind(id.to_string())
    .bind(&new.provider_slug)
    .bind(&new.method)
    .bind(&new.path_glob)
    .bind(&new.mode)
    .bind(new.http_status)
    .bind(&new.error_code)
    .bind(new.latency_ms)
    .bind(new.probability)
    .bind(new.remaining)
    .bind(&new.note)
    .bind(new.created_at.to_rfc3339())
    .bind(new.expires_at.map(|t| t.to_rfc3339()))
    .execute(pool)
    .await?;
    Ok(id)
}

pub async fn list(pool: &SqlitePool) -> anyhow::Result<Vec<FaultRow>> {
    let rows: Vec<FaultSqlRow> = sqlx::query_as(&format!(
        "SELECT {FAULT_COLUMNS} FROM sim_faults ORDER BY created_at DESC"
    ))
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(FaultSqlRow::into_row).collect())
}

/// Every rule this request could possibly match: `provider_slug`/`method`
/// either `NULL` (matches anything) or an exact match, `active = 1`, and
/// not expired. `path_glob` matching happens in `sim::fault` itself — a
/// glob isn't expressible in SQL without `LIKE`-escaping surprises, and
/// the candidate set here is already small (sandbox rule tables are
/// hand-authored, never thousands of rows).
pub async fn list_active_candidates(
    pool: &SqlitePool,
    provider_slug: &str,
    method: &str,
    now: DateTime<Utc>,
) -> anyhow::Result<Vec<FaultRow>> {
    let rows: Vec<FaultSqlRow> = sqlx::query_as(&format!(
        "SELECT {FAULT_COLUMNS} FROM sim_faults
         WHERE active = 1
           AND (provider_slug IS NULL OR provider_slug = ?1)
           AND (method IS NULL OR method = ?2)
           AND (expires_at IS NULL OR expires_at > ?3)
         ORDER BY created_at ASC"
    ))
    .bind(provider_slug)
    .bind(method)
    .bind(now.to_rfc3339())
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(FaultSqlRow::into_row).collect())
}

pub async fn delete(pool: &SqlitePool, id: FaultId) -> anyhow::Result<bool> {
    let result = sqlx::query("DELETE FROM sim_faults WHERE id = ?1")
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Decrements a fired rule's `remaining` counter and deactivates it at
/// zero. A `NULL` `remaining` (fire forever) is left untouched.
pub async fn decrement_remaining(pool: &SqlitePool, id: FaultId) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE sim_faults SET remaining = remaining - 1 WHERE id = ?1 AND remaining IS NOT NULL",
    )
    .bind(id.to_string())
    .execute(pool)
    .await?;
    sqlx::query(
        "UPDATE sim_faults SET active = 0 WHERE id = ?1 AND remaining IS NOT NULL AND remaining <= 0",
    )
    .bind(id.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(provider_slug: Option<&str>, method: Option<&str>, path_glob: &str) -> NewFault {
        NewFault {
            provider_slug: provider_slug.map(str::to_string),
            method: method.map(str::to_string),
            path_glob: path_glob.to_string(),
            mode: "error".to_string(),
            http_status: Some(503),
            error_code: Some("api_connection_error".to_string()),
            latency_ms: None,
            probability: 1.0,
            remaining: None,
            note: None,
            created_at: Utc::now(),
            expires_at: None,
        }
    }

    #[sqlx::test]
    async fn candidates_respect_provider_and_method_scoping(pool: SqlitePool) {
        let now = Utc::now();
        create(
            &pool,
            &fixture(Some("acmepay"), Some("POST"), "/acmepay/v1/*"),
        )
        .await
        .unwrap();
        create(&pool, &fixture(None, None, "/*")).await.unwrap();
        create(&pool, &fixture(Some("acmeship"), None, "/*"))
            .await
            .unwrap();

        let matches = list_active_candidates(&pool, "acmepay", "POST", now)
            .await
            .unwrap();
        assert_eq!(
            matches.len(),
            2,
            "the acmepay-specific rule and the wildcard rule"
        );

        let matches = list_active_candidates(&pool, "acmeship", "GET", now)
            .await
            .unwrap();
        assert_eq!(
            matches.len(),
            2,
            "the acmeship-specific rule and the wildcard rule"
        );
    }

    #[sqlx::test]
    async fn expired_rules_never_match(pool: SqlitePool) {
        let now = Utc::now();
        let mut fault = fixture(None, None, "/*");
        fault.expires_at = Some(now - chrono::Duration::seconds(1));
        create(&pool, &fault).await.unwrap();

        let matches = list_active_candidates(&pool, "acmepay", "POST", now)
            .await
            .unwrap();
        assert!(matches.is_empty());
    }

    #[sqlx::test]
    async fn decrement_remaining_deactivates_at_zero(pool: SqlitePool) {
        let mut fault = fixture(None, None, "/*");
        fault.remaining = Some(1);
        let id = create(&pool, &fault).await.unwrap();

        decrement_remaining(&pool, id).await.unwrap();

        let rows = list(&pool).await.unwrap();
        let row = rows.iter().find(|r| r.id == id).unwrap();
        assert_eq!(row.remaining, Some(0));
        assert!(!row.active);
    }
}
