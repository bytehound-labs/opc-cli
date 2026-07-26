## 2026-07-26T07:42:39Z
You are an Explorer agent for opc-cli verification pipeline hardening (Milestone 1: AST-Grep Rule Architecture).
Your working directory is: c:\Users\WSALIGAN\code\opc-cli\.agents\explorer_baseline_2

Your task:
1. Research ast-grep (`sg`) configuration file format (`sgconfig.yml`) and rule YAML format in `.ast-grep/rules/`.
2. Test `sg` CLI commands if `sg` is available in environment.
3. Design exact rules for:
   - Rule 1: Prohibit `.unwrap()`, `.expect()`, `panic!()`, `todo!()` in non-test library code (`opc-da-client/src/`). Make sure rules properly exclude test files (`tests/` or `#[cfg(test)]`) if applicable, or target production library code.
   - Rule 2: Require `// SAFETY:` rationale comments above `unsafe` blocks.
4. Verify YAML syntax and test rule matching behavior on sample snippets or existing codebase files.
5. Document exact rule definitions and configuration structure in `c:\Users\WSALIGAN\code\opc-cli\.agents\explorer_baseline_2\analysis.md` and send a handoff message.
