#!/bin/sh
# installed by herdr
# managed by herdr; reinstalling or updating the integration overwrites this file.
# add custom hooks beside this file instead of editing it.
# HERDR_INTEGRATION_ID=claude
# HERDR_INTEGRATION_VERSION=12

set -eu

action="${1:-}"
hook_input_file="$(mktemp "${TMPDIR:-/tmp}/herdr-claude-hook.XXXXXX")" || exit 0
trap 'rm -f "$hook_input_file"' EXIT HUP INT TERM
cat >"$hook_input_file" 2>/dev/null || true

case "$action" in
  session | progress) ;;
  *) exit 0 ;;
esac

[ "${HERDR_ENV:-}" = "1" ] || exit 0
[ -n "${HERDR_SOCKET_PATH:-}" ] || exit 0
[ -n "${HERDR_PANE_ID:-}" ] || exit 0
command -v python3 >/dev/null 2>&1 || exit 0

HERDR_ACTION="$action" HERDR_HOOK_INPUT_FILE="$hook_input_file" python3 - <<'PY'
import json
import os
import random
import socket
import time

source = "herdr:claude"
action = os.environ.get("HERDR_ACTION", "")
pane_id = os.environ.get("HERDR_PANE_ID")
socket_path = os.environ.get("HERDR_SOCKET_PATH")
hook_input_file = os.environ.get("HERDR_HOOK_INPUT_FILE")

if not pane_id or not socket_path:
    raise SystemExit(0)

hook_input = {}
if hook_input_file:
    try:
        with open(hook_input_file, encoding="utf-8") as handle:
            content = handle.read()
        if content.strip():
            hook_input = json.loads(content)
    except Exception:
        hook_input = {}

if "CURSOR_VERSION" in os.environ or "cursor_version" in hook_input:
    raise SystemExit(0)
hook_event_name = str(hook_input.get("hook_event_name") or "")


def send(request):
    try:
        client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        client.settimeout(0.5)
        client.connect(socket_path)
        client.sendall((json.dumps(request) + "\n").encode())
        try:
            client.recv(4096)
        except Exception:
            pass
        client.close()
    except Exception:
        pass


def as_json(value):
    if isinstance(value, str):
        try:
            return json.loads(value)
        except Exception:
            return None
    return value


def as_text(value):
    if isinstance(value, bool):
        return None
    if isinstance(value, int):
        return str(value)
    if isinstance(value, str) and value.strip():
        return value
    return None


PROGRESS_STATUSES = ("pending", "in_progress", "completed", "deleted")


def progress_request(request_id, report_seq):
    # Claude's task tools report one change per call; only TaskList carries the
    # whole list. The mapping is stateless: herdr merges the deltas.
    tool_name = hook_input.get("tool_name")
    tool_input = as_json(hook_input.get("tool_input"))
    tool_response = as_json(hook_input.get("tool_response"))
    if not isinstance(tool_input, dict):
        tool_input = {}
    # Deltas carry no seq: parallel tool calls run parallel hook processes whose
    # time stamps can reorder, and a stale create would be dropped. Only the
    # full TaskList report is sequenced.
    base = {"pane_id": pane_id, "source": source, "agent": "claude"}
    if tool_name == "TaskCreate":
        task = tool_response.get("task") if isinstance(tool_response, dict) else None
        task = task if isinstance(task, dict) else {}
        task_id = as_text(task.get("id"))
        subject = as_text(tool_input.get("subject")) or as_text(task.get("subject"))
        if not task_id or not subject:
            return None
        params = dict(base, id=task_id, subject=subject, status="pending")
        return {"id": request_id, "method": "pane.update_progress", "params": params}
    if tool_name == "TaskUpdate":
        task_id = as_text(tool_input.get("taskId"))
        if not task_id:
            return None
        params = dict(base, id=task_id)
        status = tool_input.get("status")
        if status in PROGRESS_STATUSES:
            params["status"] = status
        subject = as_text(tool_input.get("subject"))
        if subject:
            params["subject"] = subject
        if "status" not in params and "subject" not in params:
            return None
        return {"id": request_id, "method": "pane.update_progress", "params": params}
    if tool_name == "TaskList":
        tasks = tool_response.get("tasks") if isinstance(tool_response, dict) else tool_response
        if not isinstance(tasks, list):
            return None
        items = []
        for task in tasks:
            if not isinstance(task, dict):
                continue
            subject = as_text(task.get("subject"))
            if not subject or task.get("status") == "deleted":
                continue
            item = {"text": subject}
            task_id = as_text(task.get("id"))
            if task_id:
                item["id"] = task_id
            status = task.get("status")
            item["status"] = status if status in PROGRESS_STATUSES else "pending"
            items.append(item)
        return {
            "id": request_id,
            "method": "pane.report_progress",
            "params": dict(base, seq=report_seq, items=items),
        }
    return None


if action == "progress":
    if hook_event_name != "PostToolUse":
        raise SystemExit(0)
    progress = progress_request(
        f"{source}:progress:{int(time.time() * 1000)}:{random.randrange(1_000_000):06d}",
        time.time_ns(),
    )
    if progress:
        send(progress)
    raise SystemExit(0)

if hook_event_name != "SessionStart":
    raise SystemExit(0)
is_subagent = bool(hook_input.get("agent_id"))
if is_subagent:
    raise SystemExit(0)
request_id = f"{source}:{int(time.time() * 1000)}:{random.randrange(1_000_000):06d}"
report_seq = time.time_ns()
session_id = hook_input.get("session_id")
agent_session_id = session_id if isinstance(session_id, str) and session_id else None
transcript_path = hook_input.get("transcript_path")
agent_session_path = transcript_path if isinstance(transcript_path, str) and transcript_path else None
session_start_source = hook_input.get("source") if hook_event_name == "SessionStart" else None
if not isinstance(session_start_source, str) or not session_start_source:
    session_start_source = None
if agent_session_id:
    params = {
        "pane_id": pane_id,
        "source": source,
        "agent": "claude",
        "seq": report_seq,
        "agent_session_id": agent_session_id,
    }
    if agent_session_path:
        params["agent_session_path"] = agent_session_path
    if session_start_source:
        params["session_start_source"] = session_start_source
    request = {
        "id": request_id,
        "method": "pane.report_agent_session",
        "params": params,
    }
else:
    raise SystemExit(0)

send(request)
PY
