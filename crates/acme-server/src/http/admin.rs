//! `/admin/faults` (spec §9.3, specs/008-Simulation.md item 4): the
//! Simulator page's fault-rule form is a thin client of this same API,
//! not a separate code path. No auth — the dashboard/admin surface has
//! none by design (spec §18).
//!
//! Trimmed from specs/008-Simulation.md item 9: `/admin/traffic/*`
//! (Compare/Replay/HAR/NDJSON export, the programmatic trace API) is not
//! implemented this pass — this module is fault-rule CRUD only. Also not
//! built this pass: a dedicated Swagger/OpenAPI document for this router
//! (spec §9.3 mentions one; the dashboard's own fault-rule form is this
//! router's only client today, so the utoipa boilerplate for a second
//! document is deferred until a second client needs it).

use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get};
use serde::{Deserialize, Serialize};

use crate::db::repo::faults::{self, NewFault};
use crate::domain::ids::FaultId;
use crate::error::AppError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/faults", get(list_faults).post(create_fault))
        .route("/admin/faults/{id}", delete(delete_fault))
}

#[derive(Serialize)]
struct FaultDto {
    id: String,
    provider_slug: Option<String>,
    method: Option<String>,
    path_glob: String,
    mode: String,
    http_status: Option<i32>,
    error_code: Option<String>,
    latency_ms: Option<i64>,
    probability: f64,
    remaining: Option<i32>,
    active: bool,
    note: Option<String>,
    created_at: String,
    expires_at: Option<String>,
}

impl From<faults::FaultRow> for FaultDto {
    fn from(row: faults::FaultRow) -> Self {
        FaultDto {
            id: row.id.to_string(),
            provider_slug: row.provider_slug,
            method: row.method,
            path_glob: row.path_glob,
            mode: row.mode,
            http_status: row.http_status,
            error_code: row.error_code,
            latency_ms: row.latency_ms,
            probability: row.probability,
            remaining: row.remaining,
            active: row.active,
            note: row.note,
            created_at: row.created_at.to_rfc3339(),
            expires_at: row.expires_at.map(|t| t.to_rfc3339()),
        }
    }
}

async fn list_faults(State(state): State<AppState>) -> Result<Json<Vec<FaultDto>>, AppError> {
    let rows = faults::list(&state.db).await?;
    Ok(Json(rows.into_iter().map(FaultDto::from).collect()))
}

#[derive(Deserialize)]
pub struct CreateFaultRequest {
    pub provider_slug: Option<String>,
    pub method: Option<String>,
    pub path_glob: String,
    pub mode: String,
    pub http_status: Option<i32>,
    pub error_code: Option<String>,
    pub latency_ms: Option<i64>,
    #[serde(default = "default_probability")]
    pub probability: f64,
    pub remaining: Option<i32>,
    pub note: Option<String>,
    pub expires_in_seconds: Option<i64>,
}

fn default_probability() -> f64 {
    1.0
}

async fn create_fault(
    State(state): State<AppState>,
    Json(body): Json<CreateFaultRequest>,
) -> Result<(StatusCode, Json<FaultDto>), AppError> {
    let now = state.clock.now();
    let id = faults::create(
        &state.db,
        &NewFault {
            provider_slug: body.provider_slug,
            method: body.method,
            path_glob: body.path_glob,
            mode: body.mode,
            http_status: body.http_status,
            error_code: body.error_code,
            latency_ms: body.latency_ms,
            probability: body.probability,
            remaining: body.remaining,
            note: body.note,
            created_at: now,
            expires_at: body
                .expires_in_seconds
                .map(|secs| now + chrono::Duration::seconds(secs)),
        },
    )
    .await?;

    let rows = faults::list(&state.db).await?;
    let row = rows
        .into_iter()
        .find(|r| r.id == id)
        .ok_or_else(|| anyhow::anyhow!("fault vanished immediately after creation"))?;
    Ok((StatusCode::CREATED, Json(row.into())))
}

async fn delete_fault(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    let id: FaultId = id
        .parse()
        .map_err(|_| anyhow::anyhow!("malformed fault id"))?;
    if faults::delete(&state.db, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Ok(StatusCode::NOT_FOUND)
    }
}
