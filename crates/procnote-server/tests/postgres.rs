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
use procnote_app::action::{AttachmentSource, ExecutionAction};
use procnote_app::execution::summarize;
use procnote_app::persistence::event_log::EventLog;
use procnote_app::persistence::execution_store::ExecutionStore;
use procnote_app::repository::{
    ExecutionRepository, NewExecution, TemplateSnapshot, receive_files, record_action,
};
use procnote_core::event::types::{EventMeta, ExecutionId};
use procnote_core::execution::ExecutionState;
use procnote_core::template::parse_template;
use procnote_server::postgres::PostgresStore;
use procnote_server::transfer::{export_workspace, import_workspace};
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
            template: TemplateSnapshot::Markdown(TEMPLATE.to_string()),
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
        collab: None,
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

/// A workspace with one file-stored execution that has a checked box, an
/// attached file, and a file in its inbox. Returns the workspace, the
/// execution directory, and the execution ID.
fn file_workspace(temp: &tempfile::TempDir) -> (PathBuf, PathBuf, ExecutionId) {
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(workspace.join("postgres-test")).unwrap();
    std::fs::write(workspace.join("postgres-test/template.md"), TEMPLATE).unwrap();
    let scan = temp.path().join("scan.txt");
    std::fs::write(&scan, "scanned").unwrap();
    let photo = temp.path().join("photo.jpg");
    std::fs::write(&photo, "photo").unwrap();
    let files = ExecutionStore::new(workspace.clone());
    let mut state = ExecutionState::new();
    let initial_events = state.start(&parse_template(TEMPLATE).unwrap()).unwrap();
    let id = state.execution_id.unwrap();
    let created = files
        .create(NewExecution {
            procedure: "postgres-test".to_string(),
            template: TemplateSnapshot::Markdown(TEMPLATE.to_string()),
            state,
            initial_events,
            tool_version: "test".to_string(),
            meta: EventMeta::default(),
        })
        .unwrap();
    let source_dir = PathBuf::from(created.location.unwrap());
    let operator = EventMeta {
        event_id: Some(uuid::Uuid::new_v4()),
        recorded_by: Some("operator".to_string()),
        device: Some("desk".to_string()),
    };
    record_action(&files, id, toggle(), &operator).unwrap();
    record_action(
        &files,
        id,
        ExecutionAction::AddAttachments {
            step_id: "step-0".to_string(),
            input_id: "evidence".to_string(),
            files: vec![AttachmentSource {
                filename: "scan.txt".to_string(),
                path: scan.to_string_lossy().to_string(),
                content_type: "text/plain".to_string(),
            }],
        },
        &EventMeta::default(),
    )
    .unwrap();
    receive_files(
        &files,
        id,
        &[(photo, "photo.jpg".to_string())],
        &EventMeta::default(),
    )
    .unwrap();
    (workspace, source_dir, id)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_workspace_round_trips_through_import_and_export() {
    let Some(url) = database_url() else { return };
    let temp = tempfile::tempdir().unwrap();
    let (workspace, source_dir, id) = file_workspace(&temp);
    let source_log = std::fs::read(source_dir.join("events.jsonl")).unwrap();
    let store = store(&url).await;

    let imported = import_workspace(&store, &workspace).await.unwrap();
    let again = import_workspace(&store, &workspace).await.unwrap();
    let exported_root = temp.path().join("exported");
    let exported = export_workspace(&store, &exported_root).await.unwrap();

    assert_eq!(imported.transferred, std::slice::from_ref(&source_dir));
    assert!(imported.failed.is_empty(), "{:?}", imported.failed);
    assert_eq!(again.skipped, std::slice::from_ref(&source_dir));
    assert_eq!(
        std::fs::read(source_dir.join("events.jsonl")).unwrap(),
        source_log,
        "the source is not modified"
    );
    // The database is shared with other tests, so only this execution is checked.
    assert!(
        exported
            .failed
            .iter()
            .all(|(failed, _)| *failed != id.to_string()),
        "{:?}",
        exported.failed
    );
    let relative = source_dir.strip_prefix(&workspace).unwrap();
    let exported_dir = exported_root.join(relative);
    assert!(exported.transferred.contains(&exported_dir));
    let records = |dir: &std::path::Path| {
        EventLog::new(dir.join("events.jsonl"))
            .read_records()
            .unwrap()
    };
    assert_eq!(records(&exported_dir), records(&source_dir));
    assert_eq!(
        std::fs::read_to_string(exported_dir.join("template.md")).unwrap(),
        TEMPLATE
    );
    let stored = ExecutionStore::new(exported_root.clone()).load(id).unwrap();
    for file in stored
        .state
        .inbox
        .iter()
        .chain(&stored.state.steps["step-0"].attachments["evidence"])
    {
        assert_eq!(
            std::fs::read(exported_dir.join(&file.path)).unwrap(),
            std::fs::read(source_dir.join(&file.path)).unwrap(),
            "{}",
            file.path
        );
    }
    assert_eq!(stored.state.inbox.len(), 1);
}

/// A stand-in for the collab service's internal API: records the templates
/// it is asked to seed, answering with `status`, and describes every
/// document as the test template with `issues`.
async fn stub_collab_service(
    status: StatusCode,
    issues: &[&str],
) -> (String, Arc<std::sync::Mutex<Vec<(String, Value)>>>) {
    let seeded = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = seeded.clone();
    let current = json!({ "template": parse_template(TEMPLATE).unwrap(), "issues": issues });
    let app = axum::Router::new()
        .route(
            "/internal/templates/{id}",
            axum::routing::post(
                move |axum::extract::Path(id): axum::extract::Path<String>,
                      headers: axum::http::HeaderMap,
                      axum::Json(template): axum::Json<Value>| async move {
                    assert_eq!(headers["authorization"], "Bearer stub-secret");
                    recorded.lock().unwrap().push((id, template));
                    status
                },
            ),
        )
        .route(
            "/internal/templates/{id}/template",
            axum::routing::get(move || async move { axum::Json(current) }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(axum::serve(listener, app).into_future());
    (url, seeded)
}

/// A workspace with the test template, and an API on it whose collab
/// service is at `service_url`. Returns the workspace's template path too.
fn collab_app(
    temp: &tempfile::TempDir,
    store: &PostgresStore,
    service_url: String,
) -> (axum::Router, PathBuf) {
    use procnote_server::collab::{CollabConfig, CollabTemplates};

    let workspace = temp.path().join("procedures");
    std::fs::create_dir_all(workspace.join("postgres-test")).unwrap();
    std::fs::write(workspace.join("postgres-test/template.md"), TEMPLATE).unwrap();
    let workspace = workspace.canonicalize().unwrap();
    let app = router(ServerConfig {
        procedures_dir: workspace.clone(),
        executions: Box::new(ExecutionStore::new(workspace.clone())),
        upload_dir: temp.path().join("uploads"),
        static_dir: None,
        state_dir: temp.path().join("state"),
        auth: Auth::Disabled,
        collab: Some(CollabTemplates::new(
            store.pool(),
            CollabConfig {
                secret: "stub-secret".to_string(),
                service_url,
            },
        )),
    })
    .unwrap();
    (app, workspace.join("postgres-test/template.md"))
}

async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let request = Request::builder().method(method).uri(uri);
    let request = match body {
        Some(body) => request
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string())),
        None => request.body(Body::empty()),
    }
    .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test(flavor = "multi_thread")]
async fn shared_templates_are_seeded_through_the_collab_service() {
    let Some(url) = database_url() else { return };
    let temp = tempfile::tempdir().unwrap();
    let store = store(&url).await;
    let (service, seeded) = stub_collab_service(StatusCode::OK, &[]).await;
    let (app, template_path) = collab_app(&temp, &store, service);

    let (status, created) = call(
        &app,
        "POST",
        "/api/collab/templates",
        Some(json!({ "template_path": template_path })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let id = created["id"].as_str().unwrap().to_string();
    let (status, token) = call(
        &app,
        "POST",
        &format!("/api/collab/templates/{id}/token"),
        Some(json!({})),
    )
    .await;
    let (unknown, _) = call(
        &app,
        "POST",
        &format!("/api/collab/templates/{}/token", uuid::Uuid::new_v4()),
        Some(json!({})),
    )
    .await;
    let (_, list) = call(&app, "GET", "/api/collab/templates", None).await;

    {
        let seeded = seeded.lock().unwrap();
        assert_eq!(seeded.len(), 1);
        assert_eq!(seeded[0].0, id);
        assert_eq!(seeded[0].1["metadata"]["id"], "postgres-test");
    }
    assert_eq!(status, StatusCode::OK);
    assert!(token["token"].as_str().unwrap().contains('.'));
    assert_eq!(unknown, StatusCode::NOT_FOUND);
    assert_eq!(list["enabled"], true);
    assert!(
        list["templates"]
            .as_array()
            .unwrap()
            .iter()
            .any(|template| template["id"] == id.as_str())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_template_the_collab_service_refuses_is_not_kept() {
    let Some(url) = database_url() else { return };
    let temp = tempfile::tempdir().unwrap();
    let store = store(&url).await;
    let (service, asked) = stub_collab_service(StatusCode::INTERNAL_SERVER_ERROR, &[]).await;
    let (app, template_path) = collab_app(&temp, &store, service);

    let (status, _) = call(
        &app,
        "POST",
        "/api/collab/templates",
        Some(json!({ "template_path": template_path })),
    )
    .await;
    let (_, list) = call(&app, "GET", "/api/collab/templates", None).await;

    assert_eq!(status, StatusCode::BAD_GATEWAY);
    let refused = asked.lock().unwrap()[0].0.clone();
    assert!(
        list["templates"]
            .as_array()
            .unwrap()
            .iter()
            .all(|template| template["id"] != refused.as_str()),
        "{list}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn executions_start_from_a_shared_template_as_it_is_now() {
    let Some(url) = database_url() else { return };
    let temp = tempfile::tempdir().unwrap();
    let store = store(&url).await;
    let (service, _) = stub_collab_service(StatusCode::OK, &[]).await;
    let (app, template_path) = collab_app(&temp, &store, service);
    let (_, created) = call(
        &app,
        "POST",
        "/api/collab/templates",
        Some(json!({ "template_path": template_path })),
    )
    .await;
    let id = created["id"].as_str().unwrap();

    let (status, summary) = call(
        &app,
        "POST",
        &format!("/api/collab/templates/{id}/executions"),
        Some(json!({ "meta": { "recorded_by": "starter" } })),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{summary}");
    assert_eq!(summary["procedure_id"], "postgres-test");
    // The workspace store files it under the procedure ID, with the template as JSON.
    let execution_dir = PathBuf::from(summary["execution_dir"].as_str().unwrap());
    assert!(execution_dir.starts_with(template_path.parent().unwrap()));
    let snapshot: Value = serde_json::from_str(
        &std::fs::read_to_string(execution_dir.join("template.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(snapshot["metadata"]["id"], "postgres-test");
    assert!(!execution_dir.join("template.md").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_shared_template_with_issues_does_not_start() {
    let Some(url) = database_url() else { return };
    let temp = tempfile::tempdir().unwrap();
    let store = store(&url).await;
    let (service, _) = stub_collab_service(StatusCode::OK, &["The procedure has no title."]).await;
    let (app, template_path) = collab_app(&temp, &store, service);
    let (_, created) = call(
        &app,
        "POST",
        "/api/collab/templates",
        Some(json!({ "template_path": template_path })),
    )
    .await;
    let id = created["id"].as_str().unwrap();

    let (status, body) = call(
        &app,
        "POST",
        &format!("/api/collab/templates/{id}/executions"),
        Some(json!({})),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("The procedure has no title."),
        "{body}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn json_template_snapshots_are_kept_as_json() {
    let Some(url) = database_url() else { return };
    let store = store(&url).await;
    let template = parse_template(TEMPLATE).unwrap();
    let snapshot = TemplateSnapshot::of_template(&template).unwrap();

    let exported = {
        let store = store.clone();
        let snapshot = snapshot.clone();
        blocking(move || {
            let mut state = ExecutionState::new();
            let initial_events = state.start(&template).unwrap();
            let id = state.execution_id.unwrap();
            store
                .create(NewExecution {
                    procedure: "postgres-test".to_string(),
                    template: snapshot,
                    state,
                    initial_events,
                    tool_version: "test".to_string(),
                    meta: EventMeta::default(),
                })
                .unwrap();
            id
        })
        .await
    };
    let exported = store.export(exported).await.unwrap();

    assert_eq!(exported.template, snapshot);
    assert_eq!(exported.template.file_name(), "template.json");
}

#[tokio::test(flavor = "multi_thread")]
async fn shared_templates_export_as_procnote_markdown() {
    use procnote_server::collab::{CollabConfig, CollabTemplates};
    use procnote_server::transfer::export_templates;

    let Some(url) = database_url() else { return };
    let temp = tempfile::tempdir().unwrap();
    let store = store(&url).await;
    let (service, _) = stub_collab_service(StatusCode::OK, &[]).await;
    let (app, template_path) = collab_app(&temp, &store, service.clone());
    let (_, created) = call(
        &app,
        "POST",
        "/api/collab/templates",
        Some(json!({ "template_path": template_path })),
    )
    .await;
    let id = created["id"].as_str().unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::get(format!("/api/collab/templates/{id}/markdown"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let markdown = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert_eq!(
        parse_template(&markdown).unwrap(),
        parse_template(TEMPLATE).unwrap()
    );

    // The stub describes every shared template in the (shared) database as
    // the test template, so only this workspace's file is checked.
    let out = temp.path().join("exported");
    let target = out.join("postgres-test/template.md");
    let collab = CollabTemplates::new(
        store.pool(),
        CollabConfig {
            secret: "stub-secret".to_string(),
            service_url: service,
        },
    );
    let first = export_templates(&collab, &out, false).await.unwrap();
    std::fs::write(&target, "edited by hand").unwrap();
    let again = export_templates(&collab, &out, false).await.unwrap();
    let kept = std::fs::read_to_string(&target).unwrap();
    let replaced = export_templates(&collab, &out, true).await.unwrap();

    assert!(first.transferred.contains(&target));
    assert!(again.skipped.contains(&target));
    assert_eq!(kept, "edited by hand");
    assert!(replaced.transferred.contains(&target));
    assert_eq!(
        parse_template(&std::fs::read_to_string(&target).unwrap()).unwrap(),
        parse_template(TEMPLATE).unwrap()
    );
}
