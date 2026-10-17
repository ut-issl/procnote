//! Devices that record executions themselves (offline) and sync later.
#![expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use procnote_app::persistence::execution_store::ExecutionStore;
use procnote_core::event::types::{AttachmentRecord, Event, EventMeta, EventRecord};
use procnote_core::execution::ExecutionState;
use procnote_core::template::parse_template;
use procnote_server::{Auth, ServerConfig, router};

const TEMPLATE: &str = r"---
id: sync-test
title: Sync Test
version: 1.0.0
---

## Prepare

- [ ] Ready

```inputs
- id: evidence
  label: Evidence
  type: attachment
```
";

struct Fixture {
    _temp: tempfile::TempDir,
    app: axum::Router,
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let procedures_dir = temp.path().join("procedures");
    std::fs::create_dir_all(procedures_dir.join("sync-test")).unwrap();
    let procedures_dir = procedures_dir.canonicalize().unwrap();
    let app = router(ServerConfig {
        executions: Box::new(ExecutionStore::new(procedures_dir.clone())),
        procedures_dir,
        upload_dir: temp.path().join("uploads"),
        static_dir: None,
        state_dir: temp.path().join("state"),
        auth: Auth::Disabled,
        collab: None,
    })
    .unwrap();
    Fixture { _temp: temp, app }
}

async fn call(app: &axum::Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn json_request(method: &str, uri: &str, body: &Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// A device: its copy of the execution state, and events as it records them.
struct Device {
    state: ExecutionState,
}

impl Device {
    fn start() -> (Self, Vec<Value>) {
        let mut state = ExecutionState::new();
        let events = state.start(&parse_template(TEMPLATE).unwrap()).unwrap();
        let lines = events.into_iter().map(|event| line(event, None)).collect();
        (Self { state }, lines)
    }

    fn id(&self) -> String {
        self.state.execution_id.unwrap().to_string()
    }

    /// Record `event` locally and return its pushable log line.
    fn record(&mut self, event: Event) -> Value {
        self.state.apply(&event).unwrap();
        line(event, Some(uuid::Uuid::new_v4()))
    }
}

fn line(event: Event, event_id: Option<uuid::Uuid>) -> Value {
    serde_json::to_value(EventRecord {
        event,
        meta: EventMeta {
            event_id,
            recorded_by: Some("tablet operator".to_string()),
            device: Some("tablet".to_string()),
        },
    })
    .unwrap()
}

async fn create(fixture: &Fixture, device: &Device, lines: &[Value]) -> (StatusCode, Value) {
    call(
        &fixture.app,
        json_request(
            "PUT",
            &format!("/api/executions/{}", device.id()),
            &json!({
                "procedure": "sync-test",
                "template": parse_template(TEMPLATE).unwrap(),
                "events": lines,
            }),
        ),
    )
    .await
}

async fn push(fixture: &Fixture, device: &Device, lines: &[Value], uploads: &[&str]) -> Value {
    let (status, body) = call(
        &fixture.app,
        json_request(
            "POST",
            &format!("/api/executions/{}/events", device.id()),
            &json!({ "events": lines, "uploads": uploads }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

fn outcomes(pushed: &Value) -> Vec<&str> {
    pushed["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|result| result["outcome"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn an_execution_started_on_a_device_is_created_once() {
    let fixture = fixture();
    let (device, lines) = Device::start();

    let (status, created) = create(&fixture, &device, &lines).await;
    let (retried, _) = create(&fixture, &device, &lines).await;
    let (other_device, other_lines) = Device::start();
    let mismatched = call(
        &fixture.app,
        json_request(
            "PUT",
            &format!("/api/executions/{}", device.id()),
            &json!({
                "procedure": "sync-test",
                "template": parse_template(TEMPLATE).unwrap(),
                "events": other_lines,
            }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!(created["execution_id"], device.id());
    assert_eq!(retried, StatusCode::OK);
    assert_eq!(mismatched.0, StatusCode::BAD_REQUEST);
    assert_ne!(other_device.id(), device.id());
}

#[tokio::test]
async fn pushed_events_are_recorded_deduplicated_or_kept_as_conflicts() {
    let fixture = fixture();
    let (mut device, lines) = Device::start();
    create(&fixture, &device, &lines).await;
    let toggle = device.record(
        device
            .state
            .toggle_checkbox_event("step-0", "step-0/cb-0", true)
            .unwrap(),
    );
    let rename = device.record(device.state.rename_event("from the tablet").unwrap());

    let first = push(&fixture, &device, &[toggle.clone(), rename], &[]).await;
    let retried = push(&fixture, &device, &[toggle], &[]).await;
    // Someone completes the execution on the server meanwhile.
    call(
        &fixture.app,
        json_request(
            "POST",
            &format!("/api/executions/{}/actions", device.id()),
            &json!({ "action": { "action": "complete", "status": "pass" } }),
        ),
    )
    .await;
    let untoggle = device.record(
        device
            .state
            .toggle_checkbox_event("step-0", "step-0/cb-0", false)
            .unwrap(),
    );
    let late = push(&fixture, &device, std::slice::from_ref(&untoggle), &[]).await;
    let malformed = push(
        &fixture,
        &device,
        &[line(device.state.rename_event("no id").unwrap(), None)],
        &[],
    )
    .await;

    assert_eq!(outcomes(&first), ["recorded", "recorded"]);
    assert_eq!(outcomes(&retried), ["duplicate"]);
    assert_eq!(outcomes(&late), ["conflict"]);
    assert_eq!(outcomes(&malformed), ["rejected"]);

    let (_, pulled) = call(
        &fixture.app,
        Request::get(format!("/api/executions/{}/events?after=1", device.id()))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let records = pulled["records"].as_array().unwrap();
    assert_eq!(pulled["count"], records.len() + 1, "after=1 skips LogMeta");
    assert!(
        records
            .iter()
            .any(|record| record["name"] == "from the tablet"
                && record["recorded_by"] == "tablet operator")
    );

    let (_, conflicts) = call(
        &fixture.app,
        Request::get(format!("/api/executions/{}/conflicts", device.id()))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(conflicts[0]["record"]["event_id"], untoggle["event_id"]);
    assert!(conflicts[0]["resolved"].is_null());

    let (status, resolved) = call(
        &fixture.app,
        json_request(
            "POST",
            &format!(
                "/api/executions/{}/conflicts/{}/resolve",
                device.id(),
                untoggle["event_id"].as_str().unwrap()
            ),
            &json!({ "meta": { "recorded_by": "reviewer" } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resolved}");
    assert_eq!(resolved[0]["resolved"]["by"], "reviewer");
}

#[tokio::test]
async fn attachments_recorded_offline_arrive_with_their_upload() {
    let fixture = fixture();
    let (mut device, lines) = Device::start();
    create(&fixture, &device, &lines).await;
    let content = "taken on the bench";
    let sha256 = procnote_app::attachment::sha256_hex(content.as_bytes());
    let record = AttachmentRecord {
        filename: "bench.txt".to_string(),
        path: format!("attachments/{}-bench.txt", &sha256[..7]),
        content_type: "text/plain".to_string(),
        sha256,
    };
    let add = device.record(
        device
            .state
            .add_attachments_event("step-0", "evidence", vec![record])
            .unwrap(),
    );

    let without_upload = push(&fixture, &device, std::slice::from_ref(&add), &[]).await;
    let boundary = "sync-test-boundary";
    let (_, uploaded) = call(
        &fixture.app,
        Request::post("/api/uploads")
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"bench.txt\"\r\n\r\n{content}\r\n--{boundary}--\r\n"
            )))
            .unwrap(),
    )
    .await;
    let token = uploaded[0]["path"].as_str().unwrap();
    let with_upload = push(&fixture, &device, &[add], &[token]).await;

    assert_eq!(outcomes(&without_upload), ["rejected"]);
    assert_eq!(outcomes(&with_upload), ["recorded"]);
    let (_, summary) = call(
        &fixture.app,
        Request::get(format!("/api/executions/{}", device.id()))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let attachment = &summary["steps"][0]["content"][1]["inputs"][0]["attachments"][0];
    assert_eq!(attachment["filename"], "bench.txt");
    let (status, _) = call(
        &fixture.app,
        Request::get(format!(
            "/api/executions/{}/attachment-preview?path={}",
            device.id(),
            attachment["path"].as_str().unwrap().replace(' ', "%20")
        ))
        .body(Body::empty())
        .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}
