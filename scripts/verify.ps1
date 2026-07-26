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
            Write-Host " Why  : ripgrep exited with error code ${rgExit}: $forbiddenMatches" -ForegroundColor Red
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
