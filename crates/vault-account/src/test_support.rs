//! Shared test helpers: a fake account service on 127.0.0.1 (both the OAuth
//! endpoints and the account Worker), and lease signing with throwaway keys.
//! Compiled only for tests; no test leaves this machine (BRD §2.10).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use dryoc::classic::crypto_sign::{crypto_sign_detached, crypto_sign_keypair, SecretKey};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::lease::{LeaseKey, LeaseVerifier, LEASE_DOMAIN};

/// One request as the fake service saw it.
#[derive(Debug, Clone)]
pub(crate) struct Seen {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: String,
}

impl Seen {
    /// The body as form fields.
    pub fn form(&self) -> HashMap<String, String> {
        form(&self.body)
    }
}

/// Decides the raw HTTP response to each request; `None` never answers.
pub(crate) type Responder = Arc<dyn Fn(&Seen) -> Option<Vec<u8>> + Send + Sync>;

/// A fake HTTP service answering through a [`Responder`].
pub(crate) struct FakeService {
    pub port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl FakeService {
    /// Serve every request through `responder`.
    pub async fn start(responder: Responder) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let log = log.clone();
                let responder = responder.clone();
                tokio::spawn(async move {
                    let Some(request) = read_request(&mut stream).await else {
                        return;
                    };
                    log.lock().unwrap().push(request.clone());
                    match responder(&request) {
                        Some(bytes) => {
                            let _ = stream.write_all(&bytes).await;
                            let _ = stream.shutdown().await;
                        }
                        None => tokio::time::sleep(Duration::from_secs(5)).await,
                    }
                });
            }
        });
        Self { port, seen }
    }

    /// Answer every request with the same response (or never, for `None`).
    pub async fn fixed(response: Option<Vec<u8>>) -> Self {
        Self::start(Arc::new(move |_: &Seen| response.clone())).await
    }

    /// Every request so far.
    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    /// Every request so far to `path`.
    pub fn seen_at(&self, path: &str) -> Vec<Seen> {
        self.seen().into_iter().filter(|s| s.path == path).collect()
    }
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> Option<Seen> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let n = stream.read(&mut chunk).await.unwrap_or(0);
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or("").split(' ');
    let method = first.next().unwrap_or("").to_string();
    let path = first.next().unwrap_or("").to_string();
    let headers: HashMap<String, String> = lines
        .filter_map(|l| l.split_once(": "))
        .map(|(k, v)| (k.to_ascii_lowercase(), v.to_string()))
        .collect();
    let len: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    while buf.len() < head_end + len {
        let n = stream.read(&mut chunk).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let body = String::from_utf8_lossy(&buf[head_end..]).into_owned();
    Some(Seen {
        method,
        path,
        headers,
        body,
    })
}

/// A raw HTTP/1.1 response.
pub(crate) fn http_response(status: u16, content_type: &str, body: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    out.extend_from_slice(body);
    out
}

/// A raw HTTP/1.1 JSON response.
pub(crate) fn json_response(status: u16, body: &str) -> Vec<u8> {
    http_response(status, "application/json", body.as_bytes())
}

/// Form fields of a request body.
pub(crate) fn form(body: &str) -> HashMap<String, String> {
    url::form_urlencoded::parse(body.as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

/// A throwaway primary/backup key pair set.
pub(crate) struct TestKeys {
    pub primary_pk: [u8; 32],
    pub primary_sk: SecretKey,
    pub backup_pk: [u8; 32],
}

impl TestKeys {
    pub fn new() -> Self {
        let (primary_pk, primary_sk) = crypto_sign_keypair();
        let (backup_pk, _) = crypto_sign_keypair();
        Self {
            primary_pk,
            primary_sk,
            backup_pk,
        }
    }

    pub fn verifier(&self) -> LeaseVerifier {
        LeaseVerifier::new(
            LeaseKey::new("primary", self.primary_pk).unwrap(),
            LeaseKey::new("backup", self.backup_pk).unwrap(),
        )
        .unwrap()
    }

    /// Sign `payload` (JSON) with the primary key into the wire form.
    pub fn sign(&self, payload: &serde_json::Value) -> String {
        sign_with(payload.to_string().as_bytes(), &self.primary_sk)
    }
}

/// Sign raw payload bytes with `sk` into the wire form.
pub(crate) fn sign_with(payload: &[u8], sk: &SecretKey) -> String {
    let mut message = LEASE_DOMAIN.to_vec();
    message.extend_from_slice(payload);
    let mut sig = [0u8; 64];
    crypto_sign_detached(&mut sig, &message, sk).unwrap();
    format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(payload),
        URL_SAFE_NO_PAD.encode(sig)
    )
}
