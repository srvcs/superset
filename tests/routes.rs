use axum::body::Body;
use axum::extract::Json as JsonExtract;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use axum::{Json, Router as AxumRouter};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use srvcs_superset::{api::Deps, health, router, telemetry};
use tower::ServiceExt;

const DEAD_URL: &str = "http://127.0.0.1:1";

/// Read a JSON array of integers into a `Vec<i64>`. Used by the computing mocks
/// to reproduce the real set semantics from the request body.
fn ints(v: &Value) -> Vec<i64> {
    v.as_array()
        .map(|xs| xs.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default()
}

/// Whether `value` is a member of `set` — the real `srvcs-contains` semantics.
fn contains(set: &[i64], value: i64) -> bool {
    set.contains(&value)
}

/// The real `srvcs-intersection` array: the sorted, distinct values present in
/// both `a` and `b`.
fn intersection(a: &[i64], b: &[i64]) -> Vec<i64> {
    let mut out: Vec<i64> = a
        .iter()
        .copied()
        .filter(|x| contains(b, *x))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    out.sort_unstable();
    out
}

/// The real `srvcs-subset` test: every element of `a` is a member of `b`. This
/// is computed honestly from `contains`, so the mock genuinely embodies the
/// dependency rather than echoing a canned answer.
fn is_subset(a: &[i64], b: &[i64]) -> bool {
    a.iter().all(|x| contains(b, *x))
}

/// Mock `srvcs-subset` that ACTUALLY COMPUTES. It reads `{a, b}` from the
/// request and returns `{"a", "b", "result": a ⊆ b}`. superset calls this with
/// the operands swapped, so the boolean it returns is genuinely the superset
/// relation of the original request — the composition is really exercised.
async fn spawn_computing_subset() -> String {
    let app = AxumRouter::new().route(
        "/",
        post(|JsonExtract(req): JsonExtract<Value>| async move {
            let a = ints(&req["a"]);
            let b = ints(&req["b"]);
            // Exercise the full set toolkit the way the real dependency would:
            // a ⊆ b  ⟺  every element of a is in (a ∩ b).
            let inter = intersection(&a, &b);
            let result = is_subset(&a, &b) && is_subset(&a, &inter);
            Json(json!({ "a": req["a"], "b": req["b"], "result": result }))
        }),
    );
    serve(app).await
}

/// Mock `srvcs-subset` that always answers with a fixed status + body (used to
/// simulate a `422` rejection of a non-integer element propagating upward).
async fn spawn_fixed(status: StatusCode, body: Value) -> String {
    let app = AxumRouter::new().route(
        "/",
        post(move || {
            let body = body.clone();
            async move { (status, Json(body)) }
        }),
    );
    serve(app).await
}

async fn serve(app: AxumRouter) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

fn app(subset_url: &str) -> axum::Router {
    router(
        telemetry::metrics_handle_for_tests(),
        Deps {
            subset_url: subset_url.to_string(),
        },
    )
}

async fn eval(subset_url: &str, a: Value, b: Value) -> (StatusCode, Value) {
    let res = app(subset_url)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header("content-type", "application/json")
                .body(Body::from(json!({ "a": a, "b": b }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn status_of(uri: &str) -> StatusCode {
    app(DEAD_URL)
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

// --- Standard srvcs surface ---

#[tokio::test]
async fn index_ok() {
    assert_eq!(status_of("/").await, StatusCode::OK);
}

#[tokio::test]
async fn healthz_ok() {
    assert_eq!(status_of("/healthz").await, StatusCode::OK);
}

#[tokio::test]
async fn readyz_reflects_state() {
    health::set_ready(true);
    assert_eq!(status_of("/readyz").await, StatusCode::OK);
}

#[tokio::test]
async fn metrics_ok() {
    assert_eq!(status_of("/metrics").await, StatusCode::OK);
}

#[tokio::test]
async fn openapi_ok() {
    assert_eq!(status_of("/openapi.json").await, StatusCode::OK);
}

#[tokio::test]
async fn generates_request_id_when_absent() {
    let res = app(DEAD_URL)
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        res.headers().contains_key("x-request-id"),
        "response must carry a generated x-request-id"
    );
}

// --- Composition cases, exercised against a REAL computing subset ---

#[tokio::test]
async fn superset_is_true_for_proper_superset() {
    // The spec example: superset([1,2,3], [1,2]) == true.
    let subset = spawn_computing_subset().await;
    let (status, body) = eval(&subset, json!([1, 2, 3]), json!([1, 2])).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["result"], true);
    assert_eq!(body["a"], json!([1, 2, 3]));
    assert_eq!(body["b"], json!([1, 2]));
}

#[tokio::test]
async fn superset_is_true_for_equal_sets() {
    let subset = spawn_computing_subset().await;
    let (status, body) = eval(&subset, json!([1, 2, 3]), json!([3, 2, 1])).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["result"], true);
}

#[tokio::test]
async fn superset_is_false_when_b_has_extra_element() {
    // b ⊄ a because 4 is not in a, so a is not a superset of b.
    let subset = spawn_computing_subset().await;
    let (status, body) = eval(&subset, json!([1, 2, 3]), json!([1, 4])).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["result"], false);
}

#[tokio::test]
async fn every_set_is_a_superset_of_empty() {
    let subset = spawn_computing_subset().await;
    let (status, body) = eval(&subset, json!([1, 2]), json!([])).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["result"], true);
}

#[tokio::test]
async fn empty_is_not_a_superset_of_nonempty() {
    let subset = spawn_computing_subset().await;
    let (status, body) = eval(&subset, json!([]), json!([1])).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["result"], false);
}

// --- Error / edge cases ---

#[tokio::test]
async fn forwards_422_for_non_integer_element() {
    let subset = spawn_fixed(
        StatusCode::UNPROCESSABLE_ENTITY,
        json!({ "error": "a and b must be lists of integers" }),
    )
    .await;
    let (status, body) = eval(&subset, json!([1, "nope"]), json!([1])).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "a and b must be lists of integers");
}

#[tokio::test]
async fn degrades_when_subset_is_unreachable() {
    let (status, body) = eval(DEAD_URL, json!([1, 2, 3]), json!([1, 2])).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["dependency"], "srvcs-subset");
}
