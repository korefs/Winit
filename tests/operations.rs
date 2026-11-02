use std::sync::{Arc, Mutex};
use winit::{
    domain::*,
    infrastructure::storage::{SavedData, Storage},
    providers::Providers,
    services::operations::OperationManager,
};

fn unsupported_request() -> OperationRequest {
    OperationRequest {
        package: PackageRef {
            provider: ProviderId::Github,
            id: "owner/repo".into(),
        },
        action: Action::Install,
        asset: None,
        custom_app: None,
        profile: None,
        batch: None,
        accept_agreements: false,
        silent: false,
    }
}

#[tokio::test]
async fn queue_records_provider_errors_without_freezing_or_running_unsupported_actions() {
    let root = std::env::temp_dir().join(format!("winit-queue-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let storage = Arc::new(Storage {
        root: root.clone(),
        portable: true,
    });
    let data = Arc::new(Mutex::new(SavedData::default()));
    let queue = OperationManager::start(
        data.clone(),
        storage.clone(),
        Arc::new(Providers::new().unwrap()),
        Arc::new(|| {}),
    );
    assert_eq!(
        queue
            .enqueue(vec![unsupported_request(), unsupported_request()])
            .unwrap(),
        1
    );
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while queue.has_active() {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    {
        let d = data.lock().unwrap();
        assert_eq!(d.history.len(), 1);
        assert_eq!(d.history[0].state, OperationState::Failed);
        assert!(d.history[0].message.contains("não suporta"));
        assert!(d.history[0].finished_at.is_some());
    }
    while storage.load().unwrap().history[0].finished_at.is_none() {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        storage.load().unwrap().history[0].state,
        OperationState::Failed
    );
    std::fs::remove_file(root.join("state.json")).unwrap();
    std::fs::remove_file(root.join("state.backup.json")).unwrap();
    std::fs::remove_dir(root).unwrap();
}

#[tokio::test]
async fn pending_cancellation_is_persisted_and_prevents_dispatch() {
    let root = std::env::temp_dir().join(format!("winit-cancel-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let storage = Arc::new(Storage {
        root: root.clone(),
        portable: true,
    });
    let data = Arc::new(Mutex::new(SavedData::default()));
    let queue = OperationManager::start(
        data.clone(),
        storage.clone(),
        Arc::new(Providers::new().unwrap()),
        Arc::new(|| {}),
    );
    queue.enqueue(vec![unsupported_request()]).unwrap();
    let id = data.lock().unwrap().history[0].id;
    queue.cancel(id).unwrap();
    assert!(!queue.has_active());
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert_eq!(
        storage.load().unwrap().history[0].state,
        OperationState::Cancelled
    );
    std::fs::remove_file(root.join("state.json")).unwrap();
    std::fs::remove_file(root.join("state.backup.json")).unwrap();
    std::fs::remove_dir(root).unwrap();
}
