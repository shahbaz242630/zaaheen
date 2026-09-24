# Zaaheen — the desktop app as a client of the keeper (D4)

**Status: LOCKED 2026-09-24 (session 60).** ADR-108 and ADR-SEC-033, with an amendment to ADR-104.
Designed as `HANDOFF_V0.2_PART4_ARCHIVE.md` §8.23 decided ("D4", both session-35 reviewers). The plan
went through two rounds of independent read-only review. Round 1 was NO-GO from both reviewers on the
same blocker: a signed-out fresh install could never start. Round 2 was GO-WITH-FIXES from both, and
every fix is applied below. The iterations and the finding-by-finding disposition are kept outside the
repo in `C:\Projects\MemoryVault-artifacts\d4-design\` (`d4-plan-v1.md`, `d4-plan-v2.md` §9, §11, §12).

**Founder decisions (session 60):**
- The V0.1 bridge is deleted: *"yes remove the V0.1 code"*.
- The tidy-up line: *"message looks good"*.
- The opening line, the three plain lines and "Run now" pausing the AI apps: *"yes partner all three
  are fine, go ahead"*.
- The busy line: *"yes partner that line is fine, go ahead"*.

Not to be confused with ADR-SEC-029's rule "D4" in `VAULT-KEY-AND-LOCATION.md`, which is the rule
never to create a key over keyed data. That rule still holds, and this design relies on it.

## Context
- The desktop app builds its own `Application` (`vault-tauri/src/main.rs:597-657`) and its own retry
  worker (`application.rs:850`). It never takes `.vault.lock`, and it can create the key and run the
  V0.1 bridge (`main.rs:485`). With an AI app connected there are therefore two owners of the stores,
  two retry workers on one queue, and the 1.2 GB reranker loaded twice on a 16 GB laptop. It is the
  last multi-writer path.
- "Run now" fails busy whenever a keeper runs (`vault-cli/src/main.rs:1019`), and it holds the
  desktop's `Application` across the whole run. Catch-up fires it at every launch
  (`dist/app.js:2634-2655`).
- Erasure takes the vault from the keeper (`take_exclusive`), but the desktop's own store stays open
  while the files are deleted (ADR-SEC-029 hazard H6).

## ADR-108 — the desktop never opens a store or creates the key

### D1 Owners
The keeper and the maintenance runner are the only processes that open vault stores or create the
key, one at a time under `.vault.lock`. The desktop reads the key read-only, and only to derive
handshake keys. It re-reads the key on every connect and never caches `HandshakeKeys`.

### D2 The admin surface
The keeper serves an admin connection (ADR-SEC-033) with `AdminServer` (`vault-app/src/admin/`).
- **The tools** are the desktop's command bodies, moved into vault-app. They keep the same clamps,
  audit rows (same `tauri_command` names), JSON and current error texts.
- **Tool names** carry an `admin_` prefix: memory add, search, update, delete and list-recent;
  boundary list and create; agent list and revoke; settings info; engine status, fetch and warm; export
  page; audit event.
- **The Tauri commands** keep their names, arguments, JSON and codes, and forward.
- **Admin sessions** count as activity for the keeper, never appear in `.vault-clients.json`, and see
  every boundary.
- **`admin_memory_search`** waits at the keeper's shared `ReadDesk` (ADR-107). The desktop cancels its
  previous search when it starts a new one.
- **Export is paged**, at most 2,000 rows per call behind a cursor. The desktop writes the file, then
  writes the export audit row itself with the count and the final outcome.
- **Settings:** the `version` shown is the desktop's own.

### D3 Entitlement
- The desktop's `Entitlement::require()` stays the one real ask, with its refresh and its `note_use`.
- On the admin path, the keeper checks `ModeCheck::peek()` (a disk read only) on the full host.
- On a lock-mode host, every GATED admin tool answers `locked_unlocking` and raises the mode flip; it
  never dispatches.
- A table maps each admin tool to the desktop commands it serves, and a test pins "OPEN ⇔ every
  command it serves is in `OPEN_COMMANDS`".
- `admin_audit_event` is gated per event: `export_logs` and `export_memories` are OPEN;
  `set_maintenance_schedule` is GATED. The `run_maintenance_now` row is written by the runner itself
  (D8).

### D4 Starting a keeper from the desktop
**The one rule:** the desktop may start a keeper only when a key exists, or when
`Entitlement::require()` has just passed for a gated command. Background loops only read discovery.
They never start a keeper, so an update that ends the keepers is not undone by an open window.
- **Signed out, or locked, with no key:** no keeper is started. GATED commands answer `locked_*`.
  Export and settings do this:
  - they check with `try_exists`, on metadata only, whether `vault.db`, `lance` or `graph.sealed` is
    present;
  - if any is present, they show startup message 1 (the key is missing; don't delete);
  - otherwise they answer "nothing yet".
  - A key read that errors, rather than finding nothing, shows message 2.
- **A lock-mode keeper that finds no key** writes no Failed record and no failure file. It exits 0
  (`locked_no_key`: nothing needed).
- **The first gated command** after the lock opens starts the full keeper. It creates the key through
  `open_master_key` under K1, exactly as the CLI and the runner do.
- **How a keeper is started:** the per-user on-demand task first. If that fails, a direct spawn of
  `zaaheen-maintenance.exe --keeper --log-dir <dir>` with null stdio and `CREATE_NO_WINDOW`, and
  `CREATE_BREAKAWAY_FROM_JOB` tried first and dropped on access-denied.

### D5 The link
`KeeperLink` is the relays' `KeeperPool` with a purpose (SERVE or ADMIN), not a second client
lifecycle.
- **For ADMIN:**
  - there is no signed-out short-circuit;
  - a typed `ResolveError` replaces the English messages (the relay maps it back to today's `MSG_*`
    text byte-for-byte);
  - the discovery role is read even without a key;
  - on Failed, the link reads `.vault-start-failure.json` (believed only when its pid matches the
    Failed record) and makes one fresh start after a user action;
  - the pool's mismatched-nonce "not retried" rule does not apply.
- **The version branch** runs on the authenticated `wire_k`:
  - equal: ADMIN;
  - older: YIELD, then a fresh keeper;
  - newer: the link is poisoned and the person sees `update_needed`.
  - The desktop never sends HANDOVER on version grounds.
- **Poisoned:** the link is also poisoned after a successful erasure, and cleared when an erasure or a
  move fails.
- **Deadlines:** finding the keeper takes up to 90 s while discovery says Starting, then 55 s for the
  call. The keeper rewrites its Starting record every 20 s while it builds.
- **A dropped connection:** a write whose connection dies mid-call answers `outcome_unknown` and is
  never resent. A read is resent once.
- **Idle:** the link drops after 15 minutes without a call.
- **Cancel:** `peer()` honours the cancel token while it resolves.

### D6 Startup
- **The order:** logging; homes and key location; `finish_the_move`; `location::prepare` and
  `when_the_memories_are_missing`; guard and account; the maintenance context (paths only);
  `app.manage(KeeperLink)`; `startup.ready()`.
- **The window opens without waiting for the keeper.** `Startup` and `untilStarted` are unchanged.
- **The home screen** polls a new open command, `link_state`
  (`connecting` | `serving` | `tidying` | `failed:<code>`), and shows the opening line while it says
  `connecting`. The lock, sign-in and welcome screens never wait for a keeper.
- **Exit:** on `RunEvent::Exit`, a bounded disconnect.

### D7 Why a start failed
- The keeper writes `<vault>/.vault-start-failure.json` as `{v:1, pid, at, code}`:
  - written atomically, and removed on the next successful start;
  - listed in `VAULT_ENTRIES`;
  - never in `KeyedPaths::keyed_entries`.
- Codes: `key_missing`, `credential_store`, `folder_unavailable`, `key_unusable`, `key_busy` (retry),
  `location`, `vault_open_failed`, `locked_no_key` (never shown).
- The desktop maps each code to TODAY's dialog text. The discovery format does not change.

### D8 Maintenance
- **"Run now"** runs `consolidate run --take-over`. The runner:
  1. peeks at entitlement;
  2. calls `take_exclusive` (intent, then HANDOVER, then `.vault.lock`);
  3. publishes `Role::Maintenance`;
  4. calls `ExclusiveVault::into_lock()`, which releases the intent and keeps the lock;
  5. runs holding that lock;
  6. appends the `run_maintenance_now` audit row itself.
- While "Run now" runs, the desktop's link says `tidying`, and the AI apps hear the tidying line
  (founder-approved).
- **Catch-up** passes `catch_up: Some(true)`, which becomes `--wait-if-busy-minutes 75 --only-if-due`.
  The CHILD re-checks "still due?" once it holds the lock.
- **The nightly run** never passes `--take-over` or `--only-if-due` (pinned).
- This replaces §8.23 must-fix 6's "joinable retry worker" with ADR-SEC-033 D3's "bounded drain + exit
  holding the lock".

### D9 Erasure and the move
The flow is unchanged (`take_exclusive`, then `erase_vault` or the move). The link is disconnected
first. Paths come from `LocationContext`, never `app.vault_root()`. H6 closes because the desktop holds
no store.

### D10 Engine
- Reranker acquisition lives in a host-owned single-flight cell (cold on error) used by both the
  start-up run and `admin_engine_fetch`, followed by the warm-up.
- `admin_engine_status` maps to the existing wire strings (`preparing`, `ready`, `unavailable`,
  `not_configured`), plus the progress bytes.
- The desktop's own reranker download is deleted. The Phi-4 download stays in the desktop: it is not a
  store, and there is one downloader.

### D11 The V0.1 bridge
Deleted: `keychain/bridge.rs`, its tests and callers. ADR-SEC-029 amendment 3, and its rule D4's one
exception, are superseded.

### ADR-104 amendment (lock mode keeps export reachable)
- Today, a keeper whose `build_check` fails (`vault-cli/src/keeper.rs:151-157`) refuses to start.
- Now it serves lock mode with a check that always peeks `CannotConfirm`. It retries `build_check` on
  each tick, and on success it exits `ModeChanged`.
- `Ok(None)` (a build with no account settings) stays distinct: only `Err` falls back.
- Lock mode's `LockedHost` (one per keeper process) opens the SQLCipher metadata store only, through
  the new `MetadataStore::open_existing` without `SQLITE_OPEN_CREATE`, on the first OPEN admin call.
  - It opens read-write, because the audit chain is appended, and runs migrations as today.
  - It never touches Lance, the graph, the models or the retry worker.
  - With no `vault.db`, nothing is created.

## ADR-SEC-033 — the `ADMIN` handshake purpose

### D1 Wire
- F3 purpose `ADMIN = 3`, with `n == 0`. The proof covers `[3, 0]` and the whole transcript.
- The keeper checks `wire_r == wire_k` only after `p_r` verifies, never beside HANDOVER, and replies
  `STATUS_SERVING`.
- `WIRE` stays 4, since wire 4 has never shipped.
- The admin tool contract has its own hash beside `PINNED`; changing it needs a WIRE bump.

### D2 Trust
- **Who may open ADMIN:** any holder of the master key. That is the same trust as HANDOVER, `--direct`,
  or opening SQLCipher directly. Other Windows accounts cannot write to the pipe.
- **The audit actor** `ActorKind::User` now means "a key holder over ADMIN".
- **Relays:** the relay code never calls `admin_handshake` (a source test pins this).

### D3 Drain
- `InFlight` becomes unconditional: one per tenure, wrapping gated and plain services, and used by the
  gate, the admin handler, the drain and the mode flip.
- `close()` makes every new tool call answer a retryable busy result before dispatch, and `ReadDesk`'s
  waiting questions leave at once (a closed flag plus a wake-up).
- **On HANDOVER and YIELD:**
  1. remove discovery;
  2. `close()`;
  3. `wait_idle()` for up to 10 s;
  4. abort the connections (which cancels their requests' tokens);
  5. exit holding `.vault.lock`.
- A test pins that erasure's (20 s) and the move's (30 s) take-over waits exceed the drain plus exit.

## Plain lines (founder-approved, session 60)

| Code | Line |
|---|---|
| `vault_maintenance_in_progress` | "Zaaheen is tidying your memories. This can take a few minutes. Try again soon." |
| opening (`link_state` connecting) | "Opening your memories…" |
| `update_needed` | "Zaaheen was updated. Close it and open it again to carry on." |
| `outcome_unknown` | "We couldn't confirm that was saved. Check your memories before adding it again." |
| `keeper_unreachable` | "Zaaheen couldn't start its background part. Close it and open it again; if this keeps happening, contact customerservice@zaaheen.com." |
| `admin_busy` | "Zaaheen is answering another app right now. Try again in a moment." |

## Tests, written first (floor)
1. Fresh install, signed out, no key: the sign-in screen, and no keeper. Signed in but still locked
   (trial ended, no lease yet): no keeper, no dialog. Entitled, first gated command: the full keeper
   creates the key, and admin works.
2. Unreadable account: lock mode `CannotConfirm`; export works; GATED tools answer
   `locked_cannot_confirm`; a later successful check flips the mode.
3. Handshake: ADMIN accepted (n = 0, same wire) and rejected (n > 0, another wire, a bad proof). YIELD
   is sent to an older keeper, never HANDOVER. A newer keeper poisons the link. Relays never call
   `admin_handshake`.
4. keeper_end_to_end:
   - ADMIN and SERVE at once, sharing one desk;
   - an admin session keeps the keeper alive and is absent from the clients file;
   - peek-only gating, with no refresh and no `note_use`;
   - a GATED tool never reaches `LockedHost`;
   - lock mode with no `vault.db` creates nothing;
   - after `close()`, new calls are refused and the drain ends within 10 s;
   - the take-over waits exceed the drain.
5. The desktop never opens a store: the keeper runs as a separate child process. A share-mode-0 open of
   `vault.db` fails while it runs and succeeds after it exits, with the desktop's link still alive
   (reconnect disabled).
6. vault-tauri source tests:
   - no `Application`, `bridge_or_init`, `open_master_key`, `MetadataStore`, `StorageBackend`, retry
     worker or warm-up;
   - the only key call is `read_existing_master_key`;
   - the setup order is pinned;
   - nothing waits on the keeper before `startup.ready()`.
7. Pool: the key is swapped between connects, and there is no ProofMismatch and no cached keys.
8. `--take-over`: it takes the vault from a running keeper; Maintenance is published before the intent
   is released; the runner writes the audit row; catch-up behind it does not run twice; nightly still
   waits and never carries the new flags.
9. The start-failure file: each code produces today's dialog text; a record from another pid is not
   believed; `key_busy` retries; a lock-mode keeper with no key writes nothing; the file never blocks
   key creation.
10. The moved op tests pass unchanged; the OPEN/GATED table holds; paging round-trips 5,001 rows; the
    engine-string pin is re-pointed at the admin mapping.
11. Frontend contract: `link_state` and the opening line; every new code has its plain line;
    `catchUp` on catch-up only; the existing Run-now call is still valid.
