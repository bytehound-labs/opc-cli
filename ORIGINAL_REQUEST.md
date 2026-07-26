# Original User Request

## Initial Request — 2026-07-26T15:41:47+08:00

Enhance and harden the mechanical verification pipeline for `opc-cli` by adding `ast-grep` (`sg`) AST rules, forbidden pattern scanners (`rg`), and script quality gates.

Working directory: `c:\Users\WSALIGAN\code\opc-cli`
Integrity mode: development

## Requirements

### R1. AST-Grep (`sg`) Configuration & Rule Set
Create `sgconfig.yml` and `.ast-grep/rules/` in the workspace root to enforce structural AST constraints:
- Prohibit `.unwrap()`, `.expect()`, `panic!()`, `todo!()` in non-test library code (`opc-da-client/src/`).
- Require `// SAFETY:` rationale comments above `unsafe` blocks.

### R2. Verification Pipeline Integration (`scripts/verify.ps1`)
Extend `scripts/verify.ps1` with new automated gates:
- Gate 6: `sg scan` (AST-grep scan if `sg` CLI is available, fallback gracefully if not installed).
- Gate 7: Forbidden pattern scanner (`rg` scan for raw `println!`, `dbg!`, `todo!` in production library code).
- Gate 8: PowerShell script syntax & strict mode check.

### R3. Quality Gate Compliance & Documentation Sync
- Ensure all new gates pass cleanly on the existing codebase (`pwsh -File scripts/verify.ps1`).
- Document the updated quality gates in `architecture.md § Toolchain` and `.agents/rules/coding-standard.md §2`.

## Acceptance Criteria

### Verification Gate Compliance
- [ ] `pwsh -File scripts/verify.ps1` runs all gates and exits with code `0`.
- [ ] `sg scan` successfully loads `sgconfig.yml` and reports 0 violations on current codebase.
- [ ] Forbidden pattern scan passes with 0 violations across production modules.

### Documentation & Tooling Integration
- [ ] `architecture.md § Toolchain` lists all active verification gates including AST-grep and pattern checks.
- [ ] `.agents/rules/coding-standard.md §2` matches the updated `verify.ps1` gate sequence.
