use std::time::Duration;

use super::HubClient;
use super::support::{
    assert_operational_registry_fields, assert_registry_read_is_secret_free, registry_with_secrets,
};
use crate::store::Store;

#[tokio::test]
async fn hub_client_registry_reads_do_not_expose_endpoint_secrets() {
    let home = tempfile::tempdir().unwrap();
    registry_with_secrets().save(home.path()).unwrap();
    let store = Store::open(home.path()).unwrap();
    store
        .upsert_agent_cache(
            "stdio",
            r#"{"name":"cached-agent"}"#,
            r#"{"loadSession":true}"#,
        )
        .unwrap();
    drop(store);

    let daemon_home = home.path().to_path_buf();
    let daemon = tokio::spawn(async move {
        crate::daemon::serve(&daemon_home).await.unwrap();
    });
    tokio::time::timeout(Duration::from_secs(15), async {
        while !home.path().join("daemon.json").is_file() {
            assert!(!daemon.is_finished(), "daemon exited before becoming ready");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("daemon did not become ready");

    let client = HubClient::connect_or_spawn(home.path()).await.unwrap();
    let mut reads = vec![client.list_agents().await.unwrap()];
    for agent_id in [
        "stdio",
        "http",
        "websocket",
        "uppercase-scheme",
        "malformed",
        "unsupported",
        "newline-authority",
        "tab-authority",
        "space-authority",
    ] {
        reads.push(client.inspect_agent(agent_id).await.unwrap());
    }
    reads.push(client.list_proxies().await.unwrap());

    for read in &reads {
        assert_registry_read_is_secret_free(read);
    }
    assert_operational_registry_fields(&reads);

    drop(client);
    daemon.abort();
    let _ = daemon.await;
}
