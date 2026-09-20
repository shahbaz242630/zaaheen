//! Tests for the file somebody downloads when they are locked out.
//!
//! The awkward cases matter more than the happy one here: this runs for a
//! person who has just been told they cannot use the app, and it is the only
//! way they get their own data back.

use super::*;

use chrono::TimeZone;
use vault_core::{Boundary, MemoryId, MemoryType};

fn at(y: i32, m: u32, d: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, m, d, 12, 0, 0)
        .single()
        .expect("a real date")
}

fn memory(content: &str, topic: &str, created: DateTime<Utc>) -> Memory {
    Memory {
        id: MemoryId::new(),
        content: content.to_string(),
        memory_type: MemoryType::Semantic,
        source_agent: None,
        boundary: Boundary::new(topic).expect("a valid topic"),
        created_at: created,
        valid_from: created,
        valid_until: None,
        confidence: 0.9,
        access_count: 0,
        last_accessed: created,
        superseded_by: None,
        archived_at: None,
        embedding: None,
        metadata: serde_json::Value::Null,
    }
}

// ------------------------------------------------------------ the happy one

#[test]
fn the_file_says_what_it_is_and_where_it_came_from() {
    let out = to_markdown(
        &[memory("I prefer morning meetings", "work", at(2026, 3, 14))],
        at(2026, 9, 20),
    );

    assert!(out.starts_with("# My Zaaheen memories"));
    assert!(out.contains("Exported 20 September 2026 — 1 memory"));
    assert!(
        out.contains("This file came from your own computer. Nothing was sent anywhere."),
        "somebody who has just been locked out should not have to wonder \
         whether this was uploaded: {out}"
    );
    assert!(out.contains("## work"));
    assert!(out.contains("I prefer morning meetings"));
    assert!(out.contains("Saved 14 March 2026"));
}

#[test]
fn memories_are_grouped_by_topic_and_read_oldest_first() {
    let out = to_markdown(
        &[
            memory("later work note", "work", at(2026, 4, 2)),
            memory("a health note", "health", at(2026, 1, 9)),
            memory("earlier work note", "work", at(2026, 3, 14)),
        ],
        at(2026, 9, 20),
    );

    let health = out.find("## health").expect("health topic");
    let work = out.find("## work").expect("work topic");
    assert!(health < work, "topics should be alphabetical");

    let earlier = out.find("earlier work note").expect("earlier");
    let later = out.find("later work note").expect("later");
    assert!(
        earlier < later,
        "within a topic, a memory should read as a history: oldest first"
    );
}

/// Two exports of an unchanged vault must produce the same file, or somebody
/// comparing them cannot tell whether anything changed.
#[test]
fn the_same_vault_always_produces_the_same_file() {
    let memories = [
        memory("one", "work", at(2026, 3, 14)),
        memory("two", "health", at(2026, 3, 14)),
        memory("three", "work", at(2026, 3, 14)),
    ];
    let first = to_markdown(&memories, at(2026, 9, 20));
    let second = to_markdown(&memories, at(2026, 9, 20));
    assert_eq!(first, second);
}

// ----------------------------------------------------------- the empty vault

/// A zero-byte download reads as a broken button, and the person reading it
/// has just been locked out — the worst moment to leave somebody guessing.
#[test]
fn an_empty_vault_still_explains_itself() {
    let out = to_markdown(&[], at(2026, 9, 20));
    assert!(out.starts_with("# My Zaaheen memories"));
    assert!(out.contains("0 memories"));
    assert!(
        out.contains("no memories saved yet"),
        "an empty export must say so in words: {out}"
    );
    assert!(
        out.len() > 100,
        "an empty export must not be an empty-looking file"
    );
}

// ------------------------------------------------- a memory forging structure

/// A memory whose text starts with `##` would otherwise appear as a **topic
/// heading**, so the file would misrepresent the vault it came from — and an
/// AI reading it would file that memory under a topic nobody chose.
#[test]
fn a_memory_cannot_forge_a_topic_heading() {
    let out = to_markdown(
        &[memory("## Health\nI am fine", "work", at(2026, 3, 14))],
        at(2026, 9, 20),
    );

    assert_eq!(
        out.matches("\n## ").count(),
        1,
        "exactly one real topic heading should exist: {out}"
    );
    assert!(
        out.contains("\\## Health"),
        "the text should still be readable, just not structural: {out}"
    );
    assert!(out.contains("I am fine"));
}

#[test]
fn a_memory_cannot_forge_a_horizontal_rule_or_underline_heading() {
    let out = to_markdown(
        &[memory(
            "Important\n---\nnot a heading",
            "work",
            at(2026, 3, 14),
        )],
        at(2026, 9, 20),
    );
    assert!(
        out.contains("\\---"),
        "an underline that would turn the line above into a heading must be escaped: {out}"
    );
}

/// A `#` inside a sentence is just a character. Escaping it would make the
/// file uglier for no gain, and the whole point is that it reads well.
#[test]
fn a_hash_inside_a_sentence_is_left_alone() {
    let out = to_markdown(
        &[memory(
            "my flat is #3 on the street",
            "work",
            at(2026, 3, 14),
        )],
        at(2026, 9, 20),
    );
    assert!(out.contains("my flat is #3 on the street"));
    assert!(!out.contains("\\#3"));
}

#[test]
fn a_topic_containing_a_newline_cannot_split_its_heading() {
    // Boundary::new may well refuse this, but the export must not depend on
    // another type's validation to keep its own structure intact.
    assert_eq!(heading_safe("work\n## fake"), "work ## fake");
}

// ----------------------------------------------------------------- counting

#[test]
fn the_count_reads_naturally() {
    assert_eq!(count_phrase(0), "0 memories");
    assert_eq!(count_phrase(1), "1 memory");
    assert_eq!(count_phrase(2), "2 memories");
    assert_eq!(count_phrase(412), "412 memories");
}

#[test]
fn dates_are_written_the_way_a_person_writes_them() {
    assert_eq!(human_date(at(2026, 9, 20)), "20 September 2026");
    assert_eq!(human_date(at(2026, 1, 9)), "9 January 2026");
}

// -------------------------------------------------------------- rule detection

#[test]
fn only_real_rules_are_treated_as_rules() {
    assert!(is_rule("---"));
    assert!(is_rule("***"));
    assert!(is_rule("___"));
    assert!(is_rule("==="));
    assert!(is_rule("- - -"));
    assert!(!is_rule("--"), "two is not a rule");
    assert!(!is_rule(""), "an empty line is not a rule");
    assert!(!is_rule("-- text"), "a dash with words is not a rule");
    assert!(!is_rule("-*-"), "mixed markers are not a rule");
}

// --------------------------------- gaps the 4b/4c review found

/// `===` under a line promotes THAT line to an H1. So a memory reading
/// "My secret plan" followed by "===" turns the memory itself into a
/// heading -- the same forgery `---` was already blocked for, missed
/// because only three of the four underline characters were listed.
#[test]
fn a_memory_cannot_forge_a_setext_heading_with_equals() {
    let out = to_markdown(
        &[memory(
            "My secret plan
===",
            "work",
            at(2026, 3, 14),
        )],
        at(2026, 9, 20),
    );
    assert!(
        out.contains(r"\==="),
        "an `===` underline must be escaped, or the memory above it becomes a heading: {out}"
    );
}

/// The serious one. An unmatched code fence does not merely misformat its
/// own memory: every topic heading and memory after it is swallowed into
/// the code block, so ONE memory corrupts the rest of the document. These
/// memories come from AI conversations, so a stray fence is ordinary.
#[test]
fn one_memory_with_a_code_fence_cannot_swallow_the_rest_of_the_file() {
    let out = to_markdown(
        &[
            memory(
                "here is code:
```
let x = 1;",
                "aaa",
                at(2026, 3, 14),
            ),
            memory("a later memory", "zzz", at(2026, 4, 2)),
        ],
        at(2026, 9, 20),
    );

    assert!(out.contains(r"\```"), "the fence must be escaped: {out}");
    // Both topics must still be real headings, not code-block contents.
    assert!(out.contains(
        "
## aaa"
    ));
    assert!(out.contains(
        "
## zzz"
    ));
    assert_eq!(
        out.matches(
            "
## "
        )
        .count(),
        2,
        "both topics must survive a memory containing a fence: {out}"
    );
}

#[test]
fn a_tilde_fence_is_escaped_too() {
    let out = to_markdown(
        &[memory(
            "~~~
code",
            "work",
            at(2026, 3, 14),
        )],
        at(2026, 9, 20),
    );
    assert!(out.contains(r"\~~~"), "{out}");
}

/// A marker indented with a tab is still a marker to a renderer.
#[test]
fn an_indented_marker_is_still_escaped() {
    let out = to_markdown(
        &[memory("	## not a topic", "work", at(2026, 3, 14))],
        at(2026, 9, 20),
    );
    assert_eq!(
        out.matches(
            "
## "
        )
        .count(),
        1,
        "only the real topic heading should exist: {out}"
    );
}

#[test]
fn a_blockquote_marker_is_escaped() {
    let out = to_markdown(
        &[memory("> quoted", "work", at(2026, 3, 14))],
        at(2026, 9, 20),
    );
    assert!(out.contains(r"\> quoted"), "{out}");
}
