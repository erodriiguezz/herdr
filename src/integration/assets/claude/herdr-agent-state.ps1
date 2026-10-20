# installed by herdr
# managed by herdr; reinstalling or updating the integration overwrites this file.
# add custom hooks beside this file instead of editing it.
# HERDR_INTEGRATION_ID=claude
# HERDR_INTEGRATION_VERSION=12

param([string]$Action = "")

if ($Action -ne "session" -and $Action -ne "task") { exit 0 }
if ($env:HERDR_ENV -ne "1") { exit 0 }
if ([string]::IsNullOrWhiteSpace($env:HERDR_PANE_ID)) { exit 0 }

# Read hook input as raw bytes so UTF-8 payloads decode correctly even when the
# console input code page is not UTF-8 (for example CP949 on Korean Windows).
$stdinReader = New-Object System.IO.StreamReader([Console]::OpenStandardInput(), [System.Text.Encoding]::UTF8)
$inputText = $stdinReader.ReadToEnd()
try {
    $payload = if ([string]::IsNullOrWhiteSpace($inputText)) { $null } else { $inputText | ConvertFrom-Json }
} catch {
    exit 0
}

$propertyNames = @($payload.PSObject.Properties.Name)
if ((Test-Path Env:CURSOR_VERSION) -or $propertyNames -ccontains "cursor_version") { exit 0 }
if ($Action -eq "task") {
    $taskId = $null
    $taskStatus = $null
    $description = $null
    $agentType = $null
    if ($payload.hook_event_name -ceq "PostToolUse") {
        $response = $payload.tool_response
        if (($payload.tool_name -ceq "Agent" -or $payload.tool_name -ceq "Task") -and
            $null -ne $response -and $response.status -ceq "async_launched" -and
            $response.agentId -is [string] -and -not [string]::IsNullOrWhiteSpace($response.agentId)) {
            $taskId = $response.agentId.Trim()
            $taskStatus = "working"
            $toolInput = $payload.tool_input
            foreach ($candidate in @($toolInput.description, $response.description)) {
                if ($candidate -is [string] -and -not [string]::IsNullOrWhiteSpace($candidate)) {
                    $description = $candidate.Trim()
                    break
                }
            }
            if ($toolInput.subagent_type -is [string] -and -not [string]::IsNullOrWhiteSpace($toolInput.subagent_type)) {
                $agentType = $toolInput.subagent_type.Trim()
            }
        }
    } elseif ($payload.hook_event_name -ceq "SubagentStop") {
        if ($payload.agent_id -is [string] -and -not [string]::IsNullOrWhiteSpace($payload.agent_id)) {
            $taskId = $payload.agent_id.Trim()
            $taskStatus = "done"
        }
    }
    if ($null -eq $taskId) { exit 0 }

    # Unix nanoseconds, the same unit as the shell hook, so both order together.
    $taskSeq = [long][DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() * 1000000
    $herdr = if ([string]::IsNullOrWhiteSpace($env:HERDR_BIN_PATH)) { "herdr" } else { $env:HERDR_BIN_PATH }
    try {
        $args = @(
            "pane",
            "report-task",
            $env:HERDR_PANE_ID,
            "--source",
            "herdr:claude",
            "--seq",
            "$taskSeq",
            "--task-id=$taskId",
            "--status",
            $taskStatus
        )
        if ($null -ne $description) { $args += "--description=$description" }
        if ($null -ne $agentType) { $args += "--agent-type=$agentType" }
        & $herdr @args 2>$null | Out-Null
    } catch {
    }
    exit 0
}

if (-not ($propertyNames -ccontains "hook_event_name") -or $payload.hook_event_name -isnot [string] -or $payload.hook_event_name -cne "SessionStart") { exit 0 }
if (-not [string]::IsNullOrWhiteSpace($payload.agent_id)) { exit 0 }

$sessionId = $payload.session_id
if ([string]::IsNullOrWhiteSpace($sessionId)) { exit 0 }

$seq = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
$herdr = if ([string]::IsNullOrWhiteSpace($env:HERDR_BIN_PATH)) { "herdr" } else { $env:HERDR_BIN_PATH }
try {
    $args = @(
        "pane",
        "report-agent-session",
        $env:HERDR_PANE_ID,
        "--source",
        "herdr:claude",
        "--agent",
        "claude",
        "--seq",
        "$seq",
        "--agent-session-id",
        "$sessionId"
    )
    if ($payload.transcript_path -is [string] -and -not [string]::IsNullOrWhiteSpace($payload.transcript_path)) {
        $args += @("--agent-session-path", "$($payload.transcript_path)")
    }
    if ($payload.hook_event_name -eq "SessionStart" -and $payload.source -is [string] -and -not [string]::IsNullOrWhiteSpace($payload.source)) {
        $args += @("--session-start-source", "$($payload.source)")
    }
    & $herdr @args 2>$null | Out-Null
} catch {
}
