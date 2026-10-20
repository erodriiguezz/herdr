use std::collections::{BTreeMap, HashMap, HashSet};

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::Line,
    widgets::{Paragraph, Widget},
};

use super::*;
use crate::api::schema::AgentStatus;
use crate::protocol::endpoint::{EndpointAgentTask, EndpointAgentTaskStatus};

pub(super) struct AgentRow<'a> {
    pub(super) pane_id: String,
    pub(super) status: crate::api::schema::AgentStatus,
    pub(super) focused: bool,
    pub(super) rows: Vec<Vec<crate::ui::ResolvedToken>>,
    /// Background tasks drawn as child rows. Empty for most agents.
    tasks: Vec<AgentTaskLine<'a>>,
    tasks_collapsed: bool,
    toggle_fits: bool,
}

struct AgentTaskLine<'a> {
    status: AgentStatus,
    label: &'a str,
}

impl AgentRow<'_> {
    /// Terminal lines the row occupies: its label rows plus visible task children.
    pub(super) fn line_count(&self) -> usize {
        self.rows.len().max(1)
            + if self.tasks_collapsed {
                0
            } else {
                self.tasks.len()
            }
    }
}

/// Where one endpoint's background tasks and their collapse state live.
#[derive(Clone, Copy)]
pub(super) struct AgentTaskSource<'a> {
    tasks: &'a BTreeMap<String, Vec<EndpointAgentTask>>,
    collapsed: Option<&'a HashSet<String>>,
    /// Whether rows are wide enough to carry the collapse toggle.
    toggle_fits: bool,
}

/// Narrower rows cannot hold the toggle, so their tasks always stay expanded.
const MIN_TOGGLE_WIDTH: u16 = 10;

impl<'a> AgentTaskSource<'a> {
    pub(super) fn for_endpoint(
        endpoint: &'a ClientShellEndpoint,
        collapsed: &'a HashMap<ClientEndpointId, HashSet<String>>,
        panel_width: u16,
    ) -> Self {
        Self {
            tasks: endpoint.agent_presentation.agent_tasks(),
            collapsed: collapsed.get(&endpoint.endpoint_id),
            // A scrollbar may take one column from the rows.
            toggle_fits: panel_width.saturating_sub(1) >= MIN_TOGGLE_WIDTH,
        }
    }

    fn lines_for(&self, pane_id: &str) -> (Vec<AgentTaskLine<'a>>, bool) {
        let (lines, collapsed) = self.lines_for_inner(pane_id);
        (lines, collapsed && self.toggle_fits)
    }

    fn lines_for_inner(&self, pane_id: &str) -> (Vec<AgentTaskLine<'a>>, bool) {
        let Some(tasks) = self.tasks.get(pane_id) else {
            return (Vec::new(), false);
        };
        // A status this client does not know has no icon it could draw truthfully.
        let lines = tasks
            .iter()
            .filter_map(|task| {
                Some(AgentTaskLine {
                    status: task_agent_status(task.status)?,
                    label: task.label.as_str(),
                })
            })
            .collect::<Vec<_>>();
        let collapsed = !lines.is_empty()
            && self
                .collapsed
                .is_some_and(|collapsed| collapsed.contains(pane_id));
        (lines, collapsed)
    }
}

/// Task statuses reuse the agent status icons and colours: failed reads as blocked.
fn task_agent_status(status: EndpointAgentTaskStatus) -> Option<AgentStatus> {
    match status {
        EndpointAgentTaskStatus::Working => Some(AgentStatus::Working),
        EndpointAgentTaskStatus::Done => Some(AgentStatus::Done),
        EndpointAgentTaskStatus::Failed => Some(AgentStatus::Blocked),
        EndpointAgentTaskStatus::Unknown => None,
    }
}

/// Collapsed parents show the most urgent child: failed, then working, then done.
fn rolled_up_task_status(tasks: &[AgentTaskLine<'_>]) -> Option<AgentStatus> {
    let rank = |status: AgentStatus| match status {
        AgentStatus::Blocked => 3,
        AgentStatus::Working => 2,
        _ => 1,
    };
    tasks
        .iter()
        .map(|task| task.status)
        .max_by_key(|status| rank(*status))
}

fn task_icon(status: AgentStatus) -> &'static str {
    status_icon(status, crate::config::StatusIndicatorStyle::Symbols)
}

pub(super) fn ordered_agent_pane_ids(
    snapshot: &ClientShellSnapshot,
    sort: crate::config::AgentPanelSortConfig,
) -> Vec<String> {
    if snapshot.agent_view_label.is_some() {
        return snapshot
            .agent_order
            .iter()
            .filter(|pane_id| {
                snapshot
                    .agents
                    .iter()
                    .any(|agent| agent.pane_id == pane_id.as_str())
            })
            .cloned()
            .collect();
    }
    let mut agents = snapshot.agents.iter().collect::<Vec<_>>();
    if sort == crate::config::AgentPanelSortConfig::Priority {
        agents.sort_by_key(|agent| {
            (
                std::cmp::Reverse(status_priority(agent.agent_status)),
                std::cmp::Reverse(agent.state_change_seq),
            )
        });
    }
    agents
        .into_iter()
        .map(|agent| agent.pane_id.clone())
        .collect()
}

pub(super) fn render_agent_panel(
    buffer: &mut Buffer,
    area: Rect,
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    endpoint_id: &ClientEndpointId,
    tasks: Option<AgentTaskSource<'_>>,
    agent_scroll: &mut usize,
    hits: &mut ShellHitMap,
) {
    if !render_agent_panel_header(
        buffer,
        area,
        snapshot.agent_view_label.as_deref(),
        config,
        hits,
    ) {
        return;
    }

    let rows = agent_rows(snapshot, config, None, tasks);
    render_agent_list(
        buffer,
        area,
        &rows,
        snapshot
            .agent_view_label
            .as_ref()
            .map(|_| " no matching agents"),
        config,
        agent_scroll,
        hits,
        AgentRow::line_count,
        |buffer, rect, row, hits| {
            hits.agents.push((rect, row.pane_id.clone()));
            if let Some(toggle) = render_agent_row(buffer, rect, row, config) {
                hits.agent_task_toggles
                    .push((toggle, endpoint_id.clone(), row.pane_id.clone()));
            }
        },
    );
}

pub(super) fn render_agent_panel_header(
    buffer: &mut Buffer,
    area: Rect,
    agent_view_label: Option<&str>,
    config: &ClientShellConfig,
    hits: &mut ShellHitMap,
) -> bool {
    if area.height == 0 {
        return false;
    }
    put_text(
        buffer,
        area.x,
        area.y,
        area.width,
        &"─".repeat(area.width as usize),
        Style::default().fg(config.palette.surface_dim),
    );
    if area.height < 2 {
        return false;
    }
    put_text(
        buffer,
        area.x,
        area.y + 1,
        area.width,
        " agents",
        Style::default()
            .fg(config.palette.overlay0)
            .add_modifier(Modifier::BOLD),
    );
    let sort_label = agent_view_label.unwrap_or(match config.agent_panel_sort {
        crate::config::AgentPanelSortConfig::Spaces => "grouped",
        crate::config::AgentPanelSortConfig::Priority => "priority",
    });
    let sort_width = display_width(sort_label).min(area.width as usize) as u16;
    let sort_rect = Rect::new(
        area.right().saturating_sub(sort_width),
        area.y + 1,
        sort_width,
        1,
    );
    hits.agent_sort_toggle = if config.mouse_capture && agent_view_label.is_none() {
        sort_rect
    } else {
        Rect::default()
    };
    put_text(
        buffer,
        sort_rect.x,
        sort_rect.y,
        sort_rect.width,
        sort_label,
        Style::default()
            .fg(if agent_view_label.is_some() {
                config.palette.accent
            } else {
                config.palette.overlay0
            })
            .add_modifier(Modifier::BOLD),
    );
    true
}

pub(super) fn render_agent_list<T>(
    buffer: &mut Buffer,
    area: Rect,
    rows: &[T],
    empty_message: Option<&str>,
    config: &ClientShellConfig,
    agent_scroll: &mut usize,
    hits: &mut ShellHitMap,
    row_lines: impl Fn(&T) -> usize,
    mut render_row: impl FnMut(&mut Buffer, Rect, &T, &mut ShellHitMap),
) {
    let body = Rect::new(
        area.x,
        area.y.saturating_add(3),
        area.width,
        area.height.saturating_sub(3),
    );
    hits.agent_body = body;
    if body.is_empty() || rows.is_empty() {
        *agent_scroll = 0;
        if let Some(message) = empty_message.filter(|_| !body.is_empty()) {
            put_text(
                buffer,
                body.x,
                body.y,
                body.width,
                message,
                Style::default()
                    .fg(config.palette.overlay0)
                    .add_modifier(Modifier::DIM),
            );
        }
        return;
    }

    let row_heights = rows
        .iter()
        .map(|row| row_lines(row).max(1).min(u16::MAX as usize) as u16)
        .collect::<Vec<_>>();
    let gaps = rows
        .iter()
        .enumerate()
        .map(|(index, _)| {
            if index + 1 < rows.len() {
                config.agents.row_gap
            } else {
                0
            }
        })
        .collect::<Vec<_>>();
    let metrics =
        super::scroll::list_scroll_metrics(&row_heights, &gaps, body.height, *agent_scroll);
    hits.agent_max_scroll = metrics.max_offset_from_bottom;
    hits.agent_scroll_metrics = Some(metrics);
    *agent_scroll = metrics
        .max_offset_from_bottom
        .saturating_sub(metrics.offset_from_bottom);
    let show_scrollbar = metrics.max_offset_from_bottom > 0 && body.width > 1;
    let content_width = body.width.saturating_sub(u16::from(show_scrollbar));
    let mut y = body.y;
    for (index, row) in rows.iter().enumerate().skip(*agent_scroll) {
        let height = row_heights[index].min(body.height);
        if y.saturating_add(height) > body.bottom() {
            break;
        }
        let rect = Rect::new(body.x, y, content_width, height);
        render_row(buffer, rect, row, hits);
        y = y
            .saturating_add(height)
            .saturating_add(if index + 1 < rows.len() {
                config.agents.row_gap
            } else {
                0
            });
    }

    if show_scrollbar {
        let track = Rect::new(body.right().saturating_sub(1), body.y, 1, body.height);
        hits.agent_scrollbar = track;
        super::scroll::render_list_scrollbar(buffer, track, metrics, &config.palette);
    }
}

pub(super) fn agent_rows<'a>(
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    machine: Option<&str>,
    tasks: Option<AgentTaskSource<'a>>,
) -> Vec<AgentRow<'a>> {
    ordered_agent_pane_ids(snapshot, config.agent_panel_sort)
        .into_iter()
        .filter_map(|pane_id| agent_row(snapshot, &pane_id, config, machine, tasks))
        .collect()
}

pub(super) fn agent_row<'a>(
    snapshot: &ClientShellSnapshot,
    pane_id: &str,
    config: &ClientShellConfig,
    machine: Option<&str>,
    tasks: Option<AgentTaskSource<'a>>,
) -> Option<AgentRow<'a>> {
    let agent = snapshot
        .agents
        .iter()
        .find(|agent| agent.pane_id == pane_id)?;
    let workspace = snapshot
        .workspaces
        .iter()
        .find(|workspace| workspace.workspace_id == agent.workspace_id)?;
    let tab = snapshot.tabs.iter().find(|tab| tab.tab_id == agent.tab_id);
    let pane = snapshot
        .panes
        .iter()
        .find(|pane| pane.pane_id == agent.pane_id);
    let tab_count = snapshot
        .tabs
        .iter()
        .filter(|candidate| candidate.workspace_id == agent.workspace_id)
        .count();
    let tab_label = tab
        .filter(|tab| tab_count > 1 || tab.custom_label)
        .map(|tab| tab.label.as_str());
    let agent_label = agent
        .display_agent
        .as_deref()
        .or(agent.name.as_deref())
        .or(agent.agent.as_deref())
        .or(agent.title.as_deref());
    let labels = agent
        .state_labels
        .iter()
        .cloned()
        .collect::<HashMap<_, _>>();
    let tokens = agent.tokens.iter().cloned().collect::<HashMap<_, _>>();
    let state_text = labels
        .get(status_text(agent.agent_status))
        .map(String::as_str)
        .unwrap_or_else(|| sidebar_status_text(agent.agent_status));
    let canonical_agent = agent
        .agent
        .as_deref()
        .and_then(crate::detect::parse_agent_label);
    let rows = crate::ui::sidebar_agent_rows(
        &config.agents,
        crate::ui::AgentTokenContext {
            machine,
            workspace: &workspace.label,
            tab: tab_label,
            pane: agent
                .title
                .as_deref()
                .or_else(|| pane.and_then(|pane| pane.label.as_deref())),
            agent_label,
            terminal_title: agent.terminal_title.as_deref(),
            terminal_title_stripped: agent.terminal_title_stripped.as_deref(),
            canonical_agent,
            tokens: &tokens,
        },
        state_text,
    );
    let tasks_source_fits = tasks.is_some_and(|source| source.toggle_fits);
    let (tasks, tasks_collapsed) = tasks.map_or((Vec::new(), false), |source| {
        source.lines_for(&agent.pane_id)
    });
    Some(AgentRow {
        pane_id: agent.pane_id.clone(),
        status: agent.agent_status,
        focused: agent.focused,
        rows,
        tasks,
        tasks_collapsed,
        toggle_fits: tasks_source_fits,
    })
}

/// Draws the row and its task children. Returns the collapse toggle's hit rect when the row
/// has tasks.
pub(super) fn render_agent_row(
    buffer: &mut Buffer,
    rect: Rect,
    row: &AgentRow,
    config: &ClientShellConfig,
) -> Option<Rect> {
    let palette = &config.palette;
    let row_style = if row.focused {
        Style::default().bg(palette.active_row_bg)
    } else {
        Style::default()
    };
    let name_style = if row.focused {
        Style::default()
            .fg(palette.text)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(palette.subtext0)
            .add_modifier(Modifier::BOLD)
    };
    let status_style = Style::default().fg(status_color(row.status, palette));
    let secondary = Style::default().fg(palette.overlay0);
    let icon = (
        status_icon(row.status, config.status_indicators),
        Style::default().fg(status_color(row.status, palette)),
    );
    let rows = if row.rows.is_empty() {
        vec![vec![crate::ui::ResolvedToken {
            kind: crate::ui::ResolvedTokenKind::StateIcon,
            style: Default::default(),
        }]]
    } else {
        row.rows.clone()
    };
    // Room for the collapse toggle (and the rolled-up child status when collapsed).
    let toggle_reserve = match (row.tasks.is_empty(), row.tasks_collapsed) {
        (true, _) => 0,
        (false, false) => 2,
        (false, true) => 4,
    };
    let toggle_reserve = if row.toggle_fits { toggle_reserve } else { 0 };
    for (index, tokens) in rows.iter().take(rect.height as usize).enumerate() {
        let indent = if index == 0 { 1 } else { 3 };
        let reserve = if index == 0 { toggle_reserve } else { 0 };
        let mut spans = vec![ratatui::text::Span::raw(" ".repeat(indent))];
        spans.extend(crate::ui::resolved_token_spans(
            tokens,
            icon,
            status_style,
            name_style,
            secondary,
            secondary,
            palette,
            rect.width.saturating_sub(indent as u16 + reserve) as usize,
        ));
        Paragraph::new(Line::from(spans)).style(row_style).render(
            Rect::new(rect.x, rect.y + index as u16, rect.width, 1),
            buffer,
        );
    }
    if row.tasks.is_empty() {
        return None;
    }

    let mut toggle = None;
    if toggle_reserve > 0 && rect.height > 0 {
        let toggle_rect = Rect::new(rect.right() - 1, rect.y, 1, 1);
        put_text(
            buffer,
            toggle_rect.x,
            toggle_rect.y,
            1,
            super::sidebar::group_toggle_glyph(row.tasks_collapsed),
            Style::default().fg(palette.accent),
        );
        if row.tasks_collapsed {
            if let Some(status) = rolled_up_task_status(&row.tasks) {
                put_text(
                    buffer,
                    rect.right() - 3,
                    rect.y,
                    1,
                    task_icon(status),
                    Style::default().fg(status_color(status, palette)),
                );
            }
        }
        toggle = Some(toggle_rect);
    }
    if row.tasks_collapsed {
        return toggle;
    }

    let first_child_y = rect.y + rows.len() as u16;
    for (index, task) in row.tasks.iter().enumerate() {
        let y = first_child_y.saturating_add(index as u16);
        if y >= rect.bottom() {
            break;
        }
        let connector = if index + 1 == row.tasks.len() {
            "└─ "
        } else {
            "├─ "
        };
        if row.focused {
            buffer.set_style(Rect::new(rect.x, y, rect.width, 1), row_style);
        }
        let mut x = rect.x.saturating_add(1);
        x = super::render::put_segment(buffer, x, y, rect.right(), connector, secondary);
        x = super::render::put_segment(
            buffer,
            x,
            y,
            rect.right(),
            task_icon(task.status),
            Style::default().fg(status_color(task.status, palette)),
        );
        x = super::render::put_segment(buffer, x, y, rect.right(), " ", secondary);
        let label_style = Style::default().fg(if row.focused {
            palette.text
        } else {
            palette.subtext0
        });
        super::render::put_text(
            buffer,
            x,
            y,
            rect.right().saturating_sub(x),
            task.label,
            label_style,
        );
    }
    toggle
}

fn put_text(buffer: &mut Buffer, x: u16, y: u16, width: u16, text: &str, style: Style) {
    for (offset, character) in text.chars().take(width as usize).enumerate() {
        if let Some(cell) = buffer.cell_mut((x + offset as u16, y)) {
            cell.set_char(character).set_style(style);
        }
    }
}

fn display_width(text: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(text)
}

fn sidebar_status_text(status: crate::api::schema::AgentStatus) -> &'static str {
    use crate::api::schema::AgentStatus;
    match status {
        AgentStatus::Blocked => "blocked",
        AgentStatus::Done => "done",
        AgentStatus::Working => "working",
        AgentStatus::Idle | AgentStatus::Unknown => "idle",
    }
}
