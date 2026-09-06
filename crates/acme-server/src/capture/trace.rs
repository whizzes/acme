//! Trace-id propagation and resource tagging (spec §21.4). Task-locals let
//! domain/repo code deep in a call stack tag the current exchange without
//! threading extra parameters through every function signature.

use std::cell::RefCell;
use std::future::Future;

use tokio::task_local;

use crate::domain::ids::TraceId;

pub struct ResourceRef {
    pub resource_type: &'static str,
    pub resource_id: String,
    pub role: &'static str,
}

task_local! {
    static TRACE_ID: TraceId;
    static RESOURCES: RefCell<Vec<ResourceRef>>;
}

/// Runs `f` with `trace_id` current for its whole (async) call graph.
pub async fn scope<F: Future>(trace_id: TraceId, f: F) -> F::Output {
    TRACE_ID
        .scope(trace_id, RESOURCES.scope(RefCell::new(Vec::new()), f))
        .await
}

/// The trace id for the request currently being handled, if any.
pub fn current_trace_id() -> Option<TraceId> {
    TRACE_ID.try_with(|id| *id).ok()
}

/// Tags a domain object as touched by the exchange currently being
/// captured. A no-op outside a `scope` (e.g. from the ticker, which is not
/// itself an inbound request).
pub fn mark_resource(resource_type: &'static str, resource_id: String, role: &'static str) {
    let _ = RESOURCES.try_with(|cell| {
        cell.borrow_mut().push(ResourceRef {
            resource_type,
            resource_id,
            role,
        });
    });
}

/// Drains every resource marked so far in the current scope.
pub fn take_resources() -> Vec<ResourceRef> {
    RESOURCES
        .try_with(|cell| cell.borrow_mut().drain(..).collect())
        .unwrap_or_default()
}
