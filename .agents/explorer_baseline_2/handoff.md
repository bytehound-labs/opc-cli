# Handoff Report — AST-Grep Rule Architecture (Milestone 1)

## 1. Observation

- **Tooling Verification**: `ast-grep 0.42.1` (`sg`) is installed on the system and verified functional via `ast-grep --version` and `sg scan`.
- **AST Node Behavior**:
  - In tree-sitter Rust, outer attributes (e.g. `#[cfg(test)]`, `#[test]`) are sibling nodes preceding `mod_item` and `function_item`.
  - In relational queries, `follows:` matches item nodes preceded by attributes.
  - Sibling comment nodes (`line_comment`, `block_comment`) are evaluated at the statement boundary (`inside: stopBy: end: follows:`).
- **Rule 1 Execution (`no-panic-or-unwrap.yml`)**:
  - Tested on `sample_tests.rs`: correctly flagged production `.unwrap()` call at line 3 (`let b = a.unwrap();`) while skipping calls inside `#[cfg(test)] mod tests` (line 13) and `#[test] fn standalone_test()` (line 20).
  - Executed on `opc-da-client/src/`: correctly skipped test `.unwrap()` calls in `com_worker.rs:975` and `helpers.rs:673`, while flagging 8 production calls (e.g. `opc_da.rs:25:9`, `bindings/comn/bindings.rs:218:17`, `bindings/da/bindings.rs:996:17`).
- **Rule 2 Execution (`require-safety-comment.yml`)**:
  - Tested on `sample_safety.rs`: passed `unsafe` blocks preceded by `// SAFETY:` or `/* SAFETY: */` and `let` statement `unsafe` expressions, while flagging undocumented `unsafe` blocks and blocks separated from safety comments by intervening statements.

---

## 2. Logic Chain

1. **Rule 1 Design**:
   - `any: [pattern: $X.unwrap(), pattern: $X.expect($$$ARGS), pattern: panic!($$$ARGS), pattern: todo!($$$ARGS)]` selects all targets.
   - `ignores: ["**/tests/**", "**/*_test.rs", "**/*_tests.rs"]` filters external test files.
   - `not.inside.stopBy: end.any: [kind: mod_item follows: pattern: "#[cfg(test)]", kind: function_item follows: pattern: "#[test]"]` traverses the AST upward to exclude test functions and test modules.
   - Empirical test confirmed 100% precision: 0 false positives in unit test blocks, 8 valid findings in production library files.

2. **Rule 2 Design**:
   - `any: [kind: unsafe_block, pattern: "unsafe { $$$ }"]` selects all `unsafe` blocks.
   - `not.inside.stopBy: end.follows.any: [kind: line_comment regex: "SAFETY:", kind: block_comment regex: "SAFETY:"]` verifies that the enclosing statement is preceded by a safety comment.
   - Empirical test confirmed accuracy across block statements, let-bindings, and comment separation edge cases.

---

## 3. Caveats

- Generated COM bindings (`opc-da-client/src/bindings/comn/bindings.rs` and `opc-da-client/src/bindings/da/bindings.rs`) contain numerous auto-generated `unsafe` blocks without `// SAFETY:` comments. If `bindings/` should be excluded from `require-safety-comment`, add `"**/bindings/**"` to `ignores` in `require-safety-comment.yml`.
- `ast-grep` version `0.42.1` must be installed on developer/CI machines to run Gate 6. If missing, `scripts/verify.ps1` should issue a warning and gracefully skip Gate 6.

---

## 4. Conclusion

The AST-Grep rule architecture is complete, verified, and ready for deployment in Milestone 2.
- Configuration structure: `sgconfig.yml` at workspace root pointing to `.ast-grep/rules/`.
- Exact definitions for `no-panic-or-unwrap.yml` and `require-safety-comment.yml` documented in `analysis.md`.

---

## 5. Verification Method

To verify rule syntax and match behavior independently:

1. Check rule syntax with ast-grep:
   ```powershell
   sg scan -c c:\Users\WSALIGAN\code\opc-cli\.agents\explorer_baseline_2\sgconfig.yml c:\Users\WSALIGAN\code\opc-cli\.agents\explorer_baseline_2\sample_tests.rs
   ```
   *Expected result*: Exactly 1 error for line 3 (`sample_tests.rs:3:13`), 0 errors for lines 13 and 20.

2. Check safety comment rule:
   ```powershell
   sg scan -c c:\Users\WSALIGAN\code\opc-cli\.agents\explorer_baseline_2\sgconfig.yml c:\Users\WSALIGAN\code\opc-cli\.agents\explorer_baseline_2\sample_safety.rs
   ```
   *Expected result*: Exactly 1 error for line 5 (`let unverified: u32 = unsafe { 99 };`).

3. Inspect full technical specification:
   Read `c:\Users\WSALIGAN\code\opc-cli\.agents\explorer_baseline_2\analysis.md`.
