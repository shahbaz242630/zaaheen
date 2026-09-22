# Zaaheen — where the vault key and the vault live

> **Live text.** Holds the decisions about where the vault's master key and the vault itself are kept: ADR-SEC-029 (the key stays on this computer, session 52) and, next, ADR-105 with its ADR-SEC (the user-chosen vault location). Quote it, don't paraphrase it; record any change as a numbered amendment. Account identifiers never go here (this repo is public).

## ADR-SEC-029 — The vault key stays on this computer, and moving it can never lose it or bring it back

**Status: LOCKED 2026-09-21 (session 52).** Founder, shown the plan in plain English after four adversarial review rounds: *"yes plz"*. Round 4 of the independent review: *"I approve once D4 requires that no marker exists and a failed marker removal fails closed"* — both applied below. The draft iterations are kept outside the repo (`C:\Projects\MemoryVault-artifacts\wip-backups\adr-sec-029-draft-iter4.md`); this is the locked text.

### What changed

- **Round 3 → iteration 4:** the erased marker is written only AFTER both credentials are confirmed gone, and it names the erased folder; finishing a wipe touches only that folder (R3-1, R3-2 — both were BLOCKERs). K2 (the old per-folder lock) cut; its residual risk becomes an operating rule (R3-3). `KEYED_ENTRIES` uses the process's actual vector and graph paths, and the V0.1 bridge is named as D4's one exception (R3-4). Startup messages re-routed, two added (R3-5). R2 = `build_application` hands back the key; C2 logs at every open (R3-6, R3-7).
- **Round 2 → iteration 3:** D4 counts only data sealed under the key (a keeper-first fresh install writes `.acl-v1` and `.vault-host.json` before any key, `vault-cli/src/keeper.rs:94, 124-131`); the recovery never opens the database, never replaces a main that is present, and deletes a spare only when main holds the same bytes (`verify_sqlcipher_passphrase` cannot tell a wrong key from a busy or damaged file, `vault-storage/src/metadata_store.rs:552-569`); erasure's last check inside the same lock hold.
- **Round 1 → iteration 2:** one per-user key lock that erasure also holds (erasure took `.vault.intent` + `.vault.lock` but not the per-folder `.keyinit.lock`, `keeper/exclusive.rs:56-90`, while CLI commands open with no `.vault.lock`, `vault-cli/src/main.rs:581-592, 679`; and the lock was per folder, `main.rs:1459, 1636`, while the key is per Windows user); erasure deletes rather than reads first, spare first; every read wiped; the spare has its own service name; a new key is read back; **the startup dialog's advice to delete the key — which would destroy every memory — removed** (`vault-tauri/src/lib.rs:126-143`).

### Context

- **The finding (HANDOFF_V0.2_PART4_ARCHIVE.md, open item 14, session 43), quoted:** "The vault key is stored with Windows "Enterprise" (roaming) persistence (LOW; found session 43). `windows-native-keyring-store` 1.0.0 defaults to `CredPersist::Enterprise` (`lib.rs:40`) and `keychain.rs` never overrides it; `cmdkey /list` shows no local-persistence marker on `default.com.zaaheen.v0.2`. On a domain-joined machine with a roaming profile the master key would follow the user to other machines. Harmless for a home PC, wrong for the zero-knowledge promise. Fix with the S1 credential work (which already specifies `persistence=Local` for the account entry): set `Local` for the vault key too, and migrate an existing entry by rewriting it."
- **BRD §11.3.1:** "Hardware key binding ensures vault cannot be decrypted on a different device even with the passphrase". **§11.5.4:** "Account deletion: master key is destroyed, making all encrypted data permanently unrecoverable."
- **Library (windows-native-keyring-store 1.0.0):** `build` takes `persistence`, default `"Enterprise"` (`store.rs:117-129`); persistence "will only be applied when the credential's secret is written" (`lib.rs:40-45`); `set_secret` = an attribute read + one `CredWriteW(&cred, 0)` on the same target (`cred.rs:91-114`, `utils.rs:126-166`), so a rewrite changes persistence in place (upstream `tests.rs:415`); `ERROR_NOT_FOUND => Error::NoEntry` (`utils.rs:391`). Warnings (`lib.rs:74-82`), quoted: "setting a password on one thread and then immediately spawning another to get the password may return a `NoEntry` error on the spawned thread" and "changing a credential's persistence type immediately before reading it may cause the read to fail".
- **Hazards in today's code:**
  - H1 (latent): `read_or_init_inner` creates a key over anything that looks like not-found (`keychain.rs:263-300`); the move would add the library's documented trigger.
  - H2 (real): erasure counts any read error as "already erased" and deletes the files (`keychain.rs:742-749`), which can leave the key alive.
  - H3 (brittle): the process-global default store is set and unset around every call.
  - H4: not-found is matched on error text.
  - H5 (real): the startup dialog advises deleting the key.
  - H6 (likely; the live test confirms or refutes it): the desktop erases while its own `Application` holds `vault.db` open (`vault-tauri/src/commands/erasure.rs:64-89`), so `vault.db` probably survives; the next start lands on the V0.1 bridge's `VAULT_KEY` message (`keychain.rs:425`).

### Decision

**The lock**
- **K1 — one key lock per Windows user.** Every operation that writes or deletes the key — create, move, recover, erase, finish-a-wipe — holds `<local_data_dir>\keys\<service>.<vault_id>.lock` (`%LOCALAPPDATA%\com.zaaheen.app\keys\…`, never roaming). One function returns the path (with a test-only override); `local_data_dir()` = `None` → fail closed, never fall back to the vault folder. OS file lock (released on process death), 10 s wait. Readers never take it. Lock order everywhere: `.vault.intent` → `.vault.lock` → K1 (round 2 confirmed no inversion). The old per-folder `.keyinit.lock` is no longer taken; it stays declared in `VAULT_LOCK_FILES`.
- **Operating rule (replaces K2):** older builds serialise only on their own folder's `.keyinit.lock`. An in-place upgrade replaces them, and old processes still running have already read the key and only read from then on. The one exposed case is a developer build running beside an older installed build: **close every other Zaaheen build before the first open of this one.**

**Reading and creating**
- **D1 — one owned store:** entries built from the module's own `Arc<CredentialStore>` via `CredentialStoreApi::build`. keyring-core's process-global default is never set, read or unset by vault-app, its tests or its helpers (pinned by a test).
- **D2 — every write is Local and read back:** the bytes must match and `persistence` must read `"Local"`. A new key that does not read back is not returned as success.
- **D3 — "no key" means `Error::NoEntry` only**, matched on the enum, in every caller. Any other error is `KeychainProvenance`.
- **R1 — `NoEntry` is believed only after 5 reads, 100 ms apart.**
- **R2 — `build_application` hands the master key (or the handshake keys derived from it) to the keeper**, instead of the keeper reading it again at `keeper.rs:240`. Pinned by a test.
- **D4 — never create a key while keyed data exists.** A key is created only when all of these hold: the creator holds K1; main and spare both read `NoEntry` (after R1); **no erased marker exists** (round 4: "marker present ⇒ no live key anywhere" is the invariant C1's cleaning and spare deletion rely on, so no key may be created anywhere while one exists); and none of `KEYED_ENTRIES` exists, each checked with `try_exists()` (an error → fail closed).
  - `KEYED_ENTRIES` is built from **the process's actual paths**: the database (`--vault-db`) with its `-wal` and `-shm`; the vector folder (`--vector-dir`); the graph (`--graph-db`, and `graph.sealed` beside it); and `reports` in the vault root.
  - Every one of these is written only after a key exists (`StorageBackend::open_with_at_rest_key` runs after the key step, `vault-cli/src/main.rs:1460-1477`, `vault-tauri/src/main.rs:298`). Operational files do not count: `.acl-v1`, `.vault-host.json`, `maintenance.json`, `.keeper`, the lockfiles and `models/`.
  - Pinned: a keeper-first fresh install creates a key.
  - **The one exception:** the V0.1 bridge (branch 3) creates a key while `vault.db` exists, by design. It runs after C2 and C1, under K1.
- Nothing creates a key outside K1: the seeders and probes (`vault-app/tests/scale_eval.rs:884, 1084, 1216, 1422, 1690`) and `vault-cli`'s live test (`main.rs:2798`) go through the locked opener or `read_existing_master_key`.

**The move and its recovery**
- **D5 — the move.** A creator does this under K1, only when main's `persistence` is `Enterprise`. The spare uses service `com.zaaheen.v0.2.migrating`, user `default` (target `default.com.zaaheen.v0.2.migrating`); no `vault_id` produces that name.
  - M1: read main → K (`Zeroizing`). If main is already Local → done, no write.
  - M2: spare := K (Local), read back == K. On failure, leave whatever the spare holds, return K, leave main untouched, `warn`. There is no deletion on any failure path.
  - M3: main := K (Local), read back == K and Local. On failure, rewrite once more from memory. If it still fails, leave the spare, return K, `error`.
  - M4: delete the spare, but only after main has read back == K. Then confirm `NoEntry`. On failure, `warn`; C2 finishes it at the next open.
- **C2 — recovery, at every creator open, under K1, after C1 and before reading or creating.** It never opens the database, never replaces a main that is present, and deletes a spare only when main holds the same bytes.
  - Spare `NoEntry` → nothing to do.
  - Spare == main → run M3 if main is not Local, then M4.
  - Spare ≠ main (both present, both 32 bytes) → keep main, leave the spare, and log an `error` at every open. D5 cannot produce this state, and the next "Delete everything" removes the spare (E2).
  - Spare present, main `NoEntry` (after R1) → restore main := spare (Local, read back), then M4.
    - **Documented window:** a downgrade to a build older than this one, whose erasure deletes main but not the spare, followed by an upgrade, revives that key. It needs an interrupted move AND a downgrade AND an erasure; we do not ship downgrades.
  - Spare not 32 bytes → it cannot be a key; delete it and `warn`.

**Erasure, and finishing one**
- **E1 — one K1 hold** (inside the existing `.vault.intent` + `.vault.lock`) from E2 through E3.
- **E2 — delete, don't read first; spare first, then main.** Call `delete_credential`: `Ok` or `NoEntry` means gone. Then confirm each reads `NoEntry`.
  - Any other error → `Err`, **no files touched, no marker written**.
  - `key_destroyed` = either delete returned `Ok`.
- **E0 — the marker, written only once E2 has confirmed both gone.** It goes in `<K1 folder>\<service>.<vault_id>.erased` and holds the erased vault folder's absolute path, nothing else.
  - "Marker present" therefore always means the key was destroyed and that folder holds only dead data.
  - A crash between E2 and E0 leaves no marker. The next start then shows message 1; it never deletes anything.
- **Files:** as today, best effort, `VAULT_ENTRIES` in that folder only; undeletable files are listed.
- **E3 — inside the same hold:** both credentials read `NoEntry` (otherwise destroy again and log loudly). If the folder has no `VAULT_ENTRIES` left, remove the marker; otherwise keep it for C1.
- **C1 — finishing a wipe, at every creator open, under K1, before C2 and D4.** If a marker is present, readable and valid, run erasure's own file step on **the recorded folder only**, over the `VAULT_ENTRIES` names only (bounded, as `erasure.rs:83-87` requires).
  - **Valid** = an absolute path with no `..` component that is the same directory on disk (file identity, not string comparison) as the folder erasure runs on: `install_paths::data_dir()` today, the configured location after ADR-105. Anything else → not acted on, `warn`, marker left (and D4 still refuses while it exists → message 3).
  - That folder now clean → remove the marker. **The removal failing → fail closed with message 3** (the marker must never outlive the moment a new key could be made).
  - Not clean, and the recorded folder is this process's own vault folder → fail closed with message 3; the marker is kept.
  - Not clean, and the recorded folder is another folder → carry on with this process's own vault; the marker is kept.
  - Any spare present while a marker exists is deleted: the user asked for erasure, and E2 already confirmed main gone.
  - A marker that cannot be parsed, or names a non-absolute path → not acted on; `warn`; left for support.
  - A marker never causes a key to be created. Creation is still D4's decision, and it runs after C1.

**Hygiene and words**
- **Z1 — no stray copies of the key.** Every `get_secret` result goes straight into `Zeroizing`, and keys are compared on `Zeroizing` buffers. Keyring errors are never `Debug`-printed (`BadEncoding` carries the bytes); they are mapped to a message without the payload, as `TokenStore::keychain` does.
- **U1 — the startup dialog never tells anyone to delete the key.** It shows no internal names and no "Details:" dump (BRD §11.7.2); details go to the log. The message is chosen by error type, and the exact words go to the founder for approval when U1 is built:
  1. The key is missing but keyed data is present (D4): memories are still on this computer, don't delete them; if you haven't changed computer or Windows account, restart and try again; if you have, go back to the one you used before; support address.
  2. The credential store is failing (D3 errors): Windows' secure password store can't be reached; restart; your memories are on this computer; support address.
  3. The folder can't be checked or cleared (`try_exists` errors, C1 can't finish, `local_data_dir()` = `None`): the folder can't be reached; plug in the drive or close whatever program is using it; support address.
  4. The key is present but unusable (main not 32 bytes): don't delete anything; write to support.
  5. K1 is busy for more than 10 s (for example, an erasure running): Zaaheen is finishing another task; wait a minute, then open it again.

### Tests, written first

- **Store double:** a small internal trait (read / write-Local / delete / persistence) with the Windows implementation and a scripted double. The double injects failures and transient `NoEntry`, and can **stop after any single step** to simulate a crash. (keyring-core's mock accepts no modifiers, so it cannot model persistence.)
- **Unit tests:**
  - The first run creates a Local key and reads it back.
  - An Enterprise main moves: same bytes, Local, no spare left.
  - A Local main causes no write.
  - **Every crash point of D5, C1, C2, E2, E0 and E3, enumerated.** After each, a fresh open (and, for erasure, a re-run) must end in one of two states: K is main, Local, with no spare; or, after an erasure, no key ever returns, and a new one is made only when D4 allows.
  - Each step's failure returns K and never loses main.
  - C2's five cases.
  - A transient `NoEntry` is retried and never creates a key.
  - Keyed data present, including at a `--vault-db`, `--vector-dir` or `--graph-db` outside the root, or a `try_exists` error → fail closed, no write.
  - Operational files only (a keeper-first install) → a key is created.
  - A non-`NoEntry` error → no write.
  - **Erasure versus creator interleavings under K1** (round 1's sequence) → the key never returns.
  - A marker never exists while a key is alive: an E2 failure writes none.
  - C1 touches only the recorded folder, never a process's own folder that differs from it, and never a name outside `VAULT_ENTRIES`.
  - A marker-removal failure → no key is created.
  - A marker that is unparseable, relative, contains `..`, or names a folder other than erasure's → ignored, nothing deleted, and no key created while it exists.
  - While a marker exists, no key is created in ANY folder.
  - R2: the keeper never reads the key a second time after `build_application`.
  - Erasure removes the spare, spare first.
  - The global default store is never touched.
  - No message or log line carries key bytes.
- **Live tests** (Windows Credential Manager, throwaway namespaces, a temp K1 folder):
  - A new key is Local.
  - A real Enterprise entry moves to Local with identical bytes and no spare.
  - Erasure of a moved key and its spare.
  - **H6 confirmed or refuted:** erase while a `MetadataStore` holds the database open, then a fresh open finishes the wipe through C1 and starts clean.
- **vault-tauri:** the five texts are routed by error type, contain no delete advice and no internal names (pinned).
- **Planted bugs, each must be caught:**
  - skip M2
  - skip a verify
  - M4 before M3's read-back
  - treat any error as no key
  - create a key while keyed data is present
  - count operational files as keyed
  - a per-folder lock instead of K1
  - erasure outside K1, or E3 after releasing K1
  - erasure deletes main before the spare
  - erasure's `Err(_) => Ok(false)`
  - the marker written before E2's confirmation
  - C1 cleans the process's own folder instead of the recorded one
  - C2 replaces a present main
  - C1 restores a spare
  - D4 creates a key while a marker exists
  - C1 acts on an unvalidated marker path
  - a failed marker removal carries on
  - a write without Local
  - the old dialog text

### Consequences

- **API changes:** `read_or_init_master_key`, `bridge_or_init_master_key` and `erase_vault` take the K1 path (from the one function) and the process's actual storage paths; `build_application` also returns the key material the keeper needs. Callers: `vault-tauri/src/main.rs:298`, `vault-cli/src/main.rs:1460, 1638`, `vault-cli/src/keeper.rs:240`, `vault-tauri/src/commands/erasure.rs`, and tests.
- **Code layout:** `keychain.rs` (1,728 lines) splits into a `keychain/` module. The V0.1 bridge keeps its behaviour, with D1–D4 applied.
- **What users see:** the founder's key moves on the first open of the next build, and so does everyone else's. Nothing is visible.
- **Copies that already roamed stay where they roamed.** Local stops future roaming only; the ADR says so rather than claiming otherwise.
- **The vault folder itself is in Roaming `%APPDATA%`** (`install_paths.rs:100`). With a Local key, a roaming profile carries unreadable files to another machine, and message 1 explains it. Where the vault lives by default is the next step's decision (ADR-105).
- **Tech debt:** the V0.1 bridge's snapshot `vault.db.pre_v0_2_bridge` (`keychain.rs:597-602`) is not in `VAULT_ENTRIES`, so it survives "Delete everything". It is keyed with the V0.1 passphrase, not the master key.

### Implementation record (session 52) — the build of ADR-SEC-029

**Where it lives.** `crates/vault-app/src/keychain/` (was one 1,728-line `keychain.rs`): `store.rs` (the two credentials, D1/D2/D3/Z1), `files.rs` (vault data, the marker, erasure's step), `lifecycle.rs` (open, create, move, recover, erase, finish — pure logic over the two traits, the caller holds K1), `bridge.rs` (ADR-041), `mod.rs` (`KeyLocation`, `KeyedPaths`, K1, the public openers). Public surface: `open_master_key(&KeyLocation, &KeyedPaths)` (CLI, keeper, maintenance), `bridge_or_init_master_key(&KeyLocation, data_dir, v0_1_key)` (desktop), `read_existing_master_key(namespace, vault_id)` (unchanged, readers), `erase_vault(vault_dir, &KeyLocation)`. `with_key_init_lock` and `read_or_init_master_key` are gone. The five startup messages are routed by a new `VaultError::VaultKey(VaultKeyFailure)` (vault-core), mapped as an internal error on the MCP side like `KeychainProvenance`.

**Amendment 1 (implementation, strengthens C1).** Before acting on a valid marker, C1 reads main (with R1). A key present while a marker exists breaks the invariant "marker present ⇒ no live key" and can only come from outside this design (an older build); C1 then deletes nothing, keeps the marker, and the open carries on with that key. Pinned by `a_marker_with_a_live_key_deletes_nothing`.

**Amendment 2 (implementation, D5/C2 wording).** C2 also refuses to restore a spare while any marker exists, including an untrusted one (then the spare is neither restored nor deleted, and D4 refuses a new key → message 3). Pinned by `an_untrusted_marker_is_never_acted_on_and_blocks_new_keys`.

**Amendment 3 (implementation, the V0.1 bridge's branch 3).** With no key, a `vault.db` present and no `VAULT_KEY`, the desktop now answers `VaultKeyFailure::Missing` (message 1) instead of the V0.1 "set VAULT_KEY" text: D4's decision, with the V0.1 bridge still run whenever `VAULT_KEY` is set.

**H6 confirmed live.** `an_erasure_that_leaves_an_open_database_is_finished_by_the_next_open` holds `vault.db` open with SQLite while erasing: the file survives the erasure (as it does under the desktop's own open vault today), and the next open finishes the wipe through C1 and starts with a new key. Before this ADR, by the old bridge's branch 3 (read from the source, not run), that state made the desktop fall on the V0.1 `VAULT_KEY` message at every start until the file was removed by hand.

**Amendment 4 (review finding 4, tightens C1's "Valid").** A recorded folder that no longer exists is trusted only when it is the erasure folder itself and that folder is gone too (the whole data folder was removed; nothing is left to clean). Any other missing folder — a drive that is not plugged in — is untrusted: the marker stays and D4 makes no key (message 3). ADR-105 must check "is the vault's drive here?" before the key is opened, so a missing drive says "reconnect your drive" first.

**The independent code review (session 52), and what changed.** One read-only reviewer against this ADR. Its findings, each verified and acted on:
1. BLOCKER — items used only by the Windows store were dead code on the Linux/macOS CI legs (`-D warnings`): cfg-gated.
2. BLOCKER — the keeper's R2 source test scanned to the end of the file, so its own string literals made one assertion unpassable and the other unfailable: the window now ends at the close of `dispatch_keeper`.
3. C1 deleted a spare without confirming it gone, and C2 could then restore it in the same open: C1 now confirms (as E2 does), and C2 never restores a spare in an open that began with a marker (`ErasureState { seen, pending }`). Two tests added.
4. Amendment 4 above.
5. D4 now re-reads the spare with R1 before creating (only on a fresh install, so no cost to a normal open).
6. The live one-lock test could pass without the lock (a fresh open already waits 400 ms in R1): the key is created first.
7. Marker validation is now also tested through the marker file itself, and live through `open_master_key` (a marker naming another folder touches nothing and makes no key).
8. `KeyLocation::new` is crate-private: other crates reach the lock only through `KeyLocation::production()` (K1's "one function").
10. The marker write is tried twice. 11. A stray control byte in the test double fixed. 12. The V0.1 bridge keeps its snapshot when restoring it fails (it was deleted, though the warning told the user to copy it back).
9. Not changed, logged as tech debt: key-step errors other than `VaultKey`/`KeychainProvenance` (a V0.1 bridge `Storage` error, a `getrandom` failure) still fall through to the generic startup dialog, which shows details; reachable only with `VAULT_KEY` set or a failing OS random source.

**U1's words, founder-approved (session 52, 2026-09-21): *"partner wording is good .."*.** The five startup messages as shown to the founder are the text in `vault-tauri/src/lib.rs` `format_keychain_error_dialog` (title `KEY_ERROR_DIALOG_TITLE` = "Zaaheen can't open your memories"), pinned by `each_key_failure_gets_its_own_message`, `no_key_failure_message_advises_deleting_the_key` and `key_failure_messages_show_no_internals`.

**Evidence (session 52).** `vault-app --lib -- keychain erasure` 80 passed (live Windows Credential Manager tests included: a real Enterprise key moved to Local byte for byte, the one lock across folders, erasure waiting for the lock, H6 finished by the next open); `vault-cli --bin zaaheen` 41; `vault-tauri --lib` 89; `vault-core` error test. **28 planted bugs, 28 caught** (`C:\Projects\MemoryVault-artifacts\plant-apply.mjs` + `plants-k29.ps1`): the first run caught 22, missed P17 (a `..` marker path that resolves to the erasure folder was accepted through the directory comparison — the plain-path check had no test of its own; fixed with the "roundabout" assertion, then caught), and produced three invalid runs (P24–P26: the harness overflowed PATH by importing vcvars 23 times in one process; re-run in fresh shells, all caught). Pristine runs after every restore, each rebuilt: 80 / 41 / 89 passed.

**Review confirm round (session 52):** *"committable from my side"*. Its one residual note (two faults at once — a spare delete that silently does nothing, then a false "no entry" on C1's confirming read — could leave a copy of an erased key for a later, marker-free open to restore) is closed: `create()` now takes the `ErasureState`, and in an open that began with a confirmed erasure it deletes such a spare (confirmed gone) instead of deferring it. Pinned by `a_spare_met_at_d4_after_an_erasure_is_deleted_not_deferred`, planted as P29 and caught; the bridge's rollback message now says whether the snapshot was restored. Final count: **29 planted bugs, 29 caught**; `vault-app --lib -- keychain erasure` 81 passed.

---

## ADR-105 + ADR-SEC-030 — The person chooses where their memories live

**Status: LOCKED 2026-09-21 (session 52).** Founder, shown the plan in plain English after three adversarial review rounds: *"yes partner go ahead with the location"*. The two founder decisions inside it were given earlier the same session (*"agreed with both small decisions partner..."*). Draft iterations: `C:\Projects\MemoryVault-artifacts\wip-backups\adr-105-draft-iter*.md`; this is the locked text.


### What changed

- **Round 2 → iteration 3:**
  - R2-1 BLOCKER: erasure deleted `.vault-id` (it was in `VAULT_ENTRIES`), so after "Delete everything" `vault_dir()` failed forever. → `.vault-id` and `.move-id` are **declared, never erased** (`VAULT_KEEP_FILES`, beside `VAULT_LOCK_FILES`).
  - R2-2/R2-3: "Start again" could clean a live folder, or could not run after an erasure on a lost drive. → L6 rewritten (clears pending fields, never a live folder, removes a marker only when the key is confirmed destroyed).
  - R2-4: a relay treating a missing location as "no keeper yet" would request a keeper start every round. → answered directly, no start requested.
  - R2-5: move edges (second move during cleanup, nested targets, recovery leaving folders, order, erasure during cleanup). → L5.
  - R2-6: the maintenance runner needs `maintenance.json` from `vault_dir()` and changed forwarding/`consolidate_args`. → L3.
  - R2-7: Google Drive's virtual drive; the sync-root registry is **HKLM** (verified live this session: `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\SyncRootManager\<id>\UserSyncRoots`, value name = the user's SID, data = the folder, read by `reg.exe` without admin; the founder's OneDrive is listed there); `reg.exe` blocked → "couldn't check". → L4.3.
  - R2-8: K1 released before `.vault.lock` (lock order); `KeyLocation::production()` tolerates an unresolvable location; existing-install detection also checks Tauri's `app_data_dir()`. Amendment 5 of iteration 2 was unreachable (the ID check runs first) → replaced by the simpler rule in "ADR-SEC-029 amendment 5" below.
  - R2-9 cuts taken: **no models split** (models stay exactly where they are today, one `models_dir()`); **no exFAT retry bookkeeping**.
- **Round 1 → iteration 2:** the vault ID (a second stick at the same letter was openable as a new empty vault: `metadata_store.rs:119` creates a missing file); a pointer for every install, decided under K1; relays re-resolve; a path-free maintenance task; the move keyed on `move_id`; no PowerShell. Iteration texts: `C:\Projects\MemoryVault-artifacts\wip-backups\adr-105-draft-iter1.md`, `-iter2.md`.

### Context (see iteration 1 for the file:line map)

Every process today derives the vault folder from `install_paths::data_dir()` (`%APPDATA%\com.zaaheen.app`), explicit paths, or (desktop) Tauri's `app_data_dir()`; models from `models_dir_in(vault_root)`; the keeper task has no path, the maintenance task has explicit paths; ADR-SEC-029's erasure folder is `data_dir()`. Every crate forbids `unsafe`.

### Decision

**L1 — The pointer and the vault ID.**
- `%LOCALAPPDATA%\com.zaaheen.app\vault-location.json` = `{ "version": 1, "vault_dir": "<absolute>", "vault_id": "<32 hex>" }` (+ L5's fields). Atomic writes.
- `<vault_dir>\.vault-id` holds the same ID. **Declared, never erased** (`erasure::VAULT_KEEP_FILES`, with `.move-id`): the at-rest sweep sees them, erasure and C1 never delete them.
- **One resolver, `install_paths::vault_dir()`**, for every process: pointer valid (absolute, no `..`, not UNC), folder exists, `.vault-id` present and equal → the folder. Two failures, kept apart: **no pointer yet** (`VaultLocationUnset`: only L2 may act) and **a pointer whose folder, `.vault-id` or ID is wrong** (`VaultLocationMissing`). **The resolver never creates anything and never falls back to a default.**

**L2 — Setting up, once.** At the first run of this build, a creator (desktop, keeper, CLI on default paths, maintenance runner) takes K1 and, only if no pointer exists:
- **existing install** — a keyed entry (ADR-SEC-029's list) in `%APPDATA%\com.zaaheen.app` or (desktop) Tauri's `app_data_dir()` → write `.vault-id` there and the pointer;
- **brand-new install** → create `%LOCALAPPDATA%\com.zaaheen.app\vault` (founder, L2), `.vault-id`, pointer.
K1 is released before anything takes `.vault.lock` (lock order `.vault.intent` → `.vault.lock` → K1 is unchanged; L2 happens before both).

**L3 — Every process follows the pointer.**
- Desktop, CLI default paths, keeper, direct mode, daemon, consolidate: `vault_dir()`; explicit `--vault-db` etc. unchanged.
- **Relays** resolve `vault_dir()` at every connect round, outside their pre-serve path (they always answer `initialize`); **no pointer yet** → a keeper start is requested as today (the keeper runs L2, so an AI-app-first install and the first AI use after an upgrade work); **a pointer present but wrong** → answered directly to the AI app (like `MSG_FAILED`: "Zaaheen can't find your memories — open the Zaaheen app") and **no keeper start**.
- **Maintenance task path-free**, like the keeper task: the runner resolves `vault_dir()` and `maintenance.json` inside it; a missing location is logged, nothing is run or created. `consolidate_args` (also "Run now") and the runner's forwarding test change with it; the task label is bumped so `heal_registered_task` re-registers once.
- CLI commands on default paths refuse while `.vault.intent` is held. (Leftover, stated: a command already running when a move starts is not stopped by that check; it holds no `.vault.lock`, so the move's copy can miss its last writes.)
- **Models stay exactly where they are** (`models_dir()` = `%APPDATA%\com.zaaheen.app\models`, for every install); the six `models_dir_in(vault_root)` sites change to it.
- Settings' "Vault location" and the uninstall notice read `vault_dir()`.
- `keeper.rs:90`'s `create_dir_all` goes: only L2 and L5 create a vault folder.
- ADR-SEC-029: `KeyLocation::production()` resolves its erasure folder with `vault_dir()` **after L2, inside the same K1 hold** (so the first run of this build on an erased-with-leftovers install still finishes the erasure), before the key is opened; if that fails (developer runs on explicit paths), the erasure folder is **none** and every marker is untrusted (amendment 5).

**L4 — Checking a chosen folder (onboarding's "Change…", a move's target).**
1. absolute; typed `\\server`, `\\?\`, `\\.\` refused; `canonicalize()` resolving to `\\?\UNC\…` (a mapped drive, SUBST or junction to a share) refused;
2. not a system or program folder (`%SystemRoot%`, `%ProgramFiles%`, `%ProgramFiles(x86)%`, `%ProgramData%`, the install folder), not a drive root, **not inside** the current vault folder, the models folder, `%LOCALAPPDATA%\com.zaaheen.app` or `%APPDATA%\com.zaaheen.app`;
3. **cloud-synced folders, best effort (said so on screen):** `%OneDrive%`, `%OneDriveConsumer%`, `%OneDriveCommercial%`; every `UserSyncRoots` value for the current user's SID under `HKLM\…\Explorer\SyncRootManager` (via `reg.exe query`, the `icacls`/`schtasks` precedent); Dropbox's `info.json` paths; `%USERPROFILE%\iCloudDrive`, `%USERPROFILE%\Google Drive`, `%USERPROFILE%\Box`; a drive whose root holds `My Drive` (Google Drive for desktop's virtual drive; name from Google's docs, unverified here) — all compared after `canonicalize`. If `reg.exe` cannot run, the screen says "Zaaheen couldn't check whether a cloud app syncs this folder — make sure none does" and the person confirms;
4. writable (a probe file); 5. free space ≥ the vault's size + 10 %;
6. the vault goes into a new `<chosen>\Zaaheen Memories`, which must not already exist;
7. hardening (ADR-SEC-019) is attempted on it; on failure (FAT/exFAT) the move goes on (founder) and the screen says "This drive can't lock the folder to your Windows account. Your memories stay encrypted."
- **USB note** for any target not on `%SystemDrive%`: "Memories on another drive only open on this computer, and only while that drive is connected — AI apps can't reach them while it's out."

**L5 — Moving (Settings "Move my memories", onboarding's "Change…").**
- Refused while `pending_cleanup` is set, or while an ADR-SEC-029 erased marker exists. Otherwise L4, then the pointer gets `"pending_move": { "to": "<target>\Zaaheen Memories", "move_id": "<random>" }` and the app restarts. At startup, before anything opens the vault:
  - M1 take the vault exclusively (intent + `.vault.lock`; the keeper hands over), then K1;
  - M2 create the target folder, harden it, **then** write `.move-id`;
  - M3 copy only the keyed entries, `maintenance.json` and `.vault-id` (never `.acl-v1`, `.vault-host.json`, `.keeper`, lockfiles);
  - M4 verify every copied file (size + BLAKE3);
  - M5 **commit**: pointer → the target, with `"pending_cleanup": { "from": "<old>", "move_id" }`, without `pending_move` (atomic rename). Release K1;
  - M6 remove `.move-id` from the target; clean the old folder (erasure's bounded step, plus its `.vault-id`), **never if it is the same directory as `vault_dir()`**; clear `pending_cleanup`; release the vault.
- **Recovery, at every start first:** `pending_move` → the target is cleaned only if its `.move-id` equals `move_id` (only `VAULT_ENTRIES` names + `.vault-id` + `.move-id`), then the folder itself is removed with `remove_dir` (which refuses a non-empty folder; never `remove_dir_all`); an **empty** folder at exactly `pending_move.to` is removed the same way (a crash before `.move-id`); then the move is retried; after two failures `pending_move` is cleared and the reason shown. `pending_cleanup` → M6 is finished.
- "Delete everything" while `pending_cleanup` is set also runs erasure's file step on `pending_cleanup.from` (the key is destroyed first, so that copy is dead too).
- The key never changes (it belongs to the Windows user).
- Onboarding: the location step right after the welcome, the default preselected; "Continue" keeps it; "Change…" → folder picker → L4 → "Zaaheen will restart once to set this up" → restart → M1–M6 → onboarding resumes.

**L6 — "Start again in the default place"** (offered on `VaultLocationMissing`'s screen **only when the folder itself is absent** — a folder that is there with a missing or different `.vault-id` may hold the memories, so that screen points to support instead; confirmed by the person: "The memories on that drive can't be opened from here"). Under K1:
- the target is the new-install folder `%LOCALAPPDATA%\com.zaaheen.app\vault`; refused if it holds keyed data (it is then a vault of its own — support);
- `pending_move` and `pending_cleanup` are cleared;
- if an ADR-SEC-029 marker exists: main and spare must both read `NoEntry` (after R1) — the key is destroyed, so everything the marker names is dead — then the marker is removed; if a key exists instead, the marker is left and the key is kept;
- a new `.vault-id` and the pointer are written. The next open creates the new vault (with the existing key, or — no key and no marker — a new one through D4).

**L7 — Erasure and the pointer.** "Delete everything" erases `vault_dir()`'s `VAULT_ENTRIES` and keeps the pointer and `.vault-id`. The WiX uninstaller leaves `%LOCALAPPDATA%` (the pointer) in place, so a reinstall finds the same memories.

### ADR-SEC-029 amendment 5 (lands with this ADR's code)

`KeyLocation`'s erasure folder becomes optional: `vault_dir()` when it resolves, otherwise none — and with none, every marker is `Invalid` (never acted on; D4 makes no key while one exists). Amendment 4's "both gone" branch stays but cannot be reached in production (a resolved `vault_dir()` exists).

### ADR-SEC-030 — security of the location

- The pointer lives in `%LOCALAPPDATA%` (per-user); folders and a random ID, no secret. Read-side checks never relaxed.
- `.vault-id` binds the folder to this computer's record: a different stick at the same letter is refused, never opened as a new vault.
- Cloud and network targets refused (integrity: sync clients rewrite SQLite mid-transaction; depth: we do not copy ciphertext off the machine). Detection best effort, said so.
- A move copies ciphertext only, no key material; old copies are deleted after the commit and retried until done; "Delete everything" covers a pending cleanup.
- Downgrades unsupported (an older build ignores the pointer). Operating rule as ADR-SEC-029's: close other Zaaheen builds before the first open of this one.

### Tests (first)

- `VAULT_KEEP_FILES` added to `vault_at_rest_sweep.rs`'s allowed set and pinned "declared, never erased" (as the lockfile test in `erasure.rs`).
- `vault_dir()` and L2: existing vs new install, once, under K1, the desktop/keeper race; a keeper-first fresh install and a keeper-first upgrade both set the location up (no pointer → keeper start); the first run on an erased-with-leftovers install finishes the erasure; every invalid pointer and a missing folder / ID file / different ID → `VaultLocationMissing`, nothing created; `.vault-id` survives "Delete everything" and the next start works.
- L4: each refusal and acceptance behind a trait (environment, registry query, file system, canonicalize), including `reg.exe` unavailable.
- L5: every crash point enumerated (ADR-SEC-029's method): exactly one complete vault named by the pointer at every point; recovery deletes only a folder carrying the move ID (or empty at the exact target); M6 never touches `vault_dir()`; a second move and a marker block a move.
- L6: after an erasure on a lost drive; with a key present; never cleans a live folder.
- Relays follow a move and answer a missing location without starting a keeper; the runner resolves the pointer; CLI refuses during a move; models never follow the vault; amendment 5.
- Planted bugs for each rule; an independent code review.

### Founder decisions (session 52, 2026-09-21)

*"agreed with both small decisions partner..."* — **L2:** new installs keep their memories in `%LOCALAPPDATA%\com.zaaheen.app\vault`; existing installs stay unless moved. **L4.7:** exFAT/FAT32 allowed with the one-line note.

### Words still to approve (at build time)

The location screen, the USB note, the refusal reasons, the "couldn't check" note, `VaultLocationMissing`'s screen (desktop and the AI-app message) and "Start again in the default place". Since L-d, also: the "another window is busy" startup message (`vault-tauri/src/lib.rs` `MSG_MEMORIES_BUSY`) and the words for each `MoveOutcome` / `MoveFailure`.

**All approved in session 54 (2026-09-22):** shown to the founder as built, *"yes all good"*. Pinned by `frontend_contract::the_location_screens_say_what_the_founder_approved`, `the_location_notes_are_the_designs_words` and `vault-tauri` `lib::tests::the_location_startup_words_are_the_founders`; the exact text lives in `dist/app.js` (`friendlyLocationError`, `locationNoteLine`, `moveFailureLine`, `outcomeLines`), `dist/index.html` (`screen-location`, `move-panel`, Settings) and `vault-tauri/src/lib.rs` (`format_location_problem_dialog`, `START_AGAIN_*`, `format_start_again_refusal`, `MSG_MEMORIES_BUSY`); the AI-app message is `keeper::relay::MSG_LOCATION_MISSING`, unchanged.

### Implementation record (sessions 52–53) — the build of ADR-105

**L-a, L-b, L-c (session 52).** `vault-app/src/location/` (`mod.rs` resolver and first-run setup, `pointer.rs` the record, `check.rs` L4); every process follows the record (relays re-resolve each round; the maintenance task is path-free); the folder checks. Evidence and file list in `HANDOFF.md` §1 (session 52).

**L-d, the move and its recovery (session 53)** — `vault-app/src/location/moving/`: `mod.rs` (the logic), `fs.rs` (`MoveFs`, the file operations, and `DiskFs`, the real ones). Public surface: `request_move` (L5's first half: refusals, L4, `pending_move`), `run_pending` (a start: the old copy, then a waiting move), `clean_old_copy_now` (also called after "Delete everything"), `MoveOutcome`, `MoveFailure`, `RequestRefusal`, `MAX_ATTEMPTS` = 2. The desktop calls `run_pending` in `setup()` before `location::prepare` and before the key opens; nothing else runs a move.

Decisions made while building it, each within L5's text:

1. **M1 also waits for no window to have the memories open.** The desktop takes no vault lock (only the keeper and maintenance hold `.vault.lock`), so the intent + `.vault.lock` alone would not stop a window that has not finished closing — or a second window — from writing a memory the copy then misses and M6 deletes. M1 therefore also waits, within the same 30-second budget, until `vault.db` can be opened with no sharing (Windows refuses that while any other handle is open; every process that opens the vault keeps its database open for as long as it runs). `location::moving::fs::database_in_use`. The restart overlap (the old window exits as the new one starts) is covered by the wait.
2. **Two outcomes for "not now", told apart.** `Busy`: another process holds the vault's intent (a window moving or erasing) — this window must not open the memories and exits with `MSG_MEMORIES_BUSY`. `Deferred`: the vault could not be had in time (a keeper that did not let go, a maintenance run, a window left open) — nothing is counted, the memories open where they are, and the next start tries again.
3. **The attempt is counted under K1 before M2** (recorded in `pending_move.attempts`), so a crash counts as a failure and a move that crashes the process cannot loop. An M1 that fails counts nothing. A start that finds `attempts` at 2 gives the move up as `Interrupted`.
4. **Recovery runs inside the attempt**, with the vault held and under K1, before the copy (not before M1): a start that cannot take the vault leaves a half-made copy for the next start. Removal order inside a folder carrying the move's ID: `VAULT_ENTRIES` and `.vault-id`, then `.move-id` last (until then the folder is still provably this move's), then `remove_dir`. A failure removes the half-made copy only while the record still names this move. **"Empty" includes one more state:** `.move-id` is written beside its place and renamed in (item 8), so a crash between the two leaves a folder holding only `.move-id.tmp`; a folder holding nothing but that file, with (the start of) this move's ID in it, is removed too — otherwise it would fail both attempts and block that folder for good. Found while writing this record: the in-memory disk first wrote files in one step, so the enumeration could not reach that crash point; it now writes them as the real disk does, in two.
5. **M2 restricts the folder without the `.acl-v1` marker** (`acl::restrict_folder`), so a crash before `.move-id` leaves an empty folder that recovery may remove. The keeper hardens the moved folder again, marker and all, the first time it opens it.
6. **L4 is checked again at the attempt**, and the target must still be the one recorded; a full disk (`Refusal::NotEnoughSpace`, or the OS's "disk full" during the copy) is its own `MoveFailure`.
7. **Copies never overwrite** (`create_new`), are flushed to disk, and are hashed (BLAKE3) as they are read; M4 reads every copy back and compares size and hash. A link anywhere in the vault (a symbolic link or a junction) fails the attempt: a move never follows one out of the vault.
8. **Record writes are flushed before the rename** (`pointer::write_atomic`, also used for `.vault-id` and `.move-id`), so a power cut leaves the old record or the new, never an empty file.
9. **An old copy whose folder cannot be seen is waited for, never forgotten** (a drive that is out still holds a copy readable on this computer), so M6 is retried at every start. An old copy on a drive that never comes back therefore blocks the next move (L5 refuses a move while `pending_cleanup` is set): L-e must offer a way out.
10. **The old folder's lockfiles stay** (ADR-SEC-020: never deleted by name), so a moved-from `Zaaheen Memories` folder keeps its empty lockfiles.
11. **"Delete everything" removes a waiting old copy** once the key is destroyed (`erase_everything_inner`, best effort; still in use → the next start).

**Tests, written first** (33 of 35 ran red on empty bodies; the other two were the probe, already written, and "nothing recorded"). `vault-app --lib -- location::moving` 39: the everyday cases on the real disk (a clean move; refusals; a copy that fails, one that does not match, one that cannot be restricted; the two-attempt limit; a retry; recovery that never touches a folder without this move's ID, removes an empty one, one with the ID half-written (but not a half-written ID that is not this move's) and a half copy, but not a file that is not the vault's; the old copy waiting and removed at the next start; the folder in use never cleaned; the locked order of the steps); the start (nothing recorded, a real move with the real folder permissions, `Busy`, `Deferred` for a held vault and for an open database, the wait for a closing window); **crash enumeration on an in-memory disk** (`moving/model.rs`, ADR-SEC-029's method): every crash point of a move, **every pair** of a move and the start that recovers from it, every point of the old copy's removal and of giving up — after each, the record names one complete vault and the following starts leave the memories in one place only; the real disk crashed at four chosen points. Slowest test 0.75 s. The half-written-ID rule was planted back to its first version and caught by three tests, the enumerations at exactly that crash point among them ("crash after 4: a half-made copy was left"); the full plant run is L-d's share of step 3. `vault-tauri --lib` 92 (+3: the start order, the busy message, the erasure hook); the desktop binary compiles. Full `vault-app --lib` 396 passed.

**L-e, the screens and "Start again" (session 54).** Founder, asked to start: *"yes partner lets go"*; the words, shown as built: *"yes all good"* (above).

- **vault-app** — `location/missing.rs` (NEW): `diagnose` tells the startup's reasons apart (`RecordUnreadable`, `FolderAbsent`, `FolderUnusable`; it asks `resolve` first, so the two can never disagree) and `start_again` is L6. `location/moving/asking.rs` (NEW): `check_request` (L5's refusals and L4 without recording anything — one `decide()` shared with `request_move`, so the check before the confirmation and the request itself cannot differ), `status`, `forget_old_copy`. `keychain::settle_marker_to_start_again` (`mod.rs` + `lifecycle.rs`): L6's key half. `location::display_path`.
- **vault-tauri** — `commands/location.rs` (NEW): `location_status`, `location_check`, `location_move`, `location_forget_old_copy`, with stable codes and every folder shown through `display_path`; `main.rs`: `LocationContext` managed after the start's move, and `when_the_memories_are_missing` (the startup dialogs and L6); `settings.rs`: the folder through `display_path`; the permissions and `dialog:allow-open`; `dist/`: the setup's location step, the move's confirmation, Settings "Move my memories…" and "Stop waiting for the old copy", the home notice.

Decisions made while building it:

1. **The location step comes after the setup's sign-in step,** not "right after the welcome" as L5 reads. The four location commands are gated: they are not on §8.26 §6.4's locked allowlist, and widening it is a founder decision nobody needs to take — a locked computer shows the lock screen, whose export already takes the memories anywhere. The sign-in step is itself "straight after the welcome, before anything gated" (§8.39), so the location step is the first gated step: `ONBOARDING = [welcome, location, connect, memory, maintenance]`, with `signin` second when there is one. Cost, stated: a "Choose another folder" during setup restarts the app while the recall engine's first download may have begun; the download starts again after the restart (`model_fetch` does not resume a partial file).
2. **While a move runs, there is no usable window.** `setup()` runs the move before the vault or the key opens (L-d), and Tauri creates the configured window before the setup hook runs (`tauri-2.11.0/src/app.rs` `setup`), so the window exists but cannot draw while the setup thread copies. A real "moving your memories" screen needs the start restructured (the move after the window is up, the rest of setup after it) — tech debt. Instead, honest words before and during: *"Zaaheen will close, move them, and open again by itself. With many memories this can take a few minutes."* and *"Zaaheen is closing to move your memories. It opens again by itself once they're moved."* A second window opened meanwhile gets `MSG_MEMORIES_BUSY`.
3. **The move is recorded, then the app restarts** (`location_move`: `request_move`, a refusal returned, only then `AppHandle::request_restart`; pinned by `the_app_restarts_only_after_the_move_is_recorded`). Tauri's restart spawns the new process before the old one exits; M1's wait for an open `vault.db` (L-d decision 1) covers the overlap.
4. **One confirmation panel, moved** between the setup's step and Settings (a single DOM node, so one set of rules). In the setup, the step's own buttons step aside while it is open. When the cloud check could not run (L4.3), "Move them here" stays off until the person ticks "No cloud app syncs this folder."
5. **A setup interrupted by a move resumes at the location step:** `mv_resume_location` (which of the two step lists) is set just before `location_move`, and only during setup; the start after the restart waits for the lock, then shows the location step with what happened, which clears the flag. A refused move clears it at once.
6. **What a start did reaches the screen once:** `LocationContext` keeps the start's `MoveOutcome`; `location_status` returns it (`Busy` never gets there — that window exits); the setup's step or, otherwise, a notice on the home screen says it, once per start.
7. **L6 in the desktop** (`main.rs` `when_the_memories_are_missing`): `prepare` failing with a location error → `diagnose`. Only `FolderAbsent` is offered "Start again", then asked again ("Start again with no memories? The memories on that drive can't be opened from here. …" — L6's confirmation); Close or Cancel exits with nothing changed. Every other reason gets its own words and the support address. Then `start_again` under K1: offered only for `FolderAbsent`; refused if the default folder holds keyed data; **the erased marker settled before anything is written** (a crash after it leaves "Start again" on offer at the next start — the reverse order could leave a record naming the new folder while an untrusted marker blocks every new key, message 3 for good); then the default folder, a **fresh** `.vault-id` (never one already there), and the record with no pending fields, last. **Nothing is deleted.** Residual, stated: a half-made copy a crashed move left on another drive stays there once `pending_move` is cleared (ciphertext under this computer's key).
8. **L6's key half** (`lifecycle::settle_marker_to_start_again`): no marker → nothing decided, and no key store opened; a marker (trusted or not — which folder it names no longer matters once the key is gone) is removed only when main **and** the spare read "no entry" after R1; a key or a spare present → the marker stays and so does the key; any failure decides nothing. It never writes or deletes a key, so ADR-SEC-029's "no key is created while a marker exists" and "marker present ⇒ no live key anywhere" both still hold.
9. **"Stop waiting for the old copy"** (`forget_old_copy`, L-d decision 9's way out): only for a folder that cannot be seen — exactly what a start waits for rather than removes; a copy that can be seen is removed at the next start, never forgotten. Only `pending_cleanup` is cleared, under K1; **nothing is removed**, and the folder is named back so the person is told they may delete it if it turns up (encrypted, opens only on this computer).
10. **Folders are shown as the person reads them** (`display_path`: `\\?\D:\…` → `D:\…`; anything else, never recorded, as it is), always as text in the webview.
11. **Moving back into the usual place is refused,** because L4.2 refuses anything inside `%LOCALAPPDATA%\com.zaaheen.app`, the new-install folder included: a person who moved to a USB stick brings the memories back to a folder of their own on this computer instead. Raised with the founder: *"leave it for now and go ahead"*. A "Move back to the usual place" choice needs an L4 exception and its own tests; add it if beta testers ask.
12. **"Choose another folder…" sits right under the folder, inside its box, as a button** (founder's walk-through, session 55: *"it should be right below where we show the current location so its easily located by user"*), no longer a faint link under Continue. It steps aside with Continue while the confirmation is open (decision 4). Guard: `choose_another_folder_is_right_under_the_folder`; driven in the harness (picker → confirmation → Cancel).

**Tests, written first.** `vault-app`: 28 new tests ran red against empty bodies (4 more passed on them, each a refusal the empty body also gave), then `--lib -- location start_again` 100 passed: `missing_tests` (13), `moving::asking_tests` (11), `keychain::tests::start_again` (7), `display_path`. `vault-tauri`: `commands::location` (7, including every code having words in the app), the startup texts and L6's order in `main.rs`, the location guards in `frontend_contract`; `--lib` 103 and `--test frontend_contract` 42 at the first run, plus the approved-wording pins and two tightened guards added before the plants. **Driven in the browser harness** (`MemoryVault-artifacts\ui-harness`, its stand-in extended for the four commands and the folder picker): a new install through sign-in, a move, the restart and the resumed setup; the refusals, a cancelled picker, the cloud tick, a move refused at the last moment; Settings with an old copy connected and not, "Stop waiting" (asks first), a waiting move, a move from Settings; the home notice for all eight outcomes, once per start; a resumed setup that meets the lock (no location command while locked); a build without sign-in (step 2 of 5). No page errors. Plants and the independent review: step 3.

### ADR-105 amendment 1 + ADR-SEC-030 amendment 1 — L-f, the "Moving your memories" screen (session 55, 2026-09-22)

**Founder, in the session-55 walk-through:** told that a start which moves the memories shows no window while it copies (L-e decision 2), *"I agree with you to have the screen or lets say an animation which shows memories moving something which tells users memories are moving or a popup which shows progress of memories moving"*; offered "before the public launch": *"partner lets build the page now.. why delay.."*.

**What was wrong.** `setup()` runs the move (M1–M6) before anything opens the vault or the key, and Tauri 2.11 creates the window before the setup hook but runs no event loop until `setup()` returns (`tauri-2.11.0/src/app.rs`). So a window exists and cannot draw for the whole copy, and for up to `TAKE_WAIT` (30 s) before it while an AI app lets go. Honest words before the restart were the stopgap.

**Decision.**
1. **Only a start with a move waiting changes.** `moving::waiting_move(&homes)` reads the record: `pending_move` set → that start takes the new path; anything else (no record, a record that cannot be read, an old copy still to remove) → today's path, unchanged, in `setup()`. An old copy's removal stays on today's path because a drive that is out leaves it waiting at every start, and a screen flashing up at every start for it would be wrong.
2. **The new path:** `setup()` does steps 0–2 as today, manages `Startup` (moving), spawns one named thread, and returns, so the window draws and the page loads. The thread runs `run_pending_reporting` (the same `run_pending` with a progress sink), then **the rest of today's `setup()`, unchanged and in the same order** (Busy → `MSG_MEMORIES_BUSY`; `prepare`, L6; the key; `Application::new`; every `manage`), extracted into one function, `open_the_vault`, which both paths call. Its **last** statement marks `Startup` ready, after every piece of state is managed. If the thread cannot be spawned, the same work runs inline on the setup thread (today's behaviour, no screen).
3. **The order the key and the vault depend on does not change:** on both paths the move finishes before `prepare`, the key or `Application::new` run (ADR-SEC-029, L5). Pinned by source tests on both paths.
4. **The page waits before it asks anything.** At load, before `enterApp` (the lock's first question), it asks `startup_state` until the answer is `ready`, showing the moving screen meanwhile; a normal start answers `ready` at once and nothing is shown. If `startup_state` itself fails, it is asked again for five seconds, and only then does the page carry on as today (every gated command still asks the guard; nothing is bypassed). Tauri answers a command whose state is not yet managed with an error, never a panic (`tauri-2.11.0/src/state.rs` `from_command`), and the page asks nothing else until `ready`.
5. **Progress is real, never animated for its own sake.** `MoveProgress` (one lock, so a reader never sees one phase's count under another's name) holds the phase — `waiting` (M1: AI apps letting go, a window closing), `copying` (M3), `checking` (M4), `finishing` (M5–M6) — and bytes done of the total for the two phases that read every byte. `DiskFs` reports each 1 MiB chunk it copies or reads back; the in-memory crash model reports nothing (the trait's reporting method is a no-op by default), so the crash enumeration is untouched. One bar: copying fills the first half, checking the second (the same bytes read twice), with the phase in words and "340 MB of 1.2 GB" beside it. Then "opening" while the thread runs `open_the_vault`.
6. **Closing the window during a move** ends the process as it does today; the move's crash safety already covers every point (L-d), the attempt is counted, and the memories stay where they were. The screen says so rather than trapping the person.
7. **The words** (founder-approved, session 55, shown running in the browser preview: *"yes all good"*): **Moving your memories** · *"Getting ready to move them."* · *"Copying them to {folder}."* · *"Checking that every memory arrived safely."* · *"Almost done."* · *"Opening your memories."* · *"{n} MB of {n} GB copied"* / *"… checked"* · the note, as approved: *"Please keep Zaaheen open until this finishes. If it closes, your memories stay safe where they were, and the move tries again next time."* — shortened after the review (below) to end at "where they were."

**ADR-SEC-030 amendment 1 — one command open before the lock.** `startup_state` is callable before the entitlement guard exists (it is managed after the move), so it joins `OPEN_COMMANDS` — **a widening of §8.26 §6.4's allowlist, founder-approved** (session 55, shown what it answers and that it never touches a memory: *"yes all good"*). It is given `Startup` and nothing else: no vault, no account, no key. It answers the stage, the phase, two byte counts and the folder being moved to (through `display_path`, as every screen shows folders) — explicit serialization, nothing else (BRD §11.7.2). It takes no argument (§11.7.1: nothing to validate). A locked computer learns from it only that its own memories are being moved to a folder its own person chose.

**Independent review of the design** (one adversarial reviewer, read-only, against the code and the Tauri 2.11 / tauri-plugin-dialog 2.5.0 sources): **nothing at BLOCKER or MAJOR.** Confirmed: every open command needs state only the thread manages, so nothing can open the vault or the key, erase, or start a second move before the move ends; `manage` and `block_on` from the thread are safe; closing the window mid-move exits the process and no point removes the old copy without a verified, committed new one; progress changes no behaviour. Four MINOR, all fixed before any build:
- **The note was not always true** ("the move tries again next time" is false for a second attempt, which gives up; and from "Almost done." the memories are already moved). Now shown only while waiting, copying or checking, and shortened to *"Please keep Zaaheen open until this finishes. If it closes, your memories stay safe where they were."* (re-shown to the founder: *"yes ok"*).
- **A failed `startup_state` carried straight on** to the lock, which (its state not yet managed) would have shown "couldn't confirm your subscription" mid-move. Now asked again for five seconds first; outside the app (no bridge) it returns at once.
- **A panic on the thread** (development builds; release aborts) would have left the page on "Opening your memories." for ever. The thread now ends the app as a failed setup would.
- **Dialogs from the thread could open behind the window.** From the move's thread they are now set in front of the main window (`startup_dialog`); from the setup thread never, because there the window's own thread is the one waiting for the answer, and a dialog owned by it could deadlock (found while fixing this one).
- Notes kept: `std::process::exit` from the thread skips Tauri's exit cleanup (as the setup thread's did); `waiting_move` reads the record without K1, so a second window can change only which screen shows, never the move (`run_pending` reads again under its own locks).

**Tests, written first.** vault-app: the phases in order with their totals and every byte counted once per phase on a real move; bytes never past the total; `waiting_move` only for `pending_move`; a move with a progress sink ends exactly as one without. vault-tauri: `startup_state`'s answers and that its module names no vault or account type; both `setup()` paths finish the move before `open_the_vault`, whose order is prepare → key → `Application::new` → ready last; the allowlist pin; its permission; the page asks `startup_state` before `enterApp`, carries on when it fails, and the screen's words (founder-approved) and reduced motion.

**Implementation record (session 56, 2026-09-22) — L-f compiled and proven.** The first build was clean under `-D warnings`. Two test defects, no code defect: `only_a_move_waiting_is_a_waiting_move` wrote a record whose move ID was not an ID (the record's own check refused it), fixed; and planning the plants showed "every byte counted once per phase" could not see a chunk counted twice, because the reading's cap at the total hides it. `MoveProgress` now keeps, for tests only, every byte each phase was told of before the cap, and the test asserts each phase was told exactly its total. **Independent code review** (read-only, against the Tauri 2.11 / tauri-plugin-dialog 2.5.0 sources, including the dialog's inline-versus-queued dispatch on each thread): nothing at BLOCKER, MAJOR or MINOR. **Planted bugs, each caught by the test meant for it:** ready before the last `manage`; `startup_state` gated; the vault opened before the move; a copied chunk counted twice (caught only by the new count: told 17,092 of 8,546); a checked chunk never counted; the note shown after the switch; the page asking the lock before the start is ready. **ADR-105's remaining plants from session 54, T06–T14, all caught: 47/47 for L-a–L-e.** Evidence: vault-app `--lib` 454, vault-tauri `--lib` 113, `--bins`, `frontend_contract` 59, with no plant applied.
