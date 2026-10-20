//! Background tasks an agent started inside its pane (for example Claude's
//! Agent-tool subagents). Pure in-memory data: never persisted, no PTY access.

use crate::api::schema::AgentTaskStatus;

pub(crate) const MAX_TASKS: usize = 20;
pub(crate) const MAX_TASK_ID_CHARS: usize = 128;
const MAX_TEXT_CHARS: usize = 200;
const MAX_CWD_CHARS: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentTask {
    pub task_id: String,
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
}

/// Trim, drop control characters (C0, DEL and C1 included) and cap the length.
fn clean(value: Option<String>, max_chars: usize) -> Option<String> {
    let value: String = value?.chars().filter(|c| !c.is_control()).collect();
    let value: String = value.trim().chars().take(max_chars).collect();
    let value = value.trim_end().to_owned();
    (!value.is_empty()).then_some(value)
}

/// A usable task id: non-empty after cleaning and within the cap.
pub(crate) fn clean_task_id(task_id: &str) -> Option<String> {
    let id: String = task_id.chars().filter(|c| !c.is_control()).collect();
    let id = id.trim();
    (!id.is_empty() && id.chars().count() <= MAX_TASK_ID_CHARS).then(|| id.to_owned())
}

impl AgentTasks {
    pub fn tasks(&self) -> &[AgentTask] {
        &self.tasks
    }

    pub fn clear(&mut self) {
        self.tasks.clear();
    }

    /// Returns true when the list changed. `task_id` must come from
    /// [`clean_task_id`].
    pub fn report(&mut self, report: AgentTaskReport) -> bool {
        let description = clean(report.description, MAX_TEXT_CHARS);
        let agent_type = clean(report.agent_type, MAX_TEXT_CHARS);
        let branch = clean(report.branch, MAX_TEXT_CHARS);
        let cwd = clean(report.cwd, MAX_CWD_CHARS);

        if let Some(task) = self.tasks.iter_mut().find(|t| t.task_id == report.task_id) {
            if report.seq <= task.seq {
                return false;
            }
            task.seq = report.seq;
            task.status = report.status;
            task.description = description.or(task.description.take());
            task.agent_type = agent_type.or(task.agent_type.take());
            task.branch = branch.or(task.branch.take());
            task.cwd = cwd.or(task.cwd.take());
            return true;
        }
        // Only started tasks count: foreground subagents also fire stop events.
        if report.status != AgentTaskStatus::Working {
            return false;
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
            status: report.status,
            description,
            agent_type,
            branch,
            cwd,
            seq: report.seq,
        });
        true
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
        assert!(!tasks.report(report("t1", 2, AgentTaskStatus::Done)));
        assert!(!tasks.report(report("t1", 1, AgentTaskStatus::Done)));
        assert_eq!(tasks.tasks()[0].status, AgentTaskStatus::Working);
        assert!(tasks.report(report("t1", 3, AgentTaskStatus::Done)));
        assert_eq!(tasks.tasks()[0].status, AgentTaskStatus::Done);
        assert_eq!(tasks.tasks()[0].label(), "fix bug");
        assert_eq!(tasks.tasks()[0].branch.as_deref(), Some("feat/x"));
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
}
