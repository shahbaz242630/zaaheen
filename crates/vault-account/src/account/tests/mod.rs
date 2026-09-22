//! Account tests against a fake Clerk (token, userinfo, revoke), a fake
//! Worker that signs real leases with throwaway keys, the mock credential
//! store and a temporary account folder. Every "now" is explicit.

mod refresh;

use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use keyring_core::api::CredentialStoreApi;
use keyring_core::mock;
use serde_json::json;
use tempfile::TempDir;
use zeroize::Zeroizing;

use super::*;
use crate::config::AccountConfig;
use crate::entitlement::{Denial, Entitlement, DAY};
use crate::files::{LEASE_FILE, LOCK_FILE, MARKER_FILE, STATE_FILE};
use crate::lease::LeaseState;
use crate::oauth::RefreshToken;
use crate::test_support::{json_response, sign_with, FakeService, Seen, TestKeys};
use crate::token_store::{ADDRESS_USER, SERVICE, USER};

pub(super) const SUB: &str = "user_2abcDEF";
pub(super) const EMAIL: &str = "sam@example.com";
/// Server time at sign-in. The client clock is right unless a test says not.
pub(super) const T0: i64 = 1_760_000_000;
pub(super) const HOUR: i64 = 3_600;

const TIMINGS: AccountTimings = AccountTimings {
    call_lock_wait: Duration::from_millis(300),
    user_lock_wait: Duration::from_millis(600),
};

/// What the fake token endpoint answers to a refresh token.
pub(super) type OnRefresh = Arc<dyn Fn(&str) -> Vec<u8> + Send + Sync>;

/// How the fake Worker answers.
#[derive(Clone, Copy, Debug)]
pub(super) enum WorkerMode {
    Sign(LeaseState),
    Status(u16),
    Stranger,
    OtherUser,
}

pub(super) struct World {
    pub tmp: TempDir,
    pub dir: AccountDir,
    pub mock: Arc<mock::Store>,
    pub store: TokenStore,
    pub oauth: FakeService,
    pub worker: FakeService,
    server_now: Arc<AtomicI64>,
    worker_mode: Arc<Mutex<WorkerMode>>,
    on_refresh: Arc<Mutex<OnRefresh>>,
    revoke_status: Arc<Mutex<u16>>,
    keys: Arc<TestKeys>,
    pub account: Account,
}

pub(super) fn tokens(access: &str, refresh: &str) -> Vec<u8> {
    json_response(
        200,
        &json!({
            "access_token": access, "refresh_token": refresh,
            "token_type": "Bearer", "expires_in": 86400
        })
        .to_string(),
    )
}

pub(super) fn invalid_grant() -> Vec<u8> {
    json_response(
        400,
        r#"{"error":"invalid_grant","error_description":"The refresh token was already used"}"#,
    )
}

/// Default rotation: `rt_N` → `rt_{N+1}`.
pub(super) fn rotate(old: &str) -> Vec<u8> {
    let n: u32 = old.trim_start_matches("rt_").parse().unwrap_or(100);
    tokens(&format!("at_{}", n + 1), &format!("rt_{}", n + 1))
}

pub(super) fn token(value: &str) -> RefreshToken {
    RefreshToken::from_stored(Zeroizing::new(value.into())).unwrap()
}

fn oauth_responder(
    on_refresh: Arc<Mutex<OnRefresh>>,
    revoke_status: Arc<Mutex<u16>>,
) -> crate::test_support::Responder {
    Arc::new(move |seen: &Seen| {
        let f = seen.form();
        Some(match seen.path.as_str() {
            "/oauth/token" => match f.get("grant_type").map(String::as_str) {
                Some("authorization_code") => tokens("at_1", "rt_1"),
                Some("refresh_token") => {
                    let handler = on_refresh.lock().unwrap().clone();
                    handler(f.get("refresh_token").map(String::as_str).unwrap_or(""))
                }
                _ => json_response(400, r#"{"error":"unsupported_grant_type"}"#),
            },
            "/oauth/userinfo" => json_response(
                200,
                &json!({"sub": SUB, "email": EMAIL, "email_verified": true}).to_string(),
            ),
            "/oauth/token/revoke" => json_response(*revoke_status.lock().unwrap(), "{}"),
            _ => json_response(404, "{}"),
        })
    })
}

fn worker_responder(
    keys: Arc<TestKeys>,
    server_now: Arc<AtomicI64>,
    mode: Arc<Mutex<WorkerMode>>,
) -> crate::test_support::Responder {
    Arc::new(move |seen: &Seen| {
        let body: serde_json::Value = serde_json::from_str(&seen.body).unwrap_or_default();
        let client_now = body["client_now"].as_i64().unwrap_or(0);
        let now = server_now.load(Ordering::SeqCst);
        let payload = |state: &str, sub: &str| {
            json!({
                "v": 1, "kid": "primary", "sub": sub, "state": state,
                "trial_ends_at": now + 30 * DAY,
                "active_until": if state == "ended" { serde_json::Value::Null } else { json!(now + 30 * DAY) },
                "issued_at": now, "client_time": client_now, "offline_days": 30
            })
        };
        let lease_body = |wire: String| json_response(200, &json!({ "lease": wire }).to_string());
        Some(match *mode.lock().unwrap() {
            WorkerMode::Sign(state) => {
                let name = match state {
                    LeaseState::Trial => "trial",
                    LeaseState::Active => "active",
                    LeaseState::PaymentFailed => "payment_failed",
                    LeaseState::Ended => "ended",
                };
                lease_body(keys.sign(&payload(name, SUB)))
            }
            WorkerMode::Status(code) => json_response(code, r#"{"error":"upstream"}"#),
            WorkerMode::Stranger => {
                let (_, stranger) = dryoc::classic::crypto_sign::crypto_sign_keypair();
                lease_body(sign_with(
                    payload("active", SUB).to_string().as_bytes(),
                    &stranger,
                ))
            }
            WorkerMode::OtherUser => lease_body(keys.sign(&payload("active", "user_OTHER"))),
        })
    })
}

/// A credential store that refuses saves on demand. The mock fails only the
/// next call; this refuses the next `n` saves, or every save while `n` is
/// `usize::MAX`. Reads and deletes pass straight through.
struct FlakyStore {
    inner: Arc<mock::Store>,
    refuse_saves: Arc<AtomicUsize>,
}

struct FlakyCred {
    inner: keyring_core::Entry,
    refuse_saves: Arc<AtomicUsize>,
}

impl FlakyCred {
    fn refuse(&self) -> keyring_core::Result<()> {
        let left = self.refuse_saves.load(Ordering::SeqCst);
        if left == 0 {
            return Ok(());
        }
        if left != usize::MAX {
            self.refuse_saves.fetch_sub(1, Ordering::SeqCst);
        }
        Err(keyring_core::Error::PlatformFailure(Box::new(
            std::io::Error::other("store busy"),
        )))
    }
}

impl keyring_core::api::CredentialApi for FlakyCred {
    fn set_password(&self, password: &str) -> keyring_core::Result<()> {
        self.refuse()?;
        self.inner.set_password(password)
    }
    fn set_secret(&self, secret: &[u8]) -> keyring_core::Result<()> {
        self.refuse()?;
        self.inner.set_secret(secret)
    }
    fn get_password(&self) -> keyring_core::Result<String> {
        self.inner.get_password()
    }
    fn get_secret(&self) -> keyring_core::Result<Vec<u8>> {
        self.inner.get_secret()
    }
    fn delete_credential(&self) -> keyring_core::Result<()> {
        self.inner.delete_credential()
    }
    fn get_credential(&self) -> keyring_core::Result<Option<Arc<keyring_core::Credential>>> {
        Ok(None)
    }
    fn get_specifiers(&self) -> Option<(String, String)> {
        self.inner.get_specifiers()
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl CredentialStoreApi for FlakyStore {
    fn vendor(&self) -> String {
        "flaky test store".into()
    }
    fn id(&self) -> String {
        "flaky".into()
    }
    fn build(
        &self,
        service: &str,
        user: &str,
        modifiers: Option<&std::collections::HashMap<&str, &str>>,
    ) -> keyring_core::Result<keyring_core::Entry> {
        let inner = self.inner.build(service, user, modifiers)?;
        Ok(keyring_core::Entry::new_with_credential(Arc::new(
            FlakyCred {
                inner,
                refuse_saves: self.refuse_saves.clone(),
            },
        )))
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// A token endpoint that behaves like Clerk (S0 spike (d)): each refresh
/// token works once and rotates; presenting a spent one is `invalid_grant`.
pub(super) fn clerk_like() -> impl Fn(&str) -> Vec<u8> + Send + Sync + 'static {
    let spent = Arc::new(Mutex::new(std::collections::HashSet::<String>::new()));
    move |presented: &str| {
        if !spent.lock().unwrap().insert(presented.to_owned()) {
            return invalid_grant();
        }
        rotate(presented)
    }
}

impl World {
    pub async fn new() -> World {
        World::build(None).await
    }

    /// A world whose credential store refuses saves on demand; set the
    /// returned counter to the number of saves to refuse.
    pub async fn flaky() -> (World, Arc<AtomicUsize>) {
        let refuse = Arc::new(AtomicUsize::new(0));
        (World::build(Some(refuse.clone())).await, refuse)
    }

    async fn build(refuse_saves: Option<Arc<AtomicUsize>>) -> World {
        let tmp = TempDir::new().unwrap();
        let dir = AccountDir::open(tmp.path()).unwrap();
        let mock = mock::Store::new().unwrap();
        let store = match refuse_saves {
            None => TokenStore::with_store(mock.clone()),
            Some(refuse_saves) => TokenStore::with_store(Arc::new(FlakyStore {
                inner: mock.clone(),
                refuse_saves,
            })),
        };
        let keys = Arc::new(TestKeys::new());
        let server_now = Arc::new(AtomicI64::new(T0));
        let worker_mode = Arc::new(Mutex::new(WorkerMode::Sign(LeaseState::Trial)));
        let on_refresh: Arc<Mutex<OnRefresh>> = Arc::new(Mutex::new(Arc::new(rotate)));
        let revoke_status = Arc::new(Mutex::new(200));
        let oauth =
            FakeService::start(oauth_responder(on_refresh.clone(), revoke_status.clone())).await;
        let worker = FakeService::start(worker_responder(
            keys.clone(),
            server_now.clone(),
            worker_mode.clone(),
        ))
        .await;
        let account = Account::new(
            dir.clone(),
            store.clone(),
            OAuthClient::for_loopback_test(AccountConfig::for_loopback_test(oauth.port)),
            LeaseClient::for_loopback_test(worker.port),
            keys.verifier(),
            TIMINGS,
        );
        World {
            tmp,
            dir,
            mock,
            store,
            oauth,
            worker,
            server_now,
            worker_mode,
            on_refresh,
            revoke_status,
            keys,
            account,
        }
    }

    /// A second `Account` over the same folder and credential store, as the
    /// next app open builds: it shares nothing in memory with the first.
    pub fn reopened(&self) -> Account {
        Account::new(
            self.dir.clone(),
            self.store.clone(),
            OAuthClient::for_loopback_test(AccountConfig::for_loopback_test(self.oauth.port)),
            LeaseClient::for_loopback_test(self.worker.port),
            self.keys.verifier(),
            TIMINGS,
        )
    }

    /// What sits in the address credential, bypassing every rule.
    pub fn raw_address(&self) -> Option<String> {
        self.mock
            .build(SERVICE, ADDRESS_USER, None)
            .unwrap()
            .get_password()
            .ok()
    }

    /// Put a raw value in the address credential.
    pub fn plant_address(&self, raw: &str) {
        self.mock
            .build(SERVICE, ADDRESS_USER, None)
            .unwrap()
            .set_password(raw)
            .unwrap();
    }

    /// Make the next call on the address credential fail.
    pub fn fail_next_address_call(&self) {
        let entry = self.mock.build(SERVICE, ADDRESS_USER, None).unwrap();
        let cred: &mock::Cred = entry.as_any().downcast_ref().unwrap();
        cred.set_error(keyring_core::Error::PlatformFailure(Box::new(
            std::io::Error::other("store busy"),
        )));
    }

    /// Signed in at server time T0 with a correct clock.
    pub async fn signed_in() -> World {
        let world = World::new().await;
        world.sign_in(T0).await;
        world
    }

    pub async fn sign_in(&self, now: i64) -> SignedIn {
        self.account.complete_sign_in(code(), now).await.unwrap()
    }

    pub fn set_server_now(&self, now: i64) {
        self.server_now.store(now, Ordering::SeqCst);
    }

    pub fn set_worker(&self, mode: WorkerMode) {
        *self.worker_mode.lock().unwrap() = mode;
    }

    pub fn on_refresh(&self, handler: impl Fn(&str) -> Vec<u8> + Send + Sync + 'static) {
        *self.on_refresh.lock().unwrap() = Arc::new(handler);
    }

    pub fn set_revoke_status(&self, status: u16) {
        *self.revoke_status.lock().unwrap() = status;
    }

    /// The stored refresh token.
    pub fn token(&self) -> Option<String> {
        self.store.load().unwrap().map(|t| t.expose().to_owned())
    }

    /// Make the next credential-store call fail.
    pub fn fail_next_keychain_call(&self) {
        let entry = self.mock.build(SERVICE, USER, None).unwrap();
        let cred: &mock::Cred = entry.as_any().downcast_ref().unwrap();
        cred.set_error(keyring_core::Error::PlatformFailure(Box::new(
            std::io::Error::other("store busy"),
        )));
    }

    pub fn has(&self, name: &str) -> bool {
        self.tmp.path().join(name).exists()
    }

    pub fn lease_bytes(&self) -> Option<Vec<u8>> {
        self.dir.read_lease().unwrap()
    }

    /// Refresh tokens presented to the token endpoint, in order.
    pub fn refreshes_sent(&self) -> Vec<String> {
        self.oauth
            .seen_at("/oauth/token")
            .iter()
            .map(Seen::form)
            .filter(|f| f.get("grant_type").map(String::as_str) == Some("refresh_token"))
            .map(|f| f.get("refresh_token").cloned().unwrap_or_default())
            .collect()
    }

    /// Tokens sent for revocation, in order.
    pub fn revoked(&self) -> Vec<String> {
        self.oauth
            .seen_at("/oauth/token/revoke")
            .iter()
            .map(|s| s.form().get("token").cloned().unwrap_or_default())
            .collect()
    }
}

/// The kind of a status, for failure messages. Tests never print a lease or
/// an account's contents, even fake ones: CodeQL's cleartext-logging rule
/// cannot tell test data from real, and the habit is the right one anyway.
pub(super) fn status_kind(status: &Status) -> &'static str {
    match status {
        Status::SignedOut => "SignedOut",
        Status::NoLease { .. } => "NoLease",
        Status::Leased { .. } => "Leased",
    }
}

/// The kind of a refresh outcome, for failure messages (see [`status_kind`]).
pub(super) fn outcome_kind(outcome: &RefreshOutcome) -> &'static str {
    match outcome {
        RefreshOutcome::Refreshed(_) => "Refreshed",
        RefreshOutcome::SignedOut(_) => "SignedOut",
        RefreshOutcome::Skipped(_) => "Skipped",
    }
}

pub(super) fn code() -> AuthorizedCode {
    AuthorizedCode {
        code: Zeroizing::new("code_1".into()),
        verifier: Zeroizing::new("v".repeat(43)),
        redirect_uri: "http://127.0.0.1:1/callback".into(),
    }
}

// ---- sign-in -----------------------------------------------------------------------

#[tokio::test]
async fn signing_in_stores_the_token_marks_the_user_and_starts_the_trial() {
    let world = World::new().await;
    let signed = world.sign_in(T0).await;

    assert_eq!(signed.user.email, EMAIL);
    assert_eq!(signed.user.sub, SUB);
    assert!(!signed.clock_looks_wrong);
    let lease = signed.lease.expect("the first lease");
    assert_eq!(lease.state(), LeaseState::Trial);

    assert_eq!(world.token().as_deref(), Some("rt_1"));
    assert_eq!(world.dir.read_marker().unwrap().as_deref(), Some(SUB));
    assert_eq!(world.lease_bytes().as_deref(), Some(lease.wire()));
    assert_eq!(
        world.dir.read_state(),
        LocalState {
            lease_issued_at: T0,
            floor: T0,
            last_active_anchor: T0,
            last_refresh_attempt: 0
        }
    );

    let calls = world.worker.seen();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].headers.get("authorization").map(String::as_str),
        Some("Bearer at_1")
    );
}

#[tokio::test]
async fn sign_in_survives_an_unreachable_worker() {
    let world = World::new().await;
    world.set_worker(WorkerMode::Status(503));
    let signed = world.sign_in(T0).await;
    assert_eq!(signed.lease, None);
    assert_eq!(world.token().as_deref(), Some("rt_1"));
    assert_eq!(
        world.account.status(T0).await.unwrap(),
        Status::NoLease { sub: SUB.into() }
    );
}

#[tokio::test]
async fn sign_in_clears_a_previous_lease_and_record() {
    let world = World::new().await;
    std::fs::write(world.tmp.path().join(LEASE_FILE), b"someone.elses").unwrap();
    std::fs::write(
        world.tmp.path().join(STATE_FILE),
        br#"{"lease_issued_at":5,"floor":9999999999,"last_active_anchor":7,"last_refresh_attempt":8}"#,
    )
    .unwrap();
    world.set_worker(WorkerMode::Status(503));
    world.sign_in(T0).await;
    assert!(!world.has(LEASE_FILE));
    assert_eq!(world.dir.read_state(), LocalState::default());
}

#[tokio::test]
async fn a_wrong_clock_is_reported_at_sign_in_and_changes_nothing_else() {
    let world = World::new().await;
    let signed = world.sign_in(T0 - 3 * DAY).await;
    assert!(signed.clock_looks_wrong);
    match world.account.status(T0 - 3 * DAY).await.unwrap() {
        Status::Leased { assessment, .. } => assert!(matches!(
            assessment.entitlement,
            Entitlement::Entitled { .. }
        )),
        other => panic!("unexpected status: {}", status_kind(&other)),
    }
}

#[tokio::test]
async fn a_rejected_first_lease_is_not_written() {
    let world = World::new().await;
    world.set_worker(WorkerMode::Stranger);
    let signed = world.sign_in(T0).await;
    assert_eq!(signed.lease, None);
    assert!(!world.has(LEASE_FILE));
    world.set_worker(WorkerMode::OtherUser);
    let signed = world.sign_in(T0).await;
    assert_eq!(signed.lease, None);
    assert!(!world.has(LEASE_FILE));
}

// ---- status ------------------------------------------------------------------------

#[tokio::test]
async fn status_reads_the_folder_without_the_network() {
    let world = World::new().await;
    assert_eq!(world.account.status(T0).await.unwrap(), Status::SignedOut);

    world.sign_in(T0).await;
    let before = world.oauth.seen().len() + world.worker.seen().len();
    match world.account.status(T0 + HOUR).await.unwrap() {
        Status::Leased { lease, assessment } => {
            assert_eq!(lease.sub(), SUB);
            assert_eq!(assessment.elapsed, HOUR);
        }
        other => panic!("unexpected status: {}", status_kind(&other)),
    }
    assert_eq!(world.oauth.seen().len() + world.worker.seen().len(), before);
}

#[tokio::test]
async fn a_lease_file_that_does_not_verify_reads_as_no_lease() {
    let world = World::signed_in().await;
    let mut wire = world.lease_bytes().unwrap();
    wire[3] ^= 0x01;
    std::fs::write(world.tmp.path().join(LEASE_FILE), &wire).unwrap();
    assert_eq!(
        world.account.status(T0).await.unwrap(),
        Status::NoLease { sub: SUB.into() }
    );
    // status never writes: the damaged file is still there for a refresh.
    assert_eq!(world.lease_bytes(), Some(wire));
}

// ---- sign-out ----------------------------------------------------------------------

#[tokio::test]
async fn sign_out_clears_everything_and_revokes_the_token() {
    let world = World::signed_in().await;
    world.account.sign_out().await.unwrap();
    assert_eq!(world.token(), None);
    for name in [MARKER_FILE, LEASE_FILE, STATE_FILE] {
        assert!(!world.has(name), "{name} left behind");
    }
    assert!(world.has(LOCK_FILE));
    assert_eq!(world.revoked(), vec!["rt_1".to_string()]);
    assert_eq!(world.account.status(T0).await.unwrap(), Status::SignedOut);
}

#[tokio::test]
async fn sign_out_works_offline() {
    let world = World::signed_in().await;
    world.set_revoke_status(503);
    world.account.sign_out().await.unwrap();
    assert_eq!(world.token(), None);
    assert!(!world.has(MARKER_FILE));
}

#[tokio::test]
async fn sign_out_clears_the_folder_even_when_the_store_cannot_be_read() {
    let world = World::signed_in().await;
    world.fail_next_keychain_call();
    let _ = world.account.sign_out().await;
    assert!(!world.has(MARKER_FILE));
    assert!(!world.has(LEASE_FILE));
}

#[tokio::test]
async fn sign_out_waits_for_the_lock_and_reports_busy() {
    let world = World::signed_in().await;
    let held = world.dir.lock(Duration::ZERO).unwrap().unwrap();
    assert!(matches!(
        world.account.sign_out().await,
        Err(AccountError::Busy)
    ));
    assert!(world.has(MARKER_FILE), "nothing changed while busy");
    drop(held);
    world.account.sign_out().await.unwrap();
}

// ---- the signed-in address (ADR-SEC-027) -------------------------------------------

#[tokio::test]
async fn the_address_is_remembered_for_the_next_app_open() {
    let world = World::signed_in().await;
    assert_eq!(
        world.account.signed_in_email().await.as_deref(),
        Some(EMAIL)
    );
    assert_eq!(
        world.reopened().signed_in_email().await.as_deref(),
        Some(EMAIL),
        "a second app open, sharing nothing in memory, must still know the address"
    );
    assert_eq!(
        world.raw_address().as_deref(),
        Some(format!("{SUB}\n{EMAIL}").as_str())
    );
}

#[tokio::test]
async fn sign_out_forgets_the_address() {
    let world = World::signed_in().await;
    world.account.sign_out().await.unwrap();
    assert_eq!(
        world.raw_address(),
        None,
        "the address outlived the sign-out"
    );
    assert_eq!(world.account.signed_in_email().await, None);
}

/// The other way a computer signs out: the server ended the grant.
#[tokio::test]
async fn a_dead_grant_forgets_the_address_too() {
    let world = World::signed_in().await;
    world.on_refresh(|_| invalid_grant());
    let outcome = world
        .account
        .refresh(Trigger::UserAction, T0 + HOUR)
        .await
        .unwrap();
    assert!(
        matches!(outcome, RefreshOutcome::SignedOut(_)),
        "{}",
        outcome_kind(&outcome)
    );
    assert_eq!(
        world.raw_address(),
        None,
        "the address outlived the sign-out"
    );
}

/// Bound to the `sub`: whatever is left in the store from an earlier person
/// is never shown under this one's sign-in.
#[tokio::test]
async fn an_address_left_by_somebody_else_is_never_shown() {
    let world = World::signed_in().await;
    world.plant_address("user_OTHER\nsomeone.else@example.com");
    assert_eq!(world.account.signed_in_email().await, None);
}

#[tokio::test]
async fn nobody_signed_in_means_no_address_whatever_the_store_holds() {
    let world = World::new().await;
    world.plant_address(&format!("{SUB}\n{EMAIL}"));
    assert_eq!(world.account.signed_in_email().await, None);
}

#[tokio::test]
async fn signing_in_replaces_an_earlier_address() {
    let world = World::new().await;
    world.plant_address("user_OTHER\nsomeone.else@example.com");
    world.sign_in(T0).await;
    assert_eq!(
        world.account.signed_in_email().await.as_deref(),
        Some(EMAIL)
    );
}

/// A label is not worth a failed sign-in: the sign-in is the token.
#[tokio::test]
async fn a_refused_address_save_still_signs_in() {
    let world = World::new().await;
    world.fail_next_address_call();
    let signed = world.sign_in(T0).await;
    assert_eq!(signed.user.email, EMAIL);
    assert_eq!(world.token().as_deref(), Some("rt_1"));
    assert_eq!(world.dir.read_marker().unwrap().as_deref(), Some(SUB));
    assert_eq!(world.account.signed_in_email().await, None);
}

#[tokio::test]
async fn an_address_that_cannot_be_read_shows_nothing_rather_than_failing() {
    let world = World::signed_in().await;
    world.fail_next_address_call();
    assert_eq!(world.account.signed_in_email().await, None);
    // Once the store recovers, the address is back.
    assert_eq!(
        world.account.signed_in_email().await.as_deref(),
        Some(EMAIL)
    );
}

// ---- use bookkeeping ---------------------------------------------------------------

#[tokio::test]
async fn use_is_recorded_in_server_time_and_writes_are_throttled() {
    let world = World::signed_in().await;
    world.account.record_use(T0 + 2 * HOUR).await.unwrap();
    let after = world.dir.read_state();
    assert_eq!(after.floor, T0 + 2 * HOUR);
    assert_eq!(after.last_active_anchor, T0 + 2 * HOUR);

    // 30 s later: nothing worth writing.
    world.account.record_use(T0 + 2 * HOUR + 30).await.unwrap();
    assert_eq!(world.dir.read_state(), after);

    // 90 s later: the floor moves; activity waits for the hour.
    world.account.record_use(T0 + 2 * HOUR + 90).await.unwrap();
    let later = world.dir.read_state();
    assert_eq!(later.floor, T0 + 2 * HOUR + 90);
    assert_eq!(later.last_active_anchor, T0 + 2 * HOUR);
}

#[tokio::test]
async fn use_while_denied_records_no_activity() {
    let world = World::signed_in().await;
    let before = world.dir.read_state();
    world.account.record_use(T0 + 40 * DAY).await.unwrap();
    assert_eq!(
        world.dir.read_state().last_active_anchor,
        before.last_active_anchor
    );
}

#[tokio::test]
async fn use_bookkeeping_steps_aside_when_the_folder_is_busy() {
    let world = World::signed_in().await;
    let before = world.dir.read_state();
    let _held = world.dir.lock(Duration::ZERO).unwrap().unwrap();
    world.account.record_use(T0 + 2 * HOUR).await.unwrap();
    assert_eq!(world.dir.read_state(), before);
}

#[tokio::test]
async fn use_when_signed_out_does_nothing() {
    let world = World::new().await;
    world.account.record_use(T0).await.unwrap();
    assert!(!world.has(STATE_FILE));
}

#[test]
fn the_default_timings_are_the_locked_design() {
    let t = AccountTimings::default();
    assert_eq!(t.call_lock_wait, Duration::from_secs(2));
    assert!(t.user_lock_wait >= t.call_lock_wait);
}

#[tokio::test]
async fn denials_are_reported_through_status() {
    let world = World::signed_in().await;
    match world.account.status(T0 + 31 * DAY).await.unwrap() {
        Status::Leased { assessment, .. } => assert!(matches!(
            assessment.entitlement,
            Entitlement::Denied(Denial::DeadlinePassed | Denial::OfflineTooLong)
        )),
        other => panic!("unexpected status: {}", status_kind(&other)),
    }
}
