# Contributing

## Project and platforms

The primary product is the `bytehound-opc-da-client` library in `opc-da-client/`.
The `opc-cli/` Windows TUI is a secondary consumer of that library. The TUI and
full workspace require Windows; Linux checks can compile, lint, and test the
library only.

## Development checks

On Windows, run the full local quality gate from the repository root:

```powershell
pwsh -File scripts/verify.ps1
```

`make verify` runs the same script. It checks formatting, Clippy with warnings
denied, documentation tests, workspace tests, available ast-grep rules, forbidden
debug macros, and PowerShell script syntax.

Linux supports the library checks, not the Windows-only TUI:

```sh
cargo fmt --all -- --check
cargo clippy --locked -p bytehound-opc-da-client --all-targets --all-features -- -D warnings
cargo test --locked -p bytehound-opc-da-client --all-features
```

CI is defined in [`.github/workflows/ci.yml`](.github/workflows/ci.yml). Its Windows
job checks formatting, workspace Clippy with `-D warnings`, locked documentation and
workspace tests, 32-bit Windows-target Clippy, and PowerShell syntax. The MSRV job
checks Rust 1.88.0. The Linux job runs `cargo-deny`, ast-grep rule tests and scans,
and the library-only Clippy and test commands. The required aggregate status is
**Required validation status**.

## Pull requests

Contribute on a short-lived branch and open a pull request targeting `main`.
Changes are squash-merged to `main` after the required checks pass. The
`make release-merge` target and `scripts/Merge-ToMain.ps1` are legacy dev-to-main
tools, not the contribution flow. Use the repository's
[pull request template](.github/pull_request_template.md), and add focused tests
for code changes where practical.

## COM and unsafe code

Follow the existing COM/DCOM ownership, apartment, and thread-affinity conventions.
Every `unsafe` block must have a `// SAFETY:` comment stating the actual invariant
that makes that specific operation safe. The workspace Clippy configuration and
ast-grep rules enforce safety-comment checks; do not replace them with generic
comments or invent a separate lint.

## License

The project is MIT-licensed and does not require a CLA. Preserve existing copyright
notices, including Wendell Saligan's line in `LICENSE`.
