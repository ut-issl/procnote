//! API behavior behind Cloudflare Access, against a local stand-in for the
//! team's key endpoint and tokens signed with a test key.
#![expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]

use std::path::Path;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::{Value, json};
use tower::ServiceExt;

use procnote_app::persistence::execution_store::ExecutionStore;
use procnote_server::access::{AccessConfig, TOKEN_HEADER};
use procnote_server::{Auth, ServerConfig, router};

const SIGNER: &str = include_str!("fixtures/access-test-signer.pem");
const JWKS: &str = include_str!("fixtures/access-test-jwks.json");
const AUD: &str = "test-aud-tag";

const TEMPLATE: &str = r"---
id: access-test
title: Access Test
version: 1.0.0
---

## Prepare

- [ ] Ready
";

struct Fixture {
    _temp: tempfile::TempDir,
    app: axum::Router,
    team_domain: String,
    template_path: String,
}

/// Serve the test JWKS where Access publishes a team's signing keys.
async fn serve_team_keys() -> String {
    let keys: Value = serde_json::from_str(JWKS).unwrap();
    let app = axum::Router::new().route(
        "/cdn-cgi/access/certs",
        axum::routing::get(move || async move { axum::Json(keys) }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, app).into_future());
    format!("http://{addr}")
}

async fn fixture() -> Fixture {
    let team_domain = serve_team_keys().await;
    let temp = tempfile::tempdir().unwrap();
    let procedures_dir = temp.path().join("procedures");
    let template_dir = procedures_dir.join("access-test");
    std::fs::create_dir_all(&template_dir).unwrap();
    std::fs::write(template_dir.join("template.md"), TEMPLATE).unwrap();
    let app = router(ServerConfig {
        executions: Box::new(ExecutionStore::new(procedures_dir.canonicalize().unwrap())),
        procedures_dir: procedures_dir.canonicalize().unwrap(),
        upload_dir: temp.path().join("uploads"),
        static_dir: None,
        state_dir: temp.path().join("state"),
        auth: Auth::Access(AccessConfig {
            team_domain: team_domain.clone(),
            audience: AUD.to_string(),
        }),
        collab: None,
    })
    .unwrap();
    Fixture {
        template_path: template_dir
            .join("template.md")
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .to_string(),
        _temp: temp,
        app,
        team_domain,
    }
}

fn token(fixture: &Fixture, email: &str) -> String {
    let now = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap();
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("test-key-1".to_string());
    encode(
        &header,
        &json!({
            "aud": [AUD],
            "email": email,
            "exp": now + 600,
            "iat": now,
            "nbf": now,
            "iss": fixture.team_domain,
            "type": "app",
            "sub": "user-id",
        }),
        &EncodingKey::from_rsa_pem(SIGNER.as_bytes()).unwrap(),
    )
    .unwrap()
}

async fn send(app: &axum::Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn get(uri: &str, auth: Option<&str>) -> Request<Body> {
    let mut request = Request::get(uri);
    if let Some(token) = auth {
        request = request.header(TOKEN_HEADER, token);
    }
    request.body(Body::empty()).unwrap()
}

fn post_json(uri: &str, body: &Value, auth: Option<&str>) -> Request<Body> {
    let mut request = Request::post(uri).header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = auth {
        request = request.header(TOKEN_HEADER, token);
    }
    request.body(Body::from(body.to_string())).unwrap()
}

async fn start(fixture: &Fixture, token: &str) -> Value {
    let (status, summary) = send(
        &fixture.app,
        post_json(
            "/api/executions",
            &json!({ "template_path": fixture.template_path }),
            Some(token),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    summary
}

#[tokio::test]
async fn requests_without_a_valid_token_are_rejected() {
    let fixture = fixture().await;

    let (missing, _) = send(&fixture.app, get("/api/templates", None)).await;
    let (garbage, _) = send(&fixture.app, get("/api/templates", Some("not.a.jwt"))).await;
    let (me, _) = send(&fixture.app, get("/api/me", None)).await;

    assert_eq!(missing, StatusCode::UNAUTHORIZED);
    assert_eq!(garbage, StatusCode::UNAUTHORIZED);
    assert_eq!(me, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_valid_token_in_the_header_or_cookie_identifies_the_caller() {
    let fixture = fixture().await;
    let token = token(&fixture, "operator@example.ac.jp");

    let (status, me) = send(&fixture.app, get("/api/me", Some(&token))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        me,
        json!({ "auth": "access", "email": "operator@example.ac.jp" })
    );

    let by_cookie = Request::get("/api/templates")
        .header(header::COOKIE, format!("CF_Authorization={token}"))
        .body(Body::empty())
        .unwrap();
    let (status, templates) = send(&fixture.app, by_cookie).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(templates[0]["id"], "access-test");
}

#[tokio::test]
async fn the_signed_in_email_is_recorded_whatever_the_client_claims() {
    let fixture = fixture().await;
    let token = token(&fixture, "operator@example.ac.jp");
    let summary = start(&fixture, &token).await;
    let id = summary["execution_id"].as_str().unwrap();

    let (status, _) = send(
        &fixture.app,
        post_json(
            &format!("/api/executions/{id}/actions"),
            &json!({
                "action": { "action": "rename_execution", "name": "x" },
                "meta": { "recorded_by": "someone else" },
            }),
            Some(&token),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let log = std::fs::read_to_string(
        Path::new(summary["execution_dir"].as_str().unwrap()).join("events.jsonl"),
    )
    .unwrap();
    let authored: Vec<&str> = log
        .lines()
        .filter(|line| line.contains("recorded_by"))
        .collect();
    assert!(!authored.is_empty(), "{log}");
    assert!(
        authored
            .iter()
            .all(|line| line.contains(r#""recorded_by":"operator@example.ac.jp""#)),
        "{log}"
    );
}

#[tokio::test]
async fn upload_links_work_without_a_token() {
    let fixture = fixture().await;
    let token = token(&fixture, "operator@example.ac.jp");
    let summary = start(&fixture, &token).await;
    let id = summary["execution_id"].as_str().unwrap();
    let (status, link) = send(
        &fixture.app,
        post_json(
            &format!("/api/executions/{id}/upload-link"),
            &json!({}),
            Some(&token),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{link}");
    let link_token = link["token"].as_str().unwrap();

    let (status, target) = send(
        &fixture.app,
        get(&format!("/api/upload-links/{link_token}"), None),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{target}");
    assert_eq!(target["procedure_title"], "Access Test");
}

#[tokio::test]
async fn the_live_websocket_accepts_the_cookie_and_rejects_strangers() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let fixture = fixture().await;
    let token = token(&fixture, "operator@example.ac.jp");
    let summary = start(&fixture, &token).await;
    let id = summary["execution_id"].as_str().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, fixture.app.clone()).into_future());
    let url = format!("ws://{addr}/api/executions/{id}/live");

    let mut with_cookie = url.as_str().into_client_request().unwrap();
    with_cookie.headers_mut().insert(
        header::COOKIE,
        format!("CF_Authorization={token}").parse().unwrap(),
    );
    assert!(tokio_tungstenite::connect_async(with_cookie).await.is_ok());

    let stranger = tokio_tungstenite::connect_async(url.as_str()).await;
    assert!(matches!(
        stranger,
        Err(tokio_tungstenite::tungstenite::Error::Http(response))
            if response.status() == StatusCode::UNAUTHORIZED
    ));
}

#[tokio::test]
async fn concurrent_first_requests_all_wait_for_the_key_fetch() {
    let fixture = fixture().await;
    let token = token(&fixture, "operator@example.ac.jp");

    let requests = (0..8).map(|_| send(&fixture.app, get("/api/me", Some(&token))));
    let statuses: Vec<StatusCode> = futures_util::future::join_all(requests)
        .await
        .into_iter()
        .map(|(status, _)| status)
        .collect();

    assert!(
        statuses.iter().all(|s| *s == StatusCode::OK),
        "{statuses:?}"
    );
}

fn bearer(uri: &str, token: &str) -> Request<Body> {
    Request::get(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn desktop_apps_sync_with_device_tokens_their_owners_issue() {
    let fixture = fixture().await;
    let alice = token(&fixture, "alice@example.ac.jp");
    let bob = token(&fixture, "bob@example.ac.jp");
    let summary = start(&fixture, &alice).await;
    let id = summary["execution_id"].as_str().unwrap();

    let (status, issued) = send(
        &fixture.app,
        post_json(
            "/api/device-tokens",
            &json!({ "device": "bench PC" }),
            Some(&alice),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{issued}");
    let device_token = issued["token"].as_str().unwrap().to_string();
    assert_eq!(issued["email"], "alice@example.ac.jp");

    // The device routes take the device token, without an Access session.
    let (status, me) = send(&fixture.app, bearer("/api/device/me", &device_token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["device"], "bench PC");
    let (status, execution) = send(
        &fixture.app,
        bearer(&format!("/api/device/executions/{id}"), &device_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{execution}");
    assert_eq!(execution["template_format"], "markdown");
    assert_eq!(execution["records"][0]["type"], "log_meta");
    let (without, _) = send(&fixture.app, get("/api/device/me", None)).await;
    let (with_access_only, _) = send(&fixture.app, get("/api/device/me", Some(&alice))).await;
    assert_eq!(without, StatusCode::UNAUTHORIZED);
    assert_eq!(with_access_only, StatusCode::UNAUTHORIZED);

    // What the device pushes is recorded as its owner.
    let state_id = uuid::Uuid::new_v4();
    let push = Request::post(format!("/api/device/executions/{id}/events"))
        .header(header::AUTHORIZATION, format!("Bearer {device_token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "events": [{
                "type": "execution_renamed",
                "at": "2026-10-18T00:00:00Z",
                "execution_id": id,
                "name": "renamed on the bench",
                "event_id": state_id,
                "recorded_by": "someone else",
            }]})
            .to_string(),
        ))
        .unwrap();
    let (status, pushed) = send(&fixture.app, push).await;
    assert_eq!(status, StatusCode::OK, "{pushed}");
    let (_, pulled) = send(
        &fixture.app,
        bearer(
            &format!("/api/device/executions/{id}/events?after=1"),
            &device_token,
        ),
    )
    .await;
    let renamed = pulled["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["event_id"] == state_id.to_string())
        .unwrap();
    assert_eq!(renamed["recorded_by"], "alice@example.ac.jp");

    // Only the owner sees and revokes it; revoked tokens stop working.
    let (_, bobs) = send(&fixture.app, get("/api/device-tokens", Some(&bob))).await;
    assert_eq!(bobs, json!([]));
    let token_id = issued["id"].as_str().unwrap();
    let revoke = |as_whom: &str| {
        Request::delete(format!("/api/device-tokens/{token_id}"))
            .header(TOKEN_HEADER, as_whom)
            .body(Body::empty())
            .unwrap()
    };
    let (by_bob, _) = send(&fixture.app, revoke(&bob)).await;
    let (by_alice, _) = send(&fixture.app, revoke(&alice)).await;
    let (after, _) = send(&fixture.app, bearer("/api/device/me", &device_token)).await;
    assert_eq!(by_bob, StatusCode::NOT_FOUND);
    assert_eq!(by_alice, StatusCode::NO_CONTENT);
    assert_eq!(after, StatusCode::UNAUTHORIZED);
}
