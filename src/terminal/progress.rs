//! Per-terminal agent progress checklist.
//!
//! Pure data: validated and sanitised when a report arrives, so rendering and
//! view computation only ever read the precomputed `done`/`total`.

use std::collections::HashMap;

use crate::api::schema::{ProgressItem, ProgressStatus, ProgressUpdateStatus};

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
            id: clean_id(item.id)?,
            text,
            status: item.status,
        });
    }
    Ok(assemble(source, seq, clean))
}

fn assemble(source: &str, seq: Option<u64>, items: Vec<ProgressItem>) -> Option<AgentProgress> {
    if items.is_empty() {
        return None;
    }
    let done = items
        .iter()
        .filter(|item| item.status == ProgressStatus::Completed)
        .count();
    Some(AgentProgress {
        total: items.len(),
        done,
        items,
        source: source.to_string(),
        last_seq: seq,
    })
}

fn clean_id(id: Option<String>) -> Result<Option<String>, ProgressError> {
    let Some(id) = id else {
        return Ok(None);
    };
    if id.chars().count() > MAX_PROGRESS_ITEM_CHARS {
        return Err(ProgressError::ItemTooLong);
    }
    let id = sanitize_text(&id);
    Ok((!id.is_empty()).then_some(id))
}

/// Result of applying one `pane.update_progress` to the current checklist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProgressUpdateOutcome {
    /// Nothing to change (unknown id without subject, empty id, no-op).
    Unchanged,
    /// The new checklist; `None` means the last item was removed.
    Next(Option<AgentProgress>),
}

/// Pure upsert of one item by id. Validates before returning so a rejected
/// update never touches the sequence guard or the stored list.
pub fn apply_update(
    current: Option<&AgentProgress>,
    source: &str,
    seq: Option<u64>,
    id: String,
    subject: Option<String>,
    status: Option<ProgressUpdateStatus>,
) -> Result<ProgressUpdateOutcome, ProgressError> {
    let Some(id) = clean_id(Some(id))? else {
        return Ok(ProgressUpdateOutcome::Unchanged);
    };
    let subject = match subject {
        Some(subject) => {
            if subject.chars().count() > MAX_PROGRESS_ITEM_CHARS {
                return Err(ProgressError::ItemTooLong);
            }
            Some(sanitize_text(&subject)).filter(|subject| !subject.is_empty())
        }
        None => None,
    };
    let mut items = current.map(|c| c.items.clone()).unwrap_or_default();
    let position = items
        .iter()
        .position(|item| item.id.as_deref() == Some(id.as_str()));

    if status == Some(ProgressUpdateStatus::Deleted) {
        let Some(position) = position else {
            return Ok(ProgressUpdateOutcome::Unchanged);
        };
        items.remove(position);
    } else {
        let status = match status {
            Some(ProgressUpdateStatus::Pending) => Some(ProgressStatus::Pending),
            Some(ProgressUpdateStatus::InProgress) => Some(ProgressStatus::InProgress),
            Some(ProgressUpdateStatus::Completed) => Some(ProgressStatus::Completed),
            Some(ProgressUpdateStatus::Deleted | ProgressUpdateStatus::Unknown) | None => None,
        };
        match position {
            Some(position) => {
                let item = &mut items[position];
                if let Some(status) = status {
                    item.status = status;
                }
                if let Some(subject) = subject {
                    item.text = subject;
                }
            }
            None => {
                let Some(subject) = subject else {
                    return Ok(ProgressUpdateOutcome::Unchanged);
                };
                if items.len() >= MAX_PROGRESS_ITEMS {
                    return Err(ProgressError::TooManyItems);
                }
                items.push(ProgressItem {
                    id: Some(id),
                    text: subject,
                    status: status.unwrap_or(ProgressStatus::Pending),
                });
            }
        }
    }
    Ok(ProgressUpdateOutcome::Next(assemble(source, seq, items)))
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
            id: None,
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

        use crate::api::schema::ProgressUpdateStatus as Up;

        fn upd(
            terminal: &mut TerminalState,
            seq: Option<u64>,
            id: &str,
            subject: Option<&str>,
            status: Option<Up>,
        ) -> Result<bool, ProgressReportError> {
            terminal.update_progress("s", seq, id.into(), subject.map(String::from), status)
        }

        fn texts(terminal: &TerminalState) -> Vec<(String, ProgressStatus)> {
            terminal
                .progress
                .current
                .as_ref()
                .map(|p| p.items.iter().map(|i| (i.text.clone(), i.status)).collect())
                .unwrap_or_default()
        }

        // AC-1.9
        #[test]
        fn update_appends_new_id_and_changes_known_id() {
            let mut terminal = terminal();
            assert_eq!(upd(&mut terminal, Some(1), "1", Some("A"), None), Ok(true));
            assert_eq!(
                upd(&mut terminal, Some(2), "2", Some("B"), Some(Up::InProgress)),
                Ok(true)
            );
            assert_eq!(tokens_progress(&terminal).as_deref(), Some("0/2"));
            assert_eq!(
                upd(&mut terminal, Some(3), "1", None, Some(Up::Completed)),
                Ok(true)
            );
            assert_eq!(tokens_progress(&terminal).as_deref(), Some("1/2"));
            assert_eq!(upd(&mut terminal, Some(4), "2", Some("B2"), None), Ok(true));
            assert_eq!(
                texts(&terminal),
                vec![
                    ("A".to_string(), ProgressStatus::Completed),
                    ("B2".to_string(), ProgressStatus::InProgress)
                ]
            );
            let revision = terminal.revision;
            assert_eq!(
                upd(
                    &mut terminal,
                    Some(5),
                    "2",
                    Some("B2"),
                    Some(Up::InProgress)
                ),
                Ok(false)
            );
            assert_eq!(terminal.revision, revision);
        }

        // AC-1.9
        #[test]
        fn update_unknown_id_without_subject_is_ignored() {
            let mut terminal = terminal();
            assert_eq!(
                upd(&mut terminal, None, "9", None, Some(Up::Completed)),
                Ok(false)
            );
            assert!(terminal.progress.current.is_none());
            assert_eq!(
                upd(&mut terminal, None, "9", None, Some(Up::Deleted)),
                Ok(false)
            );
        }

        // AC-1.9
        #[test]
        fn update_deleted_removes_and_last_removal_clears() {
            let mut terminal = terminal();
            upd(&mut terminal, None, "1", Some("A"), None).unwrap();
            upd(&mut terminal, None, "2", Some("B"), None).unwrap();
            assert_eq!(
                upd(&mut terminal, None, "1", None, Some(Up::Deleted)),
                Ok(true)
            );
            assert_eq!(tokens_progress(&terminal).as_deref(), Some("0/1"));
            assert_eq!(
                upd(&mut terminal, None, "2", None, Some(Up::Deleted)),
                Ok(true)
            );
            assert!(terminal.progress.current.is_none());
            assert_eq!(tokens_progress(&terminal), None);
        }

        // AC-1.9
        #[test]
        fn update_enforces_limits_and_keeps_list() {
            let mut terminal = terminal();
            for i in 0..MAX_PROGRESS_ITEMS {
                upd(&mut terminal, None, &i.to_string(), Some("x"), None).unwrap();
            }
            assert_eq!(
                upd(&mut terminal, None, "new", Some("y"), None),
                Err(ProgressReportError::Invalid(ProgressError::TooManyItems))
            );
            assert_eq!(terminal.progress.current.as_ref().unwrap().total, 100);
            assert_eq!(
                upd(&mut terminal, None, "0", Some(&"x".repeat(501)), None),
                Err(ProgressReportError::Invalid(ProgressError::ItemTooLong))
            );
            assert_eq!(
                upd(&mut terminal, None, &"i".repeat(501), Some("y"), None),
                Err(ProgressReportError::Invalid(ProgressError::ItemTooLong))
            );
            assert_eq!(texts(&terminal)[0].0, "x");
            assert_eq!(
                upd(&mut terminal, None, "0", Some("\x1b[31mred\x07"), None),
                Ok(true)
            );
            assert_eq!(texts(&terminal)[0].0, "[31mred");
        }

        // AC-1.9
        #[test]
        fn update_shares_sequence_guard_with_report() {
            let mut terminal = terminal();
            terminal.report_progress("s", Some(5), items(0, 1)).unwrap();
            assert_eq!(
                upd(&mut terminal, Some(4), "x", Some("late"), None),
                Ok(false)
            );
            assert_eq!(texts(&terminal).len(), 1);
            assert_eq!(upd(&mut terminal, Some(6), "x", Some("ok"), None), Ok(true));
            assert_eq!(
                terminal.report_progress("s", Some(6), items(0, 3)),
                Ok(false)
            );
            assert_eq!(texts(&terminal).len(), 2);
            // Rejected updates do not consume the sequence.
            assert!(upd(&mut terminal, Some(9), "z", Some(&"x".repeat(501)), None).is_err());
            assert_eq!(upd(&mut terminal, Some(7), "z", Some("z"), None), Ok(true));
        }

        // AC-1.9
        #[test]
        fn update_unknown_status_changes_nothing_but_subject() {
            let mut terminal = terminal();
            upd(&mut terminal, None, "1", Some("A"), Some(Up::Completed)).unwrap();
            assert_eq!(
                upd(&mut terminal, None, "1", None, Some(Up::Unknown)),
                Ok(false)
            );
            assert_eq!(texts(&terminal)[0].1, ProgressStatus::Completed);
        }
    }
}
