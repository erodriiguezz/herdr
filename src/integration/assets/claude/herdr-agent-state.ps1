# installed by herdr
# managed by herdr; reinstalling or updating the integration overwrites this file.
# add custom hooks beside this file instead of editing it.
# HERDR_INTEGRATION_ID=claude
# HERDR_INTEGRATION_VERSION=12

param([string]$Action = "")

if ($Action -ne "session" -and $Action -ne "progress") { exit 0 }
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
$expectedEvent = if ($Action -eq "progress") { "PostToolUse" } else { "SessionStart" }
if (-not ($propertyNames -ccontains "hook_event_name") -or $payload.hook_event_name -isnot [string] -or $payload.hook_event_name -cne $expectedEvent) { exit 0 }

$herdr = if ([string]::IsNullOrWhiteSpace($env:HERDR_BIN_PATH)) { "herdr" } else { $env:HERDR_BIN_PATH }

if ($Action -eq "progress") {
    # Claude's task tools report one change per call; only TaskList carries the
    # whole list. The mapping is stateless: herdr merges the deltas. Deltas carry
    # no --seq so parallel hook processes cannot make herdr drop one as stale.
    # Subagent calls are forwarded: the task list is shared per session.
    function ConvertFrom-ToolValue($value) {
        if ($value -is [string]) {
            try { return ($value | ConvertFrom-Json) } catch { return $null }
        }
        return $value
    }
    function ConvertTo-TextValue($value) {
        if ($null -eq $value -or $value -is [bool]) { return $null }
        if ($value -is [string]) {
            if ([string]::IsNullOrWhiteSpace($value)) { return $null }
            return $value
        }
        if ($value -is [int] -or $value -is [long] -or $value -is [double]) { return "$value" }
        return $null
    }
    # Windows PowerShell 5.1 and PowerShell < 7.3 drop embedded double quotes
    # from native arguments unless they are escaped by hand.
    function Format-NativeArg([string]$value) {
        if ($null -eq $PSNativeCommandArgumentPassing) { return $value.Replace('"', '\"') }
        return $value
    }
    $statuses = @("pending", "in_progress", "completed", "deleted")
    try {
        $toolInput = ConvertFrom-ToolValue $payload.tool_input
        $toolResponse = ConvertFrom-ToolValue $payload.tool_response
        $common = @("--source", "herdr:claude", "--agent", "claude")
        $args = $null
        if ($payload.tool_name -ceq "TaskCreate") {
            $task = if ($null -ne $toolResponse) { $toolResponse.task } else { $null }
            $taskId = ConvertTo-TextValue $task.id
            $subject = ConvertTo-TextValue $toolInput.subject
            if (-not $subject) { $subject = ConvertTo-TextValue $task.subject }
            if (-not $taskId -or -not $subject) { exit 0 }
            $args = @("pane", "update-progress", $env:HERDR_PANE_ID) + $common + @("--id", (Format-NativeArg $taskId), "--subject", (Format-NativeArg $subject), "--status", "pending")
        } elseif ($payload.tool_name -ceq "TaskUpdate") {
            $taskId = ConvertTo-TextValue $toolInput.taskId
            if (-not $taskId) { exit 0 }
            $extra = @()
            if ($toolInput.status -is [string] -and $statuses -ccontains $toolInput.status) { $extra += @("--status", $toolInput.status) }
            $subject = ConvertTo-TextValue $toolInput.subject
            if ($subject) { $extra += @("--subject", (Format-NativeArg $subject)) }
            if ($extra.Count -eq 0) { exit 0 }
            $args = @("pane", "update-progress", $env:HERDR_PANE_ID) + $common + @("--id", (Format-NativeArg $taskId)) + $extra
        } elseif ($payload.tool_name -ceq "TaskList") {
            $tasks = $null
            $hasList = $false
            if ($toolResponse -is [array]) {
                $tasks = $toolResponse
                $hasList = $true
            } elseif ($null -ne $toolResponse -and $toolResponse.PSObject.Properties.Name -contains "tasks") {
                $tasks = $toolResponse.tasks
                $hasList = $true
            }
            if (-not $hasList) { exit 0 }
            $items = @()
            foreach ($task in @($tasks)) {
                if ($null -eq $task) { continue }
                $subject = ConvertTo-TextValue $task.subject
                if (-not $subject -or $task.status -ceq "deleted") { continue }
                $item = [ordered]@{}
                $taskId = ConvertTo-TextValue $task.id
                if ($taskId) { $item["id"] = $taskId }
                $item["text"] = $subject
                $item["status"] = if ($task.status -is [string] -and $statuses -ccontains $task.status) { $task.status } else { "pending" }
                $items += $item
            }
            $json = ConvertTo-Json -InputObject @($items) -Compress -Depth 5
            $seq = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
            $args = @("pane", "report-progress", $env:HERDR_PANE_ID) + $common + @("--seq", "$seq", "--items-json", (Format-NativeArg $json))
        } else {
            exit 0
        }
        & $herdr @args 2>$null | Out-Null
    } catch {
    }
    exit 0
}

if (-not [string]::IsNullOrWhiteSpace($payload.agent_id)) { exit 0 }

$sessionId = $payload.session_id
if ([string]::IsNullOrWhiteSpace($sessionId)) { exit 0 }

$seq = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
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
