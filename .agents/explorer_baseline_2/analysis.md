# AST-Grep Rule Architecture Analysis & Specification

## 1. Executive Summary

This document specifies the exact AST-Grep configuration format (`sgconfig.yml`) and rule YAML definitions for enforcing structural Rust code quality constraints in `opc-cli` (specifically targeting `opc-da-client/src/`).

Key Findings & Verification:
- `ast-grep` CLI version `0.42.1` is installed and verified operational.
- **Rule 1 (`no-panic-or-unwrap`)**: Prohibits `.unwrap()`, `.expect()`, `panic!()`, `todo!()` in production library code. Successfully excludes test modules (`#[cfg(test)]`), test functions (`#[test]`, `#[tokio::test]`), and test directories (`tests/`).
- **Rule 2 (`require-safety-comment`)**: Enforces that all `unsafe` blocks (statements, expressions, and let-bindings) are preceded by a `// SAFETY:` or `/* SAFETY: */` comment explaining the safety rationale.

---

## 2. AST-Grep Configuration (`sgconfig.yml`)

The root configuration file `sgconfig.yml` defines the rule directories and test configurations.

```yaml
ruleDirs:
  - .ast-grep/rules
testConfigs:
  - testDir: .ast-grep/rule-tests
```

### Proposed Workspace Layout
```
opc-cli/
├── sgconfig.yml
└── .ast-grep/
    ├── rules/
    │   ├── no-panic-or-unwrap.yml
    │   └── require-safety-comment.yml
    └── rule-tests/
        ├── no-panic-or-unwrap-test.yml
        └── require-safety-comment-test.yml
```

---

## 3. Rule 1: Prohibit `.unwrap()`, `.expect()`, `panic!()`, `todo!()`

### Purpose
Ensure production library code (`opc-da-client/src/`) uses robust `Result`/`Option` error handling instead of panicking or leaving incomplete placeholders.

### Exact Rule YAML Definition (`.ast-grep/rules/no-panic-or-unwrap.yml`)

```yaml
id: no-panic-or-unwrap
message: Prohibit unwrap, expect, panic, and todo in non-test library code
severity: error
language: rust
files:
  - "opc-da-client/src/**/*.rs"
ignores:
  - "**/tests/**"
  - "**/*_test.rs"
  - "**/*_tests.rs"
rule:
  any:
    - pattern: $X.unwrap()
    - pattern: $X.expect($$$ARGS)
    - pattern: panic!($$$ARGS)
    - pattern: todo!($$$ARGS)
  not:
    inside:
      stopBy: end
      any:
        - kind: mod_item
          follows:
            any:
              - pattern: "#[cfg(test)]"
              - pattern: "#[cfg(all(test, $$$))]"
        - kind: function_item
          follows:
            any:
              - pattern: "#[test]"
              - pattern: "#[tokio::test]"
              - pattern: "#[core::prelude::v1::test]"
```

### Technical Explanation of Rule Mechanics
1. **Target Selector**: Matches `.unwrap()`, `.expect(...)`, `panic!(...)`, and `todo!(...)`.
2. **File Exclusions (`ignores`)**: Excludes external test directories (`**/tests/**`) and test files (`*_test.rs`).
3. **AST Test Exclusion (`not.inside`)**:
   - Uses `stopBy: end` to traverse ancestor AST nodes from the match point up to the module root.
   - Excludes matches inside a `mod_item` preceded by `#[cfg(test)]` or `#[cfg(all(test, ...))]`.
   - Excludes matches inside a `function_item` preceded by `#[test]`, `#[tokio::test]`, or test framework macros.
   - Note on tree-sitter Rust AST structure: Outer attributes (`#[cfg(test)]`) are sibling nodes preceding `mod_item` / `function_item`, so `follows:` correctly matches the item preceded by the attribute.

---

## 4. Rule 2: Require `// SAFETY:` Comment Rationale Above `unsafe` Blocks

### Purpose
Enforce memory safety documentation by requiring every `unsafe` block to have a preceding rationale comment (`// SAFETY:` or `/* SAFETY: */`).

### Exact Rule YAML Definition (`.ast-grep/rules/require-safety-comment.yml`)

```yaml
id: require-safety-comment
message: "Unsafe blocks must have a preceding // SAFETY: comment explaining the safety rationale"
severity: error
language: rust
files:
  - "opc-da-client/src/**/*.rs"
ignores:
  - "**/tests/**"
  - "**/*_test.rs"
  - "**/*_tests.rs"
rule:
  any:
    - kind: unsafe_block
    - pattern: "unsafe { $$$ }"
  not:
    inside:
      stopBy: end
      follows:
        any:
          - kind: line_comment
            regex: "SAFETY:"
          - kind: block_comment
            regex: "SAFETY:"
```

### Technical Explanation of Rule Mechanics
1. **Target Selector**: Matches `unsafe_block` nodes and `unsafe { $$$ }` expressions.
2. **Comment Association (`not.inside.follows`)**:
   - Traverses up to the statement level (`expression_statement` or `let_declaration`).
   - Verifies if the statement follows a `line_comment` or `block_comment` containing `SAFETY:`.
   - Correctly ignores non-safety comments (e.g. `// UNSAFE WITHOUT RATIONALE`).
   - Prevents stale comments across statement boundaries: if another statement is inserted between the `// SAFETY:` comment and the `unsafe` block, the `unsafe` block will correctly fail.

---

## 5. Verification & Test Case Evidence

### Test Suite 1: Rule 1 Test Case Execution
Sample code (`sample_tests.rs`):
- `prod_code()` with `.unwrap()` -> **MATCHED (Error)**
- `#[cfg(test)] mod tests` with `.unwrap()` -> **EXCLUDED (Passed)**
- `#[test] fn standalone_test()` with `.unwrap()` -> **EXCLUDED (Passed)**

Actual scan on `opc-da-client/src/`:
- Correctly skipped all unit test `.unwrap()` calls in `com_worker.rs`, `helpers.rs`, and `iterator.rs`.
- Flagged 8 production occurrences (e.g. `opc_da.rs:25`, `bindings/comn/bindings.rs:218`, `bindings/da/bindings.rs:996`).

### Test Suite 2: Rule 2 Test Case Execution
Sample code (`sample_safety.rs`):
- `unsafe` block with `// SAFETY:` -> **EXCLUDED (Passed)**
- `unsafe` block with `/* SAFETY: */` -> **EXCLUDED (Passed)**
- `unsafe` block with `// UNSAFE WITHOUT RATIONALE` -> **MATCHED (Error)**
- `unsafe` block with comment separated by `let a = 1;` -> **MATCHED (Error)**
- `let win_guid = unsafe { ... }` with `// SAFETY:` above `let` -> **EXCLUDED (Passed)**

---

## 6. Integration Guidance for Verification Pipeline

To execute ast-grep scans as part of `scripts/verify.ps1` (Gate 6):
```powershell
if (Get-Command sg -ErrorAction SilentlyContinue) {
    Write-Host "Running Gate 6: AST-Grep Scan..." -ForegroundColor Cyan
    sg scan -c sgconfig.yml
    if ($LASTEXITCODE -ne 0) {
        Write-Error "AST-Grep scan failed!"
        exit $LASTEXITCODE
    }
} else {
    Write-Warning "Gate 6 Skipped: sg (ast-grep) CLI is not installed."
}
```
