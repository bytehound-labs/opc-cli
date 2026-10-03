# Agent contract

## Repository and platform

- `opc-da-client/` contains the product library, `bytehound-opc-da-client`.
- `opc-cli/` is a secondary, Windows-only TUI consumer. The full workspace requires
  Windows; Linux compiles and tests the actual portable provider/model layer.
  Native COM connections and workers remain Windows-only.
- `scripts/` contains verification and packaging tools. CI is in
  `.github/workflows/ci.yml`.
- Library architecture and behavioral references are packaged under
  `opc-da-client/docs/`. Lifecycle roots delegate to private domain modules;
  boundary/error/telemetry primitives have off-Windows tests.

## Validation

- On Windows, run `pwsh -File scripts/verify.ps1` or `make verify`.
- CI requires **Required validation status**. It runs formatting, Clippy with
  `-D warnings`, locked workspace/doc tests, Rust 1.88.0 MSRV checks, cargo-deny,
  unsuppressed ast-grep/debug scans, model-only checks, package verification,
  and a 32-bit Windows-target Clippy check.
- Linux CI runs cargo-deny, ast-grep, and locked library-only Clippy/tests. It does
  not validate the TUI or full workspace.

## COM and safety

- Preserve the MTA worker, guard-before-resource declaration/drop ordering,
  isolated inventory/session connections, native ownership, cancellation,
  pacing, exact ItemIDs, opaque token identity, and tracing targets.
- Every `unsafe` block needs a `// SAFETY:` comment stating the real invariant for
  that operation. Existing enforcement is in workspace Clippy and ast-grep; do not
  add a new lint solely for this rule.

## Contribution flow and worktree boundaries

- Use short-lived PR branches targeting `main`, then squash merge. Do not use the
  legacy `make release-merge` or `scripts/Merge-ToMain.ps1` dev-to-main flow.
- Do not retire `origin/dev` without Mike's direction.
- Do not enter or modify these other active worktrees:
  `opccli-unsafe`, `opccli-publish`, `opc-cli-browseto-canary`,
  `opc-cli-da2-canary`, and `opc-cli-throughput`.
- Preserve user canary branches, dirty files, indexes, and worktrees byte-for-byte.
  Never rebase, stash, clean, or delete them. Approved reconciled refactors use
  their own feature worktree; do not take over another task's active ownership.
- Coordinate workflow changes with any active publishing task. Real publication
  and live OPC/controller operations require separate explicit authorization.
