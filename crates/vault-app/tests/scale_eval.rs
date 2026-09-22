//! Scale correctness harness — "prove correctness at scale" arc (T0.3.x, 2026-06-04).
//!
//! The correctness core (recall-first read ADR-066 + search ADR-067, the -2.5
//! no-signal floor, honest abstention) is proven STRUCTURALLY on a ~12-fact toy
//! vault. This harness asks the next question: **does it still hold when the
//! right answer must beat ~80 unrelated distractor facts?**
//!
//! It seeds the `scale_eval.json` planted facts (synonym-gap recall targets +
//! their near-miss must-excludes + genuine no-signal questions) into a shared
//! pool, pads to `scale` (100) with the deterministic t029 distractor generator,
//! then runs the REAL production read + search paths per query and prints a
//! scorecard:
//!
//! - **Read abstention confusion matrix** — false-abstain (the A7 cliff) vs
//!   false-answer (over-correction). This is the headline number.
//! - **Read recall + rank** — did the target surface, and where did it rank
//!   relative to its near-miss distractor.
//! - **Search recall@k** — recall-first hybrid (no abstain gate), top-5 / top-20.
//! - **Near-miss leak diagnostic** — recall-first read returns candidates and
//!   trusts the agent for precision, so a returned near-miss is NOT a hard fail
//!   here; we report it (and its rank vs the target) as a precision signal.
//!
//! Characterization only — NO hard pass/fail gate yet. We read these numbers,
//! calibrate the -2.5 floor at scale, THEN pin gates (per the "measure first,
//! anchor on measured not projected" discipline). Reading the exact reranker
//! margins (to tune the floor) is a deliberate pass-2 follow-up: this pass uses
//! only what the production read/search responses expose, so it measures the
//! floor's *behaviour* at scale, not its internal scores.
//!
//! ## Why in-process (no MCP / no Claude Desktop)
//!
//! `adapter.read` / `adapter.search` are the same entry points the MCP
//! `memory_read` / `memory_search` tools call, so running them directly
//! reproduces live behaviour (reranker + the -2.5 floor) deterministically.
//!
//! ## Running
//!
//! ```text
//! cargo test -p vault-app --test scale_eval -- --ignored --nocapture
//! ```
//!
//! Needs the bge-small-en-v1.5 AND qwen3-reranker-0.6b-seq-cls fixtures
//! (run scripts/setup-dev-env.{sh,ps1}). The reranker is load-bearing: it is
//! the read relevance authority (ADR-059) and holds the -2.5 no-signal floor
//! (ADR-066) we are here to stress at scale. No Phi-4 needed (read is
//! deterministic per ADR-052).
//!
//! ## macOS deferral (ADR-033)
//!
//! Disabled on macOS — BGE/reranker transitively load ORT which SIGABRTs at
//! process exit on macOS. Linux + Windows cover the path.

#![cfg(not(target_os = "macos"))]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;
use tempfile::TempDir;

use vault_app::{AppConfig, Application};
use vault_core::{Boundary, MemoryType, NewMemory};
use vault_embedding::{BgeSmallProvider, EmbeddingProvider};
use vault_mcp::Adapter;
use vault_retrieval::structured_read_pipeline::DEFAULT_MAX_CANDIDATES;
use vault_retrieval::{ReadQuery, RetrievalOptions, RetrievalQuery};
use vault_storage::SqlCipherKey;

// ---------------------------------------------------------------------------
// Fixture path resolution (mirrors read_no_keyword_overlap.rs — test files
// don't share modules; these are 3 trivial fns).
// ---------------------------------------------------------------------------

const BGE_FIXTURE_REL: &str = "../vault-embedding/test-fixtures/bge-small-en-v1.5";
const RERANK_FIXTURE_REL: &str = "../vault-embedding/test-fixtures/qwen3-reranker-0.6b-seq-cls";

fn fixture(rel: &str, name: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push(rel);
    p.push(name);
    assert!(
        p.exists(),
        "missing fixture {p:?} — run scripts/setup-dev-env.(sh|ps1) first"
    );
    p
}

#[cfg(target_os = "windows")]
fn ort_lib() -> PathBuf {
    fixture(BGE_FIXTURE_REL, "onnxruntime.dll")
}
#[cfg(target_os = "linux")]
fn ort_lib() -> PathBuf {
    fixture(BGE_FIXTURE_REL, "libonnxruntime.so")
}

fn scale_fixture_path() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/scale_eval.json");
    p
}

// ---------------------------------------------------------------------------
// Fixture model (parsed from serde_json::Value; only the fields the harness
// consumes — _* annotations are ignored).
// ---------------------------------------------------------------------------

struct PlantedFact {
    id: String,
    boundary: String,
    content: String,
    source_agent: Option<String>,
    confidence: f32,
}

struct EvalQuery {
    id: String,
    kind: String,
    /// `_phrasing` tag (natural / plain / idiom / keyword) — buckets recall by
    /// how the question is worded, so the scorecard exposes phrasing-sensitive
    /// recall (Thread-2 Gap 2: "call home" misses, "live" hits). Absent → "natural".
    phrasing: String,
    query_text: String,
    must_surface: Vec<String>,
    must_exclude: Vec<String>,
    abstain: bool,
}

fn str_vec(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn parse_planted(v: &Value) -> Vec<PlantedFact> {
    v.get("planted")
        .and_then(Value::as_array)
        .expect("fixture must have a `planted` array")
        .iter()
        .map(|p| PlantedFact {
            id: p["id"].as_str().expect("planted.id").to_string(),
            boundary: p["boundary"]
                .as_str()
                .expect("planted.boundary")
                .to_string(),
            content: p["content"].as_str().expect("planted.content").to_string(),
            source_agent: p
                .get("source_agent")
                .and_then(Value::as_str)
                .map(String::from),
            confidence: p
                .get("confidence")
                .and_then(Value::as_f64)
                .map(|f| f as f32)
                .unwrap_or(0.9),
        })
        .collect()
}

fn parse_queries(v: &Value) -> Vec<EvalQuery> {
    v.get("queries")
        .and_then(Value::as_array)
        .expect("fixture must have a `queries` array")
        .iter()
        .map(|q| {
            let e = &q["expect"];
            EvalQuery {
                id: q["id"].as_str().expect("query.id").to_string(),
                kind: q
                    .get("_kind")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                phrasing: q
                    .get("_phrasing")
                    .and_then(Value::as_str)
                    .unwrap_or("natural")
                    .to_string(),
                query_text: q["query_text"]
                    .as_str()
                    .expect("query.query_text")
                    .to_string(),
                must_surface: str_vec(e, "must_surface"),
                must_exclude: str_vec(e, "must_exclude"),
                abstain: e.get("abstain").and_then(Value::as_bool).unwrap_or(false),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Distractor generator — ported verbatim-in-spirit from
// `crates/vault-retrieval/examples/t029_scale_1000_retrieval_diagnostic.rs`.
// Duplicated rather than shared via a feature-flagged module to avoid a
// CI-matrix change for one extra consumer (rule of three: extract if a third
// consumer appears). Deterministic (SplitMix64, fixed seed) so every run is
// byte-identical.
// ---------------------------------------------------------------------------

const DISTRACTOR_SEED: u64 = 0x5CA1_EACD_EED0;

struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn pick<'a, T>(&mut self, slice: &'a [T]) -> &'a T {
        &slice[(self.next_u64() as usize) % slice.len()]
    }
}

const DISTRACTOR_BOUNDARIES: &[&str] = &["work", "personal", "tools"];

const TOPICS: &[&str] = &[
    "office plant care schedule",
    "cafeteria menu rotation",
    "fire drill calendar",
    "lobby art swap",
    "supply closet inventory",
    "parking permit refresh",
    "team birthday lunch coordination",
    "office library book donation",
    "stationery reorder cycle",
    "coffee machine cleaning rota",
    "wellness room booking process",
    "guest visitor badge handoff",
    "elevator maintenance window",
    "rooftop garden volunteer rota",
    "company swag inventory check",
];

const PEOPLE: &[&str] = &[
    "Olivia", "Priya", "Marcus", "Diego", "Sarah", "Jenna", "Tom", "Aisha", "Marco", "Lena",
    "Kenji", "Riya", "Felix", "Maya", "Sam",
];

const DAYS: &[&str] = &[
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "this morning",
    "yesterday afternoon",
];

const MONTHS: &[&str] = &[
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "August",
    "September",
    "October",
    "November",
    "December",
];

const CITIES: &[&str] = &[
    "Austin",
    "Denver",
    "Portland",
    "Seattle",
    "Boston",
    "Chicago",
    "Atlanta",
    "Raleigh",
    "Boise",
    "Salt Lake City",
];

const VENDORS: &[&str] = &[
    "Acme Travel",
    "BlueSky Bookings",
    "Mercury Logistics",
    "Northwind Catering",
    "Lighthouse Print",
    "Vertex Furniture",
    "Cascade Cleaning",
    "Summit Coffee Supply",
];

const AMOUNTS: &[&str] = &[
    "$1,200/mo",
    "$3,400/year",
    "$450 one-time",
    "$78/seat",
    "$6,500 annual",
    "$220/mo",
    "$15K total",
    "$320/quarter",
];

const ACTIONS: &[&str] = &[
    "posted the signup sheet",
    "refilled the supply bins",
    "wiped down the meeting room whiteboards",
    "stocked the snack pantry",
    "swapped the lobby flowers",
    "labelled the storage crates",
    "watered the office plants",
    "ordered new lanyards for visitors",
];

const DISTRACTOR_TEMPLATES: &[&str] = &[
    "{topic} note: {person} led the chat on {day}; {action}.",
    "Recap of {topic} held {day} — {person} shared the quarterly facilities update.",
    "{topic} session in {city} planned for {month}; {person} coordinating logistics.",
    "Booked {vendor} for the {topic} event in {city}, rate approximately {amount}.",
    "{person} updated the office bulletin board with the {month} {topic} schedule.",
    "{person} is moving the {topic} agenda to {day} so the team can prep beforehand.",
    "Travel arrangement for {person} to {city} in {month} via {vendor}; {action}.",
    "{topic} signup sheet posted by {person} on {day}; first {action}.",
    "Team event in {city} on {day} — {person} {action} after the wrap-up.",
    "Office news: {topic} confirmed for {month}; coordinator is {person}.",
];

fn render_template(template: &str, rng: &mut SplitMix64) -> String {
    let mut out = template.to_string();
    let replacements: &[(&str, &[&str])] = &[
        ("{topic}", TOPICS),
        ("{person}", PEOPLE),
        ("{day}", DAYS),
        ("{month}", MONTHS),
        ("{city}", CITIES),
        ("{vendor}", VENDORS),
        ("{amount}", AMOUNTS),
        ("{action}", ACTIONS),
    ];
    for (placeholder, pool) in replacements {
        while out.contains(*placeholder) {
            let pick = rng.pick(pool);
            out = out.replacen(*placeholder, pick, 1);
        }
    }
    out
}

struct Distractor {
    boundary: &'static str,
    content: String,
}

fn generate_distractors(count: usize, seed: u64) -> Vec<Distractor> {
    let mut rng = SplitMix64::new(seed);
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let template = rng.pick(DISTRACTOR_TEMPLATES);
        let boundary = *rng.pick(DISTRACTOR_BOUNDARIES);
        let content = render_template(template, &mut rng);
        out.push(Distractor { boundary, content });
    }
    out
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "real BGE + Qwen3-Reranker over a ~100-fact vault (slow); run with --ignored --nocapture"]
async fn scale_correctness_eval() {
    // ---- build one real read+search stack over a temp vault ----
    let tmp = TempDir::new().expect("tempdir");
    let config = AppConfig {
        metadata_path: tmp.path().join("vault.db"),
        vector_dir: tmp.path().join("lance"),
        graph_path: tmp.path().join("graph.duckdb"),
        key: SqlCipherKey::new("scale-eval-key"),
        model_path: fixture(BGE_FIXTURE_REL, "model.onnx"),
        tokenizer_path: fixture(BGE_FIXTURE_REL, "tokenizer.json"),
        ort_lib_path: ort_lib(),
        at_rest_key: zeroize::Zeroizing::new([0u8; 32]),
        qwen_model_path: None,
        phi4_model_path: None,
        // Load-bearing: the reranker is the read relevance authority (ADR-059)
        // and holds the -2.5 no-signal floor (ADR-066) this harness stresses.
        rerank_model_path: Some(fixture(RERANK_FIXTURE_REL, "model.onnx")),
        rerank_tokenizer_path: Some(fixture(RERANK_FIXTURE_REL, "tokenizer.json")),
    };
    let app = Application::new(&config)
        .await
        .expect("Application::new must compose the read stack (BGE + reranker)");
    let _shutdown = app.spawn_retry_worker(); // spawn the cascading retry worker
    let adapter = app.adapter();

    // ---- load + parse fixture ----
    let bytes = std::fs::read(scale_fixture_path()).expect("read scale_eval.json");
    let value: Value = serde_json::from_slice(&bytes).expect("parse scale_eval.json");
    let planted = parse_planted(&value);
    let queries = parse_queries(&value);
    // Scale is the fixture default, overridable via `SCALE_EVAL_N` so the same
    // harness runs the 100 → 1k → 10k → 100k ladder without mutating the
    // committed fixture.
    let scale = std::env::var("SCALE_EVAL_N")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or_else(|| value.get("scale").and_then(Value::as_u64).unwrap_or(100) as usize);
    let authorized: Vec<Boundary> = value
        .get("authorized_boundaries")
        .and_then(Value::as_array)
        .expect("authorized_boundaries")
        .iter()
        .filter_map(Value::as_str)
        .map(|b| Boundary::new(b).expect("authorized boundary valid"))
        .collect();

    // ---- seed planted facts; record planted-id -> uuid ----
    let mut id_map: BTreeMap<String, String> = BTreeMap::new();
    for pf in &planted {
        let nm = NewMemory {
            content: pf.content.clone(),
            memory_type: MemoryType::Semantic,
            boundary: Boundary::new(pf.boundary.as_str()).expect("planted boundary valid"),
            source_agent: pf.source_agent.clone(),
            confidence: pf.confidence,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        };
        let uuid = adapter
            .write(nm)
            .await
            .expect("seed planted write")
            .to_string();
        id_map.insert(pf.id.clone(), uuid);
    }

    // ---- pad to `scale` with deterministic distractors ----
    let distractor_count = scale.saturating_sub(planted.len());
    for d in generate_distractors(distractor_count, DISTRACTOR_SEED) {
        let nm = NewMemory {
            content: d.content,
            memory_type: MemoryType::Semantic,
            boundary: Boundary::new(d.boundary).expect("distractor boundary valid"),
            source_agent: Some("scale-distractor".into()),
            confidence: 0.9,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        };
        adapter.write(nm).await.expect("seed distractor write");
    }
    let total = planted.len() + distractor_count;
    println!(
        "\n================ SCALE CORRECTNESS EVAL ================\nseeded {} facts ({} planted + {} distractors), scale target {}\n",
        total,
        planted.len(),
        distractor_count,
        scale
    );

    // ---- drain poll: wait until EVERY vector has landed in LanceDB ----
    // CRITICAL — this was a greenwash bug (fixed 2026-06-09, Thread-2 Gap 2).
    // The OLD poll broke as soon as the planted Rivian fact was *searchable*, but
    // the keyword (BM25) channel finds a fact from SQLite BEFORE its vector is
    // written to LanceDB (the same trap §1 documents for the live seeder). At 1k
    // the cascade takes ~17 min to drain, so "Rivian searchable" fired at ~0s
    // while the 978 distractor VECTORS were still absent — the query pass then ran
    // against a near-empty vector store with almost no semantic competition, and
    // phrasing-sensitive recall (Porto vs travel-noise) looked artificially
    // perfect. The only ground truth is the vector-store row count. Poll it via a
    // FRESH read-only handle each tick (a held handle pins the version it opened
    // at and never sees the worker's progress). Mirrors seed_live_vault's drain.
    use vault_storage::{LanceVectorStore, VectorStore};
    let mut drained = false;
    // 2s ticks; generous scaled cap (1k drain ≈ 17 min ≈ 510 ticks).
    let max_attempts = (scale * 4).max(600);
    for attempt in 0..max_attempts {
        let probe =
            LanceVectorStore::open_with_at_rest_key(&config.vector_dir, 384, &config.at_rest_key)
                .await
                .expect("open vector count probe");
        let n = probe.count(None).await.expect("vector count probe");
        drop(probe);
        if n >= total {
            println!(
                "all {n}/{total} vectors drained into LanceDB after ~{}s\n",
                attempt * 2
            );
            drained = true;
            break;
        }
        if attempt % 15 == 0 {
            println!(
                "  draining... {n}/{total} vectors (~{}s elapsed)",
                attempt * 2
            );
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    assert!(
        drained,
        "vectors did not fully drain into LanceDB within {}s",
        max_attempts * 2
    );

    // ---- per-query scoring ----
    // Read abstention confusion matrix.
    let mut correct_answer = 0usize; // expect answer, got answer
    let mut correct_abstain = 0usize; // expect abstain, got abstain
    let mut false_abstain = 0usize; // expect answer, got abstain  (THE CLIFF)
    let mut false_answer = 0usize; // expect abstain, got answer   (over-correction)
                                   // Recall + rank accounting (recall queries only).
    let mut read_surface_pass = 0usize;
    let mut read_surface_total = 0usize;
    let mut read_target_top1 = 0usize;
    let mut search_top5 = 0usize;
    let mut search_top20 = 0usize;
    let mut near_miss_leaks = 0usize;
    // Per-phrasing recall breakdown (Thread-2 Gap 2): phrasing tag ->
    // (read-surfaced, search-top20, total targets). The headline number for
    // phrasing-sensitive recall — "idiom 4/10 vs plain 10/10" is the bug made
    // visible; later it grades the query-expansion fix.
    let mut by_phrasing: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();

    println!("---------------- PER-QUERY ----------------");

    for q in &queries {
        // Resolve a planted-id reference to the uuid it was written as.
        let resolve = |key: &str| -> Option<String> { id_map.get(key).cloned() };

        // ── READ ──────────────────────────────────────────────────────────
        let resp = adapter
            .read(ReadQuery {
                query_text: q.query_text.clone(),
                authorized_boundaries: authorized.clone(),
            })
            .await
            .expect("read must not error");
        let read_ids: Vec<&str> = resp
            .relevant_facts
            .iter()
            .map(|f| f.memory_id.as_str())
            .collect();

        match (q.abstain, resp.abstain) {
            (false, false) => correct_answer += 1,
            (true, true) => correct_abstain += 1,
            (false, true) => false_abstain += 1,
            (true, false) => false_answer += 1,
        }

        // ── SEARCH ────────────────────────────────────────────────────────
        let hits = adapter
            .search(RetrievalQuery {
                query_text: q.query_text.clone(),
                authorized_boundaries: authorized.clone(),
                max_results: 20,
                options: RetrievalOptions::default(),
            })
            .await
            .expect("search must not error");
        let search_ids: Vec<String> = hits.iter().map(|h| h.memory.id.0.to_string()).collect();

        // Rank of a uuid in the read response / search response (1-based).
        let read_rank = |uuid: &str| read_ids.iter().position(|r| *r == uuid).map(|p| p + 1);
        let search_rank = |uuid: &str| search_ids.iter().position(|r| r == uuid).map(|p| p + 1);

        // ── recall-query scoring ──
        let mut detail = String::new();
        for key in &q.must_surface {
            read_surface_total += 1;
            let bucket = by_phrasing.entry(q.phrasing.clone()).or_insert((0, 0, 0));
            bucket.2 += 1;
            if let Some(uuid) = resolve(key) {
                let rr = read_rank(&uuid);
                let sr = search_rank(&uuid);
                if rr.is_some() {
                    read_surface_pass += 1;
                    bucket.0 += 1;
                }
                if rr == Some(1) {
                    read_target_top1 += 1;
                }
                if let Some(r) = sr {
                    if r <= 5 {
                        search_top5 += 1;
                    }
                    if r <= 20 {
                        search_top20 += 1;
                        bucket.1 += 1;
                    }
                }
                detail.push_str(&format!(
                    "\n    target {key}: read_rank={} search_rank={}",
                    rr.map(|r| r.to_string())
                        .unwrap_or_else(|| "MISSING".into()),
                    sr.map(|r| r.to_string())
                        .unwrap_or_else(|| "MISSING".into()),
                ));
            } else {
                detail.push_str(&format!("\n    target {key}: UNRESOLVED"));
            }
        }
        for key in &q.must_exclude {
            if let Some(uuid) = resolve(key) {
                if let Some(r) = read_rank(&uuid) {
                    near_miss_leaks += 1;
                    detail.push_str(&format!(
                        "\n    near-miss {key} LEAKED into read at rank {r} (recall-first: agent's call, not a hard fail)"
                    ));
                }
            }
        }

        let verdict = match (q.abstain, resp.abstain) {
            (false, false) => "answer ✅",
            (true, true) => "abstain ✅",
            (false, true) => "FALSE-ABSTAIN ❌ (cliff)",
            (true, false) => "FALSE-ANSWER ❌ (over-correction)",
        };
        println!(
            "[{}] ({}) {:?}\n    expect abstain={} got abstain={} → {} | read returned {} fact(s), search returned {}{}",
            q.id,
            q.kind,
            q.query_text,
            q.abstain,
            resp.abstain,
            verdict,
            resp.relevant_facts.len(),
            hits.len(),
            detail,
        );
        // Print the EXACT facts the vault hands the agent — this is the agent's
        // input for the live "what does Claude say?" test (Decision A, the
        // salary->job / cat->dog no-answer cases especially).
        for (i, f) in resp.relevant_facts.iter().enumerate() {
            println!("        fact[{i}]: {:?}", f.fact);
        }
    }

    // ---- aggregate scorecard ----
    println!("\n---------------- SCORECARD ----------------");
    println!("READ abstention confusion matrix:");
    println!("  correct-answer  (expect answer, got answer) : {correct_answer}");
    println!("  correct-abstain (expect abstain, got abstain): {correct_abstain}");
    println!("  FALSE-ABSTAIN   (expect answer, got abstain) : {false_abstain}   <-- the A7 cliff");
    println!("  FALSE-ANSWER    (expect abstain, got answer) : {false_answer}   <-- over-correction (the -2.5 floor's job at scale)");
    println!("READ recall  : {read_surface_pass}/{read_surface_total} targets surfaced; {read_target_top1}/{read_surface_total} at read rank 1");
    println!("SEARCH recall: {search_top5}/{read_surface_total} @top-5 ; {search_top20}/{read_surface_total} @top-20");
    println!("near-miss leaks into read: {near_miss_leaks} (diagnostic, not a gate)");
    println!("\nRECALL BY PHRASING (Thread-2 Gap 2 — phrasing-sensitive recall):");
    for (phrasing, (read_surf, search20, total)) in &by_phrasing {
        println!(
            "  {phrasing:8}: read {read_surf}/{total} surfaced ; search {search20}/{total} @top-20"
        );
    }
    println!("  (a gap between phrasings = recall depends on wording → query-expansion territory)");
    println!("===========================================\n");

    // Characterization assert — every query scored exactly once. Threshold
    // gates come in a later commit once we have calibrated the -2.5 floor at
    // scale against this scorecard.
    let scored = correct_answer + correct_abstain + false_abstain + false_answer;
    assert_eq!(
        scored,
        queries.len(),
        "every fixture query must be scored exactly once"
    );
}

// ---------------------------------------------------------------------------
// Fast diagnostic — subject-frame retrieval-depth probe.
//
// The full scorecard (2026-06-04) showed the subject-LESS cello fact
// ("Plays the cello…") falling out of BOTH read and search top-20 at scale=100:
// BGE-small embeds the raw text and ranks it below the DEFAULT_MAX_CANDIDATES=20
// candidate cap, so the reranker (the relevance authority, which DOES apply
// DOC_SUBJECT_FRAME) never sees it. This probe isolates the question that picks
// the fix: where does each recall target rank in the BGE pool RAW vs with the
// "The user — " subject frame applied to the DOCUMENT embedding? If framing
// lifts the deep targets above the cap, the fix is to frame the retrieval
// embedding (not just the reranker's). Pure cosine — no reranker, no DB, no
// cascade — so it runs in a couple of minutes, de-risking the expensive
// scorecard re-run.
//
// cargo test -p vault-app --test scale_eval subject_frame_depth_probe -- --ignored --nocapture
// ---------------------------------------------------------------------------

/// Cosine similarity. BGE outputs are L2-normalised; we normalise defensively.
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "real BGE subject-frame retrieval-depth probe; run with --ignored --nocapture"]
async fn subject_frame_depth_probe() {
    const FRAME: &str = "The user — ";

    let probe = BgeSmallProvider::open(
        &fixture(BGE_FIXTURE_REL, "model.onnx"),
        &fixture(BGE_FIXTURE_REL, "tokenizer.json"),
        &ort_lib(),
    )
    .expect("open BGE probe");

    let bytes = std::fs::read(scale_fixture_path()).expect("read scale_eval.json");
    let value: Value = serde_json::from_slice(&bytes).expect("parse scale_eval.json");
    let planted = parse_planted(&value);
    let queries = parse_queries(&value);
    let scale = value.get("scale").and_then(Value::as_u64).unwrap_or(100) as usize;

    // Pool = planted contents + generated distractors (same as the harness).
    let mut pool: Vec<String> = planted.iter().map(|p| p.content.clone()).collect();
    let distractor_count = scale.saturating_sub(planted.len());
    for d in generate_distractors(distractor_count, DISTRACTOR_SEED) {
        pool.push(d.content);
    }

    // Embed the whole pool RAW and FRAMED once.
    let mut raw_emb: Vec<Vec<f32>> = Vec::with_capacity(pool.len());
    let mut framed_emb: Vec<Vec<f32>> = Vec::with_capacity(pool.len());
    for c in &pool {
        raw_emb.push(probe.embed(c).await.expect("embed raw doc"));
        framed_emb.push(
            probe
                .embed(&format!("{FRAME}{c}"))
                .await
                .expect("embed framed doc"),
        );
    }

    // Rank a specific content string in a pool-embedding set against a query
    // embedding; return (1-based rank, cosine).
    let rank_of = |q_emb: &[f32], pool_emb: &[Vec<f32>], content: &str| -> Option<(usize, f32)> {
        let idx = pool.iter().position(|p| p == content)?;
        let mut scored: Vec<(usize, f32)> = pool_emb
            .iter()
            .enumerate()
            .map(|(i, e)| (i, cosine(q_emb, e)))
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored
            .iter()
            .position(|(i, _)| *i == idx)
            .map(|rk| (rk + 1, scored[rk].1))
    };

    println!("\n===== SUBJECT-FRAME RETRIEVAL-DEPTH PROBE =====");
    println!(
        "pool={} facts ; candidate cap today = DEFAULT_MAX_CANDIDATES = {}",
        pool.len(),
        DEFAULT_MAX_CANDIDATES
    );
    println!("(lower rank = better; a target ranked > {DEFAULT_MAX_CANDIDATES} never reaches the reranker)\n");

    let mut raw_in_cap = 0usize;
    let mut framed_in_cap = 0usize;
    let mut total = 0usize;
    // Dedupe by target key — the same recall target now appears under several
    // phrasing variants (Gap-2 ruler), but the depth probe characterizes each
    // unique planted target's BGE rank once.
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for q in &queries {
        for key in &q.must_surface {
            if !seen.insert(key.clone()) {
                continue;
            }
            let Some(content) = planted.iter().find(|p| p.id == *key).map(|p| &p.content) else {
                continue;
            };
            total += 1;
            let q_emb = probe.embed(&q.query_text).await.expect("embed query");
            let raw = rank_of(&q_emb, &raw_emb, content);
            let framed = rank_of(&q_emb, &framed_emb, content);
            let in_cap = |r: &Option<(usize, f32)>| {
                r.map(|(rk, _)| rk <= DEFAULT_MAX_CANDIDATES)
                    .unwrap_or(false)
            };
            if in_cap(&raw) {
                raw_in_cap += 1;
            }
            if in_cap(&framed) {
                framed_in_cap += 1;
            }
            let fmt = |r: Option<(usize, f32)>| {
                r.map(|(rk, c)| format!("rank {rk} (cos {c:.3})"))
                    .unwrap_or_else(|| "not found".into())
            };
            println!(
                "[{}] {key}\n    RAW   : {}\n    FRAMED: {}",
                q.id,
                fmt(raw),
                fmt(framed)
            );
        }
    }

    println!("\n---------------- SUMMARY ----------------");
    println!("targets within the top-{DEFAULT_MAX_CANDIDATES} candidate cap:");
    println!("  RAW    embedding: {raw_in_cap}/{total}");
    println!("  FRAMED embedding: {framed_in_cap}/{total}   <-- if higher, framing the retrieval embedding is the fix");
    println!("=========================================\n");

    assert_eq!(total, 10, "expected 10 recall targets in the fixture");
}

// ===========================================================================
// LIVE-VAULT SEEDER (Thread 3) — writes a REAL, Antigravity-openable vault.
//
// Unlike `scale_correctness_eval` (which seeds an ephemeral TempDir with a test
// key), this seeds a PERSISTENT vault at caller-chosen paths, keyed by the SAME
// production OS-keychain master key the MCP server uses
// (`read_existing_master_key(PRODUCTION_NAMESPACE, VAULT_ID)` → derive sqlcipher
// passphrase + at-rest key). So `vault-cli mcp serve` (and therefore Antigravity)
// opens it natively. Vectors are produced by bge-small-en-v1.5 (same weights as
// production), so query embeddings match.
//
// It seeds the planted facts (known answers) + deterministic distractors into ONE
// boundary (default `personal`), drains the cascade fully, then prints a TEST
// SCRIPT: the exact questions to ask Antigravity and the correct planted answers.
//
// ## Running (Windows — keychain is Windows-only at V0.2)
//
// ```text
// $env:SEED_N='100'                       # 100 → 1k → 10k ladder
// $env:SEED_VAULT_DIR='C:\path\to\seeded-vault'   # fresh dir; repoint Antigravity here
// # optional (default = the bge-small test fixture, same weights as prod):
// #   $env:SEED_BGE_MODEL / $env:SEED_BGE_TOKENIZER / $env:SEED_ORT_LIB
// #   $env:SEED_BOUNDARY (default 'personal')
// cargo test -p vault-app --test scale_eval seed_live_vault -- --ignored --nocapture
// ```
//
// Then point `vault-cli mcp serve` at the printed paths and test live.
//
// NOTE: single-row cascade drain is O(n) per write, so 10k takes tens of minutes
// (inherent — same path the app uses). 100 and 1k are quick. Start with 100.
// ===========================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live-vault seeder — writes a real Antigravity-openable vault; run with --ignored --nocapture + SEED_* env"]
async fn seed_live_vault() {
    fn env_required(key: &str) -> String {
        std::env::var(key)
            .unwrap_or_else(|_| panic!("{key} env var is required for the live seeder"))
    }
    fn env_path_or_fixture(key: &str, fixture_name: &str) -> PathBuf {
        match std::env::var(key) {
            Ok(p) => PathBuf::from(p),
            Err(_) => fixture(BGE_FIXTURE_REL, fixture_name),
        }
    }

    let seed_n: usize = env_required("SEED_N")
        .parse()
        .expect("SEED_N must be a positive integer");
    let vault_dir = PathBuf::from(env_required("SEED_VAULT_DIR"));
    let boundary_name = std::env::var("SEED_BOUNDARY").unwrap_or_else(|_| "personal".to_string());

    // Conventional sub-paths (mirrors vault-cli's vault_db / vector_dir / graph_db).
    let metadata_path = vault_dir.join("vault.db");
    let vector_dir = vault_dir.join("lance");
    let graph_path = vault_dir.join("graph.duckdb");
    std::fs::create_dir_all(&vault_dir).expect("create SEED_VAULT_DIR");

    // PRODUCTION key derivation — the load-bearing bit that makes the vault
    // openable by `vault-cli mcp serve` / Antigravity (Windows-only at V0.2).
    let master_key = vault_app::keychain::read_existing_master_key(
        vault_app::keychain::PRODUCTION_NAMESPACE,
        vault_app::keychain::VAULT_ID,
    )
    .expect("read the production key (Windows keychain)")
    .expect("a dev probe never creates the key (ADR-SEC-029); open the app once first");
    let sqlcipher_passphrase = vault_app::keychain::derive_sqlcipher_passphrase(&master_key);
    let at_rest_key = vault_app::keychain::derive_at_rest_key(&master_key);

    let config = AppConfig {
        metadata_path: metadata_path.clone(),
        vector_dir: vector_dir.clone(),
        graph_path: graph_path.clone(),
        key: sqlcipher_passphrase,
        model_path: env_path_or_fixture("SEED_BGE_MODEL", "model.onnx"),
        tokenizer_path: env_path_or_fixture("SEED_BGE_TOKENIZER", "tokenizer.json"),
        ort_lib_path: match std::env::var("SEED_ORT_LIB") {
            Ok(p) => PathBuf::from(p),
            Err(_) => ort_lib(),
        },
        at_rest_key,
        // Seeding only writes; the read-side models (rerank/qwen) + consolidator
        // (phi4) are not needed to embed + persist.
        qwen_model_path: None,
        phi4_model_path: None,
        rerank_model_path: None,
        rerank_tokenizer_path: None,
    };

    let app = Application::new(&config)
        .await
        .expect("Application::new (production-keyed live vault)");
    let _shutdown = app.spawn_retry_worker(); // spawn the cascading retry worker
    let adapter = app.adapter();

    // ---- load planted facts + queries ----
    let bytes = std::fs::read(scale_fixture_path()).expect("read scale_eval.json");
    let value: Value = serde_json::from_slice(&bytes).expect("parse scale_eval.json");
    let planted = parse_planted(&value);
    let queries = parse_queries(&value);

    let boundary = Boundary::new(boundary_name.as_str()).expect("SEED_BOUNDARY must be valid");

    println!("\n================ LIVE-VAULT SEEDER ================");
    println!("vault dir : {}", vault_dir.display());
    println!("boundary  : {boundary_name}");
    println!(
        "target N  : {seed_n}  ({} planted + distractors)",
        planted.len()
    );
    println!("(single-row cascade drain — 10k takes tens of minutes; 100/1k quick)\n");

    // ---- seed planted facts (override boundary so all live in SEED_BOUNDARY) ----
    let mut id_map: BTreeMap<String, String> = BTreeMap::new();
    for pf in &planted {
        let nm = NewMemory {
            content: pf.content.clone(),
            memory_type: MemoryType::Semantic,
            boundary: boundary.clone(),
            source_agent: pf.source_agent.clone(),
            confidence: pf.confidence,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        };
        let uuid = adapter
            .write(nm)
            .await
            .expect("seed planted write")
            .to_string();
        id_map.insert(pf.id.clone(), uuid);
    }

    // ---- pad to N with deterministic distractors (also into SEED_BOUNDARY) ----
    let distractor_count = seed_n.saturating_sub(planted.len());
    for d in generate_distractors(distractor_count, DISTRACTOR_SEED) {
        let nm = NewMemory {
            content: d.content,
            memory_type: MemoryType::Semantic,
            boundary: boundary.clone(),
            source_agent: Some("seed-distractor".into()),
            confidence: 0.9,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        };
        adapter.write(nm).await.expect("seed distractor write");
    }

    let total = planted.len() + distractor_count;
    println!(
        "enqueued {total} writes ({} planted + {distractor_count} distractors); draining vectors into LanceDB...",
        planted.len()
    );

    // ---- drain poll: wait until EVERY vector has landed in LanceDB ----
    // Searching for a fact is NOT a reliable drain signal — the keyword (BM25)
    // channel finds a fact from SQLite before its VECTOR is written to LanceDB,
    // so a search hit can fire while the vector store is still nearly empty
    // (the first seeder attempt shipped a 1-of-101 vault for exactly this
    // reason). The only ground truth is the vector-store row count. Poll it via
    // a FRESH read-only handle each tick — re-open to read the latest LanceDB
    // version (a held handle pins the version it opened at and would never see
    // the worker's progress). LanceDB tolerates concurrent read while the
    // cascade worker writes.
    use vault_storage::{LanceVectorStore, VectorStore};
    let mut drained = false;
    for attempt in 0..100_000usize {
        let probe_key = vault_app::keychain::derive_at_rest_key(&master_key);
        let probe = LanceVectorStore::open_with_at_rest_key(&vector_dir, 384, &probe_key)
            .await
            .expect("open vector count probe");
        let n = probe.count(None).await.expect("vector count probe");
        drop(probe);
        if n >= total {
            println!(
                "all {n}/{total} vectors drained into LanceDB after ~{}s\n",
                attempt * 2
            );
            drained = true;
            break;
        }
        if attempt % 15 == 0 {
            println!(
                "  draining... {n}/{total} vectors (~{}s elapsed)",
                attempt * 2
            );
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    assert!(
        drained,
        "vectors did not fully drain into LanceDB within the poll window"
    );

    // ---- print the TEST SCRIPT for Antigravity ----
    println!("================ ANTIGRAVITY TEST SCRIPT ================");
    println!("Point `vault-cli mcp serve` at this vault:");
    println!("  --vault-db   {}", metadata_path.display());
    println!("  --vector-dir {}", vector_dir.display());
    println!("  --graph-db   {}", graph_path.display());
    println!("  (authorize boundary: {boundary_name})\n");
    println!("Ask Antigravity each question; check the answer against 'EXPECT':\n");
    for q in &queries {
        if q.abstain {
            println!("  Q: {:?}", q.query_text);
            println!("     EXPECT: vault has NO such fact → agent should say it doesn't have it (abstain)\n");
        } else {
            println!("  Q: {:?}", q.query_text);
            for key in &q.must_surface {
                if let Some(pf) = planted.iter().find(|p| &p.id == key) {
                    println!("     EXPECT (answer): {:?}", pf.content);
                }
            }
            for key in &q.must_exclude {
                if let Some(pf) = planted.iter().find(|p| &p.id == key) {
                    println!(
                        "     NEAR-MISS (should NOT be the answer): {:?}",
                        pf.content
                    );
                }
            }
            println!();
        }
    }
    println!("========================================================\n");
    let _ = id_map; // planted-id → uuid map retained for future scripted assertions
}

// ===========================================================================
// LIVE-VAULT QUERY PROBE (Thread-2 Gap 2 diagnosis, 2026-06-09).
//
// Opens an EXISTING production-keyed vault (no seeding, no drain) and runs the
// exact live-dogfood queries through the real read + search paths, printing the
// FULL ranked list + per-result score so we can see WHERE the target fact lands
// (or that it is genuinely absent from the candidate pool). This is the faithful
// reproduction the in-process scale harness cannot be: it queries the very vault
// the live Antigravity session hit (same content, same production key, same
// code). It settles whether Gap 2 is query-string-sensitive recall (BGE/fanout)
// or a harness-vs-live artifact.
//
// Point it at a COPY of the live vault (don't mutate the evidence; Application
// may run migrations / spawn the cascade worker):
//
//   $env:PROBE_VAULT_DIR='C:\Projects\seeded-vault-1k-probe'
//   cargo test -p vault-app --test scale_eval probe_live_vault -- --ignored --nocapture
//
// Windows-only (production keychain). Needs the BGE + reranker fixtures.
// ===========================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live-vault query probe; opens an existing production-keyed vault; run with --ignored --nocapture + PROBE_VAULT_DIR"]
async fn probe_live_vault() {
    let vault_dir =
        PathBuf::from(std::env::var("PROBE_VAULT_DIR").expect("PROBE_VAULT_DIR env var required"));
    let metadata_path = vault_dir.join("vault.db");
    let vector_dir = vault_dir.join("lance");
    let graph_path = vault_dir.join("graph.duckdb");

    // PRODUCTION key derivation — the live vault was seeded with this same key,
    // so the same derivation opens it (mirrors seed_live_vault).
    let master_key = vault_app::keychain::read_existing_master_key(
        vault_app::keychain::PRODUCTION_NAMESPACE,
        vault_app::keychain::VAULT_ID,
    )
    .expect("read the production key (Windows keychain)")
    .expect("a dev probe never creates the key (ADR-SEC-029); open the app once first");
    let sqlcipher_passphrase = vault_app::keychain::derive_sqlcipher_passphrase(&master_key);
    let at_rest_key = vault_app::keychain::derive_at_rest_key(&master_key);

    let config = AppConfig {
        metadata_path,
        vector_dir,
        graph_path,
        key: sqlcipher_passphrase,
        model_path: fixture(BGE_FIXTURE_REL, "model.onnx"),
        tokenizer_path: fixture(BGE_FIXTURE_REL, "tokenizer.json"),
        ort_lib_path: ort_lib(),
        at_rest_key,
        qwen_model_path: None,
        phi4_model_path: None,
        // The reranker is the read relevance authority — load it (same fixture =
        // same weights as prod).
        rerank_model_path: Some(fixture(RERANK_FIXTURE_REL, "model.onnx")),
        rerank_tokenizer_path: Some(fixture(RERANK_FIXTURE_REL, "tokenizer.json")),
    };

    let app = Application::new(&config)
        .await
        .expect("Application::new over the live vault copy");
    let _shutdown = app.spawn_retry_worker();
    let adapter = app.adapter();

    // The live vault seeded everything into `personal`.
    let authorized = vec![Boundary::new("personal").expect("personal boundary")];

    // The exact live-dogfood queries: the idiom that MISSED Porto live, the plain
    // phrasing that FOUND it at rank 1, and the agent keyword-expansions.
    let probes = [
        "where does the user call home",
        "where does the user live",
        "what city does the user call home",
        "home location city country lives residence",
        "how do I stay fit",
    ];

    let porto_mark = |s: &str| -> &'static str {
        if s.contains("Porto") {
            "   <== PORTO (the target)"
        } else {
            ""
        }
    };

    for qtext in probes {
        println!("\n================ PROBE: {qtext:?} ================");

        // SEARCH — max_results=10 to match the live MCP default (server.rs:305).
        let hits = adapter
            .search(RetrievalQuery {
                query_text: qtext.to_string(),
                authorized_boundaries: authorized.clone(),
                max_results: 10,
                options: RetrievalOptions::default(),
            })
            .await
            .expect("search must not error");
        let porto_in_search = hits.iter().any(|h| h.memory.content.contains("Porto"));
        println!(
            "-- SEARCH (max_results=10): {} results ; PORTO {} --",
            hits.len(),
            if porto_in_search { "PRESENT" } else { "ABSENT" }
        );
        for (i, h) in hits.iter().enumerate() {
            println!(
                "  [{:>2}] score={:.4}  {}{}",
                i + 1,
                h.score,
                h.memory.content,
                porto_mark(&h.memory.content)
            );
        }

        // READ — the structured answer path (reranker + abstain hint).
        let resp = adapter
            .read(ReadQuery {
                query_text: qtext.to_string(),
                authorized_boundaries: authorized.clone(),
            })
            .await
            .expect("read must not error");
        let porto_in_read = resp.relevant_facts.iter().any(|f| f.fact.contains("Porto"));
        println!(
            "-- READ: abstain={} top_relevance={:.4} ; {} fact(s) ; PORTO {} --",
            resp.abstain,
            resp.top_relevance,
            resp.relevant_facts.len(),
            if porto_in_read { "PRESENT" } else { "ABSENT" }
        );
        for (i, f) in resp.relevant_facts.iter().enumerate() {
            println!("  [{:>2}] {}{}", i + 1, f.fact, porto_mark(&f.fact));
        }
    }
}

// ===========================================================================
// FAMILY-DOMAIN KEYWORD-SOUP PROBE (Thread-2 Gap 2, 2026-06-09).
//
// Replicates the Porto "keyword-soup misses, natural finds" pattern in a totally
// different domain (relationships / family) to settle two questions:
//   (1) Does the keyword-soup recall failure generalize beyond Porto?
//   (2) Is it DISTRACTOR-DEPENDENT — i.e. does the keyword query only drift away
//       from the target when matching-domain lexical noise exists to drift TO?
//
// It writes 3 USER target facts (natural phrasing, no overlap with the obvious
// query keywords) + 8 other-people family DISTRACTORS (the competing lexical
// noise), drains them, then probes each sub-domain NATURAL vs KEYWORD-SOUP.
//
// Run against a FRESH copy of the live vault (it WRITES — re-copy before each run
// so it starts from the clean 1000-fact baseline):
//
//   Copy-Item C:\Projects\seeded-vault-1k C:\Projects\seeded-vault-1k-probe -Recurse -Force
//   $env:PROBE_VAULT_DIR='C:\Projects\seeded-vault-1k-probe'
//   cargo test -p vault-app --test scale_eval probe_family_domain -- --ignored --nocapture
// ===========================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "family-domain keyword-soup probe; writes to an existing vault copy; run with --ignored --nocapture + PROBE_VAULT_DIR"]
async fn probe_family_domain() {
    use vault_storage::{LanceVectorStore, VectorStore};

    let vault_dir =
        PathBuf::from(std::env::var("PROBE_VAULT_DIR").expect("PROBE_VAULT_DIR env var required"));

    let master_key = vault_app::keychain::read_existing_master_key(
        vault_app::keychain::PRODUCTION_NAMESPACE,
        vault_app::keychain::VAULT_ID,
    )
    .expect("read the production key (Windows keychain)")
    .expect("a dev probe never creates the key (ADR-SEC-029); open the app once first");
    let config = AppConfig {
        metadata_path: vault_dir.join("vault.db"),
        vector_dir: vault_dir.join("lance"),
        graph_path: vault_dir.join("graph.duckdb"),
        key: vault_app::keychain::derive_sqlcipher_passphrase(&master_key),
        model_path: fixture(BGE_FIXTURE_REL, "model.onnx"),
        tokenizer_path: fixture(BGE_FIXTURE_REL, "tokenizer.json"),
        ort_lib_path: ort_lib(),
        at_rest_key: vault_app::keychain::derive_at_rest_key(&master_key),
        qwen_model_path: None,
        phi4_model_path: None,
        rerank_model_path: Some(fixture(RERANK_FIXTURE_REL, "model.onnx")),
        rerank_tokenizer_path: Some(fixture(RERANK_FIXTURE_REL, "tokenizer.json")),
    };

    let app = Application::new(&config)
        .await
        .expect("Application::new over the live vault copy");
    let _shutdown = app.spawn_retry_worker();
    let adapter = app.adapter();
    let boundary = Boundary::new("personal").expect("personal boundary");
    let authorized = vec![boundary.clone()];

    // ---- baseline vector count (so we drain-poll only the new writes) ----
    let baseline = {
        let probe =
            LanceVectorStore::open_with_at_rest_key(&config.vector_dir, 384, &config.at_rest_key)
                .await
                .expect("open baseline count probe");
        let n = probe.count(None).await.expect("baseline count");
        drop(probe);
        n
    };

    // 3 USER targets (natural phrasing, NO overlap with the query keywords) +
    // 8 other-people family DISTRACTORS (the competing lexical noise).
    let targets = [
        "The user tied the knot with Elena last spring.",
        "The user is raising twins who just started primary school.",
        "The user and Elena have been inseparable since their university days.",
    ];
    let distractors = [
        "Marcus celebrated his wedding anniversary with a trip to the coast.",
        "Diego got engaged to his long-term partner over the holidays.",
        "Sarah is busy planning her sister's wedding for next June.",
        "Priya's two sons are starting secondary school this autumn.",
        "Felix and his wife just welcomed their third child.",
        "Olivia's daughter is applying to universities this year.",
        "Tom's partner runs a small bakery in the old town.",
        "Lena's wedding photos from last month finally arrived.",
    ];
    let new_facts: Vec<&str> = targets.iter().chain(distractors.iter()).copied().collect();

    for content in &new_facts {
        let nm = NewMemory {
            content: (*content).to_string(),
            memory_type: MemoryType::Semantic,
            boundary: boundary.clone(),
            source_agent: Some("family-probe".into()),
            confidence: 0.93,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        };
        adapter.write(nm).await.expect("write family fact");
    }
    let want = baseline + new_facts.len();
    println!(
        "\nwrote {} family facts ({} targets + {} distractors); draining {} -> {} vectors...",
        new_facts.len(),
        targets.len(),
        distractors.len(),
        baseline,
        want
    );

    // ---- drain-poll the new vectors ----
    let mut drained = false;
    for attempt in 0..600usize {
        let probe =
            LanceVectorStore::open_with_at_rest_key(&config.vector_dir, 384, &config.at_rest_key)
                .await
                .expect("open drain count probe");
        let n = probe.count(None).await.expect("drain count");
        drop(probe);
        if n >= want {
            println!("drained to {n}/{want} after ~{}s\n", attempt * 2);
            drained = true;
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    assert!(drained, "family facts did not drain into LanceDB in time");

    // (query, intended target substring, phrasing)
    let probes: &[(&str, &str, &str)] = &[
        ("is the user married?", "tied the knot", "natural"),
        ("does the user have a spouse?", "tied the knot", "natural"),
        (
            "marriage spouse wife husband wedding married status",
            "tied the knot",
            "keyword",
        ),
        ("does the user have any kids?", "twins", "natural"),
        ("is the user a parent?", "twins", "natural"),
        (
            "children kids son daughter offspring parenting family",
            "twins",
            "keyword",
        ),
        ("is the user seeing anyone?", "inseparable", "natural"),
        ("does the user have a girlfriend?", "inseparable", "natural"),
        (
            "girlfriend partner relationship dating significant other romantic",
            "inseparable",
            "keyword",
        ),
    ];

    let is_user_fact = |s: &str| targets.contains(&s);

    for (qtext, target_sub, phrasing) in probes {
        let hits = adapter
            .search(RetrievalQuery {
                query_text: (*qtext).to_string(),
                authorized_boundaries: authorized.clone(),
                max_results: 10,
                options: RetrievalOptions::default(),
            })
            .await
            .expect("search must not error");
        let target_rank = hits
            .iter()
            .position(|h| h.memory.content.contains(target_sub))
            .map(|p| p + 1);
        println!(
            "================ [{phrasing}] {qtext:?}  (target: {target_sub:?}) ================"
        );
        println!(
            "-- SEARCH(10): target {} --",
            target_rank
                .map(|r| format!("PRESENT rank {r}"))
                .unwrap_or_else(|| "ABSENT".into())
        );
        for (i, h) in hits.iter().enumerate() {
            let mark = if h.memory.content.contains(target_sub) {
                "   <== TARGET"
            } else if is_user_fact(&h.memory.content) {
                "   (other user-family fact)"
            } else {
                ""
            };
            println!(
                "  [{:>2}] score={:.4}  {}{}",
                i + 1,
                h.score,
                h.memory.content,
                mark
            );
        }
        println!();
    }
}

// ===========================================================================
// ENRICHMENT + THIRD-DOMAIN PROBE (Thread-2 Gap 2, 2026-06-09).
//
// One run, three questions:
//   §1 THIRD DOMAIN (health/allergy) — does the "natural finds / keyword-soup is
//      unreliable" pattern hold in a brand-new domain, or surface another bug?
//      Includes an adversarial near-miss distractor (someone else + shellfish).
//   §2 ENRICHMENT A/B (kids) — the bare "raising twins" fact ranked #4 (behind a
//      distractor that used the literal word "child"). Write a DOCUMENT-SIDE
//      enriched twin ("… Topics: children, kids, parent, son, daughter, family")
//      alongside it and re-probe: does enrichment lift the rank? (This is the
//      proposed fix, tested cheaply.)
//   §3 ENRICHMENT ON THE HARD CASE (home) — the bare Porto fact (already in the
//      vault) was the ONE outright recall MISS: it vanished on the keyword-soup
//      "home location city country lives residence". Add an enriched Porto and
//      fire that exact killer query: does the enriched fact survive where the
//      bare one died? If yes, document-side enrichment fixes the worst case.
//
// Run against a FRESH copy (it WRITES — re-copy before each run):
//   Copy-Item C:\Projects\seeded-vault-1k C:\Projects\seeded-vault-1k-probe -Recurse -Force
//   $env:PROBE_VAULT_DIR='C:\Projects\seeded-vault-1k-probe'
//   cargo test -p vault-app --test scale_eval probe_enrichment -- --ignored --nocapture
// ===========================================================================

struct Foi {
    label: &'static str,
    must: &'static str,
    must_not: Option<&'static str>,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "enrichment + third-domain probe; writes to an existing vault copy; run with --ignored --nocapture + PROBE_VAULT_DIR"]
async fn probe_enrichment() {
    use vault_storage::{LanceVectorStore, VectorStore};

    let vault_dir =
        PathBuf::from(std::env::var("PROBE_VAULT_DIR").expect("PROBE_VAULT_DIR env var required"));
    let master_key = vault_app::keychain::read_existing_master_key(
        vault_app::keychain::PRODUCTION_NAMESPACE,
        vault_app::keychain::VAULT_ID,
    )
    .expect("read the production key (Windows keychain)")
    .expect("a dev probe never creates the key (ADR-SEC-029); open the app once first");
    let config = AppConfig {
        metadata_path: vault_dir.join("vault.db"),
        vector_dir: vault_dir.join("lance"),
        graph_path: vault_dir.join("graph.duckdb"),
        key: vault_app::keychain::derive_sqlcipher_passphrase(&master_key),
        model_path: fixture(BGE_FIXTURE_REL, "model.onnx"),
        tokenizer_path: fixture(BGE_FIXTURE_REL, "tokenizer.json"),
        ort_lib_path: ort_lib(),
        at_rest_key: vault_app::keychain::derive_at_rest_key(&master_key),
        qwen_model_path: None,
        phi4_model_path: None,
        rerank_model_path: Some(fixture(RERANK_FIXTURE_REL, "model.onnx")),
        rerank_tokenizer_path: Some(fixture(RERANK_FIXTURE_REL, "tokenizer.json")),
    };
    let app = Application::new(&config)
        .await
        .expect("Application::new over the live vault copy");
    let _shutdown = app.spawn_retry_worker();
    let adapter = app.adapter();
    let boundary = Boundary::new("personal").expect("personal boundary");
    let authorized = vec![boundary.clone()];

    let baseline = {
        let probe =
            LanceVectorStore::open_with_at_rest_key(&config.vector_dir, 384, &config.at_rest_key)
                .await
                .expect("open baseline count probe");
        let n = probe.count(None).await.expect("baseline count");
        drop(probe);
        n
    };

    // §1 health/allergy (target avoids "allerg*"; last distractor is the
    // adversarial near-miss — someone else + shellfish).
    // §2 kids bare vs enriched. §3 home enriched (bare Porto already in vault).
    let new_facts: &[&str] = &[
        // §1
        "The user comes out in hives whenever they eat shellfish.",
        "Marcus carries an epipen for his peanut allergy.",
        "Priya is lactose intolerant and avoids all dairy.",
        "Diego developed a gluten sensitivity last year.",
        "Sarah's son has a severe nut allergy at school.",
        "Felix breaks out in a rash from certain laundry detergents.",
        "Olivia gets terrible hay fever every spring.",
        "Lena is on a strict low-sodium diet for her blood pressure.",
        "Tom avoids shellfish after a bad reaction at a restaurant once.",
        // §2
        "The user is raising twins who just started primary school.",
        "The user is raising twins who just started primary school. Topics: children, kids, parent, son, daughter, family.",
        "Felix and his wife just welcomed their third child.",
        "Olivia's daughter is applying to universities this year.",
        // §3
        "The user settled in Porto after years of moving around. Topics: home, lives, residence, city, country, location.",
    ];
    for content in new_facts {
        adapter
            .write(NewMemory {
                content: (*content).to_string(),
                memory_type: MemoryType::Semantic,
                boundary: boundary.clone(),
                source_agent: Some("enrichment-probe".into()),
                confidence: 0.93,
                valid_from: None,
                valid_until: None,
                metadata: serde_json::json!({}),
            })
            .await
            .expect("write probe fact");
    }
    let want = baseline + new_facts.len();
    println!(
        "\nwrote {} facts; draining {baseline} -> {want}...",
        new_facts.len()
    );
    let mut drained = false;
    for attempt in 0..600usize {
        let probe =
            LanceVectorStore::open_with_at_rest_key(&config.vector_dir, 384, &config.at_rest_key)
                .await
                .expect("open drain count probe");
        let n = probe.count(None).await.expect("drain count");
        drop(probe);
        if n >= want {
            println!("drained to {n}/{want} after ~{}s\n", attempt * 2);
            drained = true;
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    assert!(drained, "probe facts did not drain in time");

    // (section, phrasing, query, facts-of-interest)
    let bare_twins = Foi {
        label: "bare-twins",
        must: "raising twins",
        must_not: Some("Topics"),
    };
    let enr_twins = Foi {
        label: "ENRICHED-twins",
        must: "Topics: children",
        must_not: None,
    };
    let bare_porto = Foi {
        label: "bare-Porto",
        must: "settled in Porto",
        must_not: Some("Topics"),
    };
    let enr_porto = Foi {
        label: "ENRICHED-Porto",
        must: "Topics: home",
        must_not: None,
    };
    let allergy = Foi {
        label: "user-allergy",
        must: "hives",
        must_not: None,
    };

    let probes: Vec<(&str, &str, &str, Vec<&Foi>)> = vec![
        (
            "§1 health",
            "natural",
            "is the user allergic to anything?",
            vec![&allergy],
        ),
        (
            "§1 health",
            "natural",
            "does the user have any food allergies?",
            vec![&allergy],
        ),
        (
            "§1 health",
            "keyword",
            "allergy allergic reaction intolerance sensitivity food",
            vec![&allergy],
        ),
        (
            "§2 kids A/B",
            "natural",
            "does the user have any kids?",
            vec![&bare_twins, &enr_twins],
        ),
        (
            "§2 kids A/B",
            "keyword",
            "children kids son daughter offspring family",
            vec![&bare_twins, &enr_twins],
        ),
        (
            "§3 home FIX",
            "keyword",
            "home location city country lives residence",
            vec![&bare_porto, &enr_porto],
        ),
        (
            "§3 home FIX",
            "natural",
            "where does the user live",
            vec![&bare_porto, &enr_porto],
        ),
    ];

    for (section, phrasing, qtext, fois) in &probes {
        let hits = adapter
            .search(RetrievalQuery {
                query_text: (*qtext).to_string(),
                authorized_boundaries: authorized.clone(),
                max_results: 10,
                options: RetrievalOptions::default(),
            })
            .await
            .expect("search must not error");
        let rank_of = |foi: &Foi| -> Option<usize> {
            hits.iter()
                .position(|h| {
                    h.memory.content.contains(foi.must)
                        && foi.must_not.is_none_or(|mn| !h.memory.content.contains(mn))
                })
                .map(|p| p + 1)
        };
        let resp = adapter
            .read(ReadQuery {
                query_text: (*qtext).to_string(),
                authorized_boundaries: authorized.clone(),
            })
            .await
            .expect("read must not error");

        println!("================ [{section}][{phrasing}] {qtext:?} ================");
        for foi in fois {
            println!(
                "  {:<16} SEARCH: {}",
                foi.label,
                rank_of(foi)
                    .map(|r| format!("rank {r}"))
                    .unwrap_or_else(|| "ABSENT (not in top-10)".into())
            );
        }
        println!(
            "  READ: abstain={} top_relevance={:.4}",
            resp.abstain, resp.top_relevance
        );
        for (i, h) in hits.iter().enumerate() {
            let mark = fois
                .iter()
                .find(|f| {
                    h.memory.content.contains(f.must)
                        && f.must_not.is_none_or(|mn| !h.memory.content.contains(mn))
                })
                .map(|f| format!("   <== {}", f.label))
                .unwrap_or_default();
            println!(
                "  [{:>2}] score={:.4}  {}{}",
                i + 1,
                h.score,
                h.memory.content,
                mark
            );
        }
        println!();
    }
}

// ───────────────────────────────────────────────────────────────────────────
// probe_real_enrichment_1k — REAL Phi-4 end-to-end rank-lift on a 1k vault
// (ADR-074 live validation).
//
// The mock-LLM tests prove the enrichment *logic*; `real_phi4_alias_quality`
// (vault-consolidator) proves the real model produces good alias *words*; this
// probe closes the loop: it takes the three Gap-2 killer facts (phrased WITHOUT
// the obvious keyword), drops them into the real `seeded-vault-1k` distractor
// field, records each one's BARE rank on its killer keyword query, enriches ONLY
// those three via the real Phi-4 `enrich_one` path, and re-measures. The 1k
// distractors stay bare — they are what bury the bare killer — so enriching just
// the three is the faithful + fast (~2-3 min) A/B (no full-vault enrichment, no
// merge/contradiction cost, deterministic worker drain).
//
// Rank is measured by a direct LanceDB vector search (cosine), which isolates
// the enrichment's effect on the embedding (no BM25/rerank confounds). It writes
// to the vault copy — point PROBE_VAULT_DIR at a THROWAWAY copy of
// seeded-vault-1k, never the evidence vault.
//
// Run:
//   $env:PROBE_VAULT_DIR='C:\Projects\seeded-vault-1k-probe'
//   $env:PHI4_MODEL_DIR='C:\Users\shahb\AppData\Roaming\com.shahbaz242630.memory-vault\models'
//   cargo test -p vault-app --test scale_eval probe_real_enrichment_1k -- --ignored --nocapture
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "REAL Phi-4 end-to-end rank-lift on a 1k vault copy; needs PROBE_VAULT_DIR (throwaway) + PHI4_MODEL_DIR; --ignored --nocapture"]
async fn probe_real_enrichment_1k() {
    use vault_consolidator::phases::enrich::enrich_one;
    use vault_llm::{Phi4MiniConfig, Phi4MiniProvider};
    use vault_storage::StorageBackend;

    const DIM: usize = 384;

    let vault_dir = PathBuf::from(
        std::env::var("PROBE_VAULT_DIR").expect("PROBE_VAULT_DIR (throwaway 1k copy) required"),
    );
    let phi4_dir = std::env::var("PHI4_MODEL_DIR").unwrap_or_else(|_| {
        r"C:\Users\shahb\AppData\Roaming\com.shahbaz242630.memory-vault\models".to_string()
    });

    let master_key = vault_app::keychain::read_existing_master_key(
        vault_app::keychain::PRODUCTION_NAMESPACE,
        vault_app::keychain::VAULT_ID,
    )
    .expect("read the production key (Windows keychain)")
    .expect("a dev probe never creates the key (ADR-SEC-029); open the app once first");
    let sql_key = vault_app::keychain::derive_sqlcipher_passphrase(&master_key);
    let at_rest = vault_app::keychain::derive_at_rest_key(&master_key);

    println!("\nloading BGE + Phi-4 (real)...");
    let bge = BgeSmallProvider::open(
        &fixture(BGE_FIXTURE_REL, "model.onnx"),
        &fixture(BGE_FIXTURE_REL, "tokenizer.json"),
        &ort_lib(),
    )
    .expect("open BGE");
    let phi4 = Phi4MiniProvider::new(Phi4MiniConfig::v0_2_default(phi4_dir.into()))
        .await
        .expect("load Phi-4");

    let storage = StorageBackend::open_with_at_rest_key(
        &vault_dir.join("vault.db"),
        &vault_dir.join("lance"),
        &vault_dir.join("graph.duckdb"),
        sql_key,
        DIM,
        &at_rest,
    )
    .await
    .expect("open vault copy");

    let boundary = Boundary::new("personal").expect("personal boundary");

    // Drain the cascade queue to Idle so re-embeds actually land in LanceDB.
    async fn drain(storage: &StorageBackend) {
        let mut worker = vault_storage::RetryWorker::new(storage.clone());
        let drain_at = chrono::Utc::now() + chrono::Duration::seconds(120);
        for _ in 0..100_000 {
            match worker.step_at(drain_at).await.expect("worker step") {
                vault_storage::StepResult::Idle => break,
                _ => continue,
            }
        }
    }

    // Fresh-open a LanceVectorStore (sees the latest committed version), embed
    // the query, return the target fact's 1-based rank (or None if not in top-N).
    async fn rank_of(
        vault_dir: &std::path::Path,
        at_rest: &[u8; 32],
        bge: &BgeSmallProvider,
        boundary: &vault_core::Boundary,
        query: &str,
        target: vault_core::MemoryId,
    ) -> Option<usize> {
        use vault_storage::VectorStore;
        let lance = vault_storage::LanceVectorStore::open_with_at_rest_key(
            &vault_dir.join("lance"),
            384,
            at_rest,
        )
        .await
        .expect("open lance for search");
        let qe = bge.embed(query).await.expect("embed query");
        let hits = lance
            .search(&qe, 50, std::slice::from_ref(boundary))
            .await
            .expect("vector search");
        hits.iter().position(|(id, _)| *id == target).map(|p| p + 1)
    }

    struct Killer {
        label: &'static str,
        content: &'static str,
        query: &'static str,
    }
    let killers = [
        Killer {
            label: "Porto",
            content: "The user settled in Porto after years of moving around.",
            query: "home location city country lives residence",
        },
        Killer {
            label: "twins",
            content: "The user is raising twins who just started primary school.",
            query: "children kids son daughter offspring family",
        },
        Killer {
            label: "hives",
            content: "The user comes out in hives whenever they eat shellfish.",
            query: "is the user allergic to anything",
        },
    ];

    // Write the bare killers into the 1k field, capture their rows, drain.
    let mut rows = Vec::new();
    for k in &killers {
        let m = vault_core::Memory::try_new(NewMemory {
            content: k.content.to_string(),
            memory_type: MemoryType::Semantic,
            boundary: boundary.clone(),
            source_agent: Some("enrich-1k-probe".into()),
            confidence: 0.93,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        })
        .expect("valid memory");
        let emb = bge.embed(&m.content).await.expect("embed killer");
        storage.write_memory(&m, &emb).await.expect("write killer");
        rows.push(m);
    }
    drain(&storage).await;

    println!("\n================ BASELINE (bare killers in the 1k field) ================");
    let mut baseline = Vec::new();
    for (k, m) in killers.iter().zip(&rows) {
        let r = rank_of(&vault_dir, &at_rest, &bge, &boundary, k.query, m.id).await;
        baseline.push(r);
        println!(
            "  {:<6} {:?} -> {}",
            k.label,
            k.query,
            r.map(|r| format!("rank {r}"))
                .unwrap_or_else(|| "ABSENT (not in top-50)".into())
        );
    }

    println!("\nenriching ONLY the killers with real Phi-4...");
    for m in &rows {
        match enrich_one(m, &phi4, &bge).await.expect("enrich_one") {
            Some(ef) => {
                let aliases = ef
                    .memory
                    .metadata
                    .get("enrichment")
                    .and_then(|e| e.get("aliases"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                println!("  {:?}\n     aliases: {aliases}", m.content);
                storage
                    .update_memory(&ef.memory, &ef.embedding)
                    .await
                    .expect("update_memory");
            }
            None => println!("  {:?} -> SKIP (unexpectedly already enriched)", m.content),
        }
    }
    drain(&storage).await;

    println!("\n================ RESULT (bare -> enriched) ================");
    for ((k, m), b) in killers.iter().zip(&rows).zip(&baseline) {
        let after = rank_of(&vault_dir, &at_rest, &bge, &boundary, k.query, m.id).await;
        let fmt = |r: Option<usize>| {
            r.map(|r| format!("rank {r}"))
                .unwrap_or_else(|| "ABSENT".into())
        };
        println!(
            "  {:<6} {:?}\n           bare: {:<18} ->   enriched: {}",
            k.label,
            k.query,
            fmt(*b),
            fmt(after)
        );
    }
    println!();
}

// ===========================================================================
// CONTRADICTION-PAIR COSINE DISTRIBUTION PROBE (Finding G, 2026-06-19).
//
// MEASUREMENT ONLY — no LLM, no writes. Reproduces the FULL-SWEEP Phase-2b
// candidate-pair generation on a real vault and prints the cosine distribution
// of the resulting pairs, so we can decide — with data, not a guess — whether
// raising the candidate floor can cut the pair count WITHOUT dropping a genuine
// contradiction.
//
// WHY: the session-7 1k backfill logged `candidate_pairs=1730` and the handoff
// called these "unpruned". That is FALSE — `nearest_neighbor_candidate_pairs`
// already applies a 0.70 cosine floor + top-3 cap. So the 1730 are ALREADY
// >=0.70. The known knowledge-update contradictions sit at cosine ~0.823
// (Tesla/Rivian) and ~0.905 (Vega/Atlas) per `nn_contradiction_spike.rs`, so a
// floor raised above ~0.82 would silently drop a real contradiction. This probe
// measures where the 1730 actually live relative to that 0.82 line.
//
// This mirrors the consolidator full-sweep path EXACTLY: list active facts
// (valid_until.is_none()), group by boundary, embed each fact's CONTENT via the
// same BGE weights, and call the same `nearest_neighbor_candidate_pairs`.
//
// Read-only — safe to point at the pristine seed, but a copy is tidiest:
//
//   Copy-Item C:\Projects\seeded-vault-1k C:\Projects\seeded-vault-1k-cosine -Recurse -Force
//   $env:PROBE_VAULT_DIR='C:\Projects\seeded-vault-1k-cosine'
//   cargo test -p vault-app --test scale_eval probe_contradiction_pair_distribution -- --ignored --nocapture
// ===========================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "contradiction-pair cosine distribution probe (measurement only, no LLM); run with --ignored --nocapture + PROBE_VAULT_DIR"]
async fn probe_contradiction_pair_distribution() {
    use vault_consolidator::phases::candidates::{
        nearest_neighbor_candidate_pairs, CONTRADICTION_NN_SIMILARITY_FLOOR, CONTRADICTION_NN_TOP_K,
    };
    use vault_storage::{MemoryFilter, StorageBackend};

    const DIM: usize = 384;
    // The two measured genuine knowledge-update contradictions (nn_contradiction_spike.rs).
    // Any floor that would exclude these loses real recall.
    const KNOWN_CONTRADICTION_COSINES: &[(&str, f32)] =
        &[("Tesla/Rivian", 0.823), ("Vega/Atlas", 0.905)];

    let vault_dir = PathBuf::from(
        std::env::var("PROBE_VAULT_DIR").expect("PROBE_VAULT_DIR (1k vault, read-only) required"),
    );

    let master_key = vault_app::keychain::read_existing_master_key(
        vault_app::keychain::PRODUCTION_NAMESPACE,
        vault_app::keychain::VAULT_ID,
    )
    .expect("read the production key (Windows keychain)")
    .expect("a dev probe never creates the key (ADR-SEC-029); open the app once first");
    let sql_key = vault_app::keychain::derive_sqlcipher_passphrase(&master_key);
    let at_rest = vault_app::keychain::derive_at_rest_key(&master_key);

    println!("\nloading BGE (real, same weights as prod)...");
    let bge = BgeSmallProvider::open(
        &fixture(BGE_FIXTURE_REL, "model.onnx"),
        &fixture(BGE_FIXTURE_REL, "tokenizer.json"),
        &ort_lib(),
    )
    .expect("open BGE");

    let storage = StorageBackend::open_with_at_rest_key(
        &vault_dir.join("vault.db"),
        &vault_dir.join("lance"),
        &vault_dir.join("graph.duckdb"),
        sql_key,
        DIM,
        &at_rest,
    )
    .await
    .expect("open vault");

    // EXACTLY the full-sweep Phase-2b active set: non-superseded AND not retired.
    let active: Vec<vault_core::Memory> = storage
        .list_memories(MemoryFilter::default(), None)
        .await
        .expect("list_memories")
        .into_iter()
        .filter(|m| m.valid_until.is_none())
        .collect();

    let mut by_boundary: BTreeMap<Boundary, Vec<vault_core::Memory>> = BTreeMap::new();
    for m in active {
        by_boundary.entry(m.boundary.clone()).or_default().push(m);
    }

    // Word-token containment (|A∩B| / min(|A|,|B|)) — a LOCAL copy of the dedup
    // gate's lexical axis (`phases::dedup::token_containment`, pub(crate) so not
    // importable here). Same logic, so the numbers it prints match the gate.
    fn containment(a: &str, b: &str) -> f32 {
        let toks = |s: &str| -> std::collections::HashSet<String> {
            s.split(|c: char| !c.is_alphanumeric())
                .filter(|t| !t.is_empty())
                .map(str::to_lowercase)
                .collect()
        };
        let (ta, tb) = (toks(a), toks(b));
        let min_len = ta.len().min(tb.len());
        if min_len == 0 {
            return 0.0;
        }
        ta.intersection(&tb).count() as f32 / min_len as f32
    }
    // The dedup gate's lexical floor (`phases::dedup::NEAR_IDENTICAL_LEX`).
    const DEDUP_LEX_FLOOR: f32 = 0.80;
    // The merge/dedup clustering gate (cluster.rs): only pairs >= this even
    // reach merge/dedup; below it they are contradiction-only. Read from the
    // shipped config rather than hardcoded so this diagnostic always describes
    // the gate the product actually runs (0.84 since ADR-097 — a hardcoded
    // 0.92 here would have kept printing a band that no longer exists).
    let merge_gate_cos =
        vault_consolidator::ConsolidatorConfig::default().merge_similarity_threshold;

    // Collect every candidate pair across all boundaries with cosine +
    // containment + both texts (the pair set is already 0.70-floored +
    // top-K-capped by nearest_neighbor_candidate_pairs).
    struct Pair {
        cos: f32,
        lex: f32,
        a: String,
        b: String,
    }
    let mut pairs_full: Vec<Pair> = Vec::new();
    let mut total_active = 0usize;
    for (boundary, memories) in &by_boundary {
        total_active += memories.len();
        if memories.len() < 2 {
            continue;
        }
        println!("embedding {} active facts in {boundary}...", memories.len());
        let mut embeddings = Vec::with_capacity(memories.len());
        for m in memories {
            embeddings.push(bge.embed(&m.content).await.expect("embed content"));
        }
        for (i, j) in nearest_neighbor_candidate_pairs(&embeddings) {
            pairs_full.push(Pair {
                cos: cosine(&embeddings[i], &embeddings[j]),
                lex: containment(&memories[i].content, &memories[j].content),
                a: memories[i].content.clone(),
                b: memories[j].content.clone(),
            });
        }
    }
    pairs_full.sort_by(|x, y| y.cos.total_cmp(&x.cos)); // descending by cosine
    let pair_cosines: Vec<f32> = pairs_full.iter().map(|p| p.cos).collect();

    let total_pairs = pair_cosines.len();
    println!("\n===== CONTRADICTION-PAIR COSINE DISTRIBUTION =====");
    println!(
        "active facts: {total_active} ; current floor={CONTRADICTION_NN_SIMILARITY_FLOOR} top-K={CONTRADICTION_NN_TOP_K}"
    );
    println!("candidate pairs (>= {CONTRADICTION_NN_SIMILARITY_FLOOR} floor): {total_pairs}");
    if total_pairs == 0 {
        println!("(no pairs — nothing to analyse)");
        return;
    }

    // Histogram by 0.05 band from 0.70 up to 1.00.
    println!("\n-- histogram (cosine band -> pair count) --");
    let mut band = 0.70f32;
    while band < 1.0 {
        let hi = band + 0.05;
        let n = pair_cosines
            .iter()
            .filter(|&&c| c >= band && c < hi)
            .count();
        let bar = "#".repeat((n as f32 / total_pairs as f32 * 60.0).round() as usize);
        println!("  [{band:.2}, {hi:.2}) {n:>6}  {bar}");
        band = hi;
    }

    // The decision table: if we RAISED the candidate floor to X, how many pairs
    // survive (= LLM calls) — and is X safely below the known contradictions?
    println!("\n-- if candidate floor raised to X: pairs surviving (LLM calls) & recall safety --");
    println!(
        "   (a real knowledge-update contradiction sits at ~0.82; a SAFE floor stays below it)"
    );
    for &x in &[0.70f32, 0.75, 0.78, 0.80, 0.82, 0.85, 0.88, 0.90, 0.92] {
        let surviving = pair_cosines.iter().filter(|&&c| c >= x).count();
        let pct = surviving as f32 / total_pairs as f32 * 100.0;
        let loses = KNOWN_CONTRADICTION_COSINES
            .iter()
            .filter(|(_, c)| *c < x)
            .map(|(name, _)| *name)
            .collect::<Vec<_>>();
        let verdict = if loses.is_empty() {
            "recall-safe".to_string()
        } else {
            format!("DROPS {}", loses.join(", "))
        };
        println!("   floor {x:.2}: {surviving:>6} pairs ({pct:>5.1}%)   {verdict}");
    }

    // Near-dup tail: pairs >= the merge gate that nonetheless reached
    // Phase 2b — these signal merge/dedup under-collapsing (Finding B), the
    // OTHER lever for shrinking the pair count.
    let near_dups = pair_cosines
        .iter()
        .filter(|&&c| c >= merge_gate_cos)
        .count();
    println!(
        "\nnear-duplicate pairs (>= {merge_gate_cos} merge gate, yet still contradiction-judged): {near_dups} ({:.1}%)",
        near_dups as f32 / total_pairs as f32 * 100.0
    );
    println!("  ^ if dedup/merge collapsed these, they would never reach the slow judge.");

    // ── WHY aren't the merge-eligible pairs collapsing? Diagnose dedup ──
    // The dedup gate requires cosine >= 0.93 AND containment >= 0.80 AND
    // not-a-pure-reordering (ADR-096). For pairs already above the clustering
    // gate, the LEXICAL axis is the usual blocker. Split them.
    let merge_eligible: Vec<&Pair> = pairs_full
        .iter()
        .filter(|p| p.cos >= merge_gate_cos)
        .collect();
    let lex_pass = merge_eligible
        .iter()
        .filter(|p| p.lex >= DEDUP_LEX_FLOOR)
        .count();
    let lex_fail = merge_eligible.len() - lex_pass;
    println!(
        "\n-- of the {} pairs >= {merge_gate_cos} cosine (merge-eligible) --",
        merge_eligible.len()
    );
    println!(
        "   containment >= {DEDUP_LEX_FLOOR} (dedup would fire): {lex_pass}\n   containment <  {DEDUP_LEX_FLOOR} (dedup BLOCKED by lexical axis): {lex_fail}"
    );

    // Eyeball sample: are these genuinely duplicates that SHOULD merge, or
    // distinct facts? Print the lowest-containment merge-eligible pairs (the
    // ones the lexical gate is rejecting) + a few high-cosine examples.
    println!("\n-- sample merge-eligible pairs the LEXICAL gate rejects (cos>={merge_gate_cos}, lex<{DEDUP_LEX_FLOOR}) --");
    let mut lex_rejects: Vec<&&Pair> = merge_eligible
        .iter()
        .filter(|p| p.lex < DEDUP_LEX_FLOOR)
        .collect();
    lex_rejects.sort_by(|x, y| y.cos.total_cmp(&x.cos));
    for p in lex_rejects.iter().take(12) {
        println!(
            "   cos={:.3} lex={:.2}\n      A: {}\n      B: {}",
            p.cos, p.lex, p.a, p.b
        );
    }

    // Also sample the 0.85-to-gate band (BELOW the merge gate but high) — if
    // these are ALSO duplicates, the clustering gate itself is too high.
    // ADR-097 note: this hypothesis was tested and CONFIRMED — the gate moved
    // 0.92 → 0.84 on the strength of it. The band is narrower now; a still-dup-
    // heavy sample here is the signal for the next move (0.84 is the charted
    // next step, at the cost of doubled contradiction exposure).
    println!(
        "\n-- sample pairs in the 0.85-to-gate band (below merge gate; are these dups too?) --"
    );
    let mut mid: Vec<&Pair> = pairs_full
        .iter()
        .filter(|p| p.cos >= 0.85 && p.cos < merge_gate_cos)
        .collect();
    mid.sort_by(|x, y| y.cos.total_cmp(&x.cos));
    for p in mid.iter().take(12) {
        println!(
            "   cos={:.3} lex={:.2}\n      A: {}\n      B: {}",
            p.cos, p.lex, p.a, p.b
        );
    }
    println!("===================================================\n");
}
