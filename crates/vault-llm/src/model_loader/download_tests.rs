//! The retrying download against a scripted server on this computer
//! (loopback only: no test leaves the machine).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::{download_with_verify, RetryPolicy};
use crate::error::VaultLlmError;

/// Fast enough for a unit test, the same shape as the shipped policy.
const QUICK: RetryPolicy = RetryPolicy {
    attempts: 4,
    backoff: &[Duration::from_millis(10)],
    idle_limit: Duration::from_millis(300),
    connect_limit: Duration::from_secs(2),
};

/// What the server does with one connection.
#[derive(Clone, Copy)]
enum Answer {
    /// Honour any `Range`, send everything asked for.
    Serve,
    /// Honour any `Range`, send this many bytes of it, then hang up.
    ServeThenDrop(usize),
    /// Ignore any `Range` and send the whole file.
    WholeFile,
    /// Send the headers and this many bytes, then go quiet.
    ServeThenStall(usize),
    /// Answer with this status and no body.
    Status(u16),
}

/// A server that answers connection N with `script[N]` (the last repeats),
/// recording each request's `Range` start.
struct Server {
    url: String,
    ranges: Arc<Mutex<Vec<Option<u64>>>>,
}

fn body() -> Vec<u8> {
    (0..200_000u32).map(|i| (i % 251) as u8).collect()
}

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

async fn server(script: Vec<Answer>) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://{}/model.bin", listener.local_addr().expect("addr"));
    let ranges = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&ranges);
    tokio::spawn(async move {
        let mut n = 0;
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                return;
            };
            let answer = script[n.min(script.len() - 1)];
            n += 1;
            let seen = Arc::clone(&seen);
            tokio::spawn(async move { answer_one(socket, answer, seen).await });
        }
    });
    Server { url, ranges }
}

async fn answer_one(mut socket: TcpStream, answer: Answer, seen: Arc<Mutex<Vec<Option<u64>>>>) {
    let mut request = Vec::new();
    let mut buf = [0u8; 1024];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
        match socket.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => request.extend_from_slice(&buf[..n]),
        }
    }
    let text = String::from_utf8_lossy(&request).to_lowercase();
    let start = text
        .lines()
        .find_map(|l| l.strip_prefix("range: bytes="))
        .and_then(|r| r.trim_end_matches('-').trim().parse::<u64>().ok());
    if let Ok(mut s) = seen.lock() {
        s.push(start);
    }

    let body = body();
    let total = body.len();
    let (head, from) = match (answer, start) {
        (Answer::Status(code), _) => {
            let head = format!("HTTP/1.1 {code} Nope\r\ncontent-length: 0\r\n\r\n");
            let _ = socket.write_all(head.as_bytes()).await;
            return;
        }
        (Answer::WholeFile, _) | (_, None) => (
            format!("HTTP/1.1 200 OK\r\ncontent-length: {total}\r\n\r\n"),
            0,
        ),
        (_, Some(from)) => {
            let from = from as usize;
            (
                format!(
                    "HTTP/1.1 206 Partial Content\r\ncontent-length: {}\r\ncontent-range: bytes {from}-{}/{total}\r\n\r\n",
                    total - from,
                    total - 1
                ),
                from,
            )
        }
    };
    if socket.write_all(head.as_bytes()).await.is_err() {
        return;
    }
    let rest = &body[from..];
    match answer {
        Answer::ServeThenDrop(n) => {
            let _ = socket.write_all(&rest[..n.min(rest.len())]).await;
            let _ = socket.flush().await;
            // Dropping the socket hangs up mid-body.
        }
        Answer::ServeThenStall(n) => {
            let _ = socket.write_all(&rest[..n.min(rest.len())]).await;
            let _ = socket.flush().await;
            tokio::time::sleep(Duration::from_secs(4)).await;
        }
        _ => {
            let _ = socket.write_all(rest).await;
        }
    }
}

async fn fetch(
    server: &Server,
    dir: &tempfile::TempDir,
) -> (std::path::PathBuf, Result<(), VaultLlmError>) {
    let path = dir.path().join("model.bin");
    let body = body();
    let result = download_with_verify(
        &path,
        &server.url,
        &sha(&body),
        body.len() as u64,
        |_| {},
        QUICK,
    )
    .await;
    (path, result)
}

fn ranges(server: &Server) -> Vec<Option<u64>> {
    server.ranges.lock().map(|r| r.clone()).unwrap_or_default()
}

#[tokio::test]
async fn a_dropped_connection_resumes_where_it_stopped() {
    let server = server(vec![Answer::ServeThenDrop(70_000), Answer::Serve]).await;
    let dir = tempfile::TempDir::new().expect("dir");
    let (path, result) = fetch(&server, &dir).await;
    result.expect("the download completes after one retry");
    assert_eq!(std::fs::read(&path).expect("model"), body());
    let seen = ranges(&server);
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert_eq!(seen[0], None);
    assert!(
        matches!(seen[1], Some(start) if start > 0),
        "the retry asks only for the missing bytes: {seen:?}"
    );
}

#[tokio::test]
async fn a_server_that_ignores_the_resume_request_starts_over_cleanly() {
    let server = server(vec![Answer::ServeThenDrop(70_000), Answer::WholeFile]).await;
    let dir = tempfile::TempDir::new().expect("dir");
    let (path, result) = fetch(&server, &dir).await;
    result.expect("the whole file replaces the partial one");
    assert_eq!(std::fs::read(&path).expect("model"), body());
}

#[tokio::test]
async fn a_stalled_connection_counts_as_broken_and_is_retried() {
    let server = server(vec![Answer::ServeThenStall(50_000), Answer::Serve]).await;
    let dir = tempfile::TempDir::new().expect("dir");
    let (path, result) = fetch(&server, &dir).await;
    result.expect("the stall times out and the retry finishes");
    assert_eq!(std::fs::read(&path).expect("model"), body());
}

#[tokio::test]
async fn the_servers_own_trouble_is_retried() {
    let server = server(vec![Answer::Status(503), Answer::Serve]).await;
    let dir = tempfile::TempDir::new().expect("dir");
    let (path, result) = fetch(&server, &dir).await;
    result.expect("a 503 then a good answer");
    assert_eq!(std::fs::read(&path).expect("model"), body());
}

#[tokio::test]
async fn a_missing_file_is_not_retried() {
    let server = server(vec![Answer::Status(404)]).await;
    let dir = tempfile::TempDir::new().expect("dir");
    let (path, result) = fetch(&server, &dir).await;
    assert!(
        matches!(result, Err(VaultLlmError::DownloadFailed(_))),
        "{result:?}"
    );
    assert_eq!(ranges(&server).len(), 1, "a 404 is asked once");
    assert!(!path.exists());
}

#[tokio::test]
async fn it_gives_up_after_the_attempt_limit_and_leaves_no_partial_file() {
    let server = server(vec![Answer::ServeThenDrop(10_000)]).await;
    let dir = tempfile::TempDir::new().expect("dir");
    let (path, result) = fetch(&server, &dir).await;
    assert!(
        matches!(result, Err(VaultLlmError::DownloadFailed(_))),
        "{result:?}"
    );
    assert_eq!(ranges(&server).len(), QUICK.attempts as usize);
    assert!(!path.exists());
    assert!(!super::partial_path_for(&path).exists());
}

#[tokio::test]
async fn a_server_nobody_answers_is_retried_then_reported() {
    // Nothing listens on a port we bound and released.
    let url = {
        let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        format!("http://{}/model.bin", l.local_addr().expect("addr"))
    };
    let dir = tempfile::TempDir::new().expect("dir");
    let path = dir.path().join("model.bin");
    let result = download_with_verify(&path, &url, &sha(b"x"), 1, |_| {}, QUICK).await;
    assert!(
        matches!(result, Err(VaultLlmError::DownloadFailed(_))),
        "{result:?}"
    );
}

#[test]
fn the_resume_point_is_read_from_the_content_range() {
    assert_eq!(
        super::range_start("bytes 70000-199999/200000"),
        Some(70_000)
    );
    assert_eq!(super::range_start("bytes */200000"), None);
    assert_eq!(super::range_start("items 1-2/3"), None);
}

#[test]
fn the_shipped_policy_retries_for_about_a_minute() {
    let p = RetryPolicy::SHIPPED;
    let waits: Duration = (1..p.attempts).map(|a| p.wait_before_retry(a)).sum();
    assert!(p.attempts >= 5);
    assert!(waits >= Duration::from_secs(45) && waits <= Duration::from_secs(120));
}
