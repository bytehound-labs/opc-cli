## Summary

<!-- What changed and why. Link any related issue (e.g. "Fixes #123"). -->

## Type of change

<!-- Check all that apply. -->

- [ ] Bug fix
- [ ] Feature or enhancement
- [ ] Refactor (no behavior change)
- [ ] Documentation
- [ ] Build, CI, or tooling
- [ ] Other (describe above)

Affected component:

- [ ] `bytehound-opc-da-client` (library)
- [ ] `opc-cli` (TUI)

## Validation

<!--
List each command you ran and its result (passed/failed).

On Windows, run the full gate: `pwsh -File scripts/verify.ps1` (equivalent to `./verify.sh`
or `make verify`). It runs formatting, Clippy with -D warnings, doc tests, workspace tests,
compat polyfill release builds, ast-grep (skipped when not installed), forbidden-pattern scans,
and a PowerShell syntax check.

On Linux, only compile and lint checks of the library are meaningful: the `opc-cli` TUI does not
build there, and the library's COM/DCOM code and tests are Windows-only:

  cargo test --locked -p bytehound-opc-da-client --all-features
  cargo clippy --locked -p bytehound-opc-da-client --all-targets --all-features -- -D warnings

Name any gate you skipped and why.
-->

## Hardware-in-the-loop notes

<!--
Automated tests do not exercise a live OPC DA server. For changes that affect COM/DCOM, server
browsing, reads/writes, or the TUI's interaction with a server, describe the manual verification:
OPC DA server vendor/product, Windows version, process bitness (32/64-bit), local or remote
(DCOM) connection, and the steps you performed. Otherwise write "Not applicable".
-->

## Documentation

<!-- List the documentation you updated (README, CHANGELOG, docs), or write "None needed". -->

## Checklist

- [ ] This change is on a feature branch and will be squash-merged through a pull request
- [ ] Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/)
- [ ] `pwsh -File scripts/verify.ps1` passes on Windows (or the validation above explains the
      alternative used)
- [ ] Tests were added or updated for the change, where practical
- [ ] The relevant `CHANGELOG.md` is updated for user-visible changes
- [ ] No secrets, credentials, build outputs, or log files are included
- [ ] No unrelated changes are included
- [ ] Hardware-in-the-loop changes (real OPC DA server/DCOM): manual verification steps are noted
      above
