use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use agentx_runtime_infrastructure::{RuntimeRedisSettings, connect_runtime_redis};
use testcontainers::{
    GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};
use tokio::{
    io::copy_bidirectional,
    net::{TcpListener, TcpStream},
    sync::Notify,
    task::JoinSet,
};

#[tokio::test]
async fn runtime_redis_recovers_after_a_stalled_reconnect_handshake() {
    let container = GenericImage::new("redis", "7.4-alpine")
        .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
        .with_mapped_port(0, 6379.tcp())
        .start()
        .await
        .unwrap();
    let redis_port = container.get_host_port_ipv4(6379.tcp()).await.unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_port = listener.local_addr().unwrap().port();
    let cut = Arc::new(Notify::new());
    let connections = Arc::new(AtomicUsize::new(0));
    let mut relays = JoinSet::new();
    relays.spawn({
        let cut = cut.clone();
        let connections = connections.clone();
        async move {
            let mut streams = JoinSet::new();
            loop {
                let (mut downstream, _) = listener.accept().await.unwrap();
                let number = connections.fetch_add(1, Ordering::SeqCst) + 1;
                let cut = cut.clone();
                streams.spawn(async move {
                    if number == 2 {
                        // The TCP handshake succeeds but Redis initialization
                        // never replies. Later connections reach the real Redis.
                        tokio::time::sleep(Duration::from_secs(30)).await;
                        drop(downstream);
                        return;
                    }
                    let mut upstream = TcpStream::connect(("127.0.0.1", redis_port)).await.unwrap();
                    if number == 1 {
                        tokio::select! {
                            _ = cut.notified() => {},
                            _ = copy_bidirectional(&mut downstream, &mut upstream) => {},
                        }
                    } else {
                        let _ = copy_bidirectional(&mut downstream, &mut upstream).await;
                    }
                });
            }
        }
    });
    let mut connection = connect_runtime_redis(&RuntimeRedisSettings {
        url: format!("redis://127.0.0.1:{relay_port}/").into(),
        password: None,
        tls_ca_path: None,
        tls_client_cert_path: None,
        tls_client_key_path: None,
    })
    .await
    .unwrap();
    let pong: String = redis::cmd("PING")
        .query_async(&mut connection)
        .await
        .unwrap();
    assert_eq!(pong, "PONG");
    cut.notify_one();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let result = tokio::time::timeout(
                Duration::from_secs(1),
                redis::cmd("PING").query_async::<String>(&mut connection),
            )
            .await;
            if matches!(result, Ok(Ok(ref response)) if response == "PONG")
                && connections.load(Ordering::SeqCst) >= 3
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("a stalled reconnect must time out so the SDK can reconnect again");
}
