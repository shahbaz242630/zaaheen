//! The desktop's entitlement guard (`SIGNIN-DESIGN.md` §8.26 §6.4).
//!
//! # What this module guarantees
//!
//! A desktop command that serves vault data cannot run for somebody who is
//! not entitled. Not "is unlikely to" — cannot, because the guarantee is
//! carried by the type system rather than by a runtime check every future
//! command has to remember to write.
//!
//! [`Entitled`] is proof. Its one field is private, and this module hands one
//! out from exactly one place: [`Entitlement::require`]. A gated function
//! takes `&Entitled`, so a command that never asked has nothing to pass and
//! **does not compile**.
//!
//! # Why that is not the whole story
//!
//! Three legs hold this up, and each covers what the others cannot:
//!
//! 1. **The compiler** — no gated function runs without a token.
//! 2. **This module's tests** — no token is handed out while locked.
//! 3. **The source test over `main.rs`** ([`GATED_COMMANDS`] /
//!    [`OPEN_COMMANDS`]) — no *new* command is registered without somebody
//!    deciding which side it is on. The compiler cannot catch that one: a
//!    brand-new command with a brand-new inner function compiles perfectly
//!    well while serving memories to a locked user.
//!
//! **The residual**, stated plainly because no test closes it: a command put
//! on [`OPEN_COMMANDS`] that should have been gated is a human misjudgement,
//! and nothing here objects. What the source test guarantees is that the
//! judgement is *made and visible*, not defaulted into by accident.
//!
//! # Fail-secure, without being cruel
//!
//! `vault_app::account::build_desktop` can fail at startup — build settings
//! that do not parse, or a credential store that will not open. The keeper's
//! answer to that is to refuse to start (a gate that silently disappears is
//! worse than a keeper that says why). **The desktop must not copy it.**
//! Refusing to start takes away the export, and "your memories are always
//! yours" is the promise the export exists to keep (BRD §1.6 amendment 1).
//!
//! So a setup failure is [`Source::Unavailable`]: locked, with
//! [`ERR_LOCKED_CANNOT_CONFIRM`] — never "ended", never silently open. The
//! person still reaches the lock screen and can still download their
//! memories. They cannot sign in or subscribe until the app is reopened: the
//! account those need is the one that could not be prepared, and the account
//! commands say so with `account_unavailable` (§8.38 corrects ADR-SEC-023's
//! wording on this). It is the same reading §8.33 applies to the keeper: only
//! a signed `ended` ever says ended.

use std::sync::Arc;

use vault_mcp::{EntitlementCheck, LockReason, Verdict};

use crate::commands::account::AccountSlot;

/// Proof that whoever is using this computer may reach their vault right now.
///
/// Construct it **only** through [`Entitlement::require`]. The unit field is
/// private, so no other module — including the rest of this crate — can make
/// one:
///
/// ```compile_fail
/// // `Entitled`'s field is private, so this does not build.
/// let _forged = vault_tauri::guard::Entitled(());
/// ```
#[derive(Debug)]
pub struct Entitled(());

// There is deliberately no test-only constructor. No test in this crate calls
// a gated `*_inner` function directly — they exercise the wrappers — so one
// would be an unused way to mint a token, which is the one thing this type
// exists to prevent. A future test that does need to call an inner directly
// should add a `#[cfg(test)] pub(crate) fn for_test()` at that point, and not
// before.

/// Nobody is signed in on this computer.
pub const ERR_LOCKED_SIGNED_OUT: &str = "locked_signed_out";
/// The subscription could not be confirmed (offline, a stale lease, a clock
/// that moved backwards, or an account folder that could not be read).
pub const ERR_LOCKED_CANNOT_CONFIRM: &str = "locked_cannot_confirm";
/// The free trial ended without a subscription.
pub const ERR_LOCKED_TRIAL_ENDED: &str = "locked_trial_ended";
/// A paid subscription ended.
pub const ERR_LOCKED_SUBSCRIPTION_ENDED: &str = "locked_subscription_ended";
/// Entitlement has returned and the vault is being opened again. Reachable
/// only from a lock-mode keeper, never from this crate's own check — mapped
/// anyway so the match stays exhaustive and a future reason cannot be
/// forgotten.
pub const ERR_LOCKED_UNLOCKING: &str = "locked_unlocking";

/// The stable code the frontend reads for each reason.
///
/// Deliberately **not** [`LockReason::message`]: that wording is written for
/// an AI agent to relay and says "Open the Zaaheen app on this computer",
/// which is nonsense shown inside the app itself. The desktop's own
/// plain-English lines live in `dist/app.js` and are pinned by
/// `every_lock_code_has_a_plain_english_line_in_the_app`.
#[must_use]
pub fn code_for(reason: LockReason) -> &'static str {
    match reason {
        LockReason::SignedOut => ERR_LOCKED_SIGNED_OUT,
        LockReason::CannotConfirm => ERR_LOCKED_CANNOT_CONFIRM,
        LockReason::TrialEnded => ERR_LOCKED_TRIAL_ENDED,
        LockReason::SubscriptionEnded => ERR_LOCKED_SUBSCRIPTION_ENDED,
        LockReason::Unlocking => ERR_LOCKED_UNLOCKING,
    }
}

/// Where this build's answer comes from.
enum Source {
    /// No sign-in configured in this build: every command runs, exactly as it
    /// did before the guard existed. This is what keeps the existing suite
    /// passing untouched, and what a developer build uses.
    Open,
    /// The real check, over the account folder and the lease.
    Check(Arc<dyn EntitlementCheck>),
    /// The check could not be built at startup. Locked as "could not
    /// confirm" — see this module's header.
    Unavailable,
}

/// The one door that hands out [`Entitled`].
pub struct Entitlement {
    source: Source,
}

/// Build the guard this application runs with. **The only way a binary can
/// obtain an [`Entitlement`].**
///
/// The three constructors below are deliberately private. An independent
/// review of this step found that, while they were public, any future command
/// could have written
///
/// ```text
/// let _entitled = crate::guard::Entitlement::open().require().await?;
/// ```
///
/// which compiles, mints a real [`Entitled`], never asks the account check,
/// and **satisfies `every_gated_command_asks_before_it_serves`** — that test
/// looks for a `.require().await` call, not for which value it is called on.
/// None of the module's three legs distinguished a genuine guard from a
/// locally forged always-open one. Making the constructors private closes it
/// at the language level rather than by hoping nobody writes that line.
///
/// This function stays public because what it returns is always honest: on a
/// machine with account settings it is a real check or a locked
/// [`Source::Unavailable`], never [`Source::Open`].
///
/// **It also hands out the account the commands use** (ADR-SEC-028), built
/// from the *same* account as the check: `Account` keeps a rotated refresh
/// token the credential store refused in memory, so a second copy in this
/// process would replay the spent one and sign the person out. Both views of
/// one account, or neither.
#[must_use]
pub fn build() -> (Entitlement, AccountSlot) {
    let Some(home) = vault_app::install_paths::local_data_dir() else {
        tracing::warn!(
            "no local application data directory; serving locked as 'could not confirm'"
        );
        return (Entitlement::unavailable(), AccountSlot::new(None));
    };

    match vault_app::account::build_desktop(&home) {
        Ok(Some(desktop)) => {
            let (check, ops) = desktop.into_parts();
            (Entitlement::checked(check), AccountSlot::new(Some(ops)))
        }
        // No account settings compiled in: today's ungated path, and no
        // account to show.
        Ok(None) => (Entitlement::open(), AccountSlot::new(None)),
        // Deliberately NOT the keeper's answer, which is to refuse to start.
        // See this module's header: refusing would take away the export.
        Err(e) => {
            tracing::warn!(
                error = %e,
                "the account could not be prepared; serving locked as 'could not \
                 confirm'. Export and erasure stay open."
            );
            (Entitlement::unavailable(), AccountSlot::new(None))
        }
    }
}

impl Entitlement {
    /// A build with no sign-in configured. Private: see [`build`].
    fn open() -> Self {
        Self {
            source: Source::Open,
        }
    }

    /// The real check. Private: see [`build`].
    fn checked(check: Arc<dyn EntitlementCheck>) -> Self {
        Self {
            source: Source::Check(check),
        }
    }

    /// The check could not be built. Private: see [`build`].
    fn unavailable() -> Self {
        Self {
            source: Source::Unavailable,
        }
    }

    /// Ask for permission to serve one command.
    ///
    /// # Errors
    ///
    /// One of this module's `ERR_LOCKED_*` codes. The frontend turns it into
    /// a plain-English line; nothing here reaches the user as raw text
    /// (BRD §11.7.2).
    pub async fn require(&self) -> Result<Entitled, String> {
        self.ask()
            .await
            .map(|()| Entitled(()))
            .map_err(str::to_string)
    }

    /// The code a gated command would be refused with right now, or `None`
    /// when it would be served — for the lock screen (§8.38).
    ///
    /// The same single path as [`Entitlement::require`], so the lock screen
    /// shows exactly when a gated command would be refused. It never hands
    /// out an [`Entitled`]: knowing the answer is not permission to serve.
    pub async fn lock_code(&self) -> Option<&'static str> {
        self.ask().await.err()
    }

    /// Does this build have sign-in at all? `false` only for a build with no
    /// account settings, which must never show a sign-in screen. A build
    /// whose account could not be prepared *does* have sign-in — it is
    /// locked as "could not confirm" (ADR-SEC-023).
    #[must_use]
    pub fn sign_in_available(&self) -> bool {
        !matches!(self.source, Source::Open)
    }

    /// The one place the check is asked.
    async fn ask(&self) -> Result<(), &'static str> {
        // Asked exactly once. A refresh inside the check can take five
        // seconds (§8.26 §4), so asking twice would double that and would
        // record two uses where the person took one action.
        //
        // Note for anyone making this fail on purpose, because the two
        // directions are not alike:
        //   * asking TWICE is plantable, and
        //     `the_check_is_asked_exactly_once_per_call` catches it (bug
        //     4a09);
        //   * never asking AT ALL is not plantable by deletion — `-D
        //     warnings` rejects the then-unread `Source::Check` field, so the
        //     broken version does not compile. That direction is held by the
        //     compiler, which is stronger than a test but is not one.
        match &self.source {
            Source::Open => Ok(()),
            Source::Unavailable => Err(ERR_LOCKED_CANNOT_CONFIRM),
            Source::Check(check) => match check.check().await {
                Verdict::Entitled => Ok(()),
                Verdict::Locked(reason) => Err(code_for(reason)),
            },
        }
    }
}

// ---------------------------------------------------------------- the lists
//
// Every command registered in `main.rs` must appear in exactly one of these.
// The source test below fails otherwise, which is the whole point: a future
// command cannot be added without somebody deciding whether it serves vault
// data.

/// Commands that require an [`Entitled`] token.
pub const GATED_COMMANDS: &[&str] = &[
    "add_memory",
    "search_memories",
    "update_memory",
    "delete_memory",
    "list_recent_memories",
    "list_boundaries",
    "create_boundary",
    "list_agents",
    "list_connected_apps",
    "revoke_agent",
    "ensure_recall_engine",
    "recall_engine_state",
    "warm_recall_engine",
    "ensure_maintenance_engine",
    "set_maintenance_schedule",
    "run_maintenance_now",
    // Where the memories live (ADR-105 L-e): not on the locked allowlist,
    // so gated; the onboarding asks after its sign-in step.
    "location_status",
    "location_check",
    "location_move",
    "location_forget_old_copy",
    // "Connect it for me" (ADR-106): a setup action after sign-in, so gated.
    "connect_app",
    "show_claude_extension",
];

/// Commands that run whatever the entitlement answer is.
///
/// This is §8.26 §6.4's allowlist verbatim — account, export, erasure, logs,
/// settings and maintenance *status* — and nothing else. Two readings worth
/// recording, because both could reasonably have gone the other way:
///
/// - `revoke_agent` is **gated**. There is an argument that revoking an
///   agent's access belongs beside erasure as a safety action anybody should
///   be able to take. The locked allowlist does not include it, and widening
///   a locked list is not a decision this module gets to make on its own. It
///   is moot in practice: a locked computer shows the lock screen, so the
///   Agents tab is not reachable.
/// - `set_maintenance_schedule` is **gated** while `get_maintenance_schedule`
///   is open. The allowlist says maintenance *status*, which is the reading
///   half of that pair.
pub const OPEN_COMMANDS: &[&str] = &[
    "get_settings_info",
    "get_maintenance_schedule",
    "erase_everything",
    "export_logs",
    // The **account** slot of the same locked allowlist, filled by S3 step
    // 4b. Not a widening: 8.26 6.4 has always read "account, export,
    // erasure, logs, settings, maintenance status" -- this slot was empty
    // only because the commands did not exist yet.
    //
    // They have to be open. Somebody whose trial has ended must still be
    // able to sign in and subscribe, and a gate on these would lock a paying
    // customer out of paying.
    //
    // What makes that safe is not this list. It is that none of them is
    // given the vault: see `no_account_command_receives_the_vault` below.
    "account_status",
    "account_sign_in",
    "account_sign_out",
    "account_subscribe",
    "account_refresh_now",
    // Also the **account** slot (S3 step 4d-1, SIGNIN-DESIGN 8.38): "is this
    // computer locked, and why", asked of this guard rather than guessed
    // from the account view. Given the guard and nothing else -- no vault --
    // so the argument above holds for it too.
    "account_access",
    // The **export** slot, filled by S3 step 4c. Unlike the five above, this
    // one DOES read the vault, so it cannot lean on their "holds nothing"
    // argument -- see `commands/export.rs`, which carries its own.
    "export_memories",
    // A **widening**, founder-approved (session 55; ADR-SEC-030 amendment
    // 1): "how far along is the move". A start with a move waiting serves
    // the page before this guard exists, so the question cannot be gated.
    // Given the start's own progress and nothing else -- see
    // `commands/startup.rs` and its source test.
    "startup_state",
    // A **widening**, approved with ADR-108 (session 60; D6): whether the
    // link to the keeper is connecting, serving, tidying or could not start.
    // The lock and sign-in screens never wait on it; it holds no vault and
    // no account -- see `commands/keeper.rs` and its source test.
    "link_state",
];

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Every lock reason there is, written out so that adding a variant to
    /// `LockReason` makes `code_for`'s exhaustive `match` fail to compile
    /// until it is handled here too.
    const ALL_REASONS: &[LockReason] = &[
        LockReason::SignedOut,
        LockReason::CannotConfirm,
        LockReason::TrialEnded,
        LockReason::SubscriptionEnded,
        LockReason::Unlocking,
    ];

    struct FakeCheck {
        verdict: Verdict,
        asked: Arc<AtomicUsize>,
    }

    impl FakeCheck {
        fn new(verdict: Verdict) -> (Arc<Self>, Arc<AtomicUsize>) {
            let asked = Arc::new(AtomicUsize::new(0));
            let check = Arc::new(Self {
                verdict,
                asked: Arc::clone(&asked),
            });
            (check, asked)
        }
    }

    #[async_trait::async_trait]
    impl EntitlementCheck for FakeCheck {
        async fn check(&self) -> Verdict {
            self.asked.fetch_add(1, Ordering::SeqCst);
            self.verdict
        }
    }

    /// A build with no account settings is today's path: the guard is
    /// transparent, which is why the existing suite passes untouched.
    #[tokio::test]
    async fn a_build_with_no_sign_in_hands_out_a_token() {
        assert!(Entitlement::open().require().await.is_ok());
    }

    #[tokio::test]
    async fn an_entitled_person_gets_a_token() {
        let (check, _) = FakeCheck::new(Verdict::Entitled);
        assert!(Entitlement::checked(check).require().await.is_ok());
    }

    /// The one that matters: no token, for any reason, ever.
    #[tokio::test]
    async fn a_locked_person_gets_no_token_for_any_reason() {
        for &reason in ALL_REASONS {
            let (check, _) = FakeCheck::new(Verdict::Locked(reason));
            let answer = Entitlement::checked(check).require().await;
            assert_eq!(
                answer.err().as_deref(),
                Some(code_for(reason)),
                "{reason:?} must refuse, with its own code"
            );
        }
    }

    #[test]
    fn each_lock_reason_maps_to_its_own_code() {
        let codes: Vec<&str> = ALL_REASONS.iter().copied().map(code_for).collect();
        let mut unique = codes.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            codes.len(),
            "two reasons share a code, so the frontend cannot tell them apart: {codes:?}"
        );
    }

    #[test]
    fn every_lock_code_is_a_non_empty_stable_string() {
        for &reason in ALL_REASONS {
            let code = code_for(reason);
            assert!(!code.is_empty(), "{reason:?} has an empty code");
            assert!(
                code.starts_with("locked_"),
                "{code} does not read as a lock code"
            );
        }
    }

    /// A refresh can take five seconds. Asking twice for one command would
    /// double that, and would record two uses where the user took one action.
    #[tokio::test]
    async fn the_check_is_asked_exactly_once_per_call() {
        let (check, asked) = FakeCheck::new(Verdict::Entitled);
        let entitlement = Entitlement::checked(check);

        let _ = entitlement.require().await;
        assert_eq!(asked.load(Ordering::SeqCst), 1);

        let _ = entitlement.require().await;
        assert_eq!(
            asked.load(Ordering::SeqCst),
            2,
            "one ask per call, no cache"
        );
    }

    /// A build with no sign-in must not consult anything at all.
    #[tokio::test]
    async fn a_build_with_no_sign_in_never_asks_a_check() {
        let (check, asked) = FakeCheck::new(Verdict::Locked(LockReason::TrialEnded));
        drop(check);
        let _ = Entitlement::open().require().await;
        assert_eq!(asked.load(Ordering::SeqCst), 0);
    }

    /// A check that could not be built is "could not confirm" - never
    /// "ended", and never silently open. See this module's header.
    #[tokio::test]
    async fn a_check_that_could_not_be_built_reads_as_could_not_confirm() {
        let answer = Entitlement::unavailable().require().await;
        assert_eq!(answer.err().as_deref(), Some(ERR_LOCKED_CANNOT_CONFIRM));
    }

    // ------------------------------------------- the lock screen's question

    /// The lock screen must show exactly when a gated command would be
    /// refused, with the same code (§8.38). Checked for every verdict,
    /// against a fresh check each time so the two answers are independent.
    #[tokio::test]
    async fn the_lock_screen_answer_always_agrees_with_the_gate() {
        let mut verdicts = vec![Verdict::Entitled];
        verdicts.extend(ALL_REASONS.iter().map(|&r| Verdict::Locked(r)));
        for verdict in verdicts {
            let (for_gate, _) = FakeCheck::new(verdict);
            let (for_screen, _) = FakeCheck::new(verdict);
            let gate = Entitlement::checked(for_gate).require().await.err();
            let screen = Entitlement::checked(for_screen).lock_code().await;
            assert_eq!(
                screen,
                gate.as_deref(),
                "the lock screen and the gate disagree for {verdict:?}"
            );
        }
        assert_eq!(Entitlement::open().lock_code().await, None);
        assert_eq!(
            Entitlement::unavailable().lock_code().await,
            Some(ERR_LOCKED_CANNOT_CONFIRM)
        );
    }

    /// Asking whether the screen should show is one ask, like a command.
    #[tokio::test]
    async fn the_lock_screen_question_asks_the_check_once() {
        let (check, asked) = FakeCheck::new(Verdict::Locked(LockReason::TrialEnded));
        let _ = Entitlement::checked(check).lock_code().await;
        assert_eq!(asked.load(Ordering::SeqCst), 1);
    }

    /// A developer build (no account settings) must never show a sign-in
    /// screen; any build with account settings has sign-in, including one
    /// whose account could not be prepared (it is locked, ADR-SEC-023).
    #[test]
    fn only_a_build_with_no_account_settings_lacks_sign_in() {
        let (check, _) = FakeCheck::new(Verdict::Entitled);
        assert!(!Entitlement::open().sign_in_available());
        assert!(Entitlement::checked(check).sign_in_available());
        assert!(Entitlement::unavailable().sign_in_available());
    }

    /// ADR-SEC-028: the desktop builds its account **once**. A second
    /// builder anywhere in the composition path would put the lock and the
    /// buttons on different copies, each with its own in-memory token.
    #[test]
    fn the_desktop_builds_its_account_once() {
        let strip = |src: &str| -> String {
            src.lines()
                .map(|line| line.split("//").next().unwrap_or(line))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let guard = strip(include_str!("guard.rs"));
        let main = strip(include_str!("main.rs"));
        // Only the code before the tests: the test module names these
        // strings in order to look for them.
        let guard = guard
            .split("mod tests")
            .next()
            .unwrap_or(&guard)
            .to_string();

        assert_eq!(
            guard.matches("build_desktop(").count(),
            1,
            "guard::build must build the desktop's account exactly once"
        );
        for (name, code) in [("guard.rs", &guard), ("main.rs", &main)] {
            for second_builder in [
                "build_check(",
                "build_account(",
                "build_account_ops(",
                "Account::new(",
                "DesktopAccount::new(",
            ] {
                assert!(
                    !code.contains(second_builder),
                    "{name} calls `{second_builder}`, a second way to build the account. \
                     The lock and the buttons must share one (ADR-SEC-028)."
                );
            }
        }
    }

    /// A code with no arm in the frontend falls through to showing the user
    /// the raw code, so the plain-English line ships with the code. Same
    /// mechanism as `vault_app::maintenance_state`'s outcome codes.
    #[test]
    fn every_lock_code_has_a_plain_english_line_in_the_app() {
        const APP_JS: &str = include_str!("../dist/app.js");
        for &reason in ALL_REASONS {
            let code = code_for(reason);
            assert!(
                APP_JS.contains(&format!("case \"{code}\":")),
                "{code} has no plain-English line in the desktop bundle, so the user \
                 would be shown the raw code"
            );
        }
    }

    /// Having the lines is not the same as showing them.
    ///
    /// The first version of this step defined `friendlyLockError` and never
    /// called it, so every code still reached the screen raw while the test
    /// above passed — false comfort of exactly the kind this project has been
    /// bitten by twice. Found by the step's independent review. The mapping
    /// must therefore be *reachable*, not merely present.
    #[test]
    fn the_plain_english_lines_are_actually_used() {
        const APP_JS: &str = include_str!("../dist/app.js");
        let uses = APP_JS.matches("friendlyLockError(").count();
        assert!(
            uses >= 2,
            "`friendlyLockError` appears {uses} time(s) — definition only, with no \
             caller. A locked person would be shown the raw code."
        );
    }

    // ------------------------------------------------------------ the lists

    /// The command names `main.rs` actually registers, read out of its source.
    fn registered_commands() -> Vec<String> {
        const MAIN_RS: &str = include_str!("main.rs");
        const OPEN_MARKER: &str = "generate_handler![";

        let start = MAIN_RS
            .find(OPEN_MARKER)
            .expect("main.rs registers its commands")
            + OPEN_MARKER.len();

        // Strip `//` comments BEFORE looking for the closing bracket. The
        // block opens with five lines of prose that mention
        // `#[tauri::command]`, and scanning the raw text finds *that* `]`
        // two lines in, ending the list before a single command is read.
        let cleaned: String = MAIN_RS[start..]
            .lines()
            .map(|line| line.split("//").next().unwrap_or(line))
            .collect::<Vec<_>>()
            .join("\n");
        let end = cleaned.find(']').expect("the handler list is closed");

        cleaned[..end]
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            // `vault_tauri::commands::memory::add_memory` -> `add_memory`
            .map(|item| item.rsplit("::").next().unwrap_or(item).to_string())
            .collect()
    }

    #[test]
    fn every_registered_command_is_classified() {
        let registered = registered_commands();
        assert!(
            !registered.is_empty(),
            "parsed no commands out of main.rs, so this test proves nothing"
        );

        for name in &registered {
            let gated = GATED_COMMANDS.contains(&name.as_str());
            let open = OPEN_COMMANDS.contains(&name.as_str());
            assert!(
                gated || open,
                "`{name}` is registered but classified nowhere. Decide: does it serve \
                 vault data (add it to GATED_COMMANDS and give its inner function an \
                 `&Entitled`), or is it one of the allowlisted six (add it to \
                 OPEN_COMMANDS)? Defaulting is how a locked vault leaks."
            );
        }
    }

    /// A rename that updates `main.rs` but not the lists would otherwise leave
    /// a name here guarding nothing.
    #[test]
    fn every_classified_command_is_actually_registered() {
        let registered = registered_commands();
        for name in GATED_COMMANDS.iter().chain(OPEN_COMMANDS.iter()) {
            assert!(
                registered.iter().any(|r| r == name),
                "`{name}` is classified but no longer registered in main.rs"
            );
        }
    }

    #[test]
    fn the_gated_and_open_lists_do_not_overlap() {
        for name in GATED_COMMANDS {
            assert!(
                !OPEN_COMMANDS.contains(name),
                "`{name}` is on both lists, so what it does depends on which is read first"
            );
        }
    }

    /// Session 48 left this as step 4's obligation: on a locked computer
    /// `run_maintenance_now` reports the run as done although it was paused.
    /// Gating it is the fix, so the classification is pinned rather than left
    /// to be noticed.
    #[test]
    fn run_maintenance_now_is_gated() {
        assert!(GATED_COMMANDS.contains(&"run_maintenance_now"));
        assert!(!OPEN_COMMANDS.contains(&"run_maintenance_now"));
    }

    /// Every gated command must actually ask before it serves.
    ///
    /// The compiler already covers the nine that have an `*_inner`: those
    /// cannot be called without an [`Entitled`], which only
    /// [`Entitlement::require`] mints. The other six — the engine and
    /// maintenance commands — *are* their own body, with no inner to hand a
    /// token to, so for them the gate is the `require()` call itself and this
    /// test is what holds it.
    ///
    /// Stated plainly because the asymmetry matters: for those six the
    /// guarantee is a source test, not the type system. It is weaker, and it
    /// is the honest description.
    #[test]
    fn every_gated_command_asks_before_it_serves() {
        const SOURCES: &[&str] = &[
            include_str!("commands/memory.rs"),
            include_str!("commands/boundary.rs"),
            include_str!("commands/agent.rs"),
            include_str!("commands/engine.rs"),
            include_str!("commands/maintenance.rs"),
            include_str!("commands/location.rs"),
            include_str!("commands/connect.rs"),
        ];

        for name in GATED_COMMANDS {
            // The trailing `(` keeps `add_memory(` from matching
            // `add_memory_inner(`.
            let needle = format!("pub async fn {name}(");
            let from_fn = SOURCES
                .iter()
                .find_map(|src| src.find(&needle).map(|at| &src[at..]))
                .unwrap_or_else(|| {
                    panic!("`{name}` is on GATED_COMMANDS but no command module defines it")
                });
            // A command body ends at the next `}` in column 0.
            let end = from_fn.find("\n}\n").unwrap_or(from_fn.len());

            assert!(
                from_fn[..end].contains(".require().await"),
                "`{name}` is classified as gated but its body never asks. A gated \
                 command must call `entitlement.require().await?` before it does \
                 anything else."
            );
        }
    }

    /// No command module may build an [`Entitlement`] of its own.
    ///
    /// The constructors are private, so this cannot compile today — but the
    /// test states the intent where a future reader will see it, and would
    /// fail loudly if somebody made one `pub` again to "fix" a build error.
    /// `every_gated_command_asks_before_it_serves` cannot catch this on its
    /// own: it checks that `.require().await` appears, not what it is called
    /// on.
    #[test]
    fn no_command_module_builds_a_guard_of_its_own() {
        const SOURCES: &[(&str, &str)] = &[
            ("memory.rs", include_str!("commands/memory.rs")),
            ("boundary.rs", include_str!("commands/boundary.rs")),
            ("agent.rs", include_str!("commands/agent.rs")),
            ("engine.rs", include_str!("commands/engine.rs")),
            ("maintenance.rs", include_str!("commands/maintenance.rs")),
            ("location.rs", include_str!("commands/location.rs")),
            ("connect.rs", include_str!("commands/connect.rs")),
            // The open commands too: `account_access` reads the guard, and
            // must read the application's one, not a guard of its own.
            ("account.rs", include_str!("commands/account.rs")),
            ("erasure.rs", include_str!("commands/erasure.rs")),
            ("export.rs", include_str!("commands/export.rs")),
        ];

        for (name, src) in SOURCES {
            // Code only, never prose (the rule §8.37 records): `account.rs`'s
            // own docs say the slot is filled by `guard::build()`, and a
            // scan of the raw text fires on that sentence.
            let code: String = src
                .lines()
                .map(|line| line.split("//").next().unwrap_or(line))
                .collect::<Vec<_>>()
                .join("\n");
            for forbidden in ["Entitlement::", "guard::build"] {
                assert!(
                    !code.contains(forbidden),
                    "{name} names `{forbidden}`. A command takes the one guard the \
                     application built, as `State<'_, Entitlement>`; building its own \
                     would ask a check nobody configured."
                );
            }
        }
    }

    /// **No account command may receive the vault.**
    ///
    /// These five run for somebody who is *not* entitled, and the only reason
    /// that is safe is that they cannot reach a memory: `AccountOps` holds no
    /// `Application`, so there is nothing to read one with. This is the other
    /// half of that guarantee -- that no command in `account.rs` asks Tauri
    /// for the vault either.
    ///
    /// A source test, because the guarantee is an *absence*: no runtime
    /// assertion can observe a parameter that is not there. If somebody adds
    /// `State<Application>` to a sign-in command, this is what objects.
    ///
    /// (Founder decision, 2026-09-20: "never give them the vault", chosen
    /// over relying on the allowlist plus review.)
    #[test]
    fn no_account_command_receives_the_vault() {
        const ACCOUNT_RS: &str = include_str!("commands/account.rs");

        // Comments first, exactly as `registered_commands` does. That
        // module's own doc says "no `Application`, no adapter, no key" --
        // scanning the raw text makes the test fire on the sentence
        // promising the thing it is checking for. `main.rs` carries the same
        // note about ADR-030's own forbidden-term list.
        let code: String = ACCOUNT_RS
            .lines()
            .map(|line| line.split("//").next().unwrap_or(line))
            .collect::<Vec<_>>()
            .join(
                "
",
            );

        for forbidden in ["Application", "Adapter", "MasterKey", "adapter()"] {
            assert!(
                !code.contains(forbidden),
                "commands/account.rs names `{forbidden}`. These commands are ungated by                  design, and what makes that safe is that they cannot reach a memory.                  Handing them the vault removes the guarantee, whatever the list says."
            );
        }
    }

    /// The export is the one ungated command that reads the vault, so it
    /// carries its own reasoning rather than the account commands'.
    ///
    /// This test is what that reminder became. It pins the two things the
    /// argument in `commands/export.rs` rests on: the export **reads** and
    /// never writes back, and it makes no network call. A future edit that
    /// added either would make an ungated command a way to change or send the
    /// vault while somebody is locked out.
    #[test]
    fn the_export_only_reads_and_sends_nothing() {
        const EXPORT_RS: &str = include_str!("commands/export.rs");
        let code: String = EXPORT_RS
            .lines()
            .map(|line| line.split("//").next().unwrap_or(line))
            .collect::<Vec<_>>()
            .join(
                "
",
            );

        for forbidden in [
            ".write(",
            ".update(",
            ".delete(",
            "erase",
            "reqwest",
            "http",
            "LeaseClient",
        ] {
            assert!(
                !code.contains(forbidden),
                "commands/export.rs names `{forbidden}`. The export is ungated because it                  only reads and sends nothing; writing to the vault or reaching the network                  from here breaks the argument that makes it safe."
            );
        }
    }

    /// The open list is the locked allowlist, and stays that size without a
    /// deliberate decision to widen it.
    #[test]
    fn the_open_list_is_exactly_the_locked_allowlist() {
        assert_eq!(
            OPEN_COMMANDS,
            &[
                "get_settings_info",
                "get_maintenance_schedule",
                "erase_everything",
                "export_logs",
                "account_status",
                "account_sign_in",
                "account_sign_out",
                "account_subscribe",
                "account_refresh_now",
                "account_access",
                "export_memories",
                "startup_state",
                "link_state",
            ],
            "widening the allowlist is a founder decision, not a refactor. The six \
             account entries fill 8.26 6.4's existing `account` slot (steps 4b and \
             4d-1, 8.37 and 8.38); `export_memories` fills the `export` slot (4c); \
             `startup_state` is the founder-approved widening of session 55 \
             (ADR-SEC-030 amendment 1); `link_state` is ADR-108's (session 60)."
        );
    }

    /// ADR-108 D3 (review A-S1): the keeper's admin tools are gated by the
    /// same lists. An admin tool is OPEN exactly when every desktop command
    /// it serves is open, so a locked computer can never reach through the
    /// keeper what the desktop's own guard refuses.
    #[test]
    fn every_admin_tool_is_open_exactly_when_its_commands_are() {
        use vault_app::admin::{Access, ADMIN_TOOLS};
        for tool in ADMIN_TOOLS {
            for command in tool.serves {
                assert!(
                    GATED_COMMANDS.contains(command) || OPEN_COMMANDS.contains(command),
                    "{} serves {command}, which the guard does not list",
                    tool.name
                );
            }
            let all_open = tool.serves.iter().all(|c| OPEN_COMMANDS.contains(c));
            match tool.access {
                Access::Open => {
                    assert!(all_open, "{} is open but serves a gated command", tool.name)
                }
                Access::Gated => assert!(
                    tool.serves.iter().all(|c| GATED_COMMANDS.contains(c)) || !all_open,
                    "{} is gated but serves only open commands",
                    tool.name
                ),
                // Decided per event: pinned in vault-app's ops tests.
                Access::PerEvent => {}
            }
        }
    }

    /// The keeper answers a refused admin call with the same code the
    /// desktop's guard uses, so the lock screen shows the same words.
    #[test]
    fn the_keeper_and_the_guard_name_each_lock_reason_the_same() {
        for &reason in ALL_REASONS {
            assert_eq!(vault_app::admin::locked_code(reason), code_for(reason));
        }
    }
}
