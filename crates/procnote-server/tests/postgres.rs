//! The PostgreSQL execution store, against the database at
//! `PROCNOTE_TEST_DATABASE_URL`. The tests pass without running when it is
//! unset, except on CI, where it must be set.
#![expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use object_store::local::LocalFileSystem;
use object_store::memory::InMemory;
use procnote_app::action::ExecutionAction;
use procnote_app::execution::summarize;
use procnote_app::repository::{ExecutionRepository, NewExecution, receive_files, record_action};
use procnote_core::event::types::{EventMeta, ExecutionId};
use procnote_core::execution::ExecutionState;
use procnote_core::template::parse_template;
use procnote_server::postgres::PostgresStore;
use procnote_server::{Auth, ServerConfig, router};

const TEMPLATE: &str = r"---
id: postgres-test
title: Postgres Test
version: 1.0.0
---

## Capture

- [ ] Ready

```inputs
- id: evidence
  label: Evidence
  type: attachment
```
";

fn database_url() -> Option<String> {
    let url = std::env::var("PROCNOTE_TEST_DATABASE_URL").ok();
    assert!(
        url.is_some() || std::env::var_os("CI").is_none(),
        "PROCNOTE_TEST_DATABASE_URL must be set on CI"
    );
    url
}

async fn store(url: &str) -> Arc<PostgresStore> {
    Arc::new(
        PostgresStore::connect(url, Arc::new(InMemory::new()))
            .await
            .unwrap(),
    )
}

/// Run a synchronous repository call off the async runtime, as the server does.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.unwrap()
}

fn create(store: &PostgresStore) -> ExecutionId {
    let mut state = ExecutionState::new();
    let initial_events = state.start(&parse_template(TEMPLATE).unwrap()).unwrap();
    let execution_id = state.execution_id.unwrap();
    store
        .create(NewExecution {
            procedure: "postgres-test".to_string(),
            template_source: TEMPLATE.to_string(),
            state,
            initial_events,
            tool_version: "test".to_string(),
            meta: EventMeta::default(),
        })
        .unwrap();
    execution_id
}

fn toggle() -> ExecutionAction {
    ExecutionAction::ToggleCheckbox {
        step_id: "step-0".to_string(),
        checkbox_id: "step-0/cb-0".to_string(),
        checked: true,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn records_are_replayed_and_retries_are_recorded_once() {
    let Some(url) = database_url() else { return };
    let store = store(&url).await;

    let (id, retried, loaded, listed) = blocking(move || {
        let id = create(&store);
        let meta = EventMeta {
            event_id: Some(uuid::Uuid::new_v4()),
            recorded_by: Some("operator".to_string()),
            device: None,
        };
        let first = record_action(store.as_ref(), id, toggle(), &meta).unwrap();
        let retried = record_action(store.as_ref(), id, toggle(), &meta).unwrap();
        assert_eq!(first.state.applied_events, retried.state.applied_events);
        let loaded = store.load(id).unwrap();
        let listed = store
            .list()
            .unwrap()
            .into_iter()
            .any(|stored| stored.state.execution_id == Some(id));
        (id, retried, loaded, listed)
    })
    .await;

    let summary =
        |state: &ExecutionState| serde_json::to_value(summarize(state, None).unwrap()).unwrap();
    assert_eq!(summary(&loaded.state), summary(&retried.state));
    assert!(loaded.location.is_none());
    assert!(listed, "{id} is listed");
}

#[tokio::test(flavor = "multi_thread")]
async fn received_files_are_stored_once_per_content_and_readable() {
    let Some(url) = database_url() else { return };
    let temp = tempfile::tempdir().unwrap();
    let store = store(&url).await;
    let source = |name: &str, content: &str| {
        let path = temp.path().join(name);
        std::fs::write(&path, content).unwrap();
        (path, name.to_string())
    };
    let files = vec![
        source("a.txt", "same"),
        source("b.txt", "same"),
        source("c.txt", "other"),
    ];

    let (inbox, received, content) = blocking(move || {
        let id = create(&store);
        let (stored, received) =
            receive_files(store.as_ref(), id, &files, &EventMeta::default()).unwrap();
        let content = store
            .read_attachment(id, &stored.state.inbox[1].path)
            .unwrap();
        let inbox: Vec<String> = stored
            .state
            .inbox
            .iter()
            .map(|file| file.filename.clone())
            .collect();
        (inbox, received, content)
    })
    .await;

    assert_eq!(received, 2);
    assert_eq!(inbox, ["a.txt", "c.txt"]);
    assert_eq!(content, b"other");
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_writers_are_serialized() {
    let Some(url) = database_url() else { return };
    let store = store(&url).await;
    let id = blocking({
        let store = store.clone();
        move || create(&store)
    })
    .await;
    let before = blocking({
        let store = store.clone();
        move || store.load(id).unwrap().state.applied_events
    })
    .await;

    let writers = (0..8).map(|n| {
        let store = store.clone();
        tokio::task::spawn_blocking(move || {
            record_action(
                store.as_ref(),
                id,
                ExecutionAction::AddNote {
                    text: format!("note {n}"),
                    step_id: Some("step-0".to_string()),
                },
                &EventMeta::default(),
            )
            .unwrap();
        })
    });
    futures_util::future::join_all(writers).await;

    let after = blocking(move || store.load(id).unwrap().state).await;
    assert_eq!(after.applied_events, before + 8);
    assert_eq!(after.steps["step-0"].notes.len(), 8);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_api_runs_on_postgres_without_writing_to_the_workspace() {
    let Some(url) = database_url() else { return };
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("procedures");
    let procedure_dir = workspace.join("postgres-test");
    std::fs::create_dir_all(&procedure_dir).unwrap();
    std::fs::write(procedure_dir.join("template.md"), TEMPLATE).unwrap();
    let workspace: PathBuf = workspace.canonicalize().unwrap();
    let attachment_dir = temp.path().join("attachments");
    std::fs::create_dir(&attachment_dir).unwrap();
    let attachments = LocalFileSystem::new_with_prefix(&attachment_dir).unwrap();
    let executions = PostgresStore::connect(&url, Arc::new(attachments))
        .await
        .unwrap();
    let app = router(ServerConfig {
        procedures_dir: workspace,
        executions: Box::new(executions),
        upload_dir: temp.path().join("uploads"),
        static_dir: None,
        state_dir: temp.path().join("state"),
        auth: Auth::Disabled,
    })
    .unwrap();
    let post = |uri: String, body: Value| {
        Request::post(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let send = |request: Request<Body>| async {
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice::<Value>(&bytes).unwrap())
    };

    let template_path = procedure_dir.join("template.md").canonicalize().unwrap();
    let (status, started) = send(post(
        "/api/executions".to_string(),
        json!({ "template_path": template_path }),
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "{started}");
    let id = started["execution_id"].as_str().unwrap();
    let (status, toggled) = send(post(
        format!("/api/executions/{id}/actions"),
        json!({ "action": {
            "action": "toggle_checkbox",
            "step_id": "step-0",
            "checkbox_id": "step-0/cb-0",
            "checked": true,
        }}),
    ))
    .await;

    assert_eq!(status, StatusCode::OK, "{toggled}");
    assert!(toggled.get("execution_id").is_some());
    assert!(started["execution_dir"].is_null(), "{started}");
    assert!(!procedure_dir.join(".executions").exists());
}
