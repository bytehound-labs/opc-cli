# Agent contract

## Repository and platform

- `opc-da-client/` contains the product library, `bytehound-opc-da-client`.
- `opc-cli/` is a secondary, Windows-only TUI consumer. The full workspace requires
  Windows; Linux can check the library only.
- `scripts/` contains verification and packaging tools. CI is in
  `.github/workflows/ci.yml`.

## Validation

- On Windows, run `pwsh -File scripts/verify.ps1` or `make verify`.
- CI requires **Required validation status**. It runs formatting, Clippy with
  `-D warnings`, locked workspace/doc tests, Rust 1.88.0 MSRV checks, cargo-deny,
  ast-grep rule tests/scans, and a 32-bit Windows-target Clippy check.
- Linux CI runs cargo-deny, ast-grep, and locked library-only Clippy/tests. It does
  not validate the TUI or full workspace.

## COM and safety

- Follow the existing COM/DCOM ownership, apartment, and thread-affinity conventions.
- Every `unsafe` block needs a `// SAFETY:` comment stating the real invariant for
  that operation. Existing enforcement is in workspace Clippy and ast-grep; do not
  add a new lint solely for this rule.

## Contribution flow and worktree boundaries

- Use short-lived PR branches targeting `main`, then squash merge. Do not use the
  legacy `make release-merge` or `scripts/Merge-ToMain.ps1` dev-to-main flow.
- Do not retire `origin/dev` without Mike's direction.
- Do not enter or modify these other active worktrees:
  `opccli-unsafe`, `opccli-publish`, `opc-cli-browseto-canary`,
  `opc-cli-da2-canary`, and `opc-cli-throughput`. The publish worktree owns
  `.github/workflows/**`.
- While the canary worktrees are active, do not edit:
  `Cargo.lock`, `.gitignore`, `opc-da-client/architecture.md`,
  `opc-da-client/Cargo.toml`, `opc-da-client/CHANGELOG.md`,
  `opc-da-client/examples/inventory-root.rs`, `opc-da-client/README.md`,
  `opc-da-client/spec.md`, `opc-da-client/src/backend/connector.rs`,
  `opc-da-client/src/backend/opc_da.rs`, `opc-da-client/src/com_worker.rs`,
  `opc-da-client/src/helpers.rs`, `opc-da-client/src/inventory.rs`,
  `opc-da-client/src/lib.rs`, `opc-da-client/src/native_browse.rs`,
  `opc-da-client/src/opc_da/client/iterator.rs`,
  `opc-da-client/src/opc_da/errors.rs`, or `opc-da-client/src/provider.rs`.
