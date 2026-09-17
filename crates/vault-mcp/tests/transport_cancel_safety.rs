//! ADR-SEC-021: the line reader behind every stdio and keeper-pipe message
//! must not lose a request whose read was cancelled halfway.
//!
//! rmcp polls `receive()` inside a `select!`, so an outgoing response that
//! becomes ready while a request line is only partly read drops the read.
//! rmcp 2.0.0 then cleared the half-read bytes on the next call and the
//! request vanished: the agent waited out the relay's whole deadline for a
//! call the server never saw (rust-sdk #941, fixed in 2.1.0 by #947). This
//! pins the fix, so the pin cannot slide back to 2.0.0 unnoticed.
//!
//! Deterministic: the cancellation is forced with a timeout, not raced.

use std::time::Duration;

use rmcp::transport::async_rw::AsyncRwTransport;
use rmcp::transport::Transport;
use rmcp::RoleServer;
use tokio::io::AsyncWriteExt;

const REQUEST: &[u8] = br#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#;

#[tokio::test]
async fn a_request_whose_read_was_cancelled_halfway_still_arrives() {
    let (client, server) = tokio::io::duplex(64 * 1024);
    let (server_read, server_write) = tokio::io::split(server);
    let (_client_read, mut client_write) = tokio::io::split(client);
    let mut transport = AsyncRwTransport::<RoleServer, _, _>::new(server_read, server_write);

    let (head, tail) = REQUEST.split_at(12);
    client_write
        .write_all(head)
        .await
        .expect("write first half");

    // `timeout` polls the read once, which moves the first half into the
    // reader's buffer, then drops the read: what the service loop does.
    let cancelled = tokio::time::timeout(Duration::from_millis(50), transport.receive()).await;
    assert!(
        cancelled.is_err(),
        "no complete line has been sent yet, so the read must still be pending"
    );

    client_write
        .write_all(tail)
        .await
        .expect("write second half");
    client_write.write_all(b"\n").await.expect("write newline");

    let message = tokio::time::timeout(Duration::from_secs(2), transport.receive())
        .await
        .expect("the request was lost: its first half was discarded when the read was cancelled")
        .expect("the stream is still open");
    let json = serde_json::to_value(&message).expect("message serialises");
    assert_eq!(json["id"], 7, "wrong message delivered: {json}");
    assert_eq!(json["method"], "ping", "wrong message delivered: {json}");
}
