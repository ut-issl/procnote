#![expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]

use std::path::Path;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use procnote_server::{Auth, ServerConfig, router};

const TEMPLATE: &str = r"---
id: api-test
title: API Test
version: 1.0.0
---

## Prepare

- [ ] Ready

## Capture

```inputs
- id: evidence
  label: Evidence
  type: attachment
```
";

struct Fixture {
    _temp: tempfile::TempDir,
    app: axum::Router,
    template_path: String,
}

fn fixture(static_dir: Option<&Path>) -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let procedures_dir = temp.path().join("procedures");
    let api_test_dir = procedures_dir.join("api-test");
    std::fs::create_dir_all(&api_test_dir).unwrap();
    let template_path = api_test_dir.join("template.md");
    std::fs::write(&template_path, TEMPLATE).unwrap();
    let app = router(ServerConfig {
        procedures_dir: procedures_dir.canonicalize().unwrap(),
        upload_dir: temp.path().join("uploads"),
        static_dir: static_dir.map(Path::to_path_buf),
        state_dir: temp.path().join("state"),
        auth: Auth::Disabled,
    })
    .unwrap();
    Fixture {
        template_path: template_path
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .to_string(),
        _temp: temp,
        app,
    }
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

fn get(uri: &str) -> Request<Body> {
    Request::get(uri).body(Body::empty()).unwrap()
}

fn post_json(uri: &str, body: &Value) -> Request<Body> {
    Request::post(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn multipart_upload(filename: &str, contents: &str) -> Request<Body> {
    let boundary = "procnote-test-boundary";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: text/plain\r\n\r\n{contents}\r\n--{boundary}--\r\n"
    );
    Request::post("/api/uploads")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap()
}

async fn start(fixture: &Fixture) -> String {
    let (status, summary) = send(
        &fixture.app,
        post_json(
            "/api/executions",
            &json!({ "template_path": fixture.template_path }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    summary["execution_id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn lists_templates() {
    let fixture = fixture(None);

    let (status, templates) = send(&fixture.app, get("/api/templates")).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(templates[0]["id"], "api-test");
    assert_eq!(templates[0]["path"], fixture.template_path.as_str());
}

#[tokio::test]
async fn reads_a_template_as_structured_data() {
    let fixture = fixture(None);
    let query = format!(
        "/api/templates/content?path={}",
        fixture.template_path.replace('/', "%2F")
    );

    let (status, template) = send(&fixture.app, get(&query)).await;

    assert_eq!(status, StatusCode::OK, "{template}");
    assert_eq!(template["metadata"]["id"], "api-test");
    assert_eq!(
        template["steps"][1]["content"][0]["inputs"][0]["id"],
        "evidence"
    );
}

#[tokio::test]
async fn rejects_templates_outside_the_workspace() {
    let fixture = fixture(None);

    let (status, _) = send(
        &fixture.app,
        get("/api/templates/content?path=%2Fetc%2Fhosts"),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn records_actions_and_reads_them_back() {
    let fixture = fixture(None);
    let id = start(&fixture).await;

    let (status, summary) = send(
        &fixture.app,
        post_json(
            &format!("/api/executions/{id}/actions"),
            &json!({ "action": {
                "action": "toggle_checkbox",
                "step_id": "step-0",
                "checkbox_id": "step-0/cb-0",
                "checked": true,
            }}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{summary}");

    let (_, fetched) = send(&fixture.app, get(&format!("/api/executions/{id}"))).await;
    assert_eq!(fetched["steps"][0]["content"][0]["checked"], true);
    let (_, listed) = send(&fixture.app, get("/api/executions")).await;
    assert_eq!(listed.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn records_who_performed_an_action_and_ignores_retries() {
    let fixture = fixture(None);
    let id = start(&fixture).await;
    let request = json!({
        "action": {
            "action": "toggle_checkbox",
            "step_id": "step-0",
            "checkbox_id": "step-0/cb-0",
            "checked": true,
        },
        "meta": {
            "event_id": "8d0b3a2e-6f0e-4a8e-9c1c-6c1b5b0c2f10",
            "recorded_by": "Operator A",
            "device": "browser-1",
        },
    });
    let uri = format!("/api/executions/{id}/actions");

    let (first, _) = send(&fixture.app, post_json(&uri, &request)).await;
    let (retry, _) = send(&fixture.app, post_json(&uri, &request)).await;

    assert_eq!((first, retry), (StatusCode::OK, StatusCode::OK));
    let (_, summary) = send(&fixture.app, get(&format!("/api/executions/{id}"))).await;
    let log = std::fs::read_to_string(
        Path::new(summary["execution_dir"].as_str().unwrap()).join("events.jsonl"),
    )
    .unwrap();
    let toggles: Vec<&str> = log
        .lines()
        .filter(|line| line.contains("checkbox_toggled"))
        .collect();
    assert_eq!(toggles.len(), 1, "{log}");
    assert!(
        toggles[0].contains(r#""recorded_by":"Operator A""#),
        "{log}"
    );
    assert!(toggles[0].contains(r#""device":"browser-1""#), "{log}");
}

#[tokio::test]
async fn invalid_actions_return_an_error_message() {
    let fixture = fixture(None);
    let id = start(&fixture).await;

    let (status, body) = send(
        &fixture.app,
        post_json(
            &format!("/api/executions/{id}/actions"),
            &json!({ "action": { "action": "skip_step", "step_id": "missing", "reason": "x" } }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].is_string());
}

#[tokio::test]
async fn attaches_uploaded_files_once() {
    let fixture = fixture(None);
    let id = start(&fixture).await;

    let (status, sources) = send(&fixture.app, multipart_upload("scan.txt", "hello")).await;
    assert_eq!(status, StatusCode::OK, "{sources}");
    let attach = json!({ "action": {
        "action": "add_attachments",
        "step_id": "step-1",
        "input_id": "evidence",
        "files": sources,
    }});
    let actions_uri = format!("/api/executions/{id}/actions");

    let (status, summary) = send(&fixture.app, post_json(&actions_uri, &attach)).await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    let attachment = &summary["steps"][1]["content"][0]["inputs"][0]["attachments"][0];
    assert_eq!(attachment["filename"], "scan.txt");
    assert_eq!(attachment["content_type"], "text/plain");

    // The upload token is consumed by the first use.
    let (status, _) = send(&fixture.app, post_json(&actions_uri, &attach)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn rejects_attachments_by_server_path() {
    let fixture = fixture(None);
    let id = start(&fixture).await;

    let (status, _) = send(
        &fixture.app,
        post_json(
            &format!("/api/executions/{id}/actions"),
            &json!({ "action": {
                "action": "add_attachments",
                "step_id": "step-1",
                "input_id": "evidence",
                "files": [{
                    "filename": "template.md",
                    "path": fixture.template_path,
                    "content_type": "text/plain",
                }],
            }}),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn unknown_api_paths_are_not_served_the_frontend() {
    let static_dir = tempfile::tempdir().unwrap();
    std::fs::write(static_dir.path().join("index.html"), "<html></html>").unwrap();
    let fixture = fixture(Some(static_dir.path()));

    let (status, _) = send(&fixture.app, get("/api/nope")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let response = fixture
        .app
        .clone()
        .oneshot(get("/execution/some-id"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

async fn next_seq(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> u64 {
    use futures_util::StreamExt;
    let message = tokio::time::timeout(std::time::Duration::from_secs(2), socket.next())
        .await
        .expect("no live update within 2 s")
        .unwrap()
        .unwrap();
    let value: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
    value["seq"].as_u64().unwrap()
}

#[tokio::test]
async fn live_updates_reach_every_watcher_of_the_execution() {
    let fixture = fixture(None);
    let id = start(&fixture).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, fixture.app.clone()).into_future());
    let url = format!("ws://{addr}/api/executions/{id}/live");
    let (mut alice, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let (mut bob, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let initial = next_seq(&mut alice).await;
    assert_eq!(next_seq(&mut bob).await, initial);

    let (status, summary) = send(
        &fixture.app,
        post_json(
            &format!("/api/executions/{id}/actions"),
            &json!({ "action": { "action": "rename_execution", "name": "live" } }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(summary["seq"].as_u64().unwrap(), initial + 1);
    assert_eq!(next_seq(&mut alice).await, initial + 1);
    assert_eq!(next_seq(&mut bob).await, initial + 1);
}

#[tokio::test]
async fn live_updates_close_for_unknown_executions() {
    use futures_util::StreamExt;
    let fixture = fixture(None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, fixture.app.clone()).into_future());
    let url = format!(
        "ws://{addr}/api/executions/{}/live",
        "00000000-0000-4000-8000-000000000000"
    );
    let (mut socket, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

    let message = tokio::time::timeout(std::time::Duration::from_secs(2), socket.next())
        .await
        .unwrap();

    assert!(matches!(
        message,
        None | Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_)))
    ));
}

fn multipart(uri: &str, files: &[(&str, &str)], fields: &[(&str, &str)]) -> Request<Body> {
    use std::fmt::Write as _;
    let boundary = "procnote-test-boundary";
    let mut body = String::new();
    for (name, value) in fields {
        write!(
            body,
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        )
        .unwrap();
    }
    for (filename, contents) in files {
        write!(
            body,
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: text/plain\r\n\r\n{contents}\r\n"
        )
        .unwrap();
    }
    write!(body, "--{boundary}--\r\n").unwrap();
    Request::post(uri)
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap()
}

fn delete(uri: &str) -> Request<Body> {
    Request::delete(uri).body(Body::empty()).unwrap()
}

#[tokio::test]
async fn upload_link_receives_files_into_the_inbox_until_revoked() {
    let fixture = fixture(None);
    let id = start(&fixture).await;
    let link_uri = format!("/api/executions/{id}/upload-link");

    let (status, link) = send(&fixture.app, post_json(&link_uri, &json!({}))).await;
    assert_eq!(status, StatusCode::OK, "{link}");
    let token = link["token"].as_str().unwrap().to_string();
    let (_, again) = send(&fixture.app, get(&link_uri)).await;
    assert_eq!(again["token"], token.as_str());

    let (status, target) = send(&fixture.app, get(&format!("/api/upload-links/{token}"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(target["active"], true);
    assert_eq!(target["procedure_title"], "API Test");
    assert!(target.get("execution_dir").is_none(), "{target}");

    let files_uri = format!("/api/upload-links/{token}/files");
    let (status, receipt) = send(
        &fixture.app,
        multipart(
            &files_uri,
            &[("a.txt", "one"), ("b.txt", "two"), ("a-copy.txt", "one")],
            &[("recorded_by", "Phone user")],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    assert_eq!(receipt, json!({ "received": 2, "skipped": 1 }));

    let (_, summary) = send(&fixture.app, get(&format!("/api/executions/{id}"))).await;
    let inbox = summary["inbox"].as_array().unwrap();
    assert_eq!(inbox.len(), 2);
    let path = inbox[0]["path"].as_str().unwrap().to_string();
    let log = std::fs::read_to_string(
        Path::new(summary["execution_dir"].as_str().unwrap()).join("events.jsonl"),
    )
    .unwrap();
    assert!(log.contains(r#""recorded_by":"Phone user""#), "{log}");
    assert!(log.contains(r#""device":"upload-link""#), "{log}");

    let (status, assigned) = send(
        &fixture.app,
        post_json(
            &format!("/api/executions/{id}/actions"),
            &json!({ "action": {
                "action": "assign_attachment",
                "step_id": "step-1",
                "input_id": "evidence",
                "path": path,
            }}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{assigned}");
    assert_eq!(assigned["inbox"].as_array().unwrap().len(), 1);

    let (status, _) = send(&fixture.app, delete(&link_uri)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(&fixture.app, get(&format!("/api/upload-links/{token}"))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(
        &fixture.app,
        multipart(&files_uri, &[("c.txt", "three")], &[]),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn finished_executions_take_no_uploads() {
    let fixture = fixture(None);
    let id = start(&fixture).await;
    let link_uri = format!("/api/executions/{id}/upload-link");
    let (_, link) = send(&fixture.app, post_json(&link_uri, &json!({}))).await;
    let token = link["token"].as_str().unwrap().to_string();
    send(
        &fixture.app,
        post_json(
            &format!("/api/executions/{id}/actions"),
            &json!({ "action": { "action": "complete", "status": "pass" } }),
        ),
    )
    .await;

    let (status, target) = send(&fixture.app, get(&format!("/api/upload-links/{token}"))).await;
    assert_eq!(
        (status, target["active"].clone()),
        (StatusCode::OK, json!(false))
    );
    let (status, _) = send(
        &fixture.app,
        multipart(
            &format!("/api/upload-links/{token}/files"),
            &[("a.txt", "x")],
            &[],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = send(&fixture.app, post_json(&link_uri, &json!({}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn without_access_the_caller_is_unknown() {
    let fixture = fixture(None);

    let (status, me) = send(&fixture.app, get("/api/me")).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(me, json!({ "auth": "none", "email": null }));
}
