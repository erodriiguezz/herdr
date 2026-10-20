use super::*;

use crate::api::schema::{AgentTaskStatus, Method, PaneReportAgentTaskParams, Request};
use crate::protocol::endpoint::{
    EndpointAgentTaskStatus, EndpointAgentTasks, AGENT_TASKS_KIND, ENDPOINT_SNAPSHOT_KIND,
};

fn connect(
    server: &mut HeadlessServer,
    client_id: u64,
    agent_tasks: bool,
) -> std::sync::mpsc::Receiver<Vec<u8>> {
    let (writer, control_rx, _render_rx) = test_client_writer();
    server.handle_server_event(ServerEvent::ClientShellConnected {
        surface_reuse: false,
        surface_delta: false,
        surface_scroll: false,
        agent_tasks,
        client_id,
        surface_cols: 80,
        surface_rows: 23,
        cell_width_px: 0,
        cell_height_px: 0,
        pixel_mouse: false,
        direct_graphics: false,
        endpoint_keybindings: false,
        mouse_capture: false,
        surface_active: false,
        writer,
    });
    control_rx
}

/// Every endpoint control message queued so far, as `(kind, data)`.
fn drain_controls(rx: &std::sync::mpsc::Receiver<Vec<u8>>) -> Vec<(String, String)> {
    let mut controls = Vec::new();
    // The writer hands control frames over asynchronously, so wait briefly for stragglers.
    while let Ok(bytes) = rx.recv_timeout(Duration::from_millis(100)) {
        if let ServerMessage::EndpointControl { kind, data } = read_server_message(bytes) {
            controls.push((kind, data));
        }
    }
    controls
}

fn task_messages(controls: &[(String, String)]) -> Vec<EndpointAgentTasks> {
    controls
        .iter()
        .filter(|(kind, _)| kind == AGENT_TASKS_KIND)
        .map(|(_, data)| serde_json::from_str(data).expect("agent tasks companion"))
        .collect()
}

fn report(server: &mut HeadlessServer, pane: &str, seq: u64, status: AgentTaskStatus) {
    let response = server.app.handle_api_request(Request {
        id: "req".into(),
        method: Method::PaneReportAgentTask(PaneReportAgentTaskParams {
            pane_id: pane.into(),
            source: "herdr:claude".into(),
            seq,
            task_id: "t1".into(),
            status,
            description: Some("Review the diff".into()),
            agent_type: None,
            branch: None,
            cwd: None,
        }),
    });
    assert!(response.contains("\"ok\""), "report failed: {response}");
}

fn server_with_two_workspaces() -> (HeadlessServer, String) {
    let mut server = test_headless_server();
    server.app.state.workspaces = vec![
        crate::workspace::Workspace::test_new("a"),
        crate::workspace::Workspace::test_new("b"),
    ];
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    server.app.state.mode = crate::app::Mode::Terminal;
    let pane = server.app.state.workspaces[1].tabs[0].root_pane;
    let public = server.app.public_pane_id(1, pane).expect("public pane id");
    (server, public)
}

// AC-2.7
#[tokio::test]
async fn ac_2_7_agent_tasks_companion_is_opt_in_and_sent_only_on_change() {
    let (mut server, pane) = server_with_two_workspaces();
    server.app.acknowledge_focused_agent_tasks();
    let old_rx = connect(&mut server, 1, false);
    let new_rx = connect(&mut server, 2, true);
    assert!(task_messages(&drain_controls(&old_rx)).is_empty());
    let seed = task_messages(&drain_controls(&new_rx));
    assert_eq!(seed.len(), 1, "capable clients get a baseline on connect");
    assert!(seed[0].tasks.is_empty());

    report(&mut server, &pane, 1, AgentTaskStatus::Working);
    server.render_and_stream();

    // A client without the capability sees no change at all: no companion and the v1
    // snapshot it already holds stays current.
    assert!(drain_controls(&old_rx).is_empty());

    // A client with it gets exactly one companion, paired with the snapshot revision.
    let new = drain_controls(&new_rx);
    let sent = task_messages(&new);
    assert_eq!(sent.len(), 1);
    let snapshot_revision = serde_json::from_str::<protocol::ClientShellSnapshot>(
        &new.iter()
            .find(|(kind, _)| kind == ENDPOINT_SNAPSHOT_KIND)
            .expect("snapshot")
            .1,
    )
    .unwrap()
    .revision;
    assert_eq!(sent[0].revision, snapshot_revision);
    let tasks = &sent[0].tasks[&pane];
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].label, "Review the diff");
    assert_eq!(tasks[0].status, EndpointAgentTaskStatus::Working);

    // Nothing changed: no companion. A stale report changes nothing either.
    server.render_and_stream();
    report(&mut server, &pane, 1, AgentTaskStatus::Working);
    server.render_and_stream();
    assert!(task_messages(&drain_controls(&new_rx)).is_empty());

    // A real change sends one more, and the old client still gets none.
    report(&mut server, &pane, 2, AgentTaskStatus::Done);
    server.render_and_stream();
    let controls = drain_controls(&new_rx);
    let sent = task_messages(&controls);
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].tasks[&pane][0].status,
        EndpointAgentTaskStatus::Done
    );
    assert!(drain_controls(&old_rx).is_empty());

    // Focusing the pane drops the finished task and the empty list is sent once.
    server.app.state.active = Some(1);
    assert!(server.app.acknowledge_focused_agent_tasks());
    server.render_and_stream();
    let sent = task_messages(&drain_controls(&new_rx));
    assert_eq!(sent.len(), 1);
    assert!(sent[0].tasks.is_empty());
    server.render_and_stream();
    assert!(task_messages(&drain_controls(&new_rx)).is_empty());

    // A client that connects later is seeded with the existing list, before its snapshot.
    report(&mut server, &pane, 3, AgentTaskStatus::Working);
    server.render_and_stream();
    let seeded_rx = connect(&mut server, 3, true);
    let controls = drain_controls(&seeded_rx);
    let kinds: Vec<&str> = controls.iter().map(|(kind, _)| kind.as_str()).collect();
    let tasks_at = kinds.iter().position(|kind| *kind == AGENT_TASKS_KIND);
    let snapshot_at = kinds
        .iter()
        .position(|kind| *kind == ENDPOINT_SNAPSHOT_KIND);
    assert!(
        tasks_at.is_some() && tasks_at < snapshot_at,
        "companion precedes the snapshot: {kinds:?}"
    );
    assert_eq!(task_messages(&controls)[0].tasks[&pane].len(), 1);
}
