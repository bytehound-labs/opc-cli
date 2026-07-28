# Master Plan: opc-cli Verification Pipeline Hardening

## Overview
Hardening the verification pipeline for `opc-cli` by introducing AST-grep (`sg`) rules, forbidden pattern scanners (`rg`), PowerShell script quality checks, and updating pipeline runner and documentation.

## Milestones

### Milestone 1: Explorer Baseline Analysis
- **Goal**: Check current state of tools (`sg`, `rg`, `pwsh`), inspect existing codebase for any potential violations of rules before creating them, verify existing build/test pipeline status.
- **Workers**: `teamwork_preview_explorer` (x3 parallel)

### Milestone 2: R1 - AST-Grep (`sg`) Configuration & Rule Set
- **Goal**: Create `sgconfig.yml` and `.ast-grep/rules/` in workspace root.
  - Rule 1: Prohibit `.unwrap()`, `.expect()`, `panic!()`, `todo!()` in non-test library code (`opc-da-client/src/`).
  - Rule 2: Require `// SAFETY:` rationale comments above `unsafe` blocks across codebase/library code.
- **Workers**: `teamwork_preview_worker` -> `teamwork_preview_reviewer` -> `teamwork_preview_auditor`

### Milestone 3: R2 - Verification Pipeline Integration (`scripts/verify.ps1`)
- **Goal**: Extend `scripts/verify.ps1` with new automated gates:
  - Gate 6: `sg scan` (AST-grep scan if `sg` CLI is available, fallback gracefully if not installed).
  - Gate 7: Forbidden pattern scanner (`rg` scan for raw `println!`, `dbg!`, `todo!` in production library code).
  - Gate 8: PowerShell script syntax & strict mode check.
- **Workers**: `teamwork_preview_worker` -> `teamwork_preview_reviewer` -> `teamwork_preview_auditor`

### Milestone 4: R3 - Quality Gate Compliance & Documentation Sync
- **Goal**:
  - Run full `scripts/verify.ps1` to ensure exit code 0.
  - Update `architecture.md § Toolchain` to document all active verification gates (Gates 1-8).
  - Update `.agents/rules/coding-standard.md §2` to match the updated `verify.ps1` gate sequence.
- **Workers**: `teamwork_preview_worker` -> `teamwork_preview_reviewer` -> `teamwork_preview_auditor`

## Verification Gates
1. Formatter Check (`cargo fmt`)
2. Linter Check (`cargo clippy`)
3. Doc Compilation Check (`cargo test --doc`)
4. Unit & Integration Tests (`cargo test`)
5. Polyfill Build Gate (`cargo build --release` in `compat/*`)
6. AST-Grep Scan (`sg scan`)
7. Forbidden Pattern Scan (`rg` scan)
8. PowerShell Script Syntax & Strict Mode Check
