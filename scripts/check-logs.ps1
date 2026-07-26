<#
.SYNOPSIS
    Inspect application logs for errors and warnings.
.DESCRIPTION
    Dynamically discovers the latest log file(s) in the logs/ directory,
    filters by severity level, and outputs a structured summary with
    colorized entries for human and AI diagnostics.
.PARAMETER Lines
    Number of matching tail lines to display. Default: 50.
.PARAMETER Level
    Minimum severity filter: TRACE, DEBUG, INFO, WARN, or ERROR. Default: WARN.
.PARAMETER All
    Scan all log files, not just the latest.
.PARAMETER Lifecycle
    Extract and display COM lifecycle, connection pool, and operation timing events.
.PARAMETER DeepAnalysis
    Statistical analysis: timing stats, connection churn ratio, repetition analysis, span integrity.
#>

param(
    [int]$Lines = 50,
    [ValidateSet("TRACE", "DEBUG", "INFO", "WARN", "ERROR")]
    [string]$Level = "WARN",
    [switch]$All,
    [switch]$Lifecycle,
    [switch]$DeepAnalysis
)

$ErrorActionPreference = 'Stop'

# --- Discovery ---
$logDir = Join-Path (Split-Path -Parent $PSScriptRoot) "logs"

if (-not (Test-Path $logDir)) {
    Write-Host "No logs/ directory found." -ForegroundColor Red
    exit 1
}

$logFiles = Get-ChildItem $logDir -File | Sort-Object LastWriteTime -Descending

if ($logFiles.Count -eq 0) {
    Write-Host "No log files found in $logDir" -ForegroundColor Red
    exit 1
}

if (-not $All) {
    $logFiles = @($logFiles[0])
}

# --- Severity hierarchy ---
$severityOrder = @{ "TRACE" = 0; "DEBUG" = 1; "INFO" = 2; "WARN" = 3; "ERROR" = 4 }
$minSeverity = $severityOrder[$Level]
# Tracing format: "2026-02-22T03:13:24.527Z  INFO module::path: message"
# Anchor severity to the tracing timestamp prefix to avoid false positives
# from data containing words like "Error" in tag names.
$levelPattern = switch ($Level) {
    "TRACE" { '\d{4}-\d{2}-\d{2}T[\d:.]+Z\s+(TRACE|DEBUG|INFO|WARN|ERROR)' }
    "DEBUG" { '\d{4}-\d{2}-\d{2}T[\d:.]+Z\s+(DEBUG|INFO|WARN|ERROR)' }
    "INFO"  { '\d{4}-\d{2}-\d{2}T[\d:.]+Z\s+(INFO|WARN|ERROR)' }
    "WARN"  { '\d{4}-\d{2}-\d{2}T[\d:.]+Z\s+(WARN|ERROR)' }
    "ERROR" { '\d{4}-\d{2}-\d{2}T[\d:.]+Z\s+ERROR' }
}

# --- Process each file ---
$totalLines = 0
$totalTrace = 0
$totalDebug = 0
$totalInfo = 0
$totalWarn = 0
$totalError = 0
$hasErrors = $false
$matchedLines = @()

foreach ($file in $logFiles) {
    $content = Get-Content $file.FullName
    $fileLines = $content.Count
    $totalLines += $fileLines

    # Count severities using anchored patterns
    $traceCount = ($content | Select-String -Pattern '\d{4}-\d{2}-\d{2}T[\d:.]+Z\s+TRACE').Count
    $debugCount = ($content | Select-String -Pattern '\d{4}-\d{2}-\d{2}T[\d:.]+Z\s+DEBUG').Count
    $infoCount  = ($content | Select-String -Pattern '\d{4}-\d{2}-\d{2}T[\d:.]+Z\s+INFO').Count
    $warnCount  = ($content | Select-String -Pattern '\d{4}-\d{2}-\d{2}T[\d:.]+Z\s+WARN').Count
    $errorCount = ($content | Select-String -Pattern '\d{4}-\d{2}-\d{2}T[\d:.]+Z\s+ERROR').Count

    $totalTrace += $traceCount
    $totalDebug += $debugCount
    $totalInfo  += $infoCount
    $totalWarn  += $warnCount
    $totalError += $errorCount

    if ($errorCount -gt 0) { $hasErrors = $true }

    # Collect matching lines
    $matches = $content | Select-String -Pattern $levelPattern
    $matchedLines += $matches | Select-Object -Last $Lines

    # --- File Header ---
    Write-Host "`n========================================" -ForegroundColor Cyan
    Write-Host " Log : $($file.Name)" -ForegroundColor Cyan
    Write-Host " Size: $([math]::Round($file.Length / 1KB, 1)) KB" -ForegroundColor Cyan
    Write-Host " Lines: $fileLines" -ForegroundColor Cyan
    Write-Host " TRACE: $traceCount | DEBUG: $debugCount | INFO: $infoCount | WARN: $warnCount | ERROR: $errorCount" -ForegroundColor Cyan
    Write-Host "========================================" -ForegroundColor Cyan
}

# --- Output matched lines ---
Write-Host "`n--- Last $Lines $Level+ entries ---`n" -ForegroundColor Yellow

$tail = $matchedLines | Select-Object -Last $Lines
foreach ($line in $tail) {
    $text = $line.Line
    if ($text -match '\d{4}-\d{2}-\d{2}T[\d:.]+Z\s+ERROR') {
        Write-Host $text -ForegroundColor Red
    } elseif ($text -match '\d{4}-\d{2}-\d{2}T[\d:.]+Z\s+WARN') {
        Write-Host $text -ForegroundColor DarkYellow
    } elseif ($text -match '\d{4}-\d{2}-\d{2}T[\d:.]+Z\s+INFO') {
        Write-Host $text -ForegroundColor Gray
    } elseif ($text -match '\d{4}-\d{2}-\d{2}T[\d:.]+Z\s+DEBUG') {
        Write-Host $text -ForegroundColor DarkCyan
    } else {
        Write-Host $text -ForegroundColor DarkGray
    }
}

# --- Summary ---
Write-Host "`n========================================" -ForegroundColor Cyan
Write-Host " Summary" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host " Files scanned : $($logFiles.Count)" -ForegroundColor Cyan
Write-Host " Total lines   : $totalLines" -ForegroundColor Cyan
Write-Host " TRACE          : $totalTrace" -ForegroundColor DarkGray
Write-Host " DEBUG          : $totalDebug" -ForegroundColor DarkCyan
Write-Host " INFO           : $totalInfo" -ForegroundColor Gray
Write-Host " WARN           : $totalWarn" -ForegroundColor DarkYellow
Write-Host " ERROR          : $totalError" -ForegroundColor Red
Write-Host "========================================" -ForegroundColor Cyan

# --- COM Lifecycle Extraction ---
if ($Lifecycle) {
    Write-Host "`n========================================" -ForegroundColor Magenta
    Write-Host " COM Lifecycle & Pool Events" -ForegroundColor Magenta
    Write-Host "========================================" -ForegroundColor Magenta

    # Collect all content across scanned files
    $allContent = foreach ($file in $logFiles) { Get-Content $file.FullName }

    # --- Lifecycle events ---
    $lifecyclePatterns = @(
        'COM worker thread spawned',
        'COM MTA initialized',
        'COM worker thread started',
        'COM worker thread exiting cleanly',
        'ComWorker dropping',
        'COM MTA teardown',
        'COM worker failed to initialize MTA',
        'COM worker thread panicked'
    )
    $lifecycleRegex = ($lifecyclePatterns | ForEach-Object { [regex]::Escape($_) }) -join '|'
    $lifecycleLines = $allContent | Select-String -Pattern $lifecycleRegex

    Write-Host "`n  Thread Lifecycle ($($lifecycleLines.Count) events):" -ForegroundColor White
    if ($lifecycleLines.Count -eq 0) {
        Write-Host "    (none found — run with -Level DEBUG to capture)" -ForegroundColor DarkGray
    } else {
        foreach ($line in $lifecycleLines) {
            $text = $line.Line
            if ($text -match 'ERROR') {
                Write-Host "    $text" -ForegroundColor Red
            } elseif ($text -match 'initialized|started') {
                Write-Host "    $text" -ForegroundColor Green
            } elseif ($text -match 'exiting|dropping|teardown') {
                Write-Host "    $text" -ForegroundColor DarkYellow
            } else {
                Write-Host "    $text" -ForegroundColor Gray
            }
        }
    }

    # --- Connection pool events ---
    $poolPatterns = @(
        'Connection established',
        'Evicting stale connection',
        'Reconnection successful',
        'Reconnect failed',
        'Cache hit',
        'Cache miss'
    )
    $poolRegex = ($poolPatterns | ForEach-Object { [regex]::Escape($_) }) -join '|'
    $poolLines = $allContent | Select-String -Pattern $poolRegex

    Write-Host "`n  Connection Pool ($($poolLines.Count) events):" -ForegroundColor White
    if ($poolLines.Count -eq 0) {
        Write-Host "    (none found)" -ForegroundColor DarkGray
    } else {
        foreach ($line in $poolLines) {
            $text = $line.Line
            if ($text -match 'Evicting|failed') {
                Write-Host "    $text" -ForegroundColor DarkYellow
            } elseif ($text -match 'established|successful') {
                Write-Host "    $text" -ForegroundColor Green
            } else {
                Write-Host "    $text" -ForegroundColor DarkCyan
            }
        }
    }

    # --- Timing extraction ---
    $timingLines = $allContent | Select-String -Pattern 'elapsed_ms='
    Write-Host "`n  Operation Timings ($($timingLines.Count) events):" -ForegroundColor White
    if ($timingLines.Count -eq 0) {
        Write-Host "    (none found)" -ForegroundColor DarkGray
    } else {
        foreach ($line in $timingLines) {
            Write-Host "    $($line.Line)" -ForegroundColor Cyan
        }
    }

    # --- Sequence validation ---
    Write-Host "`n  Sequence Check:" -ForegroundColor White
    $spawnCount = ($lifecycleLines | Where-Object { $_.Line -match 'spawned' }).Count
    $initCount  = ($lifecycleLines | Where-Object { $_.Line -match 'initialized successfully' }).Count
    $exitCount  = ($lifecycleLines | Where-Object { $_.Line -match 'exiting cleanly' }).Count
    $dropCount  = ($lifecycleLines | Where-Object { $_.Line -match 'dropping' }).Count

    if ($spawnCount -eq $initCount -and $initCount -ge $exitCount) {
        Write-Host "    OK: spawn=$spawnCount init=$initCount exit=$exitCount drop=$dropCount" -ForegroundColor Green
    } else {
        Write-Host "    ANOMALY: spawn=$spawnCount init=$initCount exit=$exitCount drop=$dropCount" -ForegroundColor Red
    }

    Write-Host "========================================`n" -ForegroundColor Magenta
}

# --- Deep Analysis ---
if ($DeepAnalysis) {
    Write-Host "`n========================================" -ForegroundColor Blue
    Write-Host " Deep Analysis" -ForegroundColor Blue
    Write-Host "========================================" -ForegroundColor Blue

    # Collect all content with ANSI stripping
    $deepContent = foreach ($file in $logFiles) {
        Get-Content $file.FullName | ForEach-Object { $_ -replace '\x1B\[[0-9;]*m', '' }
    }

    # --- §A. Timing Statistics ---
    $timingMatches = $deepContent | Select-String -Pattern 'elapsed_ms=(\d+)'
    Write-Host "`n  §A. Timing Statistics ($($timingMatches.Count) ops):" -ForegroundColor White
    if ($timingMatches.Count -eq 0) {
        Write-Host "      No timed operations found." -ForegroundColor DarkGray
    } else {
        $nums = $timingMatches | ForEach-Object { [int]$_.Matches[0].Groups[1].Value }
        $stats = $nums | Measure-Object -Minimum -Maximum -Average
        $avg = [math]::Round($stats.Average, 1)
        Write-Host "      Min: $($stats.Minimum)ms | Max: $($stats.Maximum)ms | Avg: ${avg}ms" -ForegroundColor Cyan

        $outliers = @($nums | Where-Object { $_ -gt 100 } | Sort-Object -Descending)
        $pct = [math]::Round(($outliers.Count / $stats.Count) * 100, 1)
        if ($outliers.Count -eq 0) {
            Write-Host "      Outliers (>100ms): 0 — all within budget" -ForegroundColor Green
        } else {
            Write-Host "      Outliers (>100ms): $($outliers.Count) of $($stats.Count) (${pct}%)" -ForegroundColor Yellow
            $outlierStr = ($outliers | ForEach-Object { "${_}ms" }) -join '  '
            Write-Host "        $outlierStr" -ForegroundColor DarkYellow
        }
    }

    # --- §B. Connection Churn ---
    $connCount = ($deepContent | Select-String -Pattern 'Connection established').Count
    $refreshCount = ($deepContent | Select-String -Pattern 'Auto-refreshing tag values').Count
    $cacheHits = ($deepContent | Select-String -Pattern 'Cache hit').Count
    $evictions = ($deepContent | Select-String -Pattern 'evict' -CaseSensitive:$false).Count

    Write-Host "`n  §B. Connection Churn:" -ForegroundColor White
    if ($refreshCount -eq 0) {
        Write-Host "      Connections: $connCount | Refreshes: 0 | Ratio: N/A" -ForegroundColor DarkGray
    } else {
        $ratio = "${connCount}:${refreshCount}"
        $churnColor = if ($connCount -le 1 -or ($connCount / $refreshCount) -le 0.1) { 'Green' } else { 'Yellow' }
        Write-Host "      Connections: $connCount | Refreshes: $refreshCount | Ratio: $ratio" -ForegroundColor $churnColor
    }
    Write-Host "      Evictions: $evictions | Cache hits: $cacheHits" -ForegroundColor Cyan

    # --- §C. Repetition Analysis ---
    Write-Host "`n  §C. Top Repeated Messages:" -ForegroundColor White
    $grouped = $deepContent |
        ForEach-Object { $_ -replace '^\d{4}-\d{2}-\d{2}T[\d:.]+Z\s+\w+\s+', '' } |
        Where-Object { $_.Trim() -ne '' } |
        Group-Object |
        Sort-Object Count -Descending |
        Select-Object -First 10

    if ($grouped.Count -eq 0) {
        Write-Host "      (none)" -ForegroundColor DarkGray
    } else {
        foreach ($g in $grouped) {
            $msg = $g.Name
            if ($msg.Length -gt 100) { $msg = $msg.Substring(0, 100) + '...' }
            Write-Host "      $($g.Count.ToString().PadLeft(5))x  $msg" -ForegroundColor Gray
        }
    }

    # --- §D. Span Integrity ---
    Write-Host "`n  §D. Span Integrity:" -ForegroundColor White
    $spanMatches = $deepContent | Select-String -Pattern 'opc\.(\w+)\{'
    if ($spanMatches.Count -eq 0) {
        Write-Host "      No opc.* spans found." -ForegroundColor DarkGray
    } else {
        $spans = $spanMatches | ForEach-Object { $_.Matches[0].Groups[1].Value } |
            Group-Object |
            Sort-Object Count -Descending
        foreach ($s in $spans) {
            Write-Host "      $($s.Count.ToString().PadLeft(5))x  opc.$($s.Name)" -ForegroundColor Cyan
        }
    }

    # --- §E. HRESULT Aggregation ---
    Write-Host "`n  §E. HRESULT Aggregation:" -ForegroundColor White
    $hresultMatches = $deepContent | Select-String -Pattern 'hresult=([0-9a-fA-Fx]+)'
    if ($hresultMatches.Count -eq 0) {
        Write-Host "      No HRESULT codes found." -ForegroundColor DarkGray
    } else {
        $hrCounts = $hresultMatches | ForEach-Object { $_.Matches[0].Groups[1].Value } |
            Group-Object |
            Sort-Object Count -Descending |
            Select-Object -First 10
        foreach ($hr in $hrCounts) {
            Write-Host "      $($hr.Count.ToString().PadLeft(5))x  $($hr.Name)" -ForegroundColor Yellow
        }
    }

    # --- §F. State Transition Validation ---
    Write-Host "`n  §F. State Transition Validation:" -ForegroundColor White
    $transMatches = $deepContent | Select-String -Pattern 'from=(\w+)\s+to=(\w+)\s+trigger=(\S+)\s+screen_transition'
    if ($transMatches.Count -eq 0) {
        Write-Host "      No screen transitions found." -ForegroundColor DarkGray
    } else {
        $validTransitions = @(
            "Home->Loading", "Loading->ServerList", "Loading->Home",
            "ServerList->Loading", "ServerList->Home",
            "Loading->TagList", "Loading->ServerList",
            "TagList->Loading", "TagList->ServerList",
            "Loading->TagValues", "Loading->TagList",
            "TagValues->WriteInput", "TagValues->TagList", "TagValues->Exiting",
            "WriteInput->Loading", "WriteInput->TagValues",
            "Home->Exiting", "ServerList->Exiting", "TagList->Exiting"
        )
        $invalidCount = 0
        foreach ($tm in $transMatches) {
            $fromState = $tm.Matches[0].Groups[1].Value
            $toState = $tm.Matches[0].Groups[2].Value
            $trig = $tm.Matches[0].Groups[3].Value
            $pair = "${fromState}->${toState}"
            if ($pair -notin $validTransitions) {
                Write-Host "      ANOMALY: $pair (trigger: $trig)" -ForegroundColor Red
                $invalidCount++
            }
        }
        if ($invalidCount -eq 0) {
            Write-Host "      All $($transMatches.Count) transition(s) valid." -ForegroundColor Green
        }
    }

    Write-Host "`n========================================`n" -ForegroundColor Blue
}


if ($hasErrors) {
    Write-Host "`nErrors detected." -ForegroundColor Red
    exit 1
} else {
    Write-Host "`nNo errors found." -ForegroundColor Green
    exit 0
}
