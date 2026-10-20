//! Background tasks an agent started inside its pane (for example Claude's
//! Agent-tool subagents). Pure in-memory data: never persisted, no PTY access.

use std::collections::VecDeque;

use crate::api::schema::AgentTaskStatus;

pub(crate) const MAX_TASKS: usize = 20;
pub(crate) const MAX_TASK_ID_CHARS: usize = 128;
const MAX_TEXT_CHARS: usize = 200;
const MAX_CWD_CHARS: usize = 1024;
/// Finishes remembered for tasks not seen yet (stop arrived before start).
const MAX_EARLY_FINISHES: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentTask {
    pub task_id: String,
    /// Reporter that owns the task; reports from other sources are ignored.
    source: String,
    pub status: AgentTaskStatus,
    pub description: Option<String>,
    pub agent_type: Option<String>,
    pub branch: Option<String>,
    pub cwd: Option<String>,
    seq: u64,
}

impl AgentTask {
    /// Description, else branch, else agent type, else the task id.
    pub fn label(&self) -> &str {
        self.description
            .as_deref()
            .or(self.branch.as_deref())
            .or(self.agent_type.as_deref())
            .unwrap_or(&self.task_id)
    }
}

/// One `pane.report_agent_task` report, before sanitising.
#[derive(Debug, Clone)]
pub(crate) struct AgentTaskReport {
    pub source: String,
    pub task_id: String,
    pub seq: u64,
    pub status: AgentTaskStatus,
    pub description: Option<String>,
    pub agent_type: Option<String>,
    pub branch: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AgentTasks {
    tasks: Vec<AgentTask>,
    early_finishes: VecDeque<EarlyFinish>,
}

/// A finish that reached the server before its task's start report.
#[derive(Debug, Clone, PartialEq, Eq)]
struct EarlyFinish {
    source: String,
    task_id: String,
    seq: u64,
    status: AgentTaskStatus,
}

/// Bidi controls, zero-width and invisible format characters.
fn is_format_char(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
    )
}

fn is_hidden_char(c: char) -> bool {
    c.is_control() || is_format_char(c)
}

/// Trim, drop control and format characters (C0, DEL and C1 included) and cap
/// the length.
fn clean(value: Option<String>, max_chars: usize) -> Option<String> {
    let value: String = value?.chars().filter(|c| !is_hidden_char(*c)).collect();
    let value: String = value.trim().chars().take(max_chars).collect();
    let value = value.trim_end().to_owned();
    (!value.is_empty()).then_some(value)
}

/// A usable task id: non-empty after cleaning and within the cap.
pub(crate) fn clean_task_id(task_id: &str) -> Option<String> {
    let id: String = task_id.chars().filter(|c| !is_hidden_char(*c)).collect();
    let id = id.trim();
    (!id.is_empty() && id.chars().count() <= MAX_TASK_ID_CHARS).then(|| id.to_owned())
}

impl AgentTasks {
    pub fn tasks(&self) -> &[AgentTask] {
        &self.tasks
    }

    /// Returns true when a visible task was dropped.
    pub fn clear(&mut self) -> bool {
        let had_tasks = !self.tasks.is_empty();
        self.tasks.clear();
        self.early_finishes.clear();
        had_tasks
    }

    /// Returns true when the list changed. `task_id` must come from
    /// [`clean_task_id`].
    pub fn report(&mut self, report: AgentTaskReport) -> bool {
        let description = clean(report.description, MAX_TEXT_CHARS);
        let agent_type = clean(report.agent_type, MAX_TEXT_CHARS);
        let branch = clean(report.branch, MAX_TEXT_CHARS);
        let cwd = clean(report.cwd, MAX_CWD_CHARS);
        let source = clean(Some(report.source), MAX_TASK_ID_CHARS).unwrap_or_default();

        if let Some(task) = self.tasks.iter_mut().find(|t| t.task_id == report.task_id) {
            if source != task.source {
                return false;
            }
            // Seqs come from separate hook processes and do not reflect event
            // order, so a finish always applies to a running task and a
            // finished task never goes back to working.
            let finishing = report.status != AgentTaskStatus::Working;
            let applies = match (task.status, finishing) {
                (AgentTaskStatus::Working, true) => true,
                (AgentTaskStatus::Working, false) => report.seq > task.seq,
                (_, true) => report.seq > task.seq,
                (_, false) => false,
            };
            if !applies {
                return false;
            }
            task.seq = task.seq.max(report.seq);
            task.status = report.status;
            task.description = description.or(task.description.take());
            task.agent_type = agent_type.or(task.agent_type.take());
            task.branch = branch.or(task.branch.take());
            task.cwd = cwd.or(task.cwd.take());
            return true;
        }
        // Only started tasks count: foreground subagents also fire stop events.
        // The finish of an unknown task is remembered briefly instead, because
        // hook processes can deliver a fast task's stop before its start.
        if report.status != AgentTaskStatus::Working {
            self.remember_early_finish(EarlyFinish {
                source,
                task_id: report.task_id,
                seq: report.seq,
                status: report.status,
            });
            return false;
        }
        let mut status = report.status;
        let mut seq = report.seq;
        if let Some(index) = self
            .early_finishes
            .iter()
            .position(|f| f.source == source && f.task_id == report.task_id)
        {
            // Task ids are unique per launch, so a remembered finish always
            // belongs after this start, even when a delayed start process
            // stamped a later seq.
            if let Some(finish) = self.early_finishes.remove(index) {
                status = finish.status;
                seq = seq.max(finish.seq);
            }
        }
        if self.tasks.len() >= MAX_TASKS {
            let evicted = self
                .tasks
                .iter()
                .position(|t| t.status != AgentTaskStatus::Working)
                .unwrap_or(0);
            self.tasks.remove(evicted);
        }
        self.tasks.push(AgentTask {
            task_id: report.task_id,
            source,
            status,
            description,
            agent_type,
            branch,
            cwd,
            seq,
        });
        true
    }

    fn remember_early_finish(&mut self, finish: EarlyFinish) {
        if let Some(index) = self
            .early_finishes
            .iter()
            .position(|f| f.source == finish.source && f.task_id == finish.task_id)
        {
            if self.early_finishes[index].seq >= finish.seq {
                return;
            }
            self.early_finishes.remove(index);
        }
        if self.early_finishes.len() >= MAX_EARLY_FINISHES {
            self.early_finishes.pop_front();
        }
        self.early_finishes.push_back(finish);
    }

    /// Drop finished tasks, keeping the working ones. Returns true on change.
    pub fn acknowledge_finished(&mut self) -> bool {
        let before = self.tasks.len();
        self.tasks.retain(|t| t.status == AgentTaskStatus::Working);
        self.tasks.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(id: &str, seq: u64, status: AgentTaskStatus) -> AgentTaskReport {
        AgentTaskReport {
            source: "src".into(),
            task_id: id.into(),
            seq,
            status,
            description: None,
            agent_type: None,
            branch: None,
            cwd: None,
        }
    }

    // AC-2.1
    #[test]
    fn ac_2_1_start_then_done_and_stale_seq_ignored() {
        let mut tasks = AgentTasks::default();
        let mut start = report("t1", 2, AgentTaskStatus::Working);
        start.description = Some("fix bug".into());
        start.branch = Some("feat/x".into());
        assert!(tasks.report(start));
        assert!(!tasks.report(report("t1", 2, AgentTaskStatus::Working)));
        assert!(!tasks.report(report("t1", 1, AgentTaskStatus::Working)));
        assert!(tasks.report(report("t1", 3, AgentTaskStatus::Done)));
        assert_eq!(tasks.tasks()[0].status, AgentTaskStatus::Done);
        assert_eq!(tasks.tasks()[0].label(), "fix bug");
        assert_eq!(tasks.tasks()[0].branch.as_deref(), Some("feat/x"));
        // finished -> finished still follows seq
        assert!(!tasks.report(report("t1", 3, AgentTaskStatus::Failed)));
        assert!(tasks.report(report("t1", 4, AgentTaskStatus::Failed)));
        assert_eq!(tasks.tasks()[0].status, AgentTaskStatus::Failed);
    }

    // AC-2.1
    #[test]
    fn ac_2_1_finish_applies_regardless_of_seq_and_is_final() {
        for finish_seq in [1, 5, 9] {
            let mut tasks = AgentTasks::default();
            // Start stamped later than the finish (delayed hook process).
            assert!(tasks.report(report("t1", 5, AgentTaskStatus::Working)));
            assert!(tasks.report(report("t1", finish_seq, AgentTaskStatus::Done)));
            assert_eq!(tasks.tasks()[0].status, AgentTaskStatus::Done);
            // A late-delivered start never revives a finished task.
            assert!(!tasks.report(report("t1", 99, AgentTaskStatus::Working)));
            assert_eq!(tasks.tasks()[0].status, AgentTaskStatus::Done);
        }
        let mut tasks = AgentTasks::default();
        tasks.report(report("t1", 5, AgentTaskStatus::Working));
        let mut other = report("t1", 1, AgentTaskStatus::Done);
        other.source = "other".into();
        assert!(!tasks.report(other), "cross-source finish is still ignored");
        assert_eq!(tasks.tasks()[0].status, AgentTaskStatus::Working);
    }

    // AC-2.1
    #[test]
    fn ac_2_1_finish_for_unknown_task_is_ignored() {
        let mut tasks = AgentTasks::default();
        assert!(!tasks.report(report("t1", 1, AgentTaskStatus::Done)));
        assert!(!tasks.report(report("t1", 2, AgentTaskStatus::Failed)));
        assert!(tasks.tasks().is_empty());
    }

    // AC-2.2
    #[test]
    fn ac_2_2_label_fallback_order() {
        let mut task = report("id", 1, AgentTaskStatus::Working);
        let label = |task: &AgentTaskReport| {
            let mut tasks = AgentTasks::default();
            tasks.report(task.clone());
            tasks.tasks()[0].label().to_owned()
        };
        assert_eq!(label(&task), "id");
        task.agent_type = Some("general".into());
        assert_eq!(label(&task), "general");
        task.branch = Some(" feat/y ".into());
        assert_eq!(label(&task), "feat/y");
        task.description = Some("  do it ".into());
        assert_eq!(label(&task), "do it");
        task.description = Some("   ".into());
        assert_eq!(label(&task), "feat/y");
    }

    // AC-2.2
    #[test]
    fn ac_2_2_control_characters_stripped_and_text_capped() {
        let mut tasks = AgentTasks::default();
        let mut r = report("t\u{1b}1", 1, AgentTaskStatus::Working);
        r.task_id = clean_task_id(&r.task_id).unwrap();
        r.description = Some("a\u{1b}[31mb\u{9b}c\u{7}\n".into());
        r.cwd = Some("é".repeat(2000));
        r.branch = Some("é".repeat(300));
        tasks.report(r);
        let task = &tasks.tasks()[0];
        assert_eq!(task.task_id, "t1");
        assert_eq!(task.description.as_deref(), Some("a[31mbc"));
        assert_eq!(task.branch.as_deref().unwrap().chars().count(), 200);
        assert_eq!(task.cwd.as_deref().unwrap().chars().count(), 1024);
        assert!(clean_task_id("").is_none());
        assert!(clean_task_id("\u{1b}").is_none());
        assert!(clean_task_id(&"x".repeat(129)).is_none());
        assert!(clean_task_id(&"x".repeat(128)).is_some());
    }

    // AC-2.2
    #[test]
    fn ac_2_2_task_beyond_cap_evicts_oldest_finished_else_oldest() {
        let mut tasks = AgentTasks::default();
        for i in 0..MAX_TASKS {
            tasks.report(report(&format!("t{i}"), 1, AgentTaskStatus::Working));
        }
        tasks.report(report("t5", 2, AgentTaskStatus::Done));
        tasks.report(report("t7", 2, AgentTaskStatus::Failed));
        tasks.report(report("new", 1, AgentTaskStatus::Working));
        let ids: Vec<_> = tasks.tasks().iter().map(|t| t.task_id.as_str()).collect();
        assert_eq!(ids.len(), MAX_TASKS);
        assert!(!ids.contains(&"t5") && ids.contains(&"t7"));
        assert_eq!(ids.last(), Some(&"new"));

        let mut all_working = AgentTasks::default();
        for i in 0..=MAX_TASKS {
            all_working.report(report(&format!("t{i}"), 1, AgentTaskStatus::Working));
        }
        assert_eq!(all_working.tasks().len(), MAX_TASKS);
        assert_eq!(all_working.tasks()[0].task_id, "t1");
    }

    // AC-2.4
    #[test]
    fn ac_2_4_acknowledge_drops_finished_keeps_working() {
        let mut tasks = AgentTasks::default();
        tasks.report(report("a", 1, AgentTaskStatus::Working));
        tasks.report(report("b", 1, AgentTaskStatus::Working));
        tasks.report(report("c", 1, AgentTaskStatus::Working));
        tasks.report(report("b", 2, AgentTaskStatus::Done));
        tasks.report(report("c", 2, AgentTaskStatus::Failed));
        assert!(tasks.acknowledge_finished());
        assert_eq!(tasks.tasks().len(), 1);
        assert_eq!(tasks.tasks()[0].task_id, "a");
        assert!(!tasks.acknowledge_finished());
    }

    // AC-2.1
    #[test]
    fn ac_2_1_report_from_another_source_is_ignored() {
        let mut tasks = AgentTasks::default();
        let mut start = report("t1", 1, AgentTaskStatus::Working);
        start.description = Some("mine".into());
        assert!(tasks.report(start));

        let mut other = report("t1", 5, AgentTaskStatus::Done);
        other.source = "other".into();
        other.description = Some("hijacked".into());
        assert!(!tasks.report(other));
        assert_eq!(tasks.tasks()[0].status, AgentTaskStatus::Working);
        assert_eq!(tasks.tasks()[0].label(), "mine");

        assert!(tasks.report(report("t1", 2, AgentTaskStatus::Done)));
        assert_eq!(tasks.tasks()[0].status, AgentTaskStatus::Done);
    }

    // AC-2.1
    #[test]
    fn ac_2_1_finish_before_start_creates_the_task_finished() {
        let mut tasks = AgentTasks::default();
        assert!(!tasks.report(report("t1", 10, AgentTaskStatus::Done)));
        assert!(
            tasks.tasks().is_empty(),
            "an unknown finish stays invisible"
        );

        let mut start = report("t1", 5, AgentTaskStatus::Working);
        start.description = Some("fast".into());
        assert!(tasks.report(start));
        assert_eq!(tasks.tasks().len(), 1);
        assert_eq!(tasks.tasks()[0].status, AgentTaskStatus::Done);
        assert_eq!(tasks.tasks()[0].label(), "fast");
        assert!(!tasks.report(report("t1", 7, AgentTaskStatus::Working)));
        assert_eq!(tasks.tasks()[0].status, AgentTaskStatus::Done);
    }

    // AC-2.1
    #[test]
    fn ac_2_1_early_finish_is_source_scoped_and_applies_to_a_later_stamped_start() {
        let mut tasks = AgentTasks::default();
        let mut foreign = report("t1", 10, AgentTaskStatus::Failed);
        foreign.source = "other".into();
        tasks.report(foreign);
        tasks.report(report("t2", 3, AgentTaskStatus::Done));

        assert!(tasks.report(report("t1", 5, AgentTaskStatus::Working)));
        // A delayed start process stamps a later seq than the finish it follows.
        assert!(tasks.report(report("t2", 4, AgentTaskStatus::Working)));
        assert_eq!(tasks.tasks()[0].status, AgentTaskStatus::Working);
        assert_eq!(tasks.tasks()[1].status, AgentTaskStatus::Done);
        assert!(!tasks.report(report("t2", 4, AgentTaskStatus::Working)));
        assert_eq!(tasks.tasks()[1].status, AgentTaskStatus::Done);
    }

    // AC-2.1
    #[test]
    fn ac_2_1_early_finish_memory_is_bounded_fifo_and_cleared() {
        let mut tasks = AgentTasks::default();
        for i in 0..=MAX_EARLY_FINISHES {
            tasks.report(report(&format!("t{i}"), 10, AgentTaskStatus::Done));
        }
        // The oldest finish was forgotten; the newest is still remembered.
        tasks.report(report("t0", 1, AgentTaskStatus::Working));
        tasks.report(report(
            &format!("t{MAX_EARLY_FINISHES}"),
            1,
            AgentTaskStatus::Working,
        ));
        assert_eq!(tasks.tasks()[0].status, AgentTaskStatus::Working);
        assert_eq!(tasks.tasks()[1].status, AgentTaskStatus::Done);

        tasks.report(report("late", 10, AgentTaskStatus::Done));
        tasks.clear();
        tasks.report(report("late", 1, AgentTaskStatus::Working));
        assert_eq!(tasks.tasks()[0].status, AgentTaskStatus::Working);
    }

    // AC-2.2
    #[test]
    fn ac_2_2_bidi_and_zero_width_format_characters_stripped() {
        let hidden: String = ('\u{200B}'..='\u{200F}')
            .chain('\u{202A}'..='\u{202E}')
            .chain('\u{2060}'..='\u{2064}')
            .chain('\u{2066}'..='\u{2069}')
            .chain(['\u{FEFF}'])
            .collect();
        let mut tasks = AgentTasks::default();
        let mut r = report("t1", 1, AgentTaskStatus::Working);
        r.description = Some(format!("a{hidden}b\u{202E}c"));
        r.agent_type = Some(format!("{hidden}type"));
        r.branch = Some(format!("fe{hidden}at"));
        r.cwd = Some(format!("/tmp{hidden}/x"));
        tasks.report(r);
        let task = &tasks.tasks()[0];
        assert_eq!(task.description.as_deref(), Some("abc"));
        assert_eq!(task.agent_type.as_deref(), Some("type"));
        assert_eq!(task.branch.as_deref(), Some("feat"));
        assert_eq!(task.cwd.as_deref(), Some("/tmp/x"));
        assert_eq!(
            clean_task_id(&format!("{hidden}id{hidden}")).as_deref(),
            Some("id")
        );
        assert!(clean_task_id(&hidden).is_none());
        assert_eq!(clean_task_id("a\u{00A0}b").as_deref(), Some("a\u{00A0}b"));
    }
}
