## 2026-07-26T07:46:47Z
You are the Implementation Worker for opc-cli verification pipeline hardening (Milestone 2, 3, 4).
Your working directory is: c:\Users\WSALIGAN\code\opc-cli\.agents\worker_impl_1

DO NOT CHEAT. All implementations must be genuine. DO NOT hardcode test results, create dummy/facade implementations, or circumvent the intended task. A Forensic Auditor will independently verify your work. Integrity violations WILL be detected and your work WILL be rejected.

Your Tasks:

1. **Fix Line Endings / Formatter**:
   Run `cargo fmt --all` to fix formatting / newline style in:
   - `opc-da-client/src/bindings/comn/mod.rs`
   - `opc-da-client/src/bindings/da/mod.rs`
   - `opc-da-client/src/com_worker.rs`
   - `opc-da-client/src/opc_da/client/mod.rs`
   Confirm `cargo fmt --all -- --check` passes cleanly.

2. **R1: AST-Grep (`sg`) Configuration & Rule Set**:
   - Create `sgconfig.yml` in workspace root:
     ```yaml
     ruleDirs:
       - .ast-grep/rules
     testConfigs:
       - testDir: .ast-grep/rule-tests
     ```
   - Create `.ast-grep/rules/no-panic-or-unwrap.yml` per Explorer 2 analysis (`c:\Users\WSALIGAN\code\opc-cli\.agents\explorer_baseline_2\analysis.md`).
     - Check `opc-da-client/src/backend/opc_da.rs` line 25 (`OpcDaClient::default()`) which calls `Self::new(ComConnector).expect(...)`. Refactor it safely (e.g. `match Self::new(ComConnector) { Ok(c) => c, Err(_) => ... }` or `unwrap_or_else`) so no `.expect()` remains in non-test library code.
     - Ensure generated FFI bindings or test files are ignored or refactored as appropriate.
   - Create `.ast-grep/rules/require-safety-comment.yml` per Explorer 2 analysis.
   - Test with `sg scan` command to confirm `sg scan` loads `sgconfig.yml` and reports 0 violations on current codebase.

3. **R2: Verification Pipeline Integration (`scripts/verify.ps1`)**:
   - Extend `scripts/verify.ps1` with Gates 6, 7, 8 per Explorer 3 blueprint (`c:\Users\WSALIGAN\code\opc-cli\.agents\explorer_baseline_3\analysis.md`):
     - Gate 6: `sg scan` (AST-grep scan if `sg` CLI is installed and `sgconfig.yml` exists, display skip message if not).
     - Gate 7: Forbidden pattern scanner (`rg` scan for raw `println!`, `dbg!`, `todo!` in `opc-da-client/src/`).
     - Gate 8: PowerShell script syntax & strict mode check (`[System.Management.Automation.Language.Parser]::ParseFile` on `scripts/*.ps1`).

4. **R3: Quality Gate Compliance & Documentation Sync**:
   - Run `pwsh -File scripts/verify.ps1` and verify all 8 gates pass with exit code `0`.
   - Update `architecture.md § Toolchain` to document all active verification gates (Gates 1-8).
   - Update `.agents/rules/coding-standard.md §2` to match the updated `verify.ps1` 8-gate sequence.

5. **Handoff & Verification**:
   - Document commands executed, output logs, and gate verification results in `c:\Users\WSALIGAN\code\opc-cli\.agents\worker_impl_1\handoff.md` and send a message back.
