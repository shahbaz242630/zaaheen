//! "Download my memories" — the readable file (S4, S3 step 4c).
//!
//! # What this is for
//!
//! BRD §1.6 amendment 1: the lock screen has exactly two actions, **Subscribe**
//! and **Download my memories**, and *"the second writes a readable export
//! file"*. The reason it exists is stated there too: *"`vault.db` is
//! SQLCipher-encrypted, so the files on disk are unreadable without the app.
//! Without the export, a locked app would cut people off from their own
//! memories, and we could not honestly say 'your memories are always
//! yours'."*
//!
//! So this must work when everything else is refused. It is on §8.26 §6.4's
//! ungated allowlist, and unlike the account commands it **does** read the
//! vault — it cannot lean on their "holds nothing" argument, which is why it
//! gets its own reasoning here.
//!
//! # Why Markdown, and one file
//!
//! **Founder-locked, 2026-09-20:** one readable `.md` file. It opens in
//! Notepad, in any editor, and renders anywhere; and the most likely next
//! thing somebody does with their exported memories is hand them to another
//! AI, which reads Markdown natively. No JSON ships beside it — there is no
//! importer to feed, and a second file is a second thing to explain.
//! (`SIGNIN-DESIGN.md` §8.36.)
//!
//! # The format is a promise, not a layout
//!
//! [`to_markdown`] is pure: memories in, text out, no clock of its own and no
//! I/O. That is what makes the awkward cases testable — an empty vault, a
//! memory whose text would otherwise forge a heading, one with no topic.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use vault_core::Memory;

/// Written at the top so the reader knows what they are holding, and that
/// nothing left their computer to make it.
const HEADING: &str = "# My Zaaheen memories";

/// Turn memories into the file somebody downloads.
///
/// Grouped by topic, and within a topic oldest first so a topic reads as a
/// history rather than a stack. Topics are alphabetical, so the same vault
/// always produces the same file — which is what makes this testable at all.
///
/// `exported_at` is passed in rather than read here, so a test can pin the
/// date and the output is a pure function of its inputs.
#[must_use]
pub fn to_markdown(memories: &[Memory], exported_at: DateTime<Utc>) -> String {
    let mut out = String::new();
    out.push_str(HEADING);
    out.push_str("\n\n");

    out.push_str(&format!(
        "Exported {} — {}\n",
        human_date(exported_at),
        count_phrase(memories.len())
    ));
    out.push_str("This file came from your own computer. Nothing was sent anywhere.\n");

    if memories.is_empty() {
        // An empty vault must still produce a file that explains itself. A
        // zero-byte download reads as a failure, and somebody who has just
        // been locked out is the last person to leave guessing.
        out.push_str(
            "\nThere are no memories saved yet, so there is nothing to list here.\n\
             If you expected to see something, your memories may be on another computer.\n",
        );
        return out;
    }

    // BTreeMap so topics come out in a stable, alphabetical order.
    let mut by_topic: BTreeMap<&str, Vec<&Memory>> = BTreeMap::new();
    for memory in memories {
        by_topic
            .entry(memory.boundary.as_str())
            .or_default()
            .push(memory);
    }

    for (topic, mut in_topic) in by_topic {
        // Oldest first within a topic; `id` breaks ties so two memories saved
        // in the same second cannot swap places between two exports of the
        // same vault.
        in_topic.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.to_string().cmp(&b.id.to_string()))
        });

        out.push_str(&format!("\n## {}\n", heading_safe(topic)));
        for memory in in_topic {
            out.push('\n');
            out.push_str(&body_safe(&memory.content));
            out.push('\n');
            out.push_str(&format!("Saved {}\n", human_date(memory.created_at)));
        }
    }

    out
}

/// "20 September 2026" — the way a person writes a date, not an ISO stamp.
fn human_date(at: DateTime<Utc>) -> String {
    at.format("%-d %B %Y").to_string()
}

/// "412 memories", "1 memory".
fn count_phrase(n: usize) -> String {
    if n == 1 {
        "1 memory".to_string()
    } else {
        format!("{n} memories")
    }
}

/// A topic name that cannot break out of its heading.
///
/// Newlines are the only thing that can: a boundary containing one would end
/// the `##` line and turn the rest into body text, or a second heading.
fn heading_safe(topic: &str) -> String {
    topic.replace(['\n', '\r'], " ")
}

/// A memory's own text, kept readable but unable to forge the file's
/// structure.
///
/// The risk is not code execution — this is a text file. It is that a memory
/// whose text begins `## Health` would appear in the file as a **topic
/// heading**, so the export would misrepresent the vault it came from, and a
/// person (or an AI reading it) would file that memory under a topic nobody
/// chose. Escaping the leading marker keeps the character visible and stops
/// it being structure.
///
/// Only line-leading markers matter: a `#` mid-sentence is just a `#`.
fn body_safe(content: &str) -> String {
    content
        .lines()
        .map(|line| {
            // `trim_start` covers tabs as well as spaces: a marker indented
            // with a tab is still a marker.
            let trimmed = line.trim_start();
            let leading = &line[..line.len() - trimmed.len()];
            if starts_structure(trimmed) {
                format!("{leading}\\{trimmed}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Three or more of `-`, `*` or `_` alone on a line.
/// Does this line, with its indentation already removed, begin something the
/// file's **structure** is made of rather than its text?
///
/// Each case is a way one memory can misrepresent the file it sits in:
/// * `#` — an ATX heading: the memory appears as a topic nobody chose;
/// * `---`, `===` — a setext underline, which promotes the line **above** it
///   to a heading, so the *previous* memory changes meaning;
/// * `***`, `___` — a horizontal rule;
/// * ```` ``` ````, `~~~` — a fence, and this is the worst of them. One
///   unmatched fence swallows **every topic and memory after it** into a code
///   block, so a single memory corrupts the rest of the document. These
///   memories come from AI conversations, where code snippets are routine, so
///   it is an ordinary case rather than a contrived one.
/// * `>` — a blockquote marker; cosmetic beside the others, escaped for
///   consistency rather than because it misleads.
///
/// The `=` and fence cases were found by step 4b/4c's independent review.
fn starts_structure(trimmed: &str) -> bool {
    trimmed.starts_with('#')
        || trimmed.starts_with('>')
        || trimmed.starts_with("```")
        || trimmed.starts_with("~~~")
        || is_rule(trimmed)
}

/// Three or more of `-`, `*`, `_` or `=` alone on a line.
fn is_rule(line: &str) -> bool {
    let mut chars = line.chars().filter(|c| !c.is_whitespace()).peekable();
    let Some(first) = chars.peek().copied() else {
        return false;
    };
    if !matches!(first, '-' | '*' | '_' | '=') {
        return false;
    }
    let all_same = line
        .chars()
        .filter(|c| !c.is_whitespace())
        .all(|c| c == first);
    let count = line.chars().filter(|c| *c == first).count();
    all_same && count >= 3
}

#[cfg(test)]
mod tests;
