<#
.SYNOPSIS
    Merges development updates to main while keeping the target branch clean of agent metadata.
.DESCRIPTION
    Merges a source branch (e.g. dev) to a target branch (e.g. main) using --no-commit,
    removes agent workflows, dev-only docs, and build artifacts from the staging index,
    strips agent-specific patterns from .gitignore, and commits the clean merge.
.PARAMETER SourceBranch
    The development or feature branch to merge from. Defaults to 'dev'.
.PARAMETER TargetBranch
    The release branch to merge into. Defaults to 'main'.
.PARAMETER Message
    Optional custom commit message for the merge.
.EXAMPLE
    .\scripts\Merge-ToMain.ps1
.EXAMPLE
    .\scripts\Merge-ToMain.ps1 -SourceBranch "refactor/opc-da-integration" -Message "release: v0.3.0"
#>
[CmdletBinding()]
param(
    [string]$SourceBranch = 'dev',
    [string]$TargetBranch = 'main',
    [string]$Message
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Helper function to run native git commands safely under Stop error action preference
function Invoke-Git {
    param([scriptblock]$ScriptBlock)
    $oldPreference = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & $ScriptBlock
    } finally {
        $ErrorActionPreference = $oldPreference
    }
}

# --- Pre-flight checks ---

$RepoRoot = Invoke-Git { git rev-parse --show-toplevel 2>$null }
if ($LASTEXITCODE -ne 0) {
    Write-Error "Not inside a git repository."
    exit 3
}
$RepoRoot = $RepoRoot.Trim()

$OriginalBranch = Invoke-Git { git branch --show-current }
if (-not $OriginalBranch) {
    Write-Error "Could not determine the current branch."
    exit 3
}

# Validate source branch exists
Invoke-Git { git rev-parse --verify $SourceBranch 2>$null | Out-Null }
if ($LASTEXITCODE -ne 0) {
    Write-Error "Source branch '$SourceBranch' does not exist."
    exit 3
}

# Validate target branch exists
Invoke-Git { git rev-parse --verify $TargetBranch 2>$null | Out-Null }
if ($LASTEXITCODE -ne 0) {
    Write-Error "Target branch '$TargetBranch' does not exist."
    exit 3
}

$Status = Invoke-Git { git status --porcelain }
if ($Status) {
    Write-Error "Working directory is not clean. Commit or stash changes before running."
    exit 3
}

if (-not $Message) {
    $Message = "release: merge $SourceBranch to $TargetBranch (clean)"
}

Write-Output "=== Clean Merge: $SourceBranch -> $TargetBranch ==="
Write-Output ""

# Files to remove on main
$removeFiles = @(
    '.agents/workflows/',
    'context.md',
    'architecture.md',
    'TODO.md',
    'long_term_todo.md',
    'clippy_output.json',
    'compat/',
    'dist/'
)

# --- Step 1: Checkout target ---

Write-Output "[1/5] Checking out $TargetBranch..."
Invoke-Git { git checkout $TargetBranch }
if ($LASTEXITCODE -ne 0) {
    Write-Error "Failed to checkout $TargetBranch."
    exit 3
}

# --- Step 2: Merge (no-commit) ---

Write-Output "[2/5] Merging $SourceBranch --no-commit --no-ff..."
Invoke-Git { git merge $SourceBranch --no-commit --no-ff }
if ($LASTEXITCODE -ne 0) {
    # Check if conflicts are ONLY on files we are going to remove anyway
    $conflicts = Invoke-Git { git diff --name-only --diff-filter=U }
    $unresolvedConflicts = @()
    foreach ($c in $conflicts) {
        if (-not $c) { continue }
        $cTrim = $c.Trim()
        if ([string]::IsNullOrEmpty($cTrim)) { continue }
        $match = $false
        foreach ($rf in $removeFiles) {
            if ($cTrim -like "$rf*") {
                $match = $true
                break
            }
        }
        if (-not $match) {
            $unresolvedConflicts += $cTrim
        }
    }

    if ($unresolvedConflicts.Count -gt 0) {
        Write-Output "[FAIL] Merge had real conflicts on: $($unresolvedConflicts -join ', ')."
        Write-Output "Aborting and returning to $OriginalBranch."
        Invoke-Git { git merge --abort 2>$null }
        Invoke-Git { git checkout $OriginalBranch 2>$null }
        exit 3
    }

    Write-Output "  Resolving modify/delete conflicts on dev-only files..."
}

# --- Step 3: Remove dev-only files from staging ---

Write-Output "[3/5] Removing dev-only files from staging..."

foreach ($f in $removeFiles) {
    # Check if path exists in the index or as conflict before attempting removal
    $inIndex = Invoke-Git { git ls-files -- $f 2>$null }
    $inConflicts = Invoke-Git { git diff --name-only --diff-filter=U -- $f 2>$null }
    if ($inIndex -or $inConflicts) {
        if ($f.EndsWith('/')) {
            Invoke-Git { git rm -r -f --quiet $f 2>$null }
        } else {
            Invoke-Git { git rm -f --quiet $f 2>$null }
        }
        Write-Output "  Removed: $f"
    }
}

# --- Step 4: Clean .gitignore ---

Write-Output "[4/5] Cleaning .gitignore..."
$GitIgnorePath = Join-Path $RepoRoot ".gitignore"

if (Test-Path $GitIgnorePath) {
    # Lines to strip (exact match after trim)
    $removeLines = @(
        '# Project Governance (Local Only)',
        'gemini.md',
        'GEMINI.md',
        'coding_standard.md',
        'coding_standards.md',
        '# Agent Configuration (synced from rules repo)',
        '.agents/rules/',
        '.agents/scripts/',
        '.agents/workflows/*',
        '# Project-specific workflows (tracked in this repo)',
        '!.agents/workflows/log-audit.md',
        '!.agents/workflows/prepublish.md'
    )

    $lines = Get-Content -Path $GitIgnorePath -Encoding UTF8
    $filtered = $lines | Where-Object { $removeLines -notcontains $_.Trim() }

    # Collapse consecutive blank lines
    $result = @()
    $lastBlank = $false
    foreach ($line in $filtered) {
        $isBlank = [string]::IsNullOrWhiteSpace($line)
        if ($isBlank -and $lastBlank) { continue }
        $result += $line
        $lastBlank = $isBlank
    }

    Set-Content -Path $GitIgnorePath -Value ($result -join "`r`n") -Encoding ASCII -NoNewline
    Invoke-Git { git add .gitignore }
    Write-Output "  Stripped agent rules from .gitignore"
}

# --- Step 5: Commit and return ---

Write-Output "[5/5] Committing clean merge..."
Invoke-Git { git commit -m $Message }
if ($LASTEXITCODE -ne 0) {
    Write-Output "[FAIL] Commit failed. Aborting merge and returning to $OriginalBranch."
    Invoke-Git { git merge --abort 2>$null }
    Invoke-Git { git checkout $OriginalBranch 2>$null }
    exit 3
}

$LogLine = Invoke-Git { git log -n 1 --oneline }
Write-Output ""
Write-Output "[OK] $LogLine"
Write-Output ""

Invoke-Git { git checkout $OriginalBranch }
Write-Output "Returned to $OriginalBranch."
Write-Output "=== Clean Merge Complete ==="
exit 0
