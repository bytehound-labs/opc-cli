# Verification Pipeline Hardening Analysis & Technical Plan

**Milestone**: Milestone 1: Verification Pipeline Planning  
**Author**: Explorer Agent (`explorer_baseline_3`)  
**Target Workspace**: `opc-cli`  
**Status**: Plan Complete (Ready for Builder Execution)

---

## 1. Executive Summary

This document provides a comprehensive technical analysis and step-by-step design for extending the `opc-cli` quality verification pipeline (`scripts/verify.ps1`) from a 5-gate pipeline to an **8-gate universal quality pipeline**.

The 3 new automated quality gates are:
1. **Gate 6: AST-grep Scan (`sg scan`)** — Conditional static analysis scan using `ast-grep`.
2. **Gate 7: Forbidden Pattern Scanner (`rg`)** — Fast regex pattern scanning using `ripgrep` to enforce zero raw debugging/placeholder macros (`println!`, `dbg!`, `todo!`) in production library code (`opc-da-client/src/`).
3. **Gate 8: PowerShell Script Syntax & Strict Mode Check** — Native AST syntax parsing using `[System.Management.Automation.Language.Parser]::ParseFile` for all PowerShell scripts in `scripts/`.

In addition, exact documentation updates are specified for `architecture.md § Toolchain` and `.agents/rules/coding-standard.md §2`.

---

## 2. Current vs. Hardened Pipeline Architecture

| Gate # | Name | Tool / Executable | Target Scope | Current Status | Proposed Status | Failure Behavior |
|---|---|---|---|---|---|---|
| **1** | Formatter Check | `cargo fmt` | Workspace | Active | Active | Halt (`exit $LASTEXITCODE`) |
| **2** | Linter Check | `cargo clippy` | Workspace (`-D warnings`) | Active | Active | Halt (`exit $LASTEXITCODE`) |
| **3** | Doc Compilation Check | `cargo test --doc` | Workspace | Active | Active | Halt (`exit $LASTEXITCODE`) |
| **4** | Unit & Integration Tests | `cargo test` | Workspace | Active | Active | Halt (`exit $LASTEXITCODE`) |
| **5** | Polyfill Compilation | `cargo build` | `compat/*` | Active | Active | Halt (`exit $LASTEXITCODE`) |
| **6** | AST-grep Scan | `sg scan` | Workspace | **New** | **Added** | Skip if missing `sg` or `sgconfig.yml`; Halt if tool present and scan fails |
| **7** | Forbidden Pattern Scanner | `rg` | `opc-da-client/src/` | **New** | **Added** | Skip if `rg` missing; Halt if `println!`, `dbg!`, or `todo!` detected |
| **8** | PowerShell Syntax & Strict Check | PowerShell AST Parser | `scripts/*.ps1` | **New** | **Added** | Halt if AST parse errors detected in any `.ps1` file |

---

## 3. Detailed Technical Design for New Gates

### 3.1 Gate 6: AST-Grep Scan (`sg scan`)

#### Requirements & Constraints
- Must attempt `sg scan` if the `sg` executable is available in PATH.
- Must verify that `sgconfig.yml` exists in the repository root before calling `sg scan` (since `sg scan` fails with exit code 1 if `sgconfig.yml` is missing).
- If `sg` CLI is not installed OR `sgconfig.yml` does not exist, display an informative skip message without breaking pipeline execution (exit code 0).
- If both prerequisites are met, execute `Invoke-Gate -GateName "AST-Grep Scan" -Command "sg scan"`.

#### Implementation Code Snippet
```powershell
# Gate 6: AST-Grep Scan (Conditional)
Write-Host "`n>>> AST-Grep Scan" -ForegroundColor Yellow
$hasSg = [bool](Get-Command sg -ErrorAction SilentlyContinue)
$hasSgConfig = Test-Path (Join-Path $PSScriptRoot ".." "sgconfig.yml")

if (-not $hasSg) {
    Write-Host "[SKIP] AST-grep ('sg') CLI is not installed in PATH. Skipping AST-grep scan." -ForegroundColor DarkYellow
} elseif (-not $hasSgConfig) {
    Write-Host "[SKIP] sgconfig.yml not found in repository root. Skipping AST-grep scan." -ForegroundColor DarkYellow
} else {
    Invoke-Gate -GateName "AST-Grep Scan" -Command "sg scan"
}
```

---

### 3.2 Gate 7: Forbidden Pattern Scanner (`rg` / ripgrep)

#### Requirements & Constraints
- Must scan production library code (`opc-da-client/src/`) for forbidden debug and placeholder macros: `println!`, `dbg!`, `todo!`.
- Uses `ripgrep` (`rg`) for high-performance pattern matching.
- **Critical `rg` Exit Code Behavior**:
  - `rg` returns exit code **`0`** when matches ARE found. (In our case, this means forbidden patterns exist, so the gate MUST fail).
  - `rg` returns exit code **`1`** when 0 matches are found. (In our case, clean code, gate MUST pass).
  - `rg` returns exit code **`> 1`** on error (e.g. invalid arguments or unreadable path).
- If `rg` CLI is not installed in PATH, display an informative skip message (`[SKIP] ripgrep ('rg') CLI is not installed in PATH.`).

#### Implementation Code Snippet
```powershell
# Gate 7: Forbidden Pattern Scanner (ripgrep)
Write-Host "`n>>> Forbidden Pattern Scanner" -ForegroundColor Yellow
if (-not (Get-Command rg -ErrorAction SilentlyContinue)) {
    Write-Host "[SKIP] ripgrep ('rg') CLI is not installed in PATH. Skipping forbidden pattern scan." -ForegroundColor DarkYellow
} else {
    $targetPath = Join-Path $PSScriptRoot ".." "opc-da-client" "src"
    if (-not (Test-Path $targetPath)) {
        Write-Host "[SKIP] Target path '$targetPath' does not exist." -ForegroundColor DarkYellow
    } else {
        $forbiddenMatches = rg --color=never -n -g "*.rs" "\b(println!|dbg!|todo!)" $targetPath 2>&1
        $rgExit = $LASTEXITCODE

        if ($rgExit -eq 0) {
            Write-Host "========================================" -ForegroundColor Red
            Write-Host " VERIFICATION FAILED" -ForegroundColor Red
            Write-Host "========================================" -ForegroundColor Red
            Write-Host " What : Forbidden Pattern Scanner" -ForegroundColor Red
            Write-Host " Where: opc-da-client/src/" -ForegroundColor Red
            Write-Host " Why  : Found forbidden macro(s) (println!, dbg!, todo!):" -ForegroundColor Red
            $forbiddenMatches | ForEach-Object { Write-Host "   $_" -ForegroundColor Red }
            Write-Host "========================================`n" -ForegroundColor Red
            exit 1
        } elseif ($rgExit -eq 1) {
            Write-Host "No forbidden patterns (println!, dbg!, todo!) found in opc-da-client/src/." -ForegroundColor Green
        } else {
            Write-Host "========================================" -ForegroundColor Red
            Write-Host " VERIFICATION FAILED" -ForegroundColor Red
            Write-Host "========================================" -ForegroundColor Red
            Write-Host " What : Forbidden Pattern Scanner" -ForegroundColor Red
            Write-Host " Where: rg execution on opc-da-client/src/" -ForegroundColor Red
            Write-Host " Why  : ripgrep exited with error code $rgExit: $forbiddenMatches" -ForegroundColor Red
            Write-Host "========================================`n" -ForegroundColor Red
            exit $rgExit
        }
    }
}
```

---

### 3.3 Gate 8: PowerShell Script Syntax & Strict Mode Check

#### Requirements & Constraints
- Must scan all PowerShell `.ps1` script files located in the `scripts/` directory (`scripts/*.ps1`).
- Uses .NET PowerShell AST parser `[System.Management.Automation.Language.Parser]::ParseFile` to validate syntax trees without executing script side-effects.
- Aggregates errors across all script files in `scripts/`.
- If any syntax error is detected, displays file path, line numbers, error details, and exits with code `1`.

#### Implementation Code Snippet
```powershell
# Gate 8: PowerShell Script Syntax & Strict Mode Check
Write-Host "`n>>> PowerShell Script Syntax & Strict Mode Check" -ForegroundColor Yellow
$scriptDir = $PSScriptRoot
$scriptFiles = Get-ChildItem -Path $scriptDir -Filter "*.ps1" -File
$totalSyntaxErrors = 0
$syntaxErrorLog = @()

foreach ($file in $scriptFiles) {
    $tokens = $null
    $errors = $null
    $null = [System.Management.Automation.Language.Parser]::ParseFile($file.FullName, [ref]$tokens, [ref]$errors)

    if ($errors.Count -gt 0) {
        $totalSyntaxErrors += $errors.Count
        foreach ($err in $errors) {
            $syntaxErrorLog += "   $($file.Name):$($err.Extent.StartLineNumber) - $($err.Message)"
        }
    }
}

if ($totalSyntaxErrors -gt 0) {
    Write-Host "========================================" -ForegroundColor Red
    Write-Host " VERIFICATION FAILED" -ForegroundColor Red
    Write-Host "========================================" -ForegroundColor Red
    Write-Host " What : PowerShell Script Syntax Check" -ForegroundColor Red
    Write-Host " Where: scripts/*.ps1 ($($scriptFiles.Count) scripts checked)" -ForegroundColor Red
    Write-Host " Why  : Found $totalSyntaxErrors AST syntax error(s):" -ForegroundColor Red
    $syntaxErrorLog | ForEach-Object { Write-Host $_ -ForegroundColor Red }
    Write-Host "========================================`n" -ForegroundColor Red
    exit 1
} else {
    Write-Host "All $($scriptFiles.Count) PowerShell scripts passed AST syntax validation." -ForegroundColor Green
}
```

---

## 4. Planned Documentation Modifications

### 4.1 Changes to `architecture.md § Toolchain`

In `architecture.md`:
- **Section 7 Item 1 (`make verify`)**:
  - *Before*: `make verify`: Executes 5-gate quality pipeline (`pwsh scripts/verify.ps1`).
  - *After*: `make verify`: Executes 8-gate quality pipeline (`pwsh scripts/verify.ps1`).
- **Section 7 Item 4 (`scripts/verify.ps1`)**:
  - *Before*: `4. **scripts/verify.ps1**: Universal 5-gate quality pipeline (formatter, linter, doc-tests, workspace tests, polyfill compilation).`
  - *After*: 
    ```markdown
    4. **scripts/verify.ps1**: Universal 8-gate quality pipeline.
       - Gate 1: Formatter Check (`cargo fmt --all -- --check`)
       - Gate 2: Linter Check (`cargo clippy --workspace --all-targets --all-features -- -D warnings`)
       - Gate 3: Doc Compilation Check (`cargo test --doc --workspace`)
       - Gate 4: Unit & Integration Tests (`cargo test --workspace`)
       - Gate 5: Polyfill Compilation (`cargo build` for `compat/*` crates)
       - Gate 6: AST-Grep Scan (`sg scan`, conditional on CLI availability & `sgconfig.yml`)
       - Gate 7: Forbidden Pattern Scanner (`rg` search for `println!`, `dbg!`, `todo!` in `opc-da-client/src/`)
       - Gate 8: PowerShell Script Syntax & Strict Mode Check (`[Parser]::ParseFile` on `scripts/*.ps1`)
    ```

### 4.2 Changes to `.agents/rules/coding-standard.md §2`

In `.agents/rules/coding-standard.md`:
- **Section 2 Overview Code Block**:
  - *Before*: Lists 4 commands (`cargo fmt`, `cargo clippy`, `cargo test`, `sg scan`).
  - *After*:
    ```sh
    cargo fmt --all -- --check                                      # Gate 1: Formatting
    cargo clippy --workspace --all-targets --all-features -- -D warnings # Gate 2: Linting
    cargo test --doc --workspace                                    # Gate 3: Doc Compilation
    cargo test --workspace                                          # Gate 4: Unit & Integration Tests
    cargo build --manifest-path compat/.../Cargo.toml --release     # Gate 5: Polyfill Compilation
    sg scan                                                         # Gate 6: AST Linting (conditional)
    rg -n -g "*.rs" "\b(println!|dbg!|todo!)" opc-da-client/src    # Gate 7: Forbidden Pattern Scanner
    [Parser]::ParseFile                                             # Gate 8: PowerShell Script Syntax Check
    ```
- **Section 2 Compliance Table**:
  - *After*: Add rows for Doc Compilation, Polyfill Compilation, Forbidden Patterns, and PowerShell Scripts compliance targets.

---

## 5. Complete Target Blueprint for `scripts/verify.ps1`

```powershell
<#
.SYNOPSIS
    Universal Quality Gate for opc-cli (8-Gate Pipeline).
.DESCRIPTION
    Runs cargo fmt, clippy, doc tests, workspace tests, polyfill compilation,
    AST-grep scan, forbidden pattern scanner, and PowerShell syntax checks.
    Halts execution strictly on any non-zero exit code.
    Reports What/Where/Why on failure for human and AI diagnostics.
.PARAMETER Verbose
    When set, captures cargo output and replays the last 20 lines on failure.
#>

param(
    [switch]$Verbose
)

$ErrorActionPreference = 'Stop'
$ErrorView = 'NormalView'

# Temp log for -Verbose stderr capture
$script:LogFile = [System.IO.Path]::GetTempFileName()

function Invoke-Gate {
    param(
        [string]$GateName,
        [string]$Command
    )

    Write-Host "`n>>> $GateName" -ForegroundColor Yellow

    if ($Verbose) {
        Invoke-Expression "$Command 2>&1" | Tee-Object -FilePath $script:LogFile
    } else {
        Invoke-Expression $Command
    }

    if ($LASTEXITCODE -ne 0) {
        Write-Host "`n========================================" -ForegroundColor Red
        Write-Host " VERIFICATION FAILED" -ForegroundColor Red
        Write-Host "========================================" -ForegroundColor Red
        Write-Host " What : $GateName" -ForegroundColor Red
        Write-Host " Where: $Command" -ForegroundColor Red
        Write-Host " Why  : Process exited with code $LASTEXITCODE" -ForegroundColor Red

        if ($Verbose -and (Test-Path $script:LogFile)) {
            Write-Host " Hint : Last 20 lines of output:" -ForegroundColor Red
            Get-Content $script:LogFile -Tail 20 | ForEach-Object {
                Write-Host "   $_" -ForegroundColor DarkRed
            }
        }

        Write-Host "========================================`n" -ForegroundColor Red
        exit $LASTEXITCODE
    }
}

Write-Host "Running Verification Pipeline..." -ForegroundColor Cyan

# Gate 1: Formatter Check
Invoke-Gate -GateName "Formatter Check" -Command "cargo fmt --all -- --check"

# Gate 2: Linter Check
Invoke-Gate -GateName "Linter Check" -Command "cargo clippy --workspace --all-targets --all-features -- -D warnings"

# Gate 3: Doc Compilation Check
Invoke-Gate -GateName "Doc Compilation Check" -Command "cargo test --doc --workspace"

# Gate 4: Unit & Integration Tests
Invoke-Gate -GateName "Unit & Integration Tests" -Command "cargo test --workspace"

# Gate 5: Polyfill Compilation Gate
$compatDir = Join-Path $PSScriptRoot ".." "compat"
if (Test-Path $compatDir) {
    $polyfillManifests = @(Get-ChildItem -Path $compatDir -Filter "Cargo.toml" -Recurse -Depth 1)
    foreach ($manifest in $polyfillManifests) {
        $crateName = (Split-Path -Parent $manifest.FullName | Split-Path -Leaf)
        Invoke-Gate -GateName "Polyfill Build: $crateName" -Command "cargo build --manifest-path `"$($manifest.FullName)`" --release"
    }
}

# Gate 6: AST-Grep Scan (Conditional)
Write-Host "`n>>> AST-Grep Scan" -ForegroundColor Yellow
$hasSg = [bool](Get-Command sg -ErrorAction SilentlyContinue)
$hasSgConfig = Test-Path (Join-Path $PSScriptRoot ".." "sgconfig.yml")

if (-not $hasSg) {
    Write-Host "[SKIP] AST-grep ('sg') CLI is not installed in PATH. Skipping AST-grep scan." -ForegroundColor DarkYellow
} elseif (-not $hasSgConfig) {
    Write-Host "[SKIP] sgconfig.yml not found in repository root. Skipping AST-grep scan." -ForegroundColor DarkYellow
} else {
    Invoke-Gate -GateName "AST-Grep Scan" -Command "sg scan"
}

# Gate 7: Forbidden Pattern Scanner (ripgrep)
Write-Host "`n>>> Forbidden Pattern Scanner" -ForegroundColor Yellow
if (-not (Get-Command rg -ErrorAction SilentlyContinue)) {
    Write-Host "[SKIP] ripgrep ('rg') CLI is not installed in PATH. Skipping forbidden pattern scan." -ForegroundColor DarkYellow
} else {
    $targetPath = Join-Path $PSScriptRoot ".." "opc-da-client" "src"
    if (-not (Test-Path $targetPath)) {
        Write-Host "[SKIP] Target path '$targetPath' does not exist." -ForegroundColor DarkYellow
    } else {
        $forbiddenMatches = rg --color=never -n -g "*.rs" "\b(println!|dbg!|todo!)" $targetPath 2>&1
        $rgExit = $LASTEXITCODE

        if ($rgExit -eq 0) {
            Write-Host "========================================" -ForegroundColor Red
            Write-Host " VERIFICATION FAILED" -ForegroundColor Red
            Write-Host "========================================" -ForegroundColor Red
            Write-Host " What : Forbidden Pattern Scanner" -ForegroundColor Red
            Write-Host " Where: opc-da-client/src/" -ForegroundColor Red
            Write-Host " Why  : Found forbidden macro(s) (println!, dbg!, todo!):" -ForegroundColor Red
            $forbiddenMatches | ForEach-Object { Write-Host "   $_" -ForegroundColor Red }
            Write-Host "========================================`n" -ForegroundColor Red
            exit 1
        } elseif ($rgExit -eq 1) {
            Write-Host "No forbidden patterns (println!, dbg!, todo!) found in opc-da-client/src/." -ForegroundColor Green
        } else {
            Write-Host "========================================" -ForegroundColor Red
            Write-Host " VERIFICATION FAILED" -ForegroundColor Red
            Write-Host "========================================" -ForegroundColor Red
            Write-Host " What : Forbidden Pattern Scanner" -ForegroundColor Red
            Write-Host " Where: rg execution on opc-da-client/src/" -ForegroundColor Red
            Write-Host " Why  : ripgrep exited with error code $rgExit: $forbiddenMatches" -ForegroundColor Red
            Write-Host "========================================`n" -ForegroundColor Red
            exit $rgExit
        }
    }
}

# Gate 8: PowerShell Script Syntax & Strict Mode Check
Write-Host "`n>>> PowerShell Script Syntax & Strict Mode Check" -ForegroundColor Yellow
$scriptDir = $PSScriptRoot
$scriptFiles = Get-ChildItem -Path $scriptDir -Filter "*.ps1" -File
$totalSyntaxErrors = 0
$syntaxErrorLog = @()

foreach ($file in $scriptFiles) {
    $tokens = $null
    $errors = $null
    $null = [System.Management.Automation.Language.Parser]::ParseFile($file.FullName, [ref]$tokens, [ref]$errors)

    if ($errors.Count -gt 0) {
        $totalSyntaxErrors += $errors.Count
        foreach ($err in $errors) {
            $syntaxErrorLog += "   $($file.Name):$($err.Extent.StartLineNumber) - $($err.Message)"
        }
    }
}

if ($totalSyntaxErrors -gt 0) {
    Write-Host "========================================" -ForegroundColor Red
    Write-Host " VERIFICATION FAILED" -ForegroundColor Red
    Write-Host "========================================" -ForegroundColor Red
    Write-Host " What : PowerShell Script Syntax Check" -ForegroundColor Red
    Write-Host " Where: scripts/*.ps1 ($($scriptFiles.Count) scripts checked)" -ForegroundColor Red
    Write-Host " Why  : Found $totalSyntaxErrors AST syntax error(s):" -ForegroundColor Red
    $syntaxErrorLog | ForEach-Object { Write-Host $_ -ForegroundColor Red }
    Write-Host "========================================`n" -ForegroundColor Red
    exit 1
} else {
    Write-Host "All $($scriptFiles.Count) PowerShell scripts passed AST syntax validation." -ForegroundColor Green
}

# Cleanup temp log
if (Test-Path $script:LogFile) { Remove-Item $script:LogFile -ErrorAction SilentlyContinue }

Write-Host "`nAll Gates Passed! ✅" -ForegroundColor Green
exit 0
```
