## 2026-07-26T07:53:54Z
You are Worker 2 (Pipeline & AST Rule Remediation Worker).
Your working directory is: c:\Users\WSALIGAN\code\opc-cli\.agents\worker_impl_2

DO NOT CHEAT. All implementations must be genuine. DO NOT hardcode test results, create dummy/facade implementations, or circumvent the intended task. A Forensic Auditor will independently verify your work. Integrity violations WILL be detected and your work WILL be rejected.

Remediation Tasks (Fixing Reviewer 1 & 2 Findings):

1. **Remove `**/opc_da/**` from AST-grep Rule Ignores**:
   - In `.ast-grep/rules/no-panic-or-unwrap.yml` and `.ast-grep/rules/require-safety-comment.yml`, remove `**/opc_da/**` from `ignores:`.
   - `opc-da-client/src/opc_da/` is production library code (81% of `opc-da-client/src/`). It MUST be scanned.
   - Inspect all `unsafe` blocks in `opc-da-client/src/opc_da/` (e.g. `opc_da/client/traits/*.rs`, `browse.rs`, `client.rs`, `iterator.rs`, etc.) and add explicit `// SAFETY:` rationale comments directly above every `unsafe` block.
   - Verify `sg scan` passes cleanly with 0 errors across `opc-da-client/src/opc_da/`.

2. **Create AST-grep Rule Tests Directory (`.ast-grep/rule-tests/`)**:
   - Create `.ast-grep/rule-tests/` directory.
   - Add rule test suites: `.ast-grep/rule-tests/no-panic-or-unwrap-test.yml` and `.ast-grep/rule-tests/require-safety-comment-test.yml`.
   - Run `sg test` using run_command and verify `sg test` passes with exit code `0`.

3. **Refactor Rule AST Matching for Multi-Attribute Tests**:
   - In `no-panic-or-unwrap.yml`, update test exclusion matching logic so functions with multiple attributes (e.g., `#[test]` + `#[should_panic]` or `#[tokio::test]`) are properly recognized as test functions and not falsely flagged.
   - In `require-safety-comment.yml`, ensure safety comment detection handles attributes and formatting cleanly.

4. **Expand AST-grep Scope**:
   - Update `files:` in `.ast-grep/rules/*.yml` to include `"opc-cli/src/**/*.rs"` in addition to `"opc-da-client/src/**/*.rs"`.

5. **Documentation Gate Count Sync**:
   - Update `architecture.md` §7 (line 67) to say "8-gate quality pipeline runner" (currently reads "5-gate").
   - Update `coding_standard.md` in repository root to reflect the 8-gate verification pipeline.

6. **Full Pipeline Verification**:
   - Run `sg test`
   - Run `sg scan`
   - Run `pwsh -File scripts/verify.ps1`
   - All MUST exit cleanly with code 0.
   - Document all changes and verification outputs in `c:\Users\WSALIGAN\code\opc-cli\.agents\worker_impl_2\handoff.md` and send a message when done.
