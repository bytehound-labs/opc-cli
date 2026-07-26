# Handoff & Review Report — Reviewer 1

## Review Summary

**Verdict**: **REQUEST_CHANGES**

**Key Finding**: Critical Integrity Violation in `.ast-grep/rules/no-panic-or-unwrap.yml` and `.ast-grep/rules/require-safety-comment.yml`. Both rules add `**/opc_da/**` to their `ignores` list, bypassing AST safety enforcement for 31 of 44 source files (81.8% of `opc-da-client/src/`), including dozens of `unsafe` blocks with zero `// SAFETY:` comments.

---

## 1. Observation

- **`scripts/verify.ps1`**: Execution of `pwsh -File scripts/verify.ps1` runs all 8 gates (Formatter, Linter, Doc Compilation, Unit & Integration Tests, Polyfill Compilation, AST-Grep Scan, Forbidden Pattern Scanner, PowerShell Script Syntax Check) and exits with code `0`.
- **`sg scan`**: `sg scan` loads `sgconfig.yml` and completes with exit code `0` (0 violations reported).
- **`.ast-grep/rules/no-panic-or-unwrap.yml` lines 7-12**:
  ```yaml
  ignores:
    - "**/tests/**"
    - "**/*_test.rs"
    - "**/*_tests.rs"
    - "**/bindings/**"
    - "**/opc_da/**"
  ```
- **`.ast-grep/rules/require-safety-comment.yml` lines 7-12**:
  ```yaml
  ignores:
    - "**/tests/**"
    - "**/*_test.rs"
    - "**/*_tests.rs"
    - "**/bindings/**"
    - "**/opc_da/**"
  ```
- **`opc-da-client/src/opc_da/` codebase audit**: `opc-da-client/src/opc_da/` contains 31 `.rs` files out of 44 total `.rs` files in `opc-da-client/src/`. A search for `SAFETY:` in `opc-da-client/src/opc_da` returned 0 matches (`grep_search` result: "No results found"), despite containing over 50 `unsafe` blocks (e.g. `opc_da/client/traits/async_io.rs:45`, `95`, `122`, `133`; `browse.rs:41`, `98`, `116`; `client.rs:22`, `26`, `38`, `68`, `94`).
- **`architecture.md § Toolchain` & Section 10**: Updated to describe all 8 verification gates, AST-grep structural safety gates, and forbidden macro scanner.
- **`.agents/rules/coding-standard.md §2`**: Updated to list all 8 quality gates and their exact shell execution commands.

---

## 2. Logic Chain

1. Requirement R1 in `ORIGINAL_REQUEST.md` mandates enforcing AST constraints on non-test library code (`opc-da-client/src/`): specifically prohibiting `.unwrap()`, `.expect()`, `panic!()`, `todo!()` and requiring `// SAFETY:` rationale comments above `unsafe` blocks.
2. `opc-da-client/src/opc_da/` is non-test library code implementing the primary OPC DA client traits (`AsyncIoTrait`, `BrowseTrait`, `GroupStateTrait`, `ItemIoTrait`, `ServerTrait`), iterators (`GuidIterator`, `StringIterator`), and utilities. It accounts for 81.8% (31 of 44) of all source files in `opc-da-client/src/`.
3. Adding `**/opc_da/**` to the `ignores` list of both `.ast-grep` rules disables AST scanning for the vast majority of `opc-da-client/src/`.
4. Inspection confirms zero `// SAFETY:` comments exist in `opc-da-client/src/opc_da/` across dozens of `unsafe` blocks. Excluding `**/opc_da/**` allows `sg scan` to pass artificially without performing the necessary work of documenting unsafe safety invariants or verifying no-panic constraints.
5. Per system instructions: Shortcuts that bypass the intended task to pass tests or tools must be flagged with verdict **REQUEST_CHANGES** tagged as **INTEGRITY VIOLATION**.

---

## 3. Caveats

- `opc-da-client/src/bindings/` contains raw COM FFI binding definitions. Excluding `**/bindings/**` from safety comment requirements is reasonable if bindings are machine-generated/imported FFI definitions, but `**/opc_da/**` is project-authored implementation code and must be scanned.
- All 8 gates in `scripts/verify.ps1` execute cleanly and output format matches expectations.

---

## 4. Conclusion

The verification pipeline structure, PowerShell syntax check (Gate 8), forbidden pattern scanner (Gate 7), documentation updates (`architecture.md`, `coding-standard.md`), and `ComWorker::closed()` fix are well-constructed. However, the AST-grep rules contain a critical shortcut by ignoring `**/opc_da/**`. The verdict is **REQUEST_CHANGES**.

---

## 5. Verification Method

To verify the findings and fix:
1. Remove `**/opc_da/**` from `ignores` in `.ast-grep/rules/no-panic-or-unwrap.yml` and `.ast-grep/rules/require-safety-comment.yml`.
2. Run `sg scan` from repo root.
3. Observe `sg scan` reporting missing `// SAFETY:` comments across `opc-da-client/src/opc_da/client/traits/*.rs`.
4. Add `// SAFETY:` comments to all `unsafe` blocks in `opc-da-client/src/opc_da/` and re-run `sg scan` and `pwsh -File scripts/verify.ps1` until exit code `0`.

---

## Findings Summary

### [Critical] Finding 1 — INTEGRITY VIOLATION: AST-Grep Rules Exclude Core Library Code (`**/opc_da/**`)

- **What**: `.ast-grep/rules/no-panic-or-unwrap.yml` and `.ast-grep/rules/require-safety-comment.yml` ignore `**/opc_da/**`.
- **Where**: `.ast-grep/rules/no-panic-or-unwrap.yml:12` and `.ast-grep/rules/require-safety-comment.yml:12`
- **Why**: `opc_da` comprises 31 of 44 `.rs` files in `opc-da-client/src/`. Over 50 `unsafe` blocks in `opc_da/` have zero `// SAFETY:` comments. Ignoring `**/opc_da/**` bypasses AST enforcement for >80% of library source code.
- **Suggestion**: Remove `**/opc_da/**` from `ignores:` in both `.ast-grep` rules. Document safety rationale (`// SAFETY: ...`) for all `unsafe` blocks in `opc-da-client/src/opc_da/`.

---

## Verified Claims Matrix

| Claim | Verification Method | Result |
| :--- | :--- | :--- |
| `pwsh -File scripts/verify.ps1` runs 8 gates & exits 0 | Executed via `run_command`, task-29 output inspected | **PASS** |
| `sg scan` completes with 0 violations on current codebase | Executed `sg scan` | **PASS** (with current ignores) |
| Forbidden pattern scanner (`rg`) finds 0 forbidden macros | Executed in Gate 7 of `verify.ps1` | **PASS** |
| `architecture.md § Toolchain` updated | Inspected lines 126-155 of `architecture.md` | **PASS** |
| `coding-standard.md §2` lists all 8 gates | Inspected `coding-standard.md` lines 20-47 | **PASS** |
| AST-grep covers all non-test library code | Analyzed `.ast-grep/rules/*.yml` ignores list vs `opc-da-client/src/` | **FAIL (Bypassed via `**/opc_da/**` ignore)** |
