//! The desktop sync against a real procnote-server (file store, no Access).
#![expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]

use std::path::{Path, PathBuf};

use procnote_app::action::{AttachmentSource, ExecutionAction};
use procnote_app::persistence::execution_store::ExecutionStore;
use procnote_app::repository::{
    ExecutionRepository, NewExecution, TemplateSnapshot, record_action,
};
use procnote_core::event::types::{CompletionStatus, EventMeta, ExecutionId};
use procnote_core::execution::{ExecutionState, ExecutionStepContent};
use procnote_core::template::parse_template;
use procnote_server::{Auth, ServerConfig, router};
use procnote_sync::client::DeviceClient;
use procnote_sync::engine::{SyncError, open_remote, sync_all, sync_execution, waiting};
use procnote_sync::link::ServerLink;
use serde_json::{Value, json};

const TEMPLATE: &str = r"---
id: desk-sync
title: Desk Sync
version: '1'
---

## Prepare

- [ ] Ready
- [ ] Steady

```inputs
- id: photo
  label: Photo
  type: attachment
```
";

struct World {
    temp_dir: tempfile::TempDir,
    server: String,
    server_store: ExecutionStore,
    desktop: ExecutionStore,
    client: DeviceClient,
    scratch: PathBuf,
}

async fn world() -> World {
    let temp = tempfile::tempdir().unwrap();
    let server_ws = temp.path().join("server");
    std::fs::create_dir_all(server_ws.join("desk-sync")).unwrap();
    std::fs::write(server_ws.join("desk-sync/template.md"), TEMPLATE).unwrap();
    let server_ws = server_ws.canonicalize().unwrap();
    let app = router(ServerConfig {
        executions: Box::new(ExecutionStore::new(server_ws.clone())),
        procedures_dir: server_ws.clone(),
        upload_dir: temp.path().join("uploads"),
        static_dir: None,
        state_dir: temp.path().join("state"),
        auth: Auth::Disabled,
        collab: None,
    })
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(axum::serve(listener, app).into_future());
    let issued: Value = reqwest::Client::new()
        .post(format!("{server}/api/device-tokens"))
        .json(&json!({ "device": "bench PC" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let desktop_ws = temp.path().join("desktop");
    std::fs::create_dir_all(desktop_ws.join("desk-sync")).unwrap();
    let scratch = temp.path().join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    World {
        client: DeviceClient::new(ServerLink {
            server: server.clone(),
            token: issued["token"].as_str().unwrap().to_string(),
            email: String::new(),
            device: "bench PC".to_string(),
        }),
        server,
        server_store: ExecutionStore::new(server_ws),
        desktop: ExecutionStore::new(desktop_ws),
        scratch,
        temp_dir: temp,
    }
}

/// Start an execution on the desktop, linked to the server.
fn start_here(world: &World) -> ExecutionId {
    let mut state = ExecutionState::new();
    let initial_events = state.start(&parse_template(TEMPLATE).unwrap()).unwrap();
    let id = state.execution_id.unwrap();
    world
        .desktop
        .create(NewExecution {
            procedure: "desk-sync".to_string(),
            template: TemplateSnapshot::Markdown(TEMPLATE.to_string()),
            state,
            initial_events,
            tool_version: "test".to_string(),
            meta: EventMeta::default(),
        })
        .unwrap();
    world.desktop.link(id, false).unwrap();
    id
}

fn act(store: &ExecutionStore, id: ExecutionId, action: ExecutionAction) {
    record_action(
        store,
        id,
        action,
        &EventMeta {
            event_id: Some(uuid::Uuid::new_v4()),
            ..EventMeta::default()
        },
    )
    .unwrap();
}

fn check(index: usize, checked: bool) -> ExecutionAction {
    ExecutionAction::ToggleCheckbox {
        step_id: "step-0".to_string(),
        checkbox_id: format!("step-0/cb-{index}"),
        checked,
    }
}

fn checked(store: &ExecutionStore, id: ExecutionId) -> Vec<bool> {
    store.load(id).unwrap().state.steps["step-0"]
        .content
        .iter()
        .filter_map(|item| match item {
            ExecutionStepContent::Checkbox(cb) => Some(cb.checked),
            _ => None,
        })
        .collect()
}

fn attach(world: &World, id: ExecutionId, name: &str, content: &str) {
    let path = world.scratch.join(name);
    std::fs::write(&path, content).unwrap();
    act(
        &world.desktop,
        id,
        ExecutionAction::AddAttachments {
            step_id: "step-0".to_string(),
            input_id: "photo".to_string(),
            files: vec![AttachmentSource {
                filename: name.to_string(),
                path: path.to_string_lossy().to_string(),
                content_type: "text/plain".to_string(),
            }],
        },
    );
}

fn attachment_paths(store: &ExecutionStore, id: ExecutionId) -> Vec<String> {
    store.load(id).unwrap().state.steps["step-0"].attachments["photo"]
        .iter()
        .map(|file| file.path.clone())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn an_execution_started_here_reaches_the_server_with_its_attachments() {
    let world = world().await;
    let id = start_here(&world);
    act(&world.desktop, id, check(0, true));
    attach(&world, id, "reading.txt", "5.01 V");
    assert_eq!(
        waiting(&world.desktop).unwrap(),
        3,
        "registration + 2 events"
    );

    let synced = sync_execution(&world.desktop, &world.client, id)
        .await
        .unwrap();

    assert!(synced.registered);
    assert_eq!(synced.pushed, 2);
    assert_eq!(waiting(&world.desktop).unwrap(), 0);
    assert_eq!(checked(&world.server_store, id), [true, false]);
    let path = &attachment_paths(&world.server_store, id)[0];
    assert_eq!(
        world.server_store.read_attachment(id, path).unwrap(),
        b"5.01 V"
    );
    assert_eq!(checked(&world.desktop, id), [true, false]);
}

#[tokio::test(flavor = "multi_thread")]
async fn what_others_record_on_the_server_comes_here_with_its_attachments() {
    let world = world().await;
    let id = start_here(&world);
    sync_execution(&world.desktop, &world.client, id)
        .await
        .unwrap();
    // Someone else checks a box and attaches a file through the server.
    act(&world.server_store, id, check(1, true));
    let elsewhere = world.scratch.join("theirs.txt");
    std::fs::write(&elsewhere, "from the browser").unwrap();
    act(
        &world.server_store,
        id,
        ExecutionAction::AddAttachments {
            step_id: "step-0".to_string(),
            input_id: "photo".to_string(),
            files: vec![AttachmentSource {
                filename: "theirs.txt".to_string(),
                path: elsewhere.to_string_lossy().to_string(),
                content_type: "text/plain".to_string(),
            }],
        },
    );

    let synced = sync_execution(&world.desktop, &world.client, id)
        .await
        .unwrap();

    assert_eq!(synced.pulled, 2);
    assert_eq!(checked(&world.desktop, id), [false, true]);
    let path = &attachment_paths(&world.desktop, id)[0];
    assert_eq!(
        world.desktop.read_attachment(id, path).unwrap(),
        b"from the browser"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn recordings_after_the_server_moved_on_become_conflicts() {
    let world = world().await;
    let id = start_here(&world);
    sync_execution(&world.desktop, &world.client, id)
        .await
        .unwrap();
    // Offline here, while the execution is completed on the server.
    act(&world.desktop, id, check(0, true));
    act(
        &world.server_store,
        id,
        ExecutionAction::Complete {
            status: CompletionStatus::Pass,
        },
    );

    sync_execution(&world.desktop, &world.client, id)
        .await
        .unwrap();

    assert_eq!(world.server_store.conflicts(id).unwrap().len(), 1);
    assert_eq!(waiting(&world.desktop).unwrap(), 0);
    let desktop = world.desktop.load(id).unwrap().state;
    assert!(matches!(
        desktop.status,
        procnote_core::execution::ExecutionStatus::Finished(CompletionStatus::Pass)
    ));
    assert_eq!(
        checked(&world.desktop, id),
        [false, false],
        "matches the server"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_execution_opened_here_syncs_both_ways() {
    let world = world().await;
    let started: Value = reqwest::Client::new()
        .post(format!("{}/api/executions", world.server))
        .json(&json!({ "template_path": Path::new(&world.server_store_path()).join("desk-sync/template.md") }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id: ExecutionId = started["execution_id"].as_str().unwrap().parse().unwrap();

    open_remote(&world.desktop, &world.client, id)
        .await
        .unwrap();
    act(&world.desktop, id, check(1, true));
    sync_all(&world.desktop, &world.client).await.unwrap();

    assert_eq!(checked(&world.server_store, id), [false, true]);
    assert!(
        open_remote(&world.desktop, &world.client, id)
            .await
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn without_the_server_recordings_wait_and_a_revoked_token_says_so() {
    let world = world().await;
    let id = start_here(&world);
    act(&world.desktop, id, check(0, true));
    let unreachable = DeviceClient::new(ServerLink {
        server: "http://127.0.0.1:9".to_string(),
        ..world.client.link().clone()
    });
    let revoked = DeviceClient::new(ServerLink {
        token: "pnd_revoked".to_string(),
        ..world.client.link().clone()
    });

    let offline = sync_execution(&world.desktop, &unreachable, id).await;
    let refused = sync_execution(&world.desktop, &revoked, id).await;

    assert!(matches!(offline, Err(SyncError::Unreachable(_))));
    assert_eq!(refused, Err(SyncError::Unauthorized));
    assert_eq!(waiting(&world.desktop).unwrap(), 2);
    assert_eq!(checked(&world.desktop, id), [true, false]);
}

impl World {
    fn server_store_path(&self) -> PathBuf {
        self.temp_dir.path().join("server").canonicalize().unwrap()
    }
}
