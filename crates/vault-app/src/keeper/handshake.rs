//! Keeper <-> relay mutual-authentication handshake, wire v1 (ADR-SEC-019).
//!
//! Runs over a fresh local IPC stream (named pipe / unix socket) BEFORE a single
//! JSON-RPC byte. Both sides prove they hold the vault's master key, bound to
//! this keeper tenure (nonce, endpoint, pid) and to fresh challenges from both
//! sides; the relay also binds the boundaries it asks for.
//!
//! # Why a handshake on the stream, not a token on requests
//!
//! Adversarial review of the first design (streamable-HTTP + bearer token)
//! found that rmcp's HTTP client opens a new TCP connection per request and
//! silently re-initialises on a 404, so a proof checked once at `initialize`
//! did not cover later requests — an impostor on a dead keeper's port would
//! receive tool-call arguments in plaintext. Binding the proof to ONE
//! persistent stream means keeper death is connection death, every reconnect
//! re-authenticates, and no reusable secret ever crosses the wire.
//!
//! # Wire v1 — FROZEN
//!
//! The magic, frames 1 and 2, and the proof construction never change. A later
//! version changes only the `wire` number, so any two versions can always
//! complete frames 1-2 and learn about each other safely.
//!
//! ```text
//! F1 relay->keeper : "ZKH1" | wire_r:u16le | c_r[32]                 (38 bytes)
//! F2 keeper->relay : "ZKH1" | wire_k:u16le | c_k[32] | p_h[32]       (70 bytes)
//! F3 relay->keeper : purpose:u8 | n:u8 | n x (len:u8 | utf8) | p_r[32]
//!                    purpose 0 = serve (n >= 1), 1 = yield (n = 0),
//!                    2 = hand over (n = 0)
//! F4 keeper->relay : status:u8   1 = serving, 2 = yielding, 0 = rejected
//!
//! TRANSCRIPT = wire_r | wire_k | nonce[32] | u16le len | endpoint utf8 | pid:u32le | c_r | c_k
//! p_h = keyed_hash(k_host,  "zkh1-host"  | TRANSCRIPT)
//! p_r = keyed_hash(k_relay, "zkh1-relay" | TRANSCRIPT | purpose | n | frames-as-sent)
//! ```
//!
//! Properties, each pinned by a test below:
//! - **Mutual authentication.** Only a holder of the master key can produce
//!   either proof. The relay verifies the keeper FIRST and sends nothing
//!   further to a keeper that fails.
//! - **No replay.** Both proofs cover both sides' fresh challenges.
//! - **No reflection.** Role labels and separate derived keys.
//! - **Boundaries are integrity-bound**, MACed exactly as sent, and interpreted
//!   only after the MAC verifies; they must arrive in canonical order.
//! - **Version handling after authentication only.** A `wire` mismatch is
//!   acted on only once the peer has proven itself, so a squatter cannot use a
//!   fake version to steer relays.
//!
//! No new primitives (SP-5): BLAKE3 `derive_key` and keyed hashing, the same
//! construction as the vault's existing subkeys. Comparisons use
//! `blake3::Hash`'s constant-time equality.

use std::fmt;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::Instant;
use vault_core::{Boundary, MAX_BOUNDARY_LEN};
use zeroize::Zeroizing;

/// Frame magic. Frozen.
pub const MAGIC: &[u8; 4] = b"ZKH1";

/// This build's wire version. Bump when the framing after F4 or the tool
/// contract changes.
pub const WIRE: u16 = 1;

/// Fresh random bytes each side contributes per connection.
pub const CHALLENGE_LEN: usize = 32;

/// Per-tenure keeper nonce, published in the discovery file.
pub const NONCE_LEN: usize = 32;

/// Most boundaries one connection may ask for.
pub const MAX_BOUNDARIES: usize = 32;

/// Longest endpoint string either side will MAC. Real endpoints are ~40 bytes.
const MAX_ENDPOINT_LEN: usize = 1024;

const F1_LEN: usize = 4 + 2 + CHALLENGE_LEN;
const F2_LEN: usize = 4 + 2 + CHALLENGE_LEN + 32;

const HOST_LABEL: &[u8] = b"zkh1-host";
const RELAY_LABEL: &[u8] = b"zkh1-relay";

const HOST_KDF_CONTEXT: &str = "zaaheen keeper host proof v1";
const RELAY_KDF_CONTEXT: &str = "zaaheen keeper relay proof v1";

const PURPOSE_SERVE: u8 = 0;
const PURPOSE_YIELD: u8 = 1;
/// Stop serving and exit so the caller can take the vault exclusively
/// ("Delete everything"). Honoured from any peer that proves the key — the
/// same user, who could equally end the process — whatever its wire.
const PURPOSE_HANDOVER: u8 = 2;

const STATUS_REJECTED: u8 = 0;
const STATUS_SERVING: u8 = 1;
const STATUS_YIELDING: u8 = 2;

/// The two proof keys, derived from the master key. Wiped on drop; no
/// `Debug`, so they cannot be logged by accident (BRD §11.5.3).
pub struct HandshakeKeys {
    host: Zeroizing<[u8; 32]>,
    relay: Zeroizing<[u8; 32]>,
}

impl HandshakeKeys {
    /// Derive both keys. Separate contexts, so a proof made with one can never
    /// be presented as the other (the reflection defence).
    pub fn derive(master_key: &[u8; 32]) -> Self {
        Self {
            host: Zeroizing::new(blake3::derive_key(HOST_KDF_CONTEXT, master_key)),
            relay: Zeroizing::new(blake3::derive_key(RELAY_KDF_CONTEXT, master_key)),
        }
    }
}

/// One keeper tenure's public identity — exactly what the discovery file
/// carries. Nothing here is secret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeeperIdentity {
    pub nonce: [u8; NONCE_LEN],
    pub endpoint: String,
    pub pid: u32,
}

/// Keeper-side result of an accepted handshake.
#[derive(Debug, PartialEq, Eq)]
pub enum KeeperAccept {
    /// Serve this stream with exactly these boundaries.
    Serve { boundaries: Vec<Boundary> },
    /// A newer relay asked this keeper to step aside.
    Yield,
    /// A key holder needs the vault to itself; stop serving and exit.
    Handover,
}

/// Relay-side result of a completed handshake.
#[derive(Debug, PartialEq, Eq)]
pub enum RelayOutcome {
    /// The keeper is serving this stream.
    Serving,
    /// The keeper was older and agreed to shut down; find the new one.
    KeeperYielded,
}

/// Why a handshake did not complete. Deliberately coarse: the peer is told
/// only "rejected" (SP-4); these variants are for our own logs and for the
/// relay's choice of message.
#[derive(Debug)]
pub enum HandshakeError {
    Io(std::io::Error),
    Timeout,
    BadMagic,
    /// A length, count or value outside what wire v1 allows.
    Malformed(&'static str),
    /// A proof did not verify: the peer does not hold this vault's key, or it
    /// is not the keeper the discovery file names.
    ProofMismatch,
    /// The keeper runs a newer wire; this relay is out of date.
    KeeperNewer {
        keeper: u16,
    },
    /// The keeper answered with status "rejected".
    Rejected,
}

impl fmt::Display for HandshakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "handshake i/o: {e}"),
            Self::Timeout => write!(f, "handshake timed out"),
            Self::BadMagic => write!(f, "handshake: not a Zaaheen keeper stream"),
            Self::Malformed(what) => write!(f, "handshake: malformed {what}"),
            Self::ProofMismatch => write!(f, "handshake: proof did not verify"),
            Self::KeeperNewer { keeper } => {
                write!(
                    f,
                    "handshake: keeper speaks wire {keeper}, this build {WIRE}"
                )
            }
            Self::Rejected => write!(f, "handshake: rejected by keeper"),
        }
    }
}

impl std::error::Error for HandshakeError {}

impl From<std::io::Error> for HandshakeError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

// ---------------------------------------------------------------------------
// Relay side
// ---------------------------------------------------------------------------

/// Run the relay half. `expected` is the identity read from the discovery
/// file; `challenge` must be fresh random bytes. Boundaries are canonicalised
/// (sorted, de-duplicated) here, so callers cannot get the order wrong.
///
/// On success the stream is positioned at the first JSON-RPC byte.
///
/// # Errors
///
/// [`HandshakeError::ProofMismatch`] means this is not the keeper we expected
/// (or not our vault) — the relay has sent nothing beyond frame 1.
pub async fn relay_handshake<S>(
    stream: &mut S,
    keys: &HandshakeKeys,
    expected: &KeeperIdentity,
    boundaries: &[Boundary],
    challenge: [u8; CHALLENGE_LEN],
    step_deadline: Duration,
) -> Result<RelayOutcome, HandshakeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    relay_handshake_as(
        stream,
        keys,
        expected,
        boundaries,
        challenge,
        step_deadline,
        WIRE,
    )
    .await
}

async fn relay_handshake_as<S>(
    stream: &mut S,
    keys: &HandshakeKeys,
    expected: &KeeperIdentity,
    boundaries: &[Boundary],
    c_r: [u8; CHALLENGE_LEN],
    step_deadline: Duration,
    wire_r: u16,
) -> Result<RelayOutcome, HandshakeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    // Validate our own request before touching the wire.
    let (n, frames) = encode_boundaries(boundaries)?;

    let mut f1 = Vec::with_capacity(F1_LEN);
    f1.extend_from_slice(MAGIC);
    f1.extend_from_slice(&wire_r.to_le_bytes());
    f1.extend_from_slice(&c_r);
    stream.write_all(&f1).await?;
    stream.flush().await?;

    let mut f2 = [0u8; F2_LEN];
    io_within(step_deadline, stream.read_exact(&mut f2)).await?;
    if &f2[0..4] != MAGIC {
        return Err(HandshakeError::BadMagic);
    }
    let wire_k = u16::from_le_bytes([f2[4], f2[5]]);
    let mut c_k = [0u8; CHALLENGE_LEN];
    c_k.copy_from_slice(&f2[6..6 + CHALLENGE_LEN]);
    let mut p_h = [0u8; 32];
    p_h.copy_from_slice(&f2[6 + CHALLENGE_LEN..F2_LEN]);

    let transcript = build_transcript(wire_r, wire_k, expected, &c_r, &c_k)?;
    if host_proof(keys, &transcript) != blake3::Hash::from(p_h) {
        // Not our keeper. Send nothing more.
        return Err(HandshakeError::ProofMismatch);
    }

    // Only now — the keeper has proven itself — may its version steer us.
    if wire_k > wire_r {
        return Err(HandshakeError::KeeperNewer { keeper: wire_k });
    }
    let (purpose, n, frames) = if wire_k < wire_r {
        (PURPOSE_YIELD, 0u8, Vec::new())
    } else {
        (PURPOSE_SERVE, n, frames)
    };
    let p_r = relay_proof(keys, &transcript, purpose, n, &frames);

    let mut f3 = Vec::with_capacity(2 + frames.len() + 32);
    f3.push(purpose);
    f3.push(n);
    f3.extend_from_slice(&frames);
    f3.extend_from_slice(p_r.as_bytes());
    stream.write_all(&f3).await?;
    stream.flush().await?;

    let mut status = [0u8; 1];
    io_within(step_deadline, stream.read_exact(&mut status)).await?;
    match (purpose, status[0]) {
        (PURPOSE_SERVE, STATUS_SERVING) => Ok(RelayOutcome::Serving),
        (PURPOSE_YIELD, STATUS_YIELDING) => Ok(RelayOutcome::KeeperYielded),
        _ => Err(HandshakeError::Rejected),
    }
}

/// Ask the keeper to stop serving and exit (purpose 2), so the caller can
/// take the vault exclusively. Authenticated exactly like a relay: the keeper
/// proves itself first, and only a holder of the vault's key can ask.
///
/// Versions are not compared: whatever build is running, a key holder that
/// needs the vault gets it.
///
/// # Errors
///
/// [`HandshakeError::ProofMismatch`] when the peer is not the keeper the
/// discovery file names (nothing is sent after frame 1);
/// [`HandshakeError::Rejected`] when the keeper refused.
pub async fn request_handover<S>(
    stream: &mut S,
    keys: &HandshakeKeys,
    expected: &KeeperIdentity,
    c_r: [u8; CHALLENGE_LEN],
    step_deadline: Duration,
) -> Result<(), HandshakeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut f1 = Vec::with_capacity(F1_LEN);
    f1.extend_from_slice(MAGIC);
    f1.extend_from_slice(&WIRE.to_le_bytes());
    f1.extend_from_slice(&c_r);
    stream.write_all(&f1).await?;
    stream.flush().await?;

    let mut f2 = [0u8; F2_LEN];
    io_within(step_deadline, stream.read_exact(&mut f2)).await?;
    if &f2[0..4] != MAGIC {
        return Err(HandshakeError::BadMagic);
    }
    let wire_k = u16::from_le_bytes([f2[4], f2[5]]);
    let mut c_k = [0u8; CHALLENGE_LEN];
    c_k.copy_from_slice(&f2[6..6 + CHALLENGE_LEN]);
    let mut p_h = [0u8; 32];
    p_h.copy_from_slice(&f2[6 + CHALLENGE_LEN..F2_LEN]);

    let transcript = build_transcript(WIRE, wire_k, expected, &c_r, &c_k)?;
    if host_proof(keys, &transcript) != blake3::Hash::from(p_h) {
        return Err(HandshakeError::ProofMismatch);
    }

    let p_r = relay_proof(keys, &transcript, PURPOSE_HANDOVER, 0, &[]);
    let mut f3 = Vec::with_capacity(2 + 32);
    f3.push(PURPOSE_HANDOVER);
    f3.push(0);
    f3.extend_from_slice(p_r.as_bytes());
    stream.write_all(&f3).await?;
    stream.flush().await?;

    let mut status = [0u8; 1];
    io_within(step_deadline, stream.read_exact(&mut status)).await?;
    if status[0] == STATUS_YIELDING {
        Ok(())
    } else {
        Err(HandshakeError::Rejected)
    }
}

/// Canonical boundary frames: sorted, de-duplicated, each `len:u8 | utf8`.
fn encode_boundaries(boundaries: &[Boundary]) -> Result<(u8, Vec<u8>), HandshakeError> {
    let mut names: Vec<&str> = boundaries.iter().map(Boundary::as_str).collect();
    names.sort_unstable();
    names.dedup();
    if names.is_empty() || names.len() > MAX_BOUNDARIES {
        return Err(HandshakeError::Malformed("boundary count"));
    }
    let mut out = Vec::new();
    for name in &names {
        let len = u8::try_from(name.len()).map_err(|_| HandshakeError::Malformed("boundary"))?;
        out.push(len);
        out.extend_from_slice(name.as_bytes());
    }
    let n = u8::try_from(names.len()).map_err(|_| HandshakeError::Malformed("boundary count"))?;
    Ok((n, out))
}

// ---------------------------------------------------------------------------
// Keeper side
// ---------------------------------------------------------------------------

/// Run the keeper half. `me` is this tenure's identity; `challenge` must be
/// fresh random bytes. `f1_deadline` bounds how long an unauthenticated
/// connection may sit silent (other local users can open the pipe read-only
/// and never speak); `total_deadline` bounds the whole exchange.
///
/// Every rejection after frame 2 sends status 0 when the stream is still
/// writable, so a peer learns nothing about WHY (SP-4).
///
/// # Errors
///
/// Any [`HandshakeError`]; the caller drops the connection.
pub async fn keeper_handshake<S>(
    stream: &mut S,
    keys: &HandshakeKeys,
    me: &KeeperIdentity,
    challenge: [u8; CHALLENGE_LEN],
    f1_deadline: Duration,
    total_deadline: Duration,
) -> Result<KeeperAccept, HandshakeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    keeper_handshake_as(
        stream,
        keys,
        me,
        challenge,
        f1_deadline,
        total_deadline,
        WIRE,
    )
    .await
}

async fn keeper_handshake_as<S>(
    stream: &mut S,
    keys: &HandshakeKeys,
    me: &KeeperIdentity,
    c_k: [u8; CHALLENGE_LEN],
    f1_deadline: Duration,
    total_deadline: Duration,
    wire_k: u16,
) -> Result<KeeperAccept, HandshakeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let started = Instant::now();

    let mut f1 = [0u8; F1_LEN];
    io_within(f1_deadline, stream.read_exact(&mut f1)).await?;
    if &f1[0..4] != MAGIC {
        return Err(HandshakeError::BadMagic);
    }
    let wire_r = u16::from_le_bytes([f1[4], f1[5]]);
    let mut c_r = [0u8; CHALLENGE_LEN];
    c_r.copy_from_slice(&f1[6..F1_LEN]);

    let transcript = build_transcript(wire_r, wire_k, me, &c_r, &c_k)?;
    let p_h = host_proof(keys, &transcript);

    let mut f2 = Vec::with_capacity(F2_LEN);
    f2.extend_from_slice(MAGIC);
    f2.extend_from_slice(&wire_k.to_le_bytes());
    f2.extend_from_slice(&c_k);
    f2.extend_from_slice(p_h.as_bytes());
    stream.write_all(&f2).await?;
    stream.flush().await?;

    let remaining = total_deadline.saturating_sub(started.elapsed());
    let frame3 = match tokio::time::timeout(remaining, read_frame3(stream)).await {
        Ok(result) => result,
        Err(_) => Err(HandshakeError::Timeout),
    };
    let (purpose, n, frames, p_r) = match frame3 {
        Ok(parts) => parts,
        Err(e) => {
            reject(stream).await;
            return Err(e);
        }
    };

    // Verify BEFORE interpreting anything the relay claimed.
    if relay_proof(keys, &transcript, purpose, n, &frames) != blake3::Hash::from(p_r) {
        reject(stream).await;
        return Err(HandshakeError::ProofMismatch);
    }

    if purpose == PURPOSE_HANDOVER {
        stream.write_all(&[STATUS_YIELDING]).await?;
        stream.flush().await?;
        return Ok(KeeperAccept::Handover);
    }

    if purpose == PURPOSE_YIELD {
        // Only a NEWER relay may retire this keeper.
        if wire_r > wire_k {
            stream.write_all(&[STATUS_YIELDING]).await?;
            stream.flush().await?;
            return Ok(KeeperAccept::Yield);
        }
        reject(stream).await;
        return Err(HandshakeError::Rejected);
    }

    // A relay on a different wire must not be served (a newer one yields; an
    // older one is told to update by frame 2 before it ever gets here).
    if wire_r != wire_k {
        reject(stream).await;
        return Err(HandshakeError::Rejected);
    }
    match decode_boundaries(n, &frames) {
        Ok(boundaries) => {
            stream.write_all(&[STATUS_SERVING]).await?;
            stream.flush().await?;
            Ok(KeeperAccept::Serve { boundaries })
        }
        Err(e) => {
            reject(stream).await;
            Err(e)
        }
    }
}

/// Read frame 3 exactly, validating every length BEFORE reading what it
/// announces, so a hostile peer cannot make the keeper allocate or wait for
/// more than wire v1 allows. Returns the frames exactly as received, because
/// the MAC covers those bytes.
async fn read_frame3<S>(stream: &mut S) -> Result<(u8, u8, Vec<u8>, [u8; 32]), HandshakeError>
where
    S: AsyncRead + Unpin,
{
    let mut head = [0u8; 2];
    stream.read_exact(&mut head).await?;
    let (purpose, n) = (head[0], head[1]);
    match purpose {
        PURPOSE_SERVE => {
            if n == 0 || usize::from(n) > MAX_BOUNDARIES {
                return Err(HandshakeError::Malformed("boundary count"));
            }
        }
        PURPOSE_YIELD | PURPOSE_HANDOVER => {
            if n != 0 {
                return Err(HandshakeError::Malformed("yield frame"));
            }
        }
        _ => return Err(HandshakeError::Malformed("purpose")),
    }

    let mut frames = Vec::new();
    for _ in 0..n {
        let mut len_byte = [0u8; 1];
        stream.read_exact(&mut len_byte).await?;
        let len = usize::from(len_byte[0]);
        if len == 0 || len > MAX_BOUNDARY_LEN {
            return Err(HandshakeError::Malformed("boundary length"));
        }
        let mut name = vec![0u8; len];
        stream.read_exact(&mut name).await?;
        frames.push(len_byte[0]);
        frames.extend_from_slice(&name);
    }

    let mut p_r = [0u8; 32];
    stream.read_exact(&mut p_r).await?;
    Ok((purpose, n, frames, p_r))
}

/// Interpret MAC-verified boundary frames. Must be valid names, in strictly
/// ascending order — a non-canonical list is rejected rather than normalised,
/// so there is exactly one byte string for every boundary set.
fn decode_boundaries(n: u8, frames: &[u8]) -> Result<Vec<Boundary>, HandshakeError> {
    let mut out: Vec<Boundary> = Vec::with_capacity(usize::from(n));
    let mut i = 0;
    for _ in 0..n {
        let len = usize::from(*frames.get(i).ok_or(HandshakeError::Malformed("frames"))?);
        let bytes = frames
            .get(i + 1..i + 1 + len)
            .ok_or(HandshakeError::Malformed("frames"))?;
        i += 1 + len;
        let name = std::str::from_utf8(bytes).map_err(|_| HandshakeError::Malformed("utf8"))?;
        let boundary = Boundary::new(name).map_err(|_| HandshakeError::Malformed("boundary"))?;
        if let Some(prev) = out.last() {
            if prev.as_str() >= boundary.as_str() {
                return Err(HandshakeError::Malformed("boundary order"));
            }
        }
        out.push(boundary);
    }
    if i != frames.len() {
        return Err(HandshakeError::Malformed("frames"));
    }
    Ok(out)
}

/// Best-effort "rejected" status. The stream may already be dead; that is fine.
async fn reject<S>(stream: &mut S)
where
    S: AsyncWrite + Unpin,
{
    let _ = stream.write_all(&[STATUS_REJECTED]).await;
    let _ = stream.flush().await;
}

// ---------------------------------------------------------------------------
// Shared construction
// ---------------------------------------------------------------------------

fn build_transcript(
    wire_r: u16,
    wire_k: u16,
    id: &KeeperIdentity,
    c_r: &[u8; CHALLENGE_LEN],
    c_k: &[u8; CHALLENGE_LEN],
) -> Result<Vec<u8>, HandshakeError> {
    let endpoint = id.endpoint.as_bytes();
    if endpoint.len() > MAX_ENDPOINT_LEN {
        return Err(HandshakeError::Malformed("endpoint"));
    }
    let endpoint_len =
        u16::try_from(endpoint.len()).map_err(|_| HandshakeError::Malformed("endpoint"))?;
    let mut t = Vec::with_capacity(4 + NONCE_LEN + 2 + endpoint.len() + 4 + 2 * CHALLENGE_LEN);
    t.extend_from_slice(&wire_r.to_le_bytes());
    t.extend_from_slice(&wire_k.to_le_bytes());
    t.extend_from_slice(&id.nonce);
    t.extend_from_slice(&endpoint_len.to_le_bytes());
    t.extend_from_slice(endpoint);
    t.extend_from_slice(&id.pid.to_le_bytes());
    t.extend_from_slice(c_r);
    t.extend_from_slice(c_k);
    Ok(t)
}

fn host_proof(keys: &HandshakeKeys, transcript: &[u8]) -> blake3::Hash {
    let mut h = blake3::Hasher::new_keyed(&keys.host);
    h.update(HOST_LABEL);
    h.update(transcript);
    h.finalize()
}

fn relay_proof(
    keys: &HandshakeKeys,
    transcript: &[u8],
    purpose: u8,
    n: u8,
    frames: &[u8],
) -> blake3::Hash {
    let mut h = blake3::Hasher::new_keyed(&keys.relay);
    h.update(RELAY_LABEL);
    h.update(transcript);
    h.update(&[purpose, n]);
    h.update(frames);
    h.finalize()
}

async fn io_within<F, T>(deadline: Duration, fut: F) -> Result<T, HandshakeError>
where
    F: std::future::Future<Output = std::io::Result<T>>,
{
    match tokio::time::timeout(deadline, fut).await {
        Ok(result) => result.map_err(HandshakeError::Io),
        Err(_) => Err(HandshakeError::Timeout),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::DuplexStream;

    const STEP: Duration = Duration::from_secs(2);

    fn keys(seed: u8) -> HandshakeKeys {
        HandshakeKeys::derive(&[seed; 32])
    }

    fn identity() -> KeeperIdentity {
        KeeperIdentity {
            nonce: [7u8; NONCE_LEN],
            endpoint: r"\\.\pipe\zaaheen-0123456789abcdef".to_string(),
            pid: 4242,
        }
    }

    fn b(names: &[&str]) -> Vec<Boundary> {
        names.iter().map(|n| Boundary::new(*n).unwrap()).collect()
    }

    fn frames_for(names: &[&str]) -> (u8, Vec<u8>) {
        let mut out = Vec::new();
        for n in names {
            out.push(u8::try_from(n.len()).unwrap());
            out.extend_from_slice(n.as_bytes());
        }
        (u8::try_from(names.len()).unwrap(), out)
    }

    /// Spawn a real keeper on one end of an in-memory stream.
    fn spawn_keeper(
        mut end: DuplexStream,
        key_seed: u8,
        wire: u16,
    ) -> tokio::task::JoinHandle<(Result<KeeperAccept, HandshakeError>, DuplexStream)> {
        tokio::spawn(async move {
            let result = keeper_handshake_as(
                &mut end,
                &keys(key_seed),
                &identity(),
                [2u8; CHALLENGE_LEN],
                STEP,
                STEP,
                wire,
            )
            .await;
            (result, end)
        })
    }

    async fn send_f1(s: &mut DuplexStream, wire: u16, c_r: [u8; CHALLENGE_LEN]) {
        let mut f1 = Vec::new();
        f1.extend_from_slice(MAGIC);
        f1.extend_from_slice(&wire.to_le_bytes());
        f1.extend_from_slice(&c_r);
        s.write_all(&f1).await.unwrap();
    }

    async fn read_f2(s: &mut DuplexStream) -> (u16, [u8; CHALLENGE_LEN]) {
        let mut f2 = [0u8; F2_LEN];
        s.read_exact(&mut f2).await.unwrap();
        let mut c_k = [0u8; CHALLENGE_LEN];
        c_k.copy_from_slice(&f2[6..6 + CHALLENGE_LEN]);
        (u16::from_le_bytes([f2[4], f2[5]]), c_k)
    }

    /// A hand-driven relay: sends F1, reads F2, then sends an arbitrary F3
    /// built from `(purpose, n, frames)` with a proof over `proof_frames`
    /// under `proof_keys`. Returns the status byte (or None on EOF).
    async fn manual_relay(
        s: &mut DuplexStream,
        proof_keys: &HandshakeKeys,
        wire: u16,
        purpose: u8,
        n: u8,
        frames: &[u8],
        proof_frames: &[u8],
    ) -> Option<u8> {
        let c_r = [3u8; CHALLENGE_LEN];
        send_f1(s, wire, c_r).await;
        let (wire_k, c_k) = read_f2(s).await;
        let t = build_transcript(wire, wire_k, &identity(), &c_r, &c_k).unwrap();
        let p = relay_proof(proof_keys, &t, purpose, n, proof_frames);
        let mut f3 = vec![purpose, n];
        f3.extend_from_slice(frames);
        f3.extend_from_slice(p.as_bytes());
        s.write_all(&f3).await.unwrap();
        let mut status = [0u8; 1];
        match s.read_exact(&mut status).await {
            Ok(_) => Some(status[0]),
            Err(_) => None,
        }
    }

    #[test]
    fn the_two_proof_keys_differ() {
        let k = keys(1);
        assert_ne!(*k.host, *k.relay, "role keys must be independent");
    }

    #[tokio::test]
    async fn a_matching_pair_serves_exactly_the_requested_boundaries() {
        let (mut relay_end, keeper_end) = tokio::io::duplex(4096);
        let keeper = spawn_keeper(keeper_end, 1, WIRE);

        let outcome = relay_handshake(
            &mut relay_end,
            &keys(1),
            &identity(),
            &b(&["work", "personal", "work"]),
            [3u8; CHALLENGE_LEN],
            STEP,
        )
        .await
        .unwrap();
        assert_eq!(outcome, RelayOutcome::Serving);

        // The stream is positioned at the first JSON-RPC byte on both sides.
        relay_end.write_all(b"{\"jsonrpc\"").await.unwrap();
        let (accept, mut keeper_end) = keeper.await.unwrap();
        assert_eq!(
            accept.unwrap(),
            KeeperAccept::Serve {
                boundaries: b(&["personal", "work"])
            },
            "canonicalised, de-duplicated, and exactly what was asked"
        );
        let mut first = [0u8; 10];
        keeper_end.read_exact(&mut first).await.unwrap();
        assert_eq!(&first, b"{\"jsonrpc\"");
    }

    /// A keeper of ANOTHER vault (or anything without this vault's key) is
    /// refused by the relay, which then sends nothing more.
    #[tokio::test]
    async fn a_keeper_without_our_key_gets_nothing_after_frame_one() {
        let (mut relay_end, mut impostor) = tokio::io::duplex(4096);

        let recorder = tokio::spawn(async move {
            let mut f1 = [0u8; F1_LEN];
            impostor.read_exact(&mut f1).await.unwrap();
            let mut c_r = [0u8; CHALLENGE_LEN];
            c_r.copy_from_slice(&f1[6..F1_LEN]);
            let c_k = [9u8; CHALLENGE_LEN];
            let t = build_transcript(WIRE, WIRE, &identity(), &c_r, &c_k).unwrap();
            let fake = host_proof(&keys(99), &t);
            let mut f2 = Vec::new();
            f2.extend_from_slice(MAGIC);
            f2.extend_from_slice(&WIRE.to_le_bytes());
            f2.extend_from_slice(&c_k);
            f2.extend_from_slice(fake.as_bytes());
            impostor.write_all(&f2).await.unwrap();
            // Everything the relay sends from here on would be a leak.
            let mut leaked = Vec::new();
            let _ = impostor.read_to_end(&mut leaked).await;
            leaked
        });

        let result = relay_handshake(
            &mut relay_end,
            &keys(1),
            &identity(),
            &b(&["personal"]),
            [3u8; CHALLENGE_LEN],
            STEP,
        )
        .await;
        assert!(matches!(result, Err(HandshakeError::ProofMismatch)));
        drop(relay_end);

        let leaked = recorder.await.unwrap();
        assert!(
            leaked.is_empty(),
            "the relay sent {} byte(s) to a keeper that failed its proof",
            leaked.len()
        );
    }

    /// The relay is bound to the keeper the discovery file names: the same
    /// key but a different pid (or endpoint) does not verify.
    #[tokio::test]
    async fn a_keeper_with_the_wrong_identity_is_refused() {
        let (mut relay_end, keeper_end) = tokio::io::duplex(4096);
        let _keeper = spawn_keeper(keeper_end, 1, WIRE);

        let mut expected = identity();
        expected.pid += 1;
        let result = relay_handshake(
            &mut relay_end,
            &keys(1),
            &expected,
            &b(&["personal"]),
            [3u8; CHALLENGE_LEN],
            STEP,
        )
        .await;
        assert!(matches!(result, Err(HandshakeError::ProofMismatch)));
    }

    /// A relay that cannot read this vault's key cannot produce p_r.
    #[tokio::test]
    async fn a_relay_without_our_key_is_rejected() {
        let (mut relay_end, keeper_end) = tokio::io::duplex(4096);
        let keeper = spawn_keeper(keeper_end, 1, WIRE);

        let (n, frames) = frames_for(&["personal"]);
        let status = manual_relay(
            &mut relay_end,
            &keys(99),
            WIRE,
            PURPOSE_SERVE,
            n,
            &frames,
            &frames,
        )
        .await;
        assert_eq!(status, Some(STATUS_REJECTED));
        let (accept, _) = keeper.await.unwrap();
        assert!(matches!(accept, Err(HandshakeError::ProofMismatch)));
    }

    /// Boundaries changed after the proof was made (a tampering MITM, or a
    /// relay trying to widen its scope) fail the MAC.
    #[tokio::test]
    async fn tampered_boundaries_are_rejected() {
        let (mut relay_end, keeper_end) = tokio::io::duplex(4096);
        let keeper = spawn_keeper(keeper_end, 1, WIRE);

        let (_, signed) = frames_for(&["personal"]);
        let (n, sent) = frames_for(&["work"]);
        let status = manual_relay(
            &mut relay_end,
            &keys(1),
            WIRE,
            PURPOSE_SERVE,
            n,
            &sent,
            &signed,
        )
        .await;
        assert_eq!(status, Some(STATUS_REJECTED));
        let (accept, _) = keeper.await.unwrap();
        assert!(matches!(accept, Err(HandshakeError::ProofMismatch)));
    }

    /// Even correctly signed, an unsorted list is refused rather than
    /// normalised: one byte string per boundary set.
    #[tokio::test]
    async fn a_non_canonical_boundary_list_is_rejected() {
        let (mut relay_end, keeper_end) = tokio::io::duplex(4096);
        let keeper = spawn_keeper(keeper_end, 1, WIRE);

        let (n, frames) = frames_for(&["work", "personal"]);
        let status = manual_relay(
            &mut relay_end,
            &keys(1),
            WIRE,
            PURPOSE_SERVE,
            n,
            &frames,
            &frames,
        )
        .await;
        assert_eq!(status, Some(STATUS_REJECTED));
        let (accept, _) = keeper.await.unwrap();
        assert!(matches!(
            accept,
            Err(HandshakeError::Malformed("boundary order"))
        ));
    }

    /// A frame 3 recorded from one connection is useless on the next: the
    /// keeper's fresh challenge is inside the MAC.
    #[tokio::test]
    async fn a_replayed_frame_three_is_rejected() {
        // Connection 1: record a valid frame 3.
        let (mut relay_end, keeper_end) = tokio::io::duplex(4096);
        let keeper = spawn_keeper(keeper_end, 1, WIRE);
        let c_r = [3u8; CHALLENGE_LEN];
        send_f1(&mut relay_end, WIRE, c_r).await;
        let (_, c_k) = read_f2(&mut relay_end).await;
        let t = build_transcript(WIRE, WIRE, &identity(), &c_r, &c_k).unwrap();
        let (n, frames) = frames_for(&["personal"]);
        let p = relay_proof(&keys(1), &t, PURPOSE_SERVE, n, &frames);
        let mut recorded = vec![PURPOSE_SERVE, n];
        recorded.extend_from_slice(&frames);
        recorded.extend_from_slice(p.as_bytes());
        relay_end.write_all(&recorded).await.unwrap();
        let (first, _) = keeper.await.unwrap();
        assert!(first.is_ok(), "the original frame 3 is valid");

        // Connection 2: a keeper with a DIFFERENT challenge; replay it.
        let (mut relay_end, mut keeper_end) = tokio::io::duplex(4096);
        let keeper = tokio::spawn(async move {
            let r = keeper_handshake_as(
                &mut keeper_end,
                &keys(1),
                &identity(),
                [8u8; CHALLENGE_LEN],
                STEP,
                STEP,
                WIRE,
            )
            .await;
            (r, keeper_end)
        });
        send_f1(&mut relay_end, WIRE, c_r).await;
        let _ = read_f2(&mut relay_end).await;
        relay_end.write_all(&recorded).await.unwrap();
        let (replayed, _) = keeper.await.unwrap();
        assert!(matches!(replayed, Err(HandshakeError::ProofMismatch)));
    }

    /// A newer relay meeting an older keeper asks it to step aside, and the
    /// older keeper agrees.
    #[tokio::test]
    async fn a_newer_relay_retires_an_older_keeper() {
        let (mut relay_end, keeper_end) = tokio::io::duplex(4096);
        let keeper = spawn_keeper(keeper_end, 1, WIRE);

        let outcome = relay_handshake_as(
            &mut relay_end,
            &keys(1),
            &identity(),
            &b(&["personal"]),
            [3u8; CHALLENGE_LEN],
            STEP,
            WIRE + 1,
        )
        .await
        .unwrap();
        assert_eq!(outcome, RelayOutcome::KeeperYielded);
        let (accept, _) = keeper.await.unwrap();
        assert_eq!(accept.unwrap(), KeeperAccept::Yield);
    }

    /// An older relay meeting a newer keeper is told it is out of date, AFTER
    /// the keeper proved itself, and sends nothing more.
    #[tokio::test]
    async fn an_older_relay_is_told_the_keeper_is_newer() {
        let (mut relay_end, keeper_end) = tokio::io::duplex(4096);
        let _keeper = spawn_keeper(keeper_end, 1, WIRE + 1);

        let result = relay_handshake(
            &mut relay_end,
            &keys(1),
            &identity(),
            &b(&["personal"]),
            [3u8; CHALLENGE_LEN],
            STEP,
        )
        .await;
        assert!(matches!(
            result,
            Err(HandshakeError::KeeperNewer { keeper }) if keeper == WIRE + 1
        ));
    }

    /// Only a newer relay may retire a keeper. A same-version "yield" —
    /// correctly signed — is refused.
    #[tokio::test]
    async fn a_same_version_yield_is_refused() {
        let (mut relay_end, keeper_end) = tokio::io::duplex(4096);
        let keeper = spawn_keeper(keeper_end, 1, WIRE);

        let status = manual_relay(&mut relay_end, &keys(1), WIRE, PURPOSE_YIELD, 0, &[], &[]).await;
        assert_eq!(status, Some(STATUS_REJECTED));
        let (accept, _) = keeper.await.unwrap();
        assert!(matches!(accept, Err(HandshakeError::Rejected)));
    }

    /// "Delete everything" asks the keeper to let go of the vault; a holder of
    /// the key gets it, whatever the versions.
    #[tokio::test]
    async fn a_key_holder_can_ask_the_keeper_to_hand_over() {
        for keeper_wire in [WIRE, WIRE + 1] {
            let (mut relay_end, keeper_end) = tokio::io::duplex(4096);
            let keeper = spawn_keeper(keeper_end, 1, keeper_wire);
            request_handover(
                &mut relay_end,
                &keys(1),
                &identity(),
                [3u8; CHALLENGE_LEN],
                STEP,
            )
            .await
            .unwrap();
            let (accept, _) = keeper.await.unwrap();
            assert_eq!(accept.unwrap(), KeeperAccept::Handover);
        }
    }

    /// Only a holder of the vault's key can make the keeper exit.
    #[tokio::test]
    async fn a_handover_without_our_key_is_rejected() {
        let (mut relay_end, keeper_end) = tokio::io::duplex(4096);
        let keeper = spawn_keeper(keeper_end, 1, WIRE);
        let status = manual_relay(
            &mut relay_end,
            &keys(99),
            WIRE,
            PURPOSE_HANDOVER,
            0,
            &[],
            &[],
        )
        .await;
        assert_eq!(status, Some(STATUS_REJECTED));
        let (accept, _) = keeper.await.unwrap();
        assert!(matches!(accept, Err(HandshakeError::ProofMismatch)));
    }

    /// The asker verifies the keeper first: an impostor on the endpoint is
    /// not even told what was wanted.
    #[tokio::test]
    async fn a_handover_request_to_the_wrong_keeper_stops_after_frame_one() {
        let (mut relay_end, keeper_end) = tokio::io::duplex(4096);
        let _keeper = spawn_keeper(keeper_end, 2, WIRE);
        let result = request_handover(
            &mut relay_end,
            &keys(1),
            &identity(),
            [3u8; CHALLENGE_LEN],
            STEP,
        )
        .await;
        assert!(matches!(result, Err(HandshakeError::ProofMismatch)));
    }

    /// A relay on another wire that asks to be SERVED (rather than yielding)
    /// is refused even with a valid proof.
    #[tokio::test]
    async fn a_mismatched_wire_is_never_served() {
        let (mut relay_end, keeper_end) = tokio::io::duplex(4096);
        let keeper = spawn_keeper(keeper_end, 1, WIRE);

        let (n, frames) = frames_for(&["personal"]);
        let status = manual_relay(
            &mut relay_end,
            &keys(1),
            WIRE + 1,
            PURPOSE_SERVE,
            n,
            &frames,
            &frames,
        )
        .await;
        assert_eq!(status, Some(STATUS_REJECTED));
        let (accept, _) = keeper.await.unwrap();
        assert!(matches!(accept, Err(HandshakeError::Rejected)));
    }

    /// Every out-of-range header is refused before the keeper reads what it
    /// announces.
    #[tokio::test]
    async fn malformed_frame_three_headers_are_rejected() {
        let cases: &[(&str, Vec<u8>)] = &[
            ("zero boundaries", vec![PURPOSE_SERVE, 0]),
            ("too many boundaries", vec![PURPOSE_SERVE, 33]),
            ("zero-length boundary", vec![PURPOSE_SERVE, 1, 0]),
            ("over-long boundary", vec![PURPOSE_SERVE, 1, 65]),
            ("unknown purpose", vec![7, 1]),
            ("yield with boundaries", vec![PURPOSE_YIELD, 1]),
            ("handover with boundaries", vec![PURPOSE_HANDOVER, 1]),
        ];
        for (label, bytes) in cases {
            let (mut relay_end, keeper_end) = tokio::io::duplex(4096);
            let keeper = spawn_keeper(keeper_end, 1, WIRE);
            send_f1(&mut relay_end, WIRE, [3u8; CHALLENGE_LEN]).await;
            let _ = read_f2(&mut relay_end).await;
            relay_end.write_all(bytes).await.unwrap();
            let (accept, _) = keeper.await.unwrap();
            assert!(
                matches!(accept, Err(HandshakeError::Malformed(_))),
                "{label}: expected Malformed, got {accept:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_wrong_magic_is_rejected() {
        let (mut relay_end, keeper_end) = tokio::io::duplex(4096);
        let keeper = spawn_keeper(keeper_end, 1, WIRE);
        relay_end.write_all(&[0u8; F1_LEN]).await.unwrap();
        let (accept, _) = keeper.await.unwrap();
        assert!(matches!(accept, Err(HandshakeError::BadMagic)));
    }

    /// A peer that connects and never speaks (another local account can open
    /// the pipe read-only and do exactly this) is dropped at the frame-1
    /// deadline, not held.
    #[tokio::test]
    async fn a_silent_peer_is_dropped_at_the_frame_one_deadline() {
        let (_silent, mut keeper_end) = tokio::io::duplex(4096);
        let started = std::time::Instant::now();
        let result = keeper_handshake(
            &mut keeper_end,
            &keys(1),
            &identity(),
            [2u8; CHALLENGE_LEN],
            Duration::from_millis(100),
            STEP,
        )
        .await;
        assert!(matches!(result, Err(HandshakeError::Timeout)));
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "the frame-1 deadline must bound a silent connection"
        );
    }

    /// A relay never sends a request it knows is invalid.
    #[tokio::test]
    async fn the_relay_refuses_an_empty_boundary_list_before_connecting() {
        let (mut relay_end, _keeper_end) = tokio::io::duplex(4096);
        let result = relay_handshake(
            &mut relay_end,
            &keys(1),
            &identity(),
            &[],
            [3u8; CHALLENGE_LEN],
            STEP,
        )
        .await;
        assert!(matches!(result, Err(HandshakeError::Malformed(_))));
    }
}
