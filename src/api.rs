use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::{OpenApi, ToSchema};

use crate::client::{self, DepError};

pub const SERVICE: &str = "srvcs-superset";
pub const CONCERN: &str = "sets: is a a superset of b";
pub const DEPENDS_ON: &[&str] = &["srvcs-subset"];

/// Dependency endpoints, injected as router state so tests can point them at
/// mock services.
#[derive(Clone)]
pub struct Deps {
    pub subset_url: String,
}

#[derive(Serialize, ToSchema)]
pub struct Info {
    pub service: &'static str,
    pub concern: &'static str,
    pub depends_on: Vec<&'static str>,
}

/// `GET /` — service identity (srvcs service standard).
#[utoipa::path(get, path = "/", responses((status = 200, body = Info)))]
pub async fn index() -> Json<Info> {
    Json(Info {
        service: SERVICE,
        concern: CONCERN,
        depends_on: DEPENDS_ON.to_vec(),
    })
}

#[derive(Deserialize, ToSchema)]
pub struct EvalRequest {
    /// The candidate superset, as a list of integers. Every element must be a
    /// JSON integer (i64).
    #[schema(value_type = Object)]
    pub a: Vec<Value>,
    /// The candidate subset, as a list of integers. Every element must be a
    /// JSON integer (i64).
    #[schema(value_type = Object)]
    pub b: Vec<Value>,
}

#[derive(Serialize, ToSchema)]
pub struct ResultResponse {
    #[schema(value_type = Object)]
    pub a: Vec<Value>,
    #[schema(value_type = Object)]
    pub b: Vec<Value>,
    pub result: bool,
}

fn degraded(dependency: &str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "error": "dependency unavailable", "dependency": dependency })),
    )
        .into_response()
}

/// Forward a dependency's response verbatim (used to propagate `422` for invalid
/// input, so superset reports the same rejection a leaf dependency did).
fn forward(status: u16, body: Value) -> Response {
    let code = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    (code, Json(body)).into_response()
}

/// Ask the boolean `srvcs-subset` dependency with `payload` for its `result`,
/// mapping its failures to the response this service should return.
///
/// - unreachable / non-`200`/`422` -> `503` degraded
/// - `422` -> forwarded `422` (the dependency rejected the input)
/// - `200` -> the `result` boolean
async fn ask(url: &str, payload: &Value, dependency: &str) -> Result<bool, Response> {
    match client::call(url, payload).await {
        Err(DepError::Unreachable) => Err(degraded(dependency)),
        Ok((200, body)) => Ok(body.get("result").and_then(Value::as_bool).unwrap_or(false)),
        // Invalid input propagates from the leaf dependency; forward it.
        Ok((422, body)) => Err(forward(422, body)),
        Ok(_) => Err(degraded(dependency)),
    }
}

/// `POST /` — decide whether `a` is a superset of `b`.
///
/// This service is a pure orchestrator: `a ⊇ b` is exactly `b ⊆ a`, so it
/// delegates to `srvcs-subset` with the arguments swapped and returns that
/// service's boolean `result`. It performs no set logic of its own and does not
/// call `srvcs-isnumber` directly — element validation propagates up from
/// `srvcs-subset`, whose `422` is forwarded unchanged.
#[utoipa::path(
    post,
    path = "/",
    request_body = EvalRequest,
    responses(
        (status = 200, body = ResultResponse),
        (status = 422, description = "an element is not a valid integer (forwarded from srvcs-subset)"),
        (status = 500, description = "a dependency returned a malformed result"),
        (status = 503, description = "the srvcs-subset dependency is unavailable")
    )
)]
pub async fn evaluate(State(deps): State<Deps>, Json(req): Json<EvalRequest>) -> Response {
    // a ⊇ b  ⟺  b ⊆ a : delegate to subset with swapped operands.
    let result = match ask(
        &deps.subset_url,
        &json!({ "a": req.b, "b": req.a }),
        "srvcs-subset",
    )
    .await
    {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    (
        StatusCode::OK,
        Json(json!({ "a": req.a, "b": req.b, "result": result })),
    )
        .into_response()
}

#[derive(OpenApi)]
#[openapi(
    paths(index, evaluate),
    components(schemas(Info, EvalRequest, ResultResponse))
)]
pub struct ApiDoc;

/// Serve OpenAPI document
pub async fn openapi_json() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openapi_documents_routes() {
        let doc = ApiDoc::openapi();
        let root = doc.paths.paths.get("/").expect("path / present");
        assert!(root.get.is_some());
        assert!(root.post.is_some());
    }

    #[tokio::test]
    async fn index_reports_dependency() {
        let Json(info) = index().await;
        assert_eq!(info.service, "srvcs-superset");
        assert_eq!(info.concern, "sets: is a a superset of b");
        assert_eq!(info.depends_on, vec!["srvcs-subset"]);
    }
}
