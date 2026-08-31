//! Pairwise contradiction detection (T0.3.x A5 — the V0.2 ship-gate).
//!
//! ## Why this exists — decoupling detection from the merge gate
//!
//! Phase-1 clustering (`phases/cluster.rs`) only forms edges at or above
//! `merge_similarity_threshold` — a gate tuned to catch near-duplicates for
//! *merging*. A knowledge-update contradiction ("the user works at Vega" →
//! later "the user works at Atlas, having left Vega") is semantically
//! *related* but its pairwise cosine typically sits **below** that gate, so the
//! pair never clusters and `decide_merge` never sees it. Result (confirmed live
//! in Claude Desktop, 2026-05-29): contradictions are never detected and reads
//! return both the stale and current facts.
//!
//! **ADR-097 refinement.** Contradictions are not cleanly below the gate as a
//! class — measured, they span cosine **0.7199–0.9979**, interleaved with
//! paraphrases (0.8237–0.9936). At the shipped 0.84 gate the top of that range
//! DOES cluster and reaches `decide_merge` first. This module is therefore not
//! a fallback for "the pairs clustering misses" — it is the **catch-all** that
//! re-judges everything still active after Phase 2, whether or not it
//! clustered. The bulk of the contradiction mass still sits below the gate and
//! is reachable ONLY here.
//!
//! ## Candidate generation — nearest neighbors, not K-means (ADR-065)
//!
//! The first fix (ADR-060/062) judged contradictions *within* K-means topic
//! groups, on the premise that a topic co-locates the conflicting pair. The
//! §7 live dogfood (2026-06-01) proved that premise FALSE: K-means split the
//! Tesla→Rivian pair across groups, so it was never judged and A5 silently
//! failed. Candidate generation now lives in [`super::candidates`]: for each
//! fact, take its top-K nearest cosine neighbors above a floor and union them
//! into an unordered candidate-pair set (the conflicting pair is each other's
//! *nearest* neighbor, so it is always included). Those pairs feed
//! [`judge_candidate_pairs`] below. The merge gate governs *merging*; this
//! module governs *contradiction* judging, independently of it.
//!
//! ## Pairwise judging — the precision fix (ADR-062, 2026-05-30)
//!
//! The first shipped version (ADR-060) asked the model ONE N-ary question per
//! topic: "is there a contradiction anywhere in these N facts?" Live dogfood
//! on real Phi-4-mini (2026-05-30) showed this **over-flags**: handed a loose
//! topic of unrelated facts (e.g. three distinct memory-ceiling probes), a
//! small model invents a conflict and names a "loser." N-ary dilutes the
//! signal.
//!
//! [`detect_contradiction`] now judges **pairwise**: for every unordered pair
//! in the group it asks the sharp, narrow question "do THESE TWO facts make
//! incompatible claims about the same attribute of the same subject, and if so
//! which is stale?" ([`detect_pair_contradiction`]), then unions the confirmed
//! stale ids ([`detect_contradictions_pairwise`]). A two-fact prompt has
//! nothing to "scan," so unrelated pairs are cleanly rejected while the
//! head-to-head Vega/Atlas comparison gets *sharper*, not weaker. Pairwise is
//! O(N²) LLM calls per topic; for groups above [`MAX_PAIRWISE_GROUP_SIZE`] the
//! dispatcher falls back (with a WARN — never a silent drop) to the single
//! N-ary call ([`detect_contradiction_nary`]) for bounded cost. A cosine
//! pre-prune to cut the pair count is a documented latency-arc fast-follow.
//!
//! ## Output shape — explicit stale ids, never a whole-group sweep
//!
//! Both paths return the **specific** memory ids that are no longer true
//! ([`ContradictionVerdict::stale_memory_ids`]) rather than a "winner,
//! everything-else-loses" verdict. A topic is a *loose* grouping — it may hold
//! many compatible facts plus one stale pair. Returning explicit stale ids
//! means the orchestrator retires only the facts the LLM identified as
//! superseded, never the whole topic. The orchestrator adds a belt-and-braces
//! guard (refuse to invalidate an entire group; ignore ids not in the group)
//! so even a misbehaving model cannot mass-retire.
//!
//! ## Conservative by construction
//!
//! The judge flags a contradiction ONLY when two facts make incompatible
//! claims about the same *single-valued* attribute of the same subject — one
//! the person holds only ONE current value of (employer, city, marital
//! status). Co-topical-but-compatible facts ("works at Atlas" + "commutes by
//! train"), near-duplicates of the same fact, and distinct *events* a person
//! accumulates many of ("met Sarah Monday" + "met Sarah Thursday"; two office
//! recaps on different days) must NOT be flagged — those are two true facts,
//! not a superseded one, and retiring one would silently lose real data
//! (ADR-083, the over-retention guard). The judge must first name the single
//! `shared_attribute` both facts describe; a contradiction with no shared
//! attribute is refused by the aggregator (ADR-062 iter 2). The `as_of`
//! (fact-time) of each memory is supplied so the model can pick the current
//! truth by recency or explicit supersession. If a pair conflicts but the
//! current truth is genuinely undecidable, the pair judge returns
//! `stale = "neither"` and the aggregator takes no action (abstain).

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};
use tracing::{instrument, warn};

use crate::prompt_guard::guarded_system_prompt;
use vault_core::{Memory, MemoryId, VaultError, VaultResult};
use vault_llm::{CompletionParams, LlmProvider};

/// Above this group size the pairwise pass (O(N²) LLM calls) falls back to a
/// single N-ary judge for bounded nightly cost. K-means topics are clamped to
/// a small K, so real topics rarely approach this; the cap is a backstop, not
/// a tuning knob. At 20 members the pairwise pass is 190 calls — comfortable
/// for an offline nightly run with a local model.
pub const MAX_PAIRWISE_GROUP_SIZE: usize = 20;

/// Deterministic seed shared by both judge paths so a re-run of the same
/// nightly cycle reproduces the same verdicts (audit replayability; mirrors
/// `topics.rs` label determinism).
const JUDGE_SEED: u32 = 0xC0_07_AD_1C;

// ─── Pairwise judge (primary path, ADR-062) ─────────────────────────────────

/// JSON schema the pairwise judge is constrained to emit.
///
/// ADR-062 iteration 2 (2026-05-30, from the live Phi-4 dogfood):
/// - `stale` is an **enum** `"a" | "b" | "neither"`, NOT a UUID. Real Phi-4
///   mangled the 36-char UUID it was asked to echo (`…51081}` — a brace inside
///   the string), which failed to parse. A 1-token side label can't be
///   corrupted, and "neither" cleanly covers no-contradiction / undecidable.
/// - `shared_attribute` is **required first**: the single attribute BOTH
///   memories describe, or `null`. Phi-4 flagged "works at Atlas" vs "enjoys
///   hiking" as a contradiction ("incompatible claims about occupation and
///   hobbies"). Forcing it to name ONE shared attribute makes that
///   different-attribute case self-reject; the aggregator also refuses to act
///   when `shared_attribute` is null (belt-and-braces precision gate).
const CONTRADICTION_PAIR_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "shared_attribute": {
            "type": ["string", "null"],
            "description": "The SINGLE-VALUED attribute that BOTH memories describe about the same subject — one the person holds only ONE current value of (e.g. 'employer', 'city of residence', 'favorite color'). null if the two memories describe DIFFERENT attributes (e.g. one a job, the other a hobby), are unrelated, OR describe distinct events/occurrences a person accumulates many of (e.g. separate meetings, trips, purchases, recaps) rather than one single-valued attribute."
        },
        "contradiction": {
            "type": "boolean",
            "description": "true ONLY when shared_attribute is non-null AND the two memories make incompatible claims about that one shared attribute. Must be false when shared_attribute is null."
        },
        "stale": {
            "enum": ["a", "b", "neither"],
            "description": "Which memory is NO LONGER TRUE: 'a' (the first) or 'b' (the second), superseded by the other. 'neither' when there is no contradiction, or they conflict but you cannot tell which is current."
        },
        "reasoning": { "type": "string" }
    },
    "required": ["shared_attribute", "contradiction", "stale", "reasoning"],
    "additionalProperties": false
}"#;

/// System prompt for the pairwise contradiction judge. Conservative; the
/// few-shot set includes the exact live false-positive (employment vs hobby)
/// alongside the real Vega→Atlas positive.
const CONTRADICTION_PAIR_SYSTEM_PROMPT: &str =
    "You are a contradiction detector for a personal memory vault. You are given EXACTLY TWO \
     memories about the same person, labelled \"a\" and \"b\", each with content and an as_of \
     date (when the fact became true). \
     First identify shared_attribute: the SINGLE attribute that BOTH memories describe about the \
     same subject (e.g. 'employer', 'city of residence', 'favorite color', 'marital status'). If \
     the two memories describe DIFFERENT attributes (for example one is about a job and the other \
     about a hobby) or are unrelated, set shared_attribute to null. \
     Set contradiction=true ONLY when shared_attribute is non-null AND the two memories make \
     INCOMPATIBLE claims about that one shared attribute, such that they cannot both be currently \
     true. If shared_attribute is null, contradiction MUST be false. Near-duplicates of the same \
     fact are NOT contradictions. \
     Only flag a contradiction when the shared attribute is SINGLE-VALUED — one a person can hold \
     only ONE current value of at a time (employer, city of residence, marital status, favorite \
     colour). Many facts instead describe distinct EVENTS or occurrences — meetings, trips, \
     purchases, deliveries, tasks, messages, recaps, sign-ups, sessions — where a person \
     legitimately accumulates MANY entries that differ in person, date, day, time, or place. Two \
     such entries are BOTH true and are NOT a contradiction even when worded almost identically: \
     set shared_attribute=null, contradiction=false, stale='neither'. A difference in date, day, \
     time, location, or the people involved is the signature of two distinct events, not a \
     superseded single-valued fact. \
     When contradiction=true, set stale to the memory that is NO LONGER TRUE — superseded by the \
     other (use the as_of dates, an explicit replacement such as 'having left X', or a more \
     specific statement): 'a' or 'b'. If they conflict on the shared attribute but you cannot \
     tell which is current, set stale='neither'. When contradiction=false, set stale='neither'. \
     Examples: \
     (1) a:'As of 2026-01-10 works at Vega Bridgeworks' b:'As of 2026-04-01 works at Atlas \
     Structures, having left Vega' → shared_attribute='employer', contradiction=true, stale='a' \
     (Vega is superseded). \
     (2) a:'Works at Atlas Structures' b:'Commutes to work by train' → shared_attribute=null \
     (employer vs commute method — different attributes), contradiction=false, stale='neither'. \
     (3) a:'Works as a structural engineer at Atlas Structures' b:'Enjoys hiking in the Cascade \
     mountains on weekends' → shared_attribute=null (occupation vs hobby — different attributes), \
     contradiction=false, stale='neither'. \
     (4) a:'Favorite color is blue' b:'Favorite food is pasta' → shared_attribute=null, \
     contradiction=false, stale='neither'. \
     (5) a:'Works at Atlas Structures' b:'Works as a structural engineer at Atlas Structures' → \
     shared_attribute='employer', contradiction=false (the same fact, a near-duplicate), \
     stale='neither'. \
     (6) a:'Lives in Berlin' b:'Lives in Lisbon' → shared_attribute='city of residence', \
     contradiction=true; if no date disambiguates, stale='neither'. \
     (7) a:'Met Sarah for coffee on Monday' b:'Met Sarah for coffee on Thursday' → \
     shared_attribute=null (two DISTINCT coffee meetings on different days — an event a person has \
     many of, not one single-valued attribute), contradiction=false, stale='neither'. \
     (8) a:'Recap of the cafeteria menu — Jenna shared the update yesterday' b:'Recap of the \
     cafeteria menu — Kenji shared the update this morning' → shared_attribute=null (two distinct \
     recap events, different people and times), contradiction=false, stale='neither'. \
     (9) a:'Flew to Paris in March for a conference' b:'Flew to Paris in June for a conference' → \
     shared_attribute=null (two SEPARATE trips to the same place on different dates — travel is an \
     event a person repeats, not a single-valued attribute; the same place/activity recurring on a \
     different date is a new occurrence, NOT a superseded fact), contradiction=false, \
     stale='neither'. \
     When in doubt about whether two facts describe ONE single-valued attribute or TWO distinct \
     occurrences, prefer shared_attribute=null and contradiction=false — keeping both facts is \
     safe; wrongly flagging two real events as a contradiction would discard a true memory. \
     Respond with strict JSON matching the schema.";

/// Which member of a judged pair is no longer true. Serialises as the
/// lowercase enum the model emits (`"a"`, `"b"`, `"neither"`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StaleSide {
    /// The first memory (`memory_a`) is superseded.
    A,
    /// The second memory (`memory_b`) is superseded.
    B,
    /// No contradiction, or the current truth is undecidable — take no action.
    Neither,
}

/// One pairwise contradiction judgement.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairVerdict {
    /// The single attribute both memories describe, or `None` when they
    /// describe different attributes. The aggregator refuses to act on a
    /// `contradiction=true` verdict whose `shared_attribute` is `None`.
    #[serde(default)]
    pub shared_attribute: Option<String>,
    /// Whether the two memories make incompatible claims about
    /// `shared_attribute`.
    pub contradiction: bool,
    /// Which member is superseded — or [`StaleSide::Neither`] to abstain.
    pub stale: StaleSide,
    /// Natural-language explanation; recorded in the invalidate audit event.
    pub reasoning: String,
}

/// JSON body sent to the N-ary judge: the memories under consideration.
#[derive(Serialize)]
struct PromptMemory<'a> {
    id: String,
    content: &'a str,
    as_of: String,
}

#[derive(Serialize)]
struct PromptBody<'a> {
    memories: Vec<PromptMemory<'a>>,
}

fn prompt_body<'a>(group: &[&'a Memory]) -> PromptBody<'a> {
    PromptBody {
        memories: group
            .iter()
            .map(|m| PromptMemory {
                id: m.id.to_string(),
                content: &m.content,
                as_of: m.valid_from.to_rfc3339(),
            })
            .collect(),
    }
}

/// JSON body for the pairwise judge: two labelled memories, NO ids (the model
/// returns a side label `a`/`b`, never an id — ADR-062 iteration 2).
#[derive(Serialize)]
struct PairPromptMemory<'a> {
    content: &'a str,
    as_of: String,
}

#[derive(Serialize)]
struct PairPromptBody<'a> {
    memory_a: PairPromptMemory<'a>,
    memory_b: PairPromptMemory<'a>,
}

/// Ask the LLM whether a single pair of memories contradicts on a shared
/// attribute and, if so, which side (`a`/`b`) is stale.
///
/// # Errors
///
/// [`VaultError::Llm`] — prompt serialisation, the LLM call, or parsing the
/// response into a [`PairVerdict`] failed. The aggregator logs and skips the
/// pair (one bad pair does not abort the topic).
#[instrument(skip(a, b, llm), fields(a = %a.id, b = %b.id))]
pub async fn detect_pair_contradiction(
    a: &Memory,
    b: &Memory,
    llm: &dyn LlmProvider,
) -> VaultResult<PairVerdict> {
    let body = PairPromptBody {
        memory_a: PairPromptMemory {
            content: &a.content,
            as_of: a.valid_from.to_rfc3339(),
        },
        memory_b: PairPromptMemory {
            content: &b.content,
            as_of: b.valid_from.to_rfc3339(),
        },
    };
    let user_prompt = serde_json::to_string(&body).map_err(|e| {
        VaultError::Llm(format!(
            "pairwise contradiction prompt JSON serialisation failed: {e}"
        ))
    })?;

    let params = CompletionParams {
        max_tokens: 256,
        temperature: 0.0,
        top_p: 1.0,
        seed: Some(JUDGE_SEED),
        system_prompt: Some(guarded_system_prompt(CONTRADICTION_PAIR_SYSTEM_PROMPT)),
    };

    let raw = llm
        .complete_json(&user_prompt, CONTRADICTION_PAIR_SCHEMA, &params)
        .await
        .map_err(|e| VaultError::Llm(format!("pairwise contradiction LLM call failed: {e}")))?;

    let verdict: PairVerdict = serde_json::from_str(&raw).map_err(|e| {
        warn!(raw_response = %raw, "pairwise contradiction judge returned malformed JSON");
        VaultError::Llm(format!(
            "pairwise contradiction response failed to parse as PairVerdict: {e} (raw: {raw})"
        ))
    })?;

    Ok(verdict)
}

/// Pick the STALE member of a confirmed contradicting pair deterministically
/// by recency: the memory with the OLDER `valid_from` is superseded
/// ("newest-wins"). Returns `None` on a tie (identical `valid_from`) — the
/// current truth is then undecidable by recency, so the aggregator abstains
/// (retires neither) rather than risk serving the wrong fact as current truth.
///
/// **Bug-1 fix (2026-05-31).** The LLM only DETECTS the contradiction (and
/// names the shared attribute); CODE chooses which side to retire. Trusting the
/// model's `stale` label inverted a live Tesla→Rivian knowledge update — it
/// retired the *newer* Rivian fact and left the vault serving the stale Tesla as
/// current truth, the exact failure the product exists to prevent. Deciding the
/// stale side by recency makes that inversion structurally impossible regardless
/// of how the model labels the pair.
fn stale_by_recency(a: &Memory, b: &Memory) -> Option<MemoryId> {
    match a.valid_from.cmp(&b.valid_from) {
        Ordering::Less => Some(a.id),    // a is older → a is stale
        Ordering::Greater => Some(b.id), // b is older → b is stale
        Ordering::Equal => None,         // tie → undecidable by recency → abstain
    }
}

/// Judge an explicit set of candidate pairs and union the confirmed stale ids.
///
/// This is the A5 aggregator the orchestrator's Phase 2b calls directly with
/// the nearest-neighbor candidate pairs from [`super::candidates`]. Each pair
/// is judged by [`detect_pair_contradiction`]. A pair is acted on ONLY when the
/// model confirms a contradiction AND names a `shared_attribute` (the precision
/// gate — a null shared attribute means it could not articulate common ground,
/// so we refuse). Which side is then retired is decided by CODE, deterministically
/// by recency ([`stale_by_recency`]: older `valid_from` is stale, "newest-wins").
/// The model's `stale` label does NOT drive retirement (the Bug-1 fix — a
/// mislabelled side inverted a live knowledge update). A pair whose two facts
/// share an identical `valid_from` is undecidable by recency and is skipped
/// (abstain). Ids are deduplicated; a pair-level LLM error is logged and skipped
/// (one bad pair never aborts the run).
///
/// Because recency keeps the single newest fact in any contradiction chain, the
/// aggregator never flags the globally-newest member; the orchestrator's "never
/// retire the entire active set" guard remains as belt-and-braces defense.
#[instrument(skip(pairs, llm), fields(pair_count = pairs.len()))]
pub async fn judge_candidate_pairs(
    pairs: &[(&Memory, &Memory)],
    llm: &dyn LlmProvider,
) -> VaultResult<ContradictionVerdict> {
    let mut stale_ids: Vec<MemoryId> = Vec::new();
    let mut reasons: Vec<String> = Vec::new();

    for &(a, b) in pairs {
        let verdict = match detect_pair_contradiction(a, b, llm).await {
            Ok(v) => v,
            Err(e) => {
                warn!(
                    a = %a.id,
                    b = %b.id,
                    error = %e,
                    "pairwise judge failed for pair; skipping (next cycle retries)"
                );
                continue;
            }
        };

        if !verdict.contradiction {
            continue;
        }
        // Precision gate: a contradiction must name the single shared
        // attribute. No shared attribute = the model could not articulate
        // common ground (the employment-vs-hobby false positive) → refuse.
        if verdict.shared_attribute.is_none() {
            warn!(
                a = %a.id,
                b = %b.id,
                "pairwise judge flagged a contradiction without a shared attribute; refusing"
            );
            continue;
        }
        // Bug-1 fix: the model DETECTED the contradiction; CODE picks which
        // side is stale, by recency (newest-wins) — NOT `verdict.stale`. A
        // mislabelled side inverted a live knowledge update (retired the
        // newer fact). A tie (identical `valid_from`) is undecidable by
        // recency → abstain, keeping both, rather than risk serving the
        // wrong fact as current truth.
        let stale = match stale_by_recency(a, b) {
            Some(id) => id,
            None => {
                warn!(
                    a = %a.id,
                    b = %b.id,
                    "contradiction confirmed but the two facts share an identical valid_from; \
                     cannot pick the stale side by recency — abstaining (keeping both)"
                );
                continue;
            }
        };
        if !stale_ids.contains(&stale) {
            stale_ids.push(stale);
            reasons.push(verdict.reasoning);
        }
    }

    Ok(ContradictionVerdict {
        stale_memory_ids: stale_ids,
        reasoning: reasons.join("; "),
    })
}

/// Judge a group all-pairs and union the confirmed stale ids — the convenience
/// wrapper that builds every unordered pair of `group` and delegates to
/// [`judge_candidate_pairs`]. Used by the N-ary dispatcher
/// ([`detect_contradiction`]) at/below the pairwise cap and by the unit tests;
/// the production A5 path generates candidate pairs by nearest-neighbor
/// (ADR-065) and calls [`judge_candidate_pairs`] directly rather than judging
/// every pair of a group.
#[instrument(skip(group, llm), fields(group_size = group.len()))]
pub async fn detect_contradictions_pairwise(
    group: &[&Memory],
    llm: &dyn LlmProvider,
) -> VaultResult<ContradictionVerdict> {
    let mut pairs: Vec<(&Memory, &Memory)> = Vec::new();
    for i in 0..group.len() {
        for j in (i + 1)..group.len() {
            pairs.push((group[i], group[j]));
        }
    }
    judge_candidate_pairs(&pairs, llm).await
}

// ─── Dispatcher ─────────────────────────────────────────────────────────────

/// Detect contradictions in a topic group, returning the stale member ids.
///
/// Dispatches to the pairwise judge ([`detect_contradictions_pairwise`], the
/// precision path) for groups up to [`MAX_PAIRWISE_GROUP_SIZE`], and to the
/// single N-ary judge ([`detect_contradiction_nary`]) above that for bounded
/// cost. The fallback is logged at WARN — never a silent correctness drop.
///
/// `group` is the hydrated set of memories assigned to one topic (size ≥ 2 —
/// the caller skips singletons).
///
/// # Errors
///
/// [`VaultError::Llm`] only from the N-ary fallback path (its single call /
/// parse can fail). The pairwise path never returns an error: per-pair
/// failures are logged and skipped. The caller logs and continues either way.
#[instrument(skip(group, llm), fields(group_size = group.len()))]
pub async fn detect_contradiction(
    group: &[&Memory],
    llm: &dyn LlmProvider,
) -> VaultResult<ContradictionVerdict> {
    if group.len() <= MAX_PAIRWISE_GROUP_SIZE {
        detect_contradictions_pairwise(group, llm).await
    } else {
        warn!(
            group_size = group.len(),
            cap = MAX_PAIRWISE_GROUP_SIZE,
            "topic group exceeds pairwise cap; falling back to single N-ary judge for bounded \
             cost (cosine-prune fast-follow tracked in HANDOFF)"
        );
        detect_contradiction_nary(group, llm).await
    }
}

// ─── N-ary judge (bounded-cost fallback for oversized groups) ───────────────

/// JSON schema (string form) the N-ary judge is constrained to emit.
const CONTRADICTION_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "stale_memory_ids": {
            "type": "array",
            "items": { "type": "string" },
            "description": "IDs of the facts in the group that are NO LONGER TRUE (superseded). Empty when there is no contradiction or none can be confidently retired."
        },
        "reasoning": { "type": "string" }
    },
    "required": ["stale_memory_ids", "reasoning"],
    "additionalProperties": false
}"#;

/// System prompt for the N-ary contradiction judge (oversized-group fallback).
const CONTRADICTION_SYSTEM_PROMPT: &str =
    "You are a contradiction detector for a personal memory vault. You receive a group of \
     memories about one person that a clustering pass grouped under a shared topic. Each carries \
     an id, content, and as_of date (when the fact became true). Decide whether the group \
     contains a CONTRADICTION: two or more facts that make INCOMPATIBLE claims about the SAME \
     attribute of the SAME subject, such that they cannot both be currently true (e.g., 'works \
     at Vega' vs 'works at Atlas, having left Vega'; 'lives in Berlin' vs 'lives in Lisbon'). \
     Do NOT flag facts that are merely related or co-topical but compatible (e.g., 'works at \
     Atlas' and 'commutes by train' are BOTH true — return an empty list). Do NOT flag facts \
     about different subjects or different attributes. When a contradiction exists, return \
     stale_memory_ids = the ids of the fact(s) that are NO LONGER TRUE — superseded by a more \
     recent fact (use the as_of dates), a more specific fact, or an explicit replacement \
     ('having left X'). NEVER include every id: at least one fact must remain as the current \
     truth. If facts contradict but you cannot tell which is current, return an empty list and \
     explain why in reasoning. Respond with strict JSON matching the schema.";

/// One contradiction judgement over a topic group.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContradictionVerdict {
    /// IDs of memories in the group that are no longer true and should be
    /// invalidated. Empty = no contradiction (or none confidently
    /// resolvable). The orchestrator filters these to the group and refuses
    /// to invalidate the entire group as a safety net.
    #[serde(default)]
    pub stale_memory_ids: Vec<MemoryId>,
    /// The model's natural-language explanation; recorded in the invalidate
    /// audit event for each retired fact.
    pub reasoning: String,
}

/// Single N-ary judge over the whole group — the bounded-cost fallback for
/// groups above [`MAX_PAIRWISE_GROUP_SIZE`]. Less precise than pairwise (this
/// is the very dilution ADR-062 moved away from) but O(1) calls.
///
/// # Errors
///
/// [`VaultError::Llm`] — prompt serialisation, the LLM call, or parsing the
/// response into a [`ContradictionVerdict`] failed.
#[instrument(skip(group, llm), fields(group_size = group.len()))]
pub async fn detect_contradiction_nary(
    group: &[&Memory],
    llm: &dyn LlmProvider,
) -> VaultResult<ContradictionVerdict> {
    let body = prompt_body(group);
    let user_prompt = serde_json::to_string(&body).map_err(|e| {
        VaultError::Llm(format!(
            "contradiction prompt JSON serialisation failed: {e}"
        ))
    })?;

    let params = CompletionParams {
        max_tokens: 256,
        temperature: 0.0,
        top_p: 1.0,
        seed: Some(JUDGE_SEED),
        system_prompt: Some(guarded_system_prompt(CONTRADICTION_SYSTEM_PROMPT)),
    };

    let raw = llm
        .complete_json(&user_prompt, CONTRADICTION_SCHEMA, &params)
        .await
        .map_err(|e| VaultError::Llm(format!("contradiction LLM call failed: {e}")))?;

    let verdict: ContradictionVerdict = serde_json::from_str(&raw).map_err(|e| {
        warn!(raw_response = %raw, "contradiction judge returned malformed JSON");
        VaultError::Llm(format!(
            "contradiction LLM response failed to parse as ContradictionVerdict: {e} (raw: {raw})"
        ))
    })?;

    Ok(verdict)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};
    use vault_core::{Boundary, MemoryType, NewMemory};
    use vault_llm::error::VaultLlmResult;
    use vault_llm::MockLlmProvider;

    fn mem(content: &str) -> Memory {
        Memory::try_new(NewMemory {
            content: content.into(),
            memory_type: MemoryType::Semantic,
            boundary: Boundary::new("testeval").unwrap(),
            source_agent: None,
            confidence: 0.95,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        })
        .expect("valid memory")
    }

    /// A memory with an explicit `valid_from` (fact-time). Recency-deterministic
    /// stale selection (the Bug-1 fix) decides which side of a contradiction is
    /// retired by comparing `valid_from`, so aggregator tests must pin dates
    /// rather than lean on construction-order `now()` resolution.
    fn mem_at(content: &str, valid_from: chrono::DateTime<Utc>) -> Memory {
        Memory::try_new(NewMemory {
            content: content.into(),
            memory_type: MemoryType::Semantic,
            boundary: Boundary::new("testeval").unwrap(),
            source_agent: None,
            confidence: 0.95,
            valid_from: Some(valid_from),
            valid_until: None,
            metadata: serde_json::json!({}),
        })
        .expect("valid memory")
    }

    /// 2026-MM-DD at midnight UTC — terse fact-time builder for tests.
    fn day(month: u32, d: u32) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, month, d, 0, 0, 0)
            .single()
            .expect("valid date")
    }

    /// Per-call routing closure: given the two memory contents in a pairwise
    /// judge prompt (`memory_a`, `memory_b`), return the raw JSON verdict.
    type RouteFn = dyn Fn(&str, &str) -> String + Send + Sync;

    /// Test mock that parses the pairwise judge prompt, extracts the two
    /// memory contents, and delegates to a per-call closure to decide the raw
    /// JSON response. Lets one provider serve every pair in a group with
    /// pair-specific verdicts — `MockLlmProvider` returns one canned string
    /// for all calls, which cannot exercise pairwise routing. The pair prompt
    /// carries NO ids (the model returns a side label `a`/`b`), so the mock
    /// routes on content.
    struct RoutedMock {
        route: Arc<RouteFn>,
    }

    impl std::fmt::Debug for RoutedMock {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("RoutedMock")
        }
    }

    impl RoutedMock {
        fn new(route: impl Fn(&str, &str) -> String + Send + Sync + 'static) -> Self {
            Self {
                route: Arc::new(route),
            }
        }
    }

    #[async_trait]
    impl LlmProvider for RoutedMock {
        async fn complete_json(
            &self,
            prompt: &str,
            _schema: &str,
            _params: &CompletionParams,
        ) -> VaultLlmResult<String> {
            #[derive(Deserialize)]
            struct ParsedMem {
                content: String,
            }
            #[derive(Deserialize)]
            struct ParsedBody {
                memory_a: ParsedMem,
                memory_b: ParsedMem,
            }
            let parsed: ParsedBody = serde_json::from_str(prompt)
                .expect("RoutedMock: pairwise judge prompt must be valid JSON");
            Ok((self.route)(
                &parsed.memory_a.content,
                &parsed.memory_b.content,
            ))
        }

        fn model_id(&self) -> &str {
            "routed-mock"
        }
    }

    /// Build a raw pairwise verdict JSON. `shared` = the shared attribute (or
    /// `None` for null); `stale` is the side label `"a"`/`"b"`/`"neither"`.
    fn verdict_json(
        shared: Option<&str>,
        contradiction: bool,
        stale: &str,
        reasoning: &str,
    ) -> String {
        let shared = match shared {
            Some(s) => format!("\"{s}\""),
            None => "null".to_string(),
        };
        format!(
            r#"{{"shared_attribute":{shared},"contradiction":{contradiction},"stale":"{stale}","reasoning":"{reasoning}"}}"#
        )
    }

    /// True for the Vega fact (mentions Vega, not the Atlas replacement).
    fn is_vega(content: &str) -> bool {
        content.contains("Vega") && !content.contains("Atlas")
    }

    // ─── pairwise judge (single pair) ──────────────────────────────────────

    #[tokio::test]
    async fn pair_judge_parses_contradiction_with_stale_side() {
        let vega = mem("As of 2026-01-10 the user worked at Vega Bridgeworks.");
        let atlas = mem("As of 2026-04-01 the user works at Atlas Structures, having left Vega.");
        let llm = MockLlmProvider::new(
            "phi-4-test",
            verdict_json(Some("employer"), true, "a", "Atlas supersedes Vega"),
        );
        let v = detect_pair_contradiction(&vega, &atlas, &llm)
            .await
            .unwrap();
        assert!(v.contradiction);
        assert_eq!(v.shared_attribute.as_deref(), Some("employer"));
        assert_eq!(v.stale, StaleSide::A);
    }

    #[tokio::test]
    async fn pair_judge_parses_no_contradiction_with_null_shared_attribute() {
        let a = mem("The user works at Atlas Structures.");
        let b = mem("The user commutes to work by train.");
        let llm = MockLlmProvider::new(
            "phi-4-test",
            verdict_json(None, false, "neither", "different attributes; both true"),
        );
        let v = detect_pair_contradiction(&a, &b, &llm).await.unwrap();
        assert!(!v.contradiction);
        assert_eq!(v.shared_attribute, None);
        assert_eq!(v.stale, StaleSide::Neither);
    }

    #[tokio::test]
    async fn pair_judge_conflict_but_undecidable_yields_neither() {
        // A genuine conflict where the current truth can't be determined must
        // parse as contradiction=true, stale="neither" → the aggregator abstains.
        let a = mem("The user lives in Berlin.");
        let b = mem("The user lives in Lisbon.");
        let llm = MockLlmProvider::new(
            "phi-4-test",
            verdict_json(
                Some("city of residence"),
                true,
                "neither",
                "conflicting cities, no date to disambiguate",
            ),
        );
        let v = detect_pair_contradiction(&a, &b, &llm).await.unwrap();
        assert!(v.contradiction);
        assert_eq!(v.stale, StaleSide::Neither);
    }

    #[tokio::test]
    async fn pair_judge_malformed_json_surfaces_as_llm_error() {
        let a = mem("a");
        let b = mem("b");
        let llm = MockLlmProvider::new("phi-4-test", "not json at all");
        let err = detect_pair_contradiction(&a, &b, &llm).await.unwrap_err();
        assert!(matches!(err, VaultError::Llm(_)), "got {err:?}");
    }

    // ─── aggregator (pairwise over a group) ────────────────────────────────

    /// THE precision case (the live false-positive bug, 2026-05-30). A group
    /// of three facts where exactly ONE pair contradicts; the unrelated third
    /// fact must never be flagged.
    #[tokio::test]
    async fn aggregator_flags_only_the_contradicting_pair_not_unrelated_third() {
        let vega = mem_at(
            "As of 2026-01-10 the user worked at Vega Bridgeworks.",
            day(1, 10),
        );
        let atlas = mem_at(
            "As of 2026-04-01 the user works at Atlas Structures, having left Vega.",
            day(4, 1),
        );
        let probe = mem_at("The user's determinism token is xyzzy-plugh.", day(2, 15));
        let vega_id = vega.id;
        let group = [&vega, &atlas, &probe];

        // Any pair touching the unrelated probe is compatible; the Vega/Atlas
        // pair contradicts. Recency retires the OLDER (Vega) regardless of the
        // model's `stale` label — so the mock can label either side.
        let llm = RoutedMock::new(|ca, cb| {
            if ca.contains("xyzzy") || cb.contains("xyzzy") {
                verdict_json(
                    None,
                    false,
                    "neither",
                    "unrelated probe; different attribute",
                )
            } else {
                verdict_json(Some("employer"), true, "b", "Atlas supersedes Vega")
            }
        });

        let verdict = detect_contradictions_pairwise(&group, &llm).await.unwrap();
        assert_eq!(
            verdict.stale_memory_ids,
            vec![vega_id],
            "only the older Vega fact must be flagged; the unrelated probe must NOT be"
        );
    }

    /// Union/dedup: Vega + two duplicate Atlas facts. Each Atlas/Vega pair
    /// marks Vega stale; the Atlas/Atlas pair is a duplicate (no conflict).
    /// Result: Vega appears once; both Atlas facts survive.
    #[tokio::test]
    async fn aggregator_dedups_stale_id_across_pairs_and_keeps_duplicates() {
        let vega = mem_at(
            "As of 2026-01-10 the user worked at Vega Bridgeworks.",
            day(1, 10),
        );
        let atlas1 = mem_at(
            "As of 2026-04-01 the user works at Atlas Structures, having left Vega.",
            day(4, 1),
        );
        let atlas2 = mem_at(
            "As of 2026-04-02 the user is a structural engineer at Atlas Structures.",
            day(4, 2),
        );
        let vega_id = vega.id;
        let group = [&vega, &atlas1, &atlas2];

        // Both Vega/Atlas pairs contradict (recency → Vega, the oldest, stale);
        // the Atlas/Atlas pair is a duplicate, not a contradiction.
        let llm = RoutedMock::new(|ca, cb| {
            if is_vega(ca) || is_vega(cb) {
                verdict_json(Some("employer"), true, "a", "Atlas supersedes Vega")
            } else {
                verdict_json(
                    None,
                    false,
                    "neither",
                    "duplicate Atlas facts; not a contradiction",
                )
            }
        });

        let verdict = detect_contradictions_pairwise(&group, &llm).await.unwrap();
        assert_eq!(
            verdict.stale_memory_ids,
            vec![vega_id],
            "Vega (oldest) must appear exactly once; both newer Atlas facts must survive"
        );
    }

    /// A per-pair LLM error must not abort the topic: the other pairs are
    /// still judged. Here any pair touching the malformed-response fact errors
    /// while the clean Vega/Atlas pair is still resolved.
    #[tokio::test]
    async fn aggregator_skips_failed_pair_and_judges_the_rest() {
        let vega = mem_at(
            "As of 2026-01-10 the user worked at Vega Bridgeworks.",
            day(1, 10),
        );
        let atlas = mem_at(
            "As of 2026-04-01 the user works at Atlas Structures, having left Vega.",
            day(4, 1),
        );
        let noisy = mem_at(
            "NOISE the user's unrelated favorite number is seven.",
            day(2, 1),
        );
        let vega_id = vega.id;
        let group = [&vega, &atlas, &noisy];

        let llm = RoutedMock::new(|ca, cb| {
            if ca.contains("NOISE") || cb.contains("NOISE") {
                "not valid json".to_string() // pair judge errors → skipped
            } else {
                verdict_json(Some("employer"), true, "b", "Atlas supersedes Vega")
            }
        });

        let verdict = detect_contradictions_pairwise(&group, &llm).await.unwrap();
        assert_eq!(
            verdict.stale_memory_ids,
            vec![vega_id],
            "the clean pair must still resolve even though a pair touching the noisy fact errored"
        );
    }

    /// The precision gate (ADR-062 iteration 2): a verdict that flags a
    /// contradiction WITHOUT naming a shared attribute (the live
    /// employment-vs-hobby false positive) must be refused — nothing retired.
    #[tokio::test]
    async fn aggregator_refuses_contradiction_without_shared_attribute() {
        let employment = mem("The user works as a structural engineer at Atlas Structures.");
        let hobby = mem("The user enjoys hiking in the Cascade mountains on weekends.");
        let group = [&employment, &hobby];

        // Mirrors real Phi-4: contradiction=true, stale="a", but shared=null.
        let llm = RoutedMock::new(|_ca, _cb| {
            verdict_json(
                None,
                true,
                "a",
                "wrongly called occupation and hobby incompatible",
            )
        });

        let verdict = detect_contradictions_pairwise(&group, &llm).await.unwrap();
        assert!(
            verdict.stale_memory_ids.is_empty(),
            "a contradiction with no named shared attribute must be refused, not acted on"
        );
    }

    /// Multi-fact conflict chain: three same-attribute facts with distinct
    /// dates all contradict pairwise. Recency keeps the single NEWEST as the
    /// current truth and retires the two older ones — the globally-newest member
    /// is never flagged (it is never the older side of any pair). This is the
    /// correct knowledge-update behavior, and it means a pairwise run can never
    /// sweep an entire group (the orchestrator's mass-invalidate guard remains
    /// as belt-and-braces defense, unit-tested at the orchestrator layer).
    #[tokio::test]
    async fn aggregator_recency_keeps_only_the_newest_in_a_conflict_chain() {
        let berlin = mem_at("The user lives in Berlin.", day(1, 1));
        let lisbon = mem_at("The user lives in Lisbon.", day(2, 1));
        let madrid = mem_at("The user lives in Madrid.", day(3, 1));
        let (berlin_id, lisbon_id, madrid_id) = (berlin.id, lisbon.id, madrid.id);
        let group = [&berlin, &lisbon, &madrid];

        // Every pair conflicts on the same attribute; the model's `stale` label
        // is irrelevant (recency decides).
        let llm =
            RoutedMock::new(|_ca, _cb| verdict_json(Some("city of residence"), true, "a", "chain"));

        let verdict = detect_contradictions_pairwise(&group, &llm).await.unwrap();
        assert_eq!(
            verdict.stale_memory_ids,
            vec![berlin_id, lisbon_id],
            "the two older cities retire; the newest (Madrid) survives as current truth"
        );
        assert!(
            !verdict.stale_memory_ids.contains(&madrid_id),
            "the newest fact must never be flagged stale"
        );
    }

    /// THE Bug-1 regression (live dogfood 2026-05-31). The model INVERTS — it
    /// labels the NEWER fact as stale, exactly as Phi-4 did on the Tesla→Rivian
    /// update (retiring the newer Rivian and serving the stale Tesla as current
    /// truth). The aggregator must IGNORE the model's side label and retire the
    /// OLDER fact by recency. This test FAILS on the pre-fix code (which mapped
    /// `stale="b"` → the newer Rivian) and PASSES with recency-deterministic
    /// selection.
    #[tokio::test]
    async fn aggregator_retires_older_fact_even_when_model_mislabels_newer_as_stale() {
        let tesla = mem_at("The user drives a Tesla Model 3.", day(2, 1));
        let rivian = mem_at(
            "The user now drives a Rivian R1S, having sold the Tesla.",
            day(5, 1),
        );
        let tesla_id = tesla.id;
        let rivian_id = rivian.id;
        // tesla = a (older), rivian = b (newer). The model wrongly names the
        // newer side ("b") as stale.
        let group = [&tesla, &rivian];

        let llm = RoutedMock::new(|_ca, _cb| {
            verdict_json(
                Some("vehicle"),
                true,
                "b",
                "model wrongly retires the newer Rivian",
            )
        });

        let verdict = detect_contradictions_pairwise(&group, &llm).await.unwrap();
        assert_eq!(
            verdict.stale_memory_ids,
            vec![tesla_id],
            "recency must retire the OLDER Tesla even though the model labelled the newer Rivian stale"
        );
        assert!(
            !verdict.stale_memory_ids.contains(&rivian_id),
            "the newer Rivian fact must remain current"
        );
    }

    /// A genuine contradiction whose two facts carry an IDENTICAL `valid_from`
    /// is undecidable by recency. The aggregator abstains (retires neither),
    /// keeping both rather than risk serving the wrong fact as current truth.
    #[tokio::test]
    async fn aggregator_abstains_on_contradiction_with_tied_valid_from() {
        let same = day(3, 1);
        let berlin = mem_at("The user lives in Berlin.", same);
        let lisbon = mem_at("The user lives in Lisbon.", same);
        let group = [&berlin, &lisbon];

        // Real contradiction, but the model's `stale` label cannot save us —
        // identical dates mean recency can't choose, so we keep both.
        let llm = RoutedMock::new(|_ca, _cb| {
            verdict_json(
                Some("city of residence"),
                true,
                "a",
                "conflict but identical dates",
            )
        });

        let verdict = detect_contradictions_pairwise(&group, &llm).await.unwrap();
        assert!(
            verdict.stale_memory_ids.is_empty(),
            "identical valid_from is undecidable by recency → abstain, retire neither"
        );
    }

    // ─── judge_candidate_pairs (explicit pair list — the A5/ADR-065 path) ───

    /// The defining property of candidate-pair judging: ONLY the pairs handed
    /// in are judged. A contradiction that exists between two facts is NOT
    /// detected if that pair was never generated as a candidate. Here the real
    /// Tesla/Rivian conflict is omitted from the list (only the unrelated pairs
    /// are passed), so nothing is retired — proving the judge never silently
    /// compares pairs outside the candidate set (unlike the old all-pairs group
    /// scan). This is why candidate generation MUST surface the conflicting pair
    /// (it does — they are mutual nearest neighbors; see `phases::candidates`).
    #[tokio::test]
    async fn judge_only_acts_on_pairs_it_is_given() {
        let tesla = mem_at("The user drives a Tesla Model 3.", day(2, 1));
        let rivian = mem_at(
            "The user now drives a Rivian R1S, having sold the Tesla.",
            day(5, 1),
        );
        let unrelated = mem_at("The user's favourite colour is amber.", day(3, 1));

        // The judge would flag tesla/rivian IF asked — but we never ask.
        let llm = RoutedMock::new(|ca, cb| {
            let is_car = |c: &str| c.contains("Tesla") || c.contains("Rivian");
            if is_car(ca) && is_car(cb) {
                verdict_json(Some("vehicle"), true, "a", "Rivian supersedes Tesla")
            } else {
                verdict_json(None, false, "neither", "different attributes")
            }
        });

        // Pass only the unrelated pairs; the contradicting (tesla, rivian) pair
        // is deliberately absent.
        let pairs: &[(&Memory, &Memory)] = &[(&tesla, &unrelated), (&rivian, &unrelated)];
        let verdict = judge_candidate_pairs(pairs, &llm).await.unwrap();
        assert!(
            verdict.stale_memory_ids.is_empty(),
            "a contradiction whose pair was not a candidate must NOT be detected; got {:?}",
            verdict.stale_memory_ids
        );
    }

    /// The positive: hand the conflicting pair in and the older fact retires by
    /// recency — independent of the model's `stale` label.
    #[tokio::test]
    async fn judge_retires_older_fact_for_a_given_conflicting_pair() {
        let tesla = mem_at("The user drives a Tesla Model 3.", day(2, 1));
        let rivian = mem_at(
            "The user now drives a Rivian R1S, having sold the Tesla.",
            day(5, 1),
        );
        let tesla_id = tesla.id;
        let llm =
            RoutedMock::new(|_ca, _cb| verdict_json(Some("vehicle"), true, "b", "model mislabels"));

        let pairs: &[(&Memory, &Memory)] = &[(&tesla, &rivian)];
        let verdict = judge_candidate_pairs(pairs, &llm).await.unwrap();
        assert_eq!(
            verdict.stale_memory_ids,
            vec![tesla_id],
            "the older Tesla must retire by recency even though the model labelled Rivian stale"
        );
    }

    // ─── dispatcher ────────────────────────────────────────────────────────

    /// At/below the cap the dispatcher uses the pairwise path: a 2-fact group
    /// makes exactly one pairwise call, mapping side `a` to the first member.
    #[tokio::test]
    async fn dispatcher_uses_pairwise_at_or_below_cap() {
        let a = mem_at(
            "As of 2026-01-10 the user worked at Vega Bridgeworks.",
            day(1, 10),
        );
        let b = mem_at(
            "As of 2026-04-01 the user works at Atlas Structures, having left Vega.",
            day(4, 1),
        );
        let a_id = a.id;
        let group = [&a, &b];
        let llm = MockLlmProvider::new(
            "phi-4-test",
            verdict_json(Some("employer"), true, "a", "superseded"),
        );
        let verdict = detect_contradiction(&group, &llm).await.unwrap();
        assert_eq!(verdict.stale_memory_ids, vec![a_id]);
        assert_eq!(
            llm.call_count(),
            1,
            "a 2-fact group is exactly one pairwise call"
        );
    }

    /// Above the cap the dispatcher falls back to the single N-ary judge:
    /// exactly one LLM call regardless of the (large) group size, parsing the
    /// N-ary `stale_memory_ids` array shape.
    #[tokio::test]
    async fn dispatcher_falls_back_to_nary_above_cap() {
        let mems: Vec<Memory> = (0..MAX_PAIRWISE_GROUP_SIZE + 1)
            .map(|i| mem(&format!("fact number {i} about the user")))
            .collect();
        let group: Vec<&Memory> = mems.iter().collect();
        let stale_id = mems[0].id;
        // N-ary response shape (array), not the pairwise shape.
        let llm = MockLlmProvider::new(
            "phi-4-test",
            format!(r#"{{"stale_memory_ids":["{stale_id}"],"reasoning":"nary fallback"}}"#),
        );
        let verdict = detect_contradiction(&group, &llm).await.unwrap();
        assert_eq!(verdict.stale_memory_ids, vec![stale_id]);
        assert_eq!(
            llm.call_count(),
            1,
            "oversized group must collapse to a single N-ary call, not O(N²) pairwise calls"
        );
    }

    // ─── N-ary fallback judge (direct) ─────────────────────────────────────

    #[tokio::test]
    async fn nary_parses_stale_ids_on_contradiction() {
        let vega = mem("As of 2026-01-10 the user worked at Vega Bridgeworks.");
        let atlas = mem("As of 2026-04-01 the user works at Atlas Structures, having left Vega.");
        let group = [&vega, &atlas];
        let llm = MockLlmProvider::new(
            "phi-4-test",
            format!(
                r#"{{"stale_memory_ids":["{}"],"reasoning":"Atlas supersedes Vega"}}"#,
                vega.id
            ),
        );
        let verdict = detect_contradiction_nary(&group, &llm).await.unwrap();
        assert_eq!(verdict.stale_memory_ids, vec![vega.id]);
    }

    #[tokio::test]
    async fn nary_empty_stale_ids_field_defaults_when_absent() {
        let a = mem("a");
        let b = mem("b");
        let group = [&a, &b];
        let llm = MockLlmProvider::new("phi-4-test", r#"{"reasoning":"no conflict"}"#);
        let verdict = detect_contradiction_nary(&group, &llm).await.unwrap();
        assert!(verdict.stale_memory_ids.is_empty());
    }

    // ─── REAL Phi-4 — the ADR-083 over-retention guard ──────────────────────
    //
    // The recall-safety acceptance for the single-valued-vs-event prompt fix.
    // A prompt change only matters if the REAL model behaves differently, so a
    // MockLlm test cannot prove it — this loads Phi-4 and asserts the agreed
    // "keep when unsure" posture across three buckets:
    //   - CLEAR distinct events (two coffee meetings, two office recaps, two
    //     Paris trips) MUST keep BOTH facts — the over-retention guard, the one
    //     unrescuable risk (a retired fact leaves default retrieval);
    //   - CLEAR single-valued updates (Berlin→Lisbon, Vega→Atlas) MUST still
    //     retire the older fact — the fix must not blunt real detection;
    //   - GENUINELY AMBIGUOUS cases (Denver coordinator reassign-vs-two-sessions;
    //     Tesla→Rivian, since one can own two cars) are INFORMATIONAL — printed,
    //     not asserted, because neither outcome is wrong. Phi-4-mini's judgment
    //     ceiling lives here; the safe-direction failure (keep) is covered by
    //     decay + cold-archive (A1) + reversible checkpoints, not by forcing a
    //     retire the model cannot reliably make.
    //
    // Ignored (needs the ~2.5 GB GGUF + minutes of CPU inference). Run:
    //   $env:PHI4_MODEL_DIR="$env:APPDATA\com.zaaheen.app\models"
    //   cargo test -p vault-consolidator --lib real_phi4_distinct_events_not_retired -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "loads the real Phi-4 GGUF; set PHI4_MODEL_DIR; run with --ignored --nocapture"]
    async fn real_phi4_distinct_events_not_retired() {
        use vault_llm::{Phi4MiniConfig, Phi4MiniProvider};

        let model_dir = std::env::var("PHI4_MODEL_DIR").unwrap_or_else(|_| {
            // Derived, never hardcoded. The literal that was here embedded a
            // username AND named com.shahbaz242630.memory-vault - a directory
            // deleted with the pre-Zaaheen build in session 34, so the fallback
            // pointed at nothing on the one machine it was written for.
            let appdata = std::env::var("APPDATA")
                .expect("set PHI4_MODEL_DIR, or run where APPDATA names the data directory");
            format!(r"{appdata}\com.zaaheen.app\models")
        });
        println!("\nloading Phi-4-mini from {model_dir}...");
        let provider = Phi4MiniProvider::new(Phi4MiniConfig::v0_2_default(model_dir.into()))
            .await
            .expect("load real Phi-4-mini");

        // Helper: judge one ordered pair (a = older) and return the retired ids.
        async fn retired(a: &Memory, b: &Memory, llm: &dyn LlmProvider) -> Vec<MemoryId> {
            judge_candidate_pairs(&[(a, b)], llm)
                .await
                .expect("judge pair")
                .stale_memory_ids
        }

        // ── BUCKET 1: CLEAR distinct events — MUST keep both (retired empty). ──
        // The over-retention guard: retiring either is unrescuable data loss.
        let clear_events = [
            (
                mem_at("Met Sarah for coffee on Monday", day(3, 2)),
                mem_at("Met Sarah for coffee on Thursday", day(3, 5)),
            ),
            (
                mem_at(
                    "Recap of the cafeteria menu — Jenna shared the update",
                    day(4, 1),
                ),
                mem_at(
                    "Recap of the cafeteria menu — Kenji shared the update",
                    day(4, 8),
                ),
            ),
            (
                mem_at("Flew to Paris in March for a conference", day(3, 15)),
                mem_at("Flew to Paris in June for a conference", day(6, 15)),
            ),
        ];
        let mut event_failures = Vec::new();
        for (a, b) in &clear_events {
            let r = retired(a, b, &provider).await;
            println!(
                "EVENT  retired={r:?}\n   a: {}\n   b: {}",
                a.content, b.content
            );
            if !r.is_empty() {
                event_failures.push((a.content.clone(), b.content.clone()));
            }
        }

        // ── BUCKET 2: CLEAR single-valued updates — MUST retire the older. ──
        let clear_updates = [
            (
                mem_at("The user lives in Berlin", day(1, 10)),
                mem_at("The user lives in Lisbon", day(5, 1)),
            ),
            (
                mem_at("The user works at Vega Bridgeworks", day(1, 10)),
                mem_at(
                    "The user works at Atlas Structures, having left Vega",
                    day(4, 1),
                ),
            ),
        ];
        let mut update_failures = Vec::new();
        for (older, newer) in &clear_updates {
            let r = retired(older, newer, &provider).await;
            println!(
                "UPDATE retired={r:?} (expect [{}])\n   older: {}\n   newer: {}",
                older.id, older.content, newer.content
            );
            if r != vec![older.id] {
                update_failures.push((older.content.clone(), newer.content.clone()));
            }
        }

        // ── BUCKET 3: GENUINELY AMBIGUOUS — informational only, never asserted. ──
        // Either outcome is defensible; we print to observe, not to gate.
        let ambiguous = [
            (
                mem_at(
                    "Supply closet inventory in Denver — Sam coordinating",
                    day(2, 10),
                ),
                mem_at(
                    "Supply closet inventory in Denver — Aisha coordinating",
                    day(2, 17),
                ),
            ),
            (
                mem_at("The user drives a Tesla Model 3", day(1, 1)),
                mem_at("The user drives a Rivian R1T", day(6, 1)),
            ),
        ];
        for (a, b) in &ambiguous {
            let r = retired(a, b, &provider).await;
            println!(
                "AMBIG  retired={r:?} (informational)\n   a: {}\n   b: {}",
                a.content, b.content
            );
        }

        println!("\n=== SUMMARY ===");
        println!(
            "clear-event pairs WRONGLY retired (must be 0): {}",
            event_failures.len()
        );
        println!(
            "clear-update pairs NOT resolved (must be 0): {}",
            update_failures.len()
        );
        assert!(
            event_failures.is_empty(),
            "OVER-RETENTION GUARD FAILED — clear distinct events were retired (data loss): {event_failures:?}"
        );
        assert!(
            update_failures.is_empty(),
            "regression — clear single-valued updates were not resolved: {update_failures:?}"
        );
    }
}
