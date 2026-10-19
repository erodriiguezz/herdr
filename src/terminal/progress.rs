//! Per-terminal agent progress checklist.
//!
//! Pure data: validated and sanitised when a report arrives, so rendering and
//! view computation only ever read the precomputed `done`/`total`.

use std::collections::HashMap;

use crate::api::schema::{ProgressItem, ProgressStatus};

pub const MAX_PROGRESS_ITEMS: usize = 100;
pub const MAX_PROGRESS_ITEM_CHARS: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProgress {
    pub items: Vec<ProgressItem>,
    pub done: usize,
    pub total: usize,
    pub source: String,
    pub last_seq: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressError {
    TooManyItems,
    ItemTooLong,
}

impl ProgressError {
    pub fn message(self) -> String {
        match self {
            Self::TooManyItems => {
                format!("progress may contain at most {MAX_PROGRESS_ITEMS} items")
            }
            Self::ItemTooLong => {
                format!("progress items may be at most {MAX_PROGRESS_ITEM_CHARS} characters")
            }
        }
    }
}

/// Current checklist plus the per-source report sequences that guard it.
#[derive(Debug, Default)]
pub struct ProgressSlot {
    pub current: Option<AgentProgress>,
    pub sequences: HashMap<String, u64>,
}

impl ProgressSlot {
    /// Drops the checklist and its sequences. Returns whether a checklist existed.
    pub fn clear(&mut self) -> bool {
        self.sequences.clear();
        self.current.take().is_some()
    }

    /// `done/total` for the sidebar, or `None` when no checklist is set.
    pub fn summary(&self) -> Option<String> {
        self.current
            .as_ref()
            .map(|progress| format!("{}/{}", progress.done, progress.total))
    }
}

/// Validates and sanitises a report. An empty (or all-blank) report yields
/// `None`, meaning "clear the checklist".
pub fn build_progress(
    source: &str,
    seq: Option<u64>,
    items: Vec<ProgressItem>,
) -> Result<Option<AgentProgress>, ProgressError> {
    if items.len() > MAX_PROGRESS_ITEMS {
        return Err(ProgressError::TooManyItems);
    }
    let mut clean = Vec::with_capacity(items.len());
    for item in items {
        if item.text.chars().count() > MAX_PROGRESS_ITEM_CHARS {
            return Err(ProgressError::ItemTooLong);
        }
        let text = sanitize_text(&item.text);
        if text.is_empty() {
            continue;
        }
        clean.push(ProgressItem {
            text,
            status: item.status,
        });
    }
    if clean.is_empty() {
        return Ok(None);
    }
    let done = clean
        .iter()
        .filter(|item| item.status == ProgressStatus::Completed)
        .count();
    Ok(Some(AgentProgress {
        total: clean.len(),
        done,
        items: clean,
        source: source.to_string(),
        last_seq: seq,
    }))
}

/// Control characters (ESC, C0, DEL, C1) are removed; line breaks and tabs
/// become spaces so an item stays on one row.
fn sanitize_text(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .filter_map(|c| match c {
            '\n' | '\r' | '\t' => Some(' '),
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect();
    cleaned.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(text: &str, status: ProgressStatus) -> ProgressItem {
        ProgressItem {
            text: text.into(),
            status,
        }
    }

    // AC-1.1
    #[test]
    fn counts_completed_items_in_order() {
        let items = vec![
            item("a", ProgressStatus::Completed),
            item("b", ProgressStatus::InProgress),
            item("c", ProgressStatus::Pending),
            item("d", ProgressStatus::Completed),
        ];
        let progress = build_progress("src", Some(3), items).unwrap().unwrap();
        assert_eq!((progress.done, progress.total), (2, 4));
        assert_eq!(progress.items[1].text, "b");
        assert_eq!(progress.last_seq, Some(3));
    }

    // AC-1.3
    #[test]
    fn empty_report_clears() {
        assert_eq!(build_progress("src", None, vec![]), Ok(None));
    }

    // AC-1.5
    #[test]
    fn rejects_too_many_items() {
        let items = (0..=MAX_PROGRESS_ITEMS)
            .map(|i| item(&i.to_string(), ProgressStatus::Pending))
            .collect();
        assert_eq!(
            build_progress("src", None, items),
            Err(ProgressError::TooManyItems)
        );
        let at_limit = (0..MAX_PROGRESS_ITEMS)
            .map(|i| item(&i.to_string(), ProgressStatus::Pending))
            .collect();
        assert!(build_progress("src", None, at_limit).is_ok());
    }

    // AC-1.5
    #[test]
    fn rejects_overlong_item() {
        let long = "x".repeat(MAX_PROGRESS_ITEM_CHARS + 1);
        assert_eq!(
            build_progress("src", None, vec![item(&long, ProgressStatus::Pending)]),
            Err(ProgressError::ItemTooLong)
        );
        let ok = "x".repeat(MAX_PROGRESS_ITEM_CHARS);
        assert!(build_progress("src", None, vec![item(&ok, ProgressStatus::Pending)]).is_ok());
    }

    // AC-1.5
    #[test]
    fn strips_escape_and_control_characters() {
        let progress = build_progress(
            "src",
            None,
            vec![item(
                "\x1b[31mred\x07\x00 \u{9b}ok\x7f",
                ProgressStatus::Pending,
            )],
        )
        .unwrap()
        .unwrap();
        assert_eq!(progress.items[0].text, "[31mred ok");
        let multiline = build_progress(
            "src",
            None,
            vec![item("one\ntwo\tthree", ProgressStatus::Pending)],
        )
        .unwrap()
        .unwrap();
        assert_eq!(multiline.items[0].text, "one two three");
    }

    #[test]
    fn unknown_status_deserialises_as_unknown_and_is_not_done() {
        let parsed: ProgressItem =
            serde_json::from_str(r#"{"text":"a","status":"future_state"}"#).unwrap();
        assert_eq!(parsed.status, ProgressStatus::Unknown);
        let progress = build_progress("src", None, vec![parsed]).unwrap().unwrap();
        assert_eq!((progress.done, progress.total), (0, 1));
    }

    #[test]
    fn slot_summary_and_clear() {
        let mut slot = ProgressSlot::default();
        assert_eq!(slot.summary(), None);
        slot.current = build_progress(
            "src",
            None,
            vec![
                item("a", ProgressStatus::Completed),
                item("b", ProgressStatus::Pending),
            ],
        )
        .unwrap();
        slot.sequences.insert("src".into(), 1);
        assert_eq!(slot.summary().as_deref(), Some("1/2"));
        assert!(slot.clear());
        assert!(slot.sequences.is_empty());
        assert_eq!(slot.summary(), None);
        assert!(!slot.clear());
    }

    mod terminal_state {
        use std::time::Instant;

        use super::super::super::{ProgressReportError, TerminalState};
        use crate::detect::{Agent, AgentState};
        use crate::terminal::TerminalId;

        use super::*;

        fn terminal() -> TerminalState {
            TerminalState::new(TerminalId::alloc(), "/tmp".into())
        }

        fn items(done: usize, total: usize) -> Vec<ProgressItem> {
            (0..total)
                .map(|i| {
                    item(
                        &format!("step {i}"),
                        if i < done {
                            ProgressStatus::Completed
                        } else {
                            ProgressStatus::Pending
                        },
                    )
                })
                .collect()
        }

        fn tokens_progress(terminal: &TerminalState) -> Option<String> {
            terminal.display_tokens().get("progress").cloned()
        }

        // AC-1.1, AC-1.2
        #[test]
        fn report_stores_items_and_derives_progress_token() {
            let mut terminal = terminal();
            assert_eq!(tokens_progress(&terminal), None);
            assert_eq!(
                terminal.report_progress("herdr:claude", Some(1), items(3, 7)),
                Ok(true)
            );
            let current = terminal.progress.current.as_ref().unwrap();
            assert_eq!((current.done, current.total), (3, 7));
            assert_eq!(current.items.len(), 7);
            assert_eq!(tokens_progress(&terminal).as_deref(), Some("3/7"));
        }

        // AC-1.2
        #[test]
        fn derived_token_wins_over_reported_progress_token() {
            let mut terminal = terminal();
            terminal.metadata_tokens.patch(
                [("progress".to_string(), Some("manual".to_string()))].into(),
                None,
                Instant::now(),
            );
            assert_eq!(tokens_progress(&terminal).as_deref(), Some("manual"));
            terminal.report_progress("s", None, items(1, 2)).unwrap();
            assert_eq!(tokens_progress(&terminal).as_deref(), Some("1/2"));
            terminal.report_progress("s", None, vec![]).unwrap();
            assert_eq!(tokens_progress(&terminal).as_deref(), Some("manual"));
        }

        // AC-1.3
        #[test]
        fn newer_replaces_older_ignored_empty_clears() {
            let mut terminal = terminal();
            terminal.report_progress("s", Some(5), items(1, 3)).unwrap();
            assert_eq!(
                terminal.report_progress("s", Some(4), items(3, 3)),
                Ok(false)
            );
            assert_eq!(tokens_progress(&terminal).as_deref(), Some("1/3"));
            assert_eq!(
                terminal.report_progress("s", Some(6), items(2, 3)),
                Ok(true)
            );
            assert_eq!(tokens_progress(&terminal).as_deref(), Some("2/3"));
            assert_eq!(terminal.report_progress("s", Some(7), vec![]), Ok(true));
            assert!(terminal.progress.current.is_none());
            assert_eq!(tokens_progress(&terminal), None);
        }

        // AC-1.5
        #[test]
        fn invalid_report_keeps_previous_list_and_sequence() {
            let mut terminal = terminal();
            terminal.report_progress("s", Some(1), items(1, 2)).unwrap();
            assert_eq!(
                terminal.report_progress("s", Some(2), items(0, 101)),
                Err(ProgressReportError::Invalid(ProgressError::TooManyItems))
            );
            let long = vec![item(&"x".repeat(501), ProgressStatus::Pending)];
            assert_eq!(
                terminal.report_progress("s", Some(2), long),
                Err(ProgressReportError::Invalid(ProgressError::ItemTooLong))
            );
            assert_eq!(tokens_progress(&terminal).as_deref(), Some("1/2"));
            // The rejected seq 2 was not consumed.
            assert_eq!(
                terminal.report_progress("s", Some(2), items(2, 2)),
                Ok(true)
            );
        }

        // AC-1.4
        #[test]
        fn cleared_when_agent_session_changes() {
            let mut terminal = terminal();
            let session = |id: &str| crate::agent_resume::PersistedAgentSession {
                source: "herdr:claude".into(),
                agent: "claude".into(),
                session_ref: crate::agent_resume::AgentSessionRef::id(id).unwrap(),
            };
            terminal.set_persisted_agent_session(session("one"));
            terminal.report_progress("s", None, items(1, 2)).unwrap();
            terminal.set_persisted_agent_session(session("one"));
            assert_eq!(tokens_progress(&terminal).as_deref(), Some("1/2"));
            terminal.set_persisted_agent_session(session("two"));
            assert_eq!(tokens_progress(&terminal), None);
        }

        // AC-1.4
        #[test]
        fn cleared_when_agent_is_released() {
            let mut terminal = terminal();
            terminal.set_detected_state(Some(Agent::Pi), AgentState::Idle);
            terminal
                .set_hook_authority(
                    "custom:pi".into(),
                    "pi".into(),
                    AgentState::Working,
                    None,
                    Some(10),
                )
                .unwrap();
            terminal.report_progress("s", None, items(1, 2)).unwrap();
            terminal
                .release_agent_with_mutation("custom:pi", "pi", Some(11))
                .unwrap();
            assert!(terminal.progress.current.is_none());
        }

        // AC-1.4
        #[test]
        fn cleared_when_agent_process_exits() {
            let mut terminal = terminal();
            terminal.set_detected_state(Some(Agent::Pi), AgentState::Idle);
            terminal.report_progress("s", None, items(1, 2)).unwrap();
            let mutation =
                terminal.set_detected_state_with_mutation(Some(Agent::Pi), AgentState::Idle);
            assert!(!mutation.agent_released);
            assert!(terminal.progress.current.is_some());
            terminal.set_detected_state_with_visible_blocker(
                Some(Agent::Pi),
                AgentState::Unknown,
                false,
                false,
                true,
            );
            assert!(terminal.progress.current.is_none());
            assert!(terminal.progress.sequences.is_empty());
        }
    }
}
