use std::fs;
use std::sync::Arc;
use std::time::Duration;

use super::ReplayMethod;
use super::support::{fixture_hub, prompt, stored_conversation, wait_for_marker};
use serde_json::json;

#[tokio::test]
async fn stale_live_runtime_restores_before_prompting() {
    let (home, hub) = fixture_hub("stale-live", 0);
    let conv = stored_conversation(&hub, "conv-stale", "stale-session", home.path());
    hub.runtime.insert(
        &conv.id,
        crate::runtime::SessionState::Live,
        hub.runtime.next_generation(),
    );
    assert!(!hub.ctx.is_session_bound("fixture", "stale-session"));

    let result = hub
        .send_prompt(prompt("conv-stale", "restore"))
        .await
        .unwrap();
    let prompt_seq = hub
        .store()
        .messages("conv-stale", false)
        .unwrap()
        .into_iter()
        .find(|message| message.role == "user")
        .unwrap()
        .seq;
    assert_eq!(
        serde_json::to_value(&result).unwrap()["promptSeq"],
        json!(prompt_seq)
    );

    let methods = fs::read_to_string(home.path().join("methods")).unwrap();
    assert_eq!(
        methods.lines().collect::<Vec<_>>(),
        vec!["load:stale-session", "prompt:stale-session"]
    );
}
#[tokio::test]
async fn completed_replay_locks_are_reclaimed_under_churn() {
    let (_home, hub) = fixture_hub("churn", 2_000);
    let sessions = hub.list_agent_sessions("fixture").await.unwrap();
    assert_eq!(sessions.len(), 2_000);
    assert!(
        hub.replay_locks.lock().is_empty(),
        "completed refresh ids accumulated replay locks"
    );
}

#[tokio::test]
async fn replay_lock_is_retained_until_the_last_waiter_finishes() {
    let (home, hub) = fixture_hub("replay-waiter", 0);
    let conv = stored_conversation(&hub, "conv-replay", "replay-session", home.path());
    let handle = hub.agent_handle("fixture").await.unwrap();

    let first_hub = Arc::clone(&hub);
    let first_handle = Arc::clone(&handle);
    let first_conv = conv.clone();
    let first_cwd = home.path().to_path_buf();
    let first = tokio::spawn(async move {
        first_hub
            .refresh_session_projection(&first_handle, &first_conv, first_cwd, ReplayMethod::Load)
            .await
    });
    wait_for_marker(&home.path().join("load-1-ready")).await;
    let original_lock = hub
        .replay_locks
        .lock()
        .get(&conv.id)
        .map(|entry| Arc::clone(&entry.lock))
        .unwrap();

    let second_hub = Arc::clone(&hub);
    let second_handle = Arc::clone(&handle);
    let second_conv = conv.clone();
    let second_cwd = home.path().to_path_buf();
    let second = tokio::spawn(async move {
        second_hub
            .refresh_session_projection(
                &second_handle,
                &second_conv,
                second_cwd,
                ReplayMethod::Load,
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    fs::write(home.path().join("load-1-release"), "").unwrap();
    first.await.unwrap().unwrap();
    wait_for_marker(&home.path().join("load-2-ready")).await;
    let retained_lock = hub
        .replay_locks
        .lock()
        .get(&conv.id)
        .map(|entry| Arc::clone(&entry.lock))
        .expect("first completion removed the lock from under a waiter");
    assert!(Arc::ptr_eq(&original_lock, &retained_lock));
    drop(original_lock);
    drop(retained_lock);

    fs::write(home.path().join("load-2-release"), "").unwrap();
    second.await.unwrap().unwrap();
    assert!(
        hub.replay_locks.lock().is_empty(),
        "last replay waiter did not reclaim its lock"
    );
}
#[tokio::test]
async fn aborted_external_refresh_releases_its_operation_admission() {
    let (home, hub) = fixture_hub("stale-live", 0);
    let conv = stored_conversation(&hub, "conv-refresh-abort", "abort-session", home.path());
    let replay_lock = Arc::new(tokio::sync::Mutex::new(()));
    hub.replay_locks.lock().insert(
        conv.id.clone(),
        super::ReplayLockEntry {
            lock: Arc::clone(&replay_lock),
            users: 0,
        },
    );
    let replay_guard = replay_lock.lock().await;

    let refresh_hub = Arc::clone(&hub);
    let refresh_conv = conv.clone();
    let refresh_cwd = home.path().to_path_buf();
    let refresh = tokio::spawn(async move {
        refresh_hub
            .refresh_session_projection_external(&refresh_conv, refresh_cwd, ReplayMethod::Load)
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if hub.operations.lock().contains_key(&conv.id) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("refresh never acquired admission");
    refresh.abort();
    assert!(refresh.await.unwrap_err().is_cancelled());
    drop(replay_guard);

    tokio::time::timeout(
        Duration::from_secs(10),
        hub.refresh_session_projection_external(
            &conv,
            home.path().to_path_buf(),
            ReplayMethod::Load,
        ),
    )
    .await
    .expect("aborted refresh did not release admission")
    .unwrap();
}
#[tokio::test]
async fn aborted_replay_waiters_prune_the_last_matching_lock() {
    const WAITER_COUNT: usize = 64;

    let (home, hub) = fixture_hub("refresh-block", 0);
    let conv = stored_conversation(&hub, "conv-replay-abort", "refresh-session", home.path());
    let handle = hub.agent_handle("fixture").await.unwrap();
    let first_hub = Arc::clone(&hub);
    let first_handle = Arc::clone(&handle);
    let first_conv = conv.clone();
    let first_cwd = home.path().to_path_buf();
    let first = tokio::spawn(async move {
        first_hub
            .refresh_session_projection(&first_handle, &first_conv, first_cwd, ReplayMethod::Load)
            .await
    });
    wait_for_marker(&home.path().join("load-ready")).await;

    let mut waiters = Vec::with_capacity(WAITER_COUNT);
    for _ in 0..WAITER_COUNT {
        let waiter_hub = Arc::clone(&hub);
        let waiter_handle = Arc::clone(&handle);
        let waiter_conv = conv.clone();
        let waiter_cwd = home.path().to_path_buf();
        waiters.push(tokio::spawn(async move {
            waiter_hub
                .refresh_session_projection(
                    &waiter_handle,
                    &waiter_conv,
                    waiter_cwd,
                    ReplayMethod::Load,
                )
                .await
        }));
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let waiter_count = {
                let locks = hub.replay_locks.lock();
                locks
                    .get(&conv.id)
                    .map(|entry| entry.users)
                    .unwrap_or_default()
            };
            if waiter_count > WAITER_COUNT {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("replay waiters never joined the shared lock");

    for waiter in &waiters {
        waiter.abort();
    }
    for waiter in waiters {
        assert!(waiter.await.unwrap_err().is_cancelled());
    }
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    assert_eq!(
        hub.replay_locks
            .lock()
            .get(&conv.id)
            .map(|entry| entry.users),
        Some(1),
        "aborted waiters leaked replay-lock users beyond the owned refresh"
    );
    fs::write(home.path().join("load-release"), "").unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if hub.replay_locks.lock().is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("owned refresh completion did not prune its replay lock");
}
