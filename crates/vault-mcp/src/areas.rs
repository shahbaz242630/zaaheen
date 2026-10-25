//! Areas (boundaries) as an AI app sees them — ADR-115, ADR-SEC-050.
//!
//! A connection is granted either a fixed list of areas ([`Grant::Only`], the
//! pre-0.3.7 behaviour, still used by the daemon's per-agent tokens) or
//! **every area** ([`Grant::Every`], what `zaaheen mcp serve` asks for by
//! default from 0.3.7). Every is resolved to the registry's names at each tool
//! call, so an area the user creates is readable at once, and the resolved
//! list is what reaches the storage-layer filter — exactly where a fixed list
//! went before (BRD §11.4.3 rules 3 and 4 unchanged). An empty list is refused,
//! never treated as "no filter".
//!
//! Also here: the readable answer for a save into an area that does not exist
//! (Every only; G5), and the agent guide served by `memory_help` (G7).

use vault_core::{Boundary, VaultError, VaultResult};

use crate::Adapter;

/// Which areas one AI-app connection may reach. Set by the host when the
/// connection is made (the keeper, from the authenticated handshake), never by
/// a tool argument (ADR-025).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Grant {
    /// Exactly these areas.
    Only(Vec<Boundary>),
    /// Every area in the user's registry, as it stands at each call.
    Every,
}

/// One area as `memory_help` and the unknown-area answer describe it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AreaInfo {
    pub name: Boundary,
    /// The user's short label, if they gave one.
    pub description: Option<String>,
    /// Active memories in the area.
    pub memory_count: u64,
}

/// The areas `grant` reaches right now. Never empty: an empty result is
/// `AccessDenied` (BRD §11.4.3 — a retrieval must declare its areas).
pub(crate) async fn resolve(grant: &Grant, adapter: &dyn Adapter) -> VaultResult<Vec<Boundary>> {
    let areas = match grant {
        Grant::Only(list) => list.clone(),
        Grant::Every => adapter
            .list_areas()
            .await?
            .into_iter()
            .map(|a| a.name)
            .collect(),
    };
    if areas.is_empty() {
        return Err(VaultError::AccessDenied("no boundaries authorized".into()));
    }
    Ok(areas)
}

/// Steps for the user to create an area, quoted in the refusal and the guide.
pub const CREATE_AREA_STEPS: &str = "To create a new area, the user opens the Zaaheen app, \
     clicks Boundaries at the top, clicks + New boundary, types the name (letters, numbers, \
     - or _ only, no spaces) and clicks Create boundary. Then save again.";

/// The answer to a save into `asked` when it is not one of `areas` (or is not
/// a valid name). Names only areas the app can already read (Every grant).
pub fn unknown_area_text(asked: &str, areas: &[Boundary]) -> String {
    let names: Vec<&str> = areas.iter().map(Boundary::as_str).collect();
    let close = names
        .iter()
        .find(|n| n.eq_ignore_ascii_case(asked.trim()))
        .map(|n| format!(" Did you mean \"{n}\"? Area names are exact, capitals included."))
        .unwrap_or_default();
    // Quote at most 64 bytes of what the app asked for: it is its own input,
    // but the answer should stay short.
    let shown: String = asked.chars().take(64).collect();
    format!(
        "Not saved: there is no area called \"{shown}\" in the user's Zaaheen.{close} \
         The user's areas: {}. If the user named no area, save into \"personal\". {CREATE_AREA_STEPS} \
         You cannot create areas yourself.",
        names.join(", ")
    )
}

/// Topics `memory_help` serves, with their text (`src/guide/*.md`).
pub const HELP_TOPICS: &[(&str, &str)] = &[
    ("journey", include_str!("guide/journey.md")),
    ("areas", include_str!("guide/areas.md")),
    ("memories", include_str!("guide/memories.md")),
    ("install", include_str!("guide/install.md")),
    ("update", include_str!("guide/update.md")),
    ("uninstall", include_str!("guide/uninstall.md")),
    ("connect", include_str!("guide/connect.md")),
    ("account", include_str!("guide/account.md")),
    ("consolidation", include_str!("guide/consolidation.md")),
    ("settings", include_str!("guide/settings.md")),
    ("privacy", include_str!("guide/privacy.md")),
    ("messages", include_str!("guide/messages.md")),
    ("troubleshooting", include_str!("guide/troubleshooting.md")),
];

/// The guide's front page, served when no topic (or an unknown one) is asked.
pub const HELP_OVERVIEW: &str = include_str!("guide/overview.md");

/// The `memory_help` answer: one topic, or the overview followed by the
/// user's areas as they stand now.
pub fn help_text(topic: Option<&str>, areas: &[AreaInfo]) -> String {
    let wanted = topic.map(|t| t.trim().to_ascii_lowercase());
    if let Some(wanted) = &wanted {
        if let Some((_, text)) = HELP_TOPICS.iter().find(|(name, _)| name == wanted) {
            return (*text).to_string();
        }
    }
    let mut out = String::from(HELP_OVERVIEW);
    if let Some(wanted) = wanted.filter(|w| !w.is_empty()) {
        let shown: String = wanted.chars().take(40).collect();
        out.push_str(&format!(
            "\n\n(There is no topic \"{shown}\"; the topics are listed above.)"
        ));
    }
    out.push_str("\n\n## The user's areas right now\n\n");
    for a in areas {
        let label = a
            .description
            .as_deref()
            .map(|d| format!(" ({d})"))
            .unwrap_or_default();
        let count = if a.memory_count == 1 {
            "1 memory".to_string()
        } else {
            format!("{} memories", a.memory_count)
        };
        out.push_str(&format!("- {}{label}: {count}\n", a.name.as_str()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(names: &[&str]) -> Vec<Boundary> {
        names.iter().map(|n| Boundary::new(*n).unwrap()).collect()
    }

    #[test]
    fn the_refusal_names_the_areas_and_a_case_match() {
        let text = unknown_area_text("health", &b(&["Health", "default", "personal"]));
        assert!(text.starts_with("Not saved: there is no area called \"health\""));
        assert!(text.contains("Did you mean \"Health\"?"));
        assert!(text.contains("Health, default, personal"));
        assert!(text.contains("+ New boundary"));
        assert!(text.contains("You cannot create areas yourself."));
    }

    #[test]
    fn the_refusal_suggests_nothing_without_a_case_match() {
        let text = unknown_area_text("fitness", &b(&["default", "personal"]));
        assert!(!text.contains("Did you mean"));
    }

    #[test]
    fn the_refusal_quotes_at_most_64_characters_of_the_name() {
        let long = "x".repeat(500);
        let text = unknown_area_text(&long, &b(&["personal"]));
        assert!(text.contains(&format!("\"{}\"", "x".repeat(64))));
        assert!(!text.contains(&"x".repeat(65)));
    }

    #[test]
    fn every_topic_is_served_by_name_ignoring_case() {
        for (name, text) in HELP_TOPICS {
            assert!(!text.trim().is_empty(), "guide topic {name} is empty");
            assert_eq!(help_text(Some(&name.to_ascii_uppercase()), &[]), *text);
        }
    }

    #[test]
    fn the_overview_lists_every_topic_and_the_users_areas() {
        let areas = vec![
            AreaInfo {
                name: Boundary::new("Health").unwrap(),
                description: Some("Doctor visits".into()),
                memory_count: 1,
            },
            AreaInfo {
                name: Boundary::new("personal").unwrap(),
                description: None,
                memory_count: 2,
            },
        ];
        let text = help_text(None, &areas);
        for (name, _) in HELP_TOPICS {
            assert!(
                HELP_OVERVIEW.contains(name),
                "overview must list topic {name}"
            );
        }
        assert!(text.contains("- Health (Doctor visits): 1 memory"));
        assert!(text.contains("- personal: 2 memories"));
    }

    #[test]
    fn an_unknown_topic_gets_the_overview() {
        let text = help_text(Some("cooking"), &[]);
        assert!(text.starts_with(HELP_OVERVIEW));
        assert!(text.contains("There is no topic \"cooking\""));
    }

    #[test]
    fn the_guide_names_no_supplier_or_engine() {
        // White label (founder 2026-07-11): no model, storage or supplier
        // names anywhere an AI app may repeat to the user. Whole words only,
        // so "glance" is not "lance" (CI s113).
        let banned = [
            "bge",
            "qwen",
            "phi",
            "phi4",
            "onnx",
            "lance",
            "lancedb",
            "duckdb",
            "sqlcipher",
            "sqlite",
            "tantivy",
            "cloudflare",
            "clerk",
            "paddle",
            "stripe",
            "tauri",
            "hostinger",
            "supabase",
            "reranker",
        ];
        let all = std::iter::once(HELP_OVERVIEW).chain(HELP_TOPICS.iter().map(|(_, t)| *t));
        for text in all {
            let words = whole_words(text);
            for word in banned {
                assert!(!words.iter().any(|w| w == word), "the guide names {word:?}");
            }
        }
    }

    /// The text's words, lower-cased, split at anything not a letter or digit.
    fn whole_words(text: &str) -> Vec<String> {
        text.to_ascii_lowercase()
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// The check above reads whole words: it must still catch a name, and
    /// must not trip on an ordinary word that contains one.
    #[test]
    fn the_supplier_check_reads_whole_words() {
        let has = |text: &str, word: &str| whole_words(text).iter().any(|w| w == word);
        assert!(!has("The app at a glance", "lance"));
        assert!(has("stored by LanceDB.", "lancedb"));
        assert!(has("runs Phi-4 locally", "phi"));
    }
}
