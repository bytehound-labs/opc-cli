# Project Context Summary

## 2026-02-19: Write/Read Error Observability
> 📝 **Context Update:**
> * **Feature:** Write/Read Error Observability
> * **Changes:** Added `0xC0040006`/`0xC0040007`/`0xC0040008` to `friendly_com_hint`. `read_tag_values` now produces short "Error" in Value + hint in Quality + `warn!` log. `poll_read_result` routes per-item errors to TUI status log.
> * **New Constraints:** Sentinel value `"Error"` in `TagValue.value` indicates a per-item read failure — if changed, update both `opc_da.rs` and `app.rs`.
> * **Pruned:** Raw `HRESULT(0x...)` formatting in `TagValue.value` no longer occurs. Old debug logs showing `Error: HRESULT(0x Bad` are obsolete.

## 2026-02-19: Tag Values Page Fixes
> 📝 **Context Update:**
> * **Feature:** Tag Values UI & Currency Support
> * **Changes:**
>   - Implemented `select_next`/`select_prev` sync for `table_state` in `TagValues` screen.
>   - Added `VT_CY` (Currency) variant support in `helpers.rs` with 4-decimal formatting.
>   - Compressed repeated read status messages into summary line with error counts.
>   - Status bar now shows last 2 messages for better visibility.
> * **New Constraints:** `VT_CY` is now a supported type; ensure generic `VARIANT` handling accounts for it. Status log messages are now stateful/compacted.
> * **Pruned:** Generic `(VT 6)` display for currency values is gone. Single-line status bar limitation is removed.

## 2026-02-19: Cursor Preservation & Missing Variant Types
> 📝 **Context Update:**
> * **Feature:** Cursor Preservation & Variant Type Display
> * **Changes:**
>   - `poll_read_result` in `app.rs` now clamps `selected_index` to bounds instead of resetting to 0 on refresh.
>   - `variant_to_string` in `helpers.rs` gained support for `VT_DATE` (7), `VT_I1` (16), `VT_UI1` (17), `VT_UI2` (18), `VT_UI4` (19), `VT_I8` (20), `VT_UI8` (21), and `VT_ARRAY` (8192+).
>   - New helper `ole_date_to_string` converts OLE Automation dates to local datetime strings via `chrono`.
>   - `VT_I8`/`VT_UI8` use pointer-cast since windows-rs 0.61.3 doesn't expose `hVal`/`uhVal` fields.
>   - SafeArray display shows `Array[N] (type)` for 1-D; `Array[ND]` for multi-dimensional.
> * **Pruned:** Generic `(VT VARENUM(...))` displays for Date, integers, and arrays are gone. Previous audit report for Tag Values Page Fixes is superseded.

## 2026-02-20: Security & Quality Audit of opc-da-client
> 📝 **Context Update:**
> * **Feature:** Pre-implementation Audit of `opc-da-client`
> * **Changes:** Ran narsil MCP security scan and `cargo clippy`/`test`. Identified and fixed `clippy::approx_constant` warnings in `opc-da-client/src/helpers.rs` by replacing `3.14` with `3.5` in tests. Tests are green.
> * **New Constraints:** Maintain strict adherence to workspace clippy policies.
> * **Pruned:** None.

## 2026-02-21: Audit Remediation of opc-da-client & opc-cli
> 📝 **Context Update:**
> * **Feature:** Audit Remediation (ComGuard, clippy sweep, doctest fixes)
> * **Changes:** Implemented `ComGuard` RAII guard for COM initialization. Resolved 100+ clippy findings across both crates. Fixed doctest in `com_guard.rs`. Standardized workspace lint config in root `Cargo.toml`. Removed manual `CoUninitialize` from `main.rs`.
> * **New Constraints:**
>   - Use `pwsh` (not `powershell`) for all script invocations.
>   - Use `ComGuard::new()` for COM initialization — never call `CoInitializeEx`/`CoUninitialize` manually.
>   - Workspace lint allows are managed in root `Cargo.toml` `[workspace.lints.clippy]`.
> * **Pruned:** Manual COM teardown logic. Legacy `pub(crate)` visibility workarounds.

## ⚠️ 2026-02-21: Compliance Violations — Lessons Learned

> [!CAUTION]
> The following workflow and `GEMINI.md` violations occurred during the audit remediation session. **All future sessions MUST strictly follow `GEMINI.md` rules and `.agent/workflows/` definitions.**

### Violations Identified

| # | Rule Violated | Source | What Happened |
|---|---------------|--------|---------------|
| 1 | **Planning Gate** (§ GEMINI.md) | `GEMINI.md` lines 77–90 | Execution began without a formal Think Phase or user "Proceed" approval. Code edits were made in the same turn as analysis. |
| 2 | **Sequential Execution** | `GEMINI.md` line 197 | Used `&&` chaining in PowerShell commands (e.g., `cargo fmt --all && pwsh -File ./scripts/verify.ps1`). GEMINI.md explicitly prohibits this. |
| 3 | **Git Checkpoints** | `GEMINI.md` line 128 | No git commits were made before or after functional blocks. Changes were not checkpointed for reversibility. |
| 4 | **Audit Workflow** | `.agent/workflows/audit.md` | The `/audit` workflow was not followed. Steps 1–6 (Gather Context → Compliance Audit → Verification Gate → Findings Report → Summarize → Completion) were not executed in order. |
| 5 | **Plan-Making Workflow** | `.agent/workflows/plan-making.md` | The `/plan-making` workflow was not consulted. No implementation plan was created before execution for the CLI-side fixes. |
| 6 | **No `context.md` Update** | `.agent/workflows/audit.md` step 5 | Context was not compressed and appended to `context.md` during the session. |
| 7 | **Shell Preference** | User directive | Used `powershell` instead of `pwsh` throughout the session. |

### Binding Rules for Future Sessions

1. **Always read `GEMINI.md` first** — it is the Operational Source of Truth.
2. **Always follow the applicable workflow** from `.agent/workflows/` — they define step-by-step procedures that must not be skipped.
3. **Never chain commands with `&&`** in PowerShell — use sequential tool calls.
4. **Always create git checkpoints** before and after functional blocks.
5. **Always run the Planning Gate** before touching source code — produce an artifact, request approval, then execute.
6. **Always update `context.md`** at the end of every completed task per the Summarize phase.
7. **Use `pwsh`** (not `powershell`) for all script and command invocations.

## 2026-02-21: Documentation Refresh
> 📝 **Context Update:**
> * **Feature:** Documentation Refresh (READMEs, architecture, spec, Cargo descriptions)
> * **Changes:** Updated both READMEs with write support, controls table, `pwsh` commands; updated both `Cargo.toml` descriptions; added `ComGuard` § 1.4 to `spec.md` and updated test checklist; updated both `architecture.md` files with WriteInput state, write key, `ComGuard` in diagrams/threading model, and `pwsh` references.
> * **New Constraints:** All documentation now reflects `ComGuard`, write support, and `pwsh`. Keep docs in sync when adding features.
> * **Pruned:** Outdated test count (was "37 tests"), manual `CoInitializeEx`/`CoUninitialize` references in architecture docs.

## 2026-02-21: Vendored opc_da crates
> 📝 **Context Update:**
> * **Feature:** Vendored upstream `opc_da` crates
> * **Changes:** Cloned `Ronbb/rust_opc` master branch and extracted `opc_da`, `opc_da_bindings`, `opc_comn_bindings`, and `opc_classic_utils` into `vendor/`. Replaced crates.io dependencies with workspace path dependencies. Added unified workspace dependencies for `windows`, `thiserror`, etc. Added missing `[lib]` to `opc_da` v0.3.1 source and implemented lint suppression so the vendored code passes the workspace gate.
> * **New Constraints:** The vendored code is now part of the project and passes all verification gates. Future plans involve fully merging the crates into `opc-da-client` (Phase 2 & 3 tracked in `long_term_todo.md`).
> * **Pruned:** Removed reliance on crates.io for OPC DA backend.


## 2026-02-21: Audit - Vendored opc_da crates
> 📝 **Context Update:**
> * **Feature:** Structural Audit of opc-da-client and vendor/ crates
> * **Changes:** Verified that Phase 1 vendoring aligns precisely with GEMINI.md and coding_standard.md. Validated clean execution of verification gates and confirmed that Narsil CWE/OWASP findings are contained to expected COM/DCOM raw pointer operations.
> * **New Constraints:** The vendored crates must maintain their #[allow(...)] directives to bypass overly pedantic workspace lints, but any logic moved natively into opc-da-client (Phase 2) must adhere to the stricter zero-warning policy.
> * **Pruned:** Intermediate build errors and clippy suppression iterations during the initial vendor phase.


## 2026-02-21: Merge - Phase 2 opc_da inline
> 📝 **Context Update:**
> * **Feature:** Merged vendor/opc_da into opc-da-client/src/opc_da/
> * **Changes:** Completed Phase 2 of the OPC DA integration. Moved client modules, defs, and utils inline. Actix, globset, and duplicate tokio dependencies were entirely dropped by selectively excluding the 'unified' and 'server' modules. The opc-da-backend feature is now triggered by the COM binding crates.
> * **New Constraints:** opc-da-client now holds its own OPC DA logic, but continues to reference vendor/opc_da_bindings and vendor/opc_comn_bindings (Phase 3 remaining).
> * **Pruned:** The entire vendor/opc_da boundary layer.


## 2026-02-21: Audit - Phase 2 opc_da merge compliance
> 📝 **Context Update:**
> * **Feature:** Post-merge compliance audit of opc-da-client
> * **Changes:** Verified all coding_standard.md and GEMINI.md requirements after Phase 2 merge. Zero unwraps in library code, 15 structured tracing calls at consumer layer, 19 unit tests passing, full clippy/fmt/test gates green.
> * **New Constraints:** The merged opc_da/ module uses #[allow] attributes inherited from upstream. Any code moved to native opc-da-client modules must adopt the strict workspace lint policy. OpcProvider integration tests require a live OPC DA server.
> * **Pruned:** Phase 2 intermediate build/format/clippy iterations. Audit scan data from Narsil.


## 2026-02-21: Audit - Phase 2 opc_da merge compliance
> 📝 **Context Update:**
> * **Feature:** Post-merge compliance audit of opc-da-client
> * **Changes:** Verified all coding_standard.md and GEMINI.md requirements after Phase 2 merge. Zero unwraps in library code, 15 structured tracing calls at consumer layer, 19 unit tests passing, full clippy/fmt/test gates green.
> * **New Constraints:** The merged opc_da/ module uses #[allow] attributes inherited from upstream. Any code moved to native opc-da-client modules must adopt the strict workspace lint policy. OpcProvider integration tests require a live OPC DA server.
> * **Pruned:** Phase 2 intermediate build/format/clippy iterations. Audit scan data from Narsil.


## 2026-02-21: ComGuard RAII Refactor & Observability Upgrade
> 📝 **Context Update:**
> * **Feature:** ComGuard RAII compliance and backend tracing.
> * **Changes:**
>   - Rewrote com_guard.rs: added PhantomData<*mut ()> for !Send/!Sync, changed 
ew() to return Err on failure (was silently succeeding), added 	racing::debug! on init/teardown.
>   - Added 	racing::info_span! to all 4 OpcProvider methods in ackend/opc_da.rs with structured fields (server, tag_count, etc.).
>   - 
emove_group errors now logged instead of silently discarded.
>   - Removed superfluous inner blocks and deduplicated SAFETY comments.
>   - Added success-path tracing to connect_server() in helpers.rs.
> * **New Constraints:** ComGuard is now !Send + !Sync. It can only be created and dropped on the same OS thread. This doesn't affect current spawn_blocking usage.
> * **Pruned:** The old initialized: bool field pattern and duplicate SAFETY comments.

## 2026-02-21: Phase 3 Bindings Merge
> 📝 **Context Update:**
> * **Feature:** Merged generated COM bindings and dropped unused vendor crates.
> * **Changes:** Built on Phase 2 by freezing windows-bindgen outputs from opc_da_bindings and opc_comn_bindings. Natively incorporated indings.rs as mod bindings; (da and comn) directly into opc-da-client. Removed the windows-bindgen build dependency. Dropped the completely unused opc_classic_utils crate.
> * **New Constraints:** The OPC DA bindings are now "frozen." If the underlying Windows metadata (OPCDA.winmd) ever needs regeneration, the files stored in opc-da-client/.winmd/ must be manually processed with the windows bindgen CLI.
> * **Pruned:** The endor/opc_da_bindings/, endor/opc_comn_bindings/, and endor/opc_classic_utils/ directories. Cargo metadata references to generating bindings on-the-fly.

## 2026-02-21: Phase 4 Testability Refactor & SafeArray
> 📝 **Context Update:**
> * **Feature:** OPC DA Mocking & SafeArray iteration.
> * **Changes:**
>   - Abstracted concrete COM bindings via the `ServerConnector` trait inside `connector.rs`.
>   - Bound `OpcDaClient<C>` to `<C: ServerConnector>`.
>   - Implemented `MockServerConnector` along with realistic integration test cases in `backend/opc_da.rs`.
>   - Validated array bounds parsing with `SafeArrayGetElemsize` and `SafeArrayAccessData` inside `variant_to_string` printing full arrays (capped at 20 max items).
> * **New Constraints:** Mock backend testing can now be used for logic testing without a real COM server. Any new methods on `OpcDaClient` should use `self.connector` rather than raw COM instantiation. SafeArrays now return JSON stringified vectors instead of the default `Array[N]`.
> * **Pruned:** Outdated constraints requiring live Windows COM environment for integration testing bounds.

## 2026-02-21: Compliance Audit & Remediation
> 📝 **Context Update:**
> * **Feature:** Deep compliance audit of `opc-da-client` against `coding_standard.md` and `GEMINI.md`.
> * **Changes:** Remediated 11 findings across `connector.rs`, `opc_da.rs`, `helpers.rs`, `iterator.rs`: full doc coverage on all public traits/structs, `// SAFETY:` on `transmute_copy`, `&raw mut` for `borrow_as_ptr`, `cast_unsigned()` for sign-loss, collapsed `if let`, removed 5 stale imports, cleaned stale comments, removed unnecessary cast.
> * **New Constraints:** All public items in `connector.rs` now have `///` docs with `# Errors`. The `transmute_copy` GUID conversion references the `const_assert_eq!` in `iterator.rs` for layout validation.
> * **Pruned:** Raw clippy output and intermediate verification logs from this audit cycle.

## 2026-02-22: Workspace Cargo.toml Config Fixes
> 📝 **Context Update:**
> * **Feature:** Re-integrated `opc-cli` into workspace and aligned dependencies.
> * **Changes:** Added `opc-cli` to workspace members so `cargo build` produces the TUI executable again. Lifted overlapping dependencies (`anyhow`, `tokio`, `tracing`) to `[workspace.dependencies]`. Updated `opc-cli/src/main.rs` to instantiate `OpcDaClient::new(ComConnector)` due to the Phase 4 mockability refactor.
> * **New Constraints:** `vendor/opc_classic_utils/` is explicitly retained in the repo until new code is fully tested, but deliberately kept out of workspace members.
> * **Pruned:** Outdated inline `version` declarations for shared dependencies inside crate-level `Cargo.toml`s.

## 2026-02-22: Documentation Sync (Post-Phase 4)
> 📝 **Context Update:**
> * **Feature:** Synchronized READMEs and crate descriptions with Phase 4 architecture.
> * **Changes:** Fixed all 4 code examples in `opc-da-client/README.md` to use `ComGuard::new()?` and `OpcDaClient::default()` (since `new()` now requires `ComConnector`). Updated feature descriptions and doc comments to explicitly declare the native `windows-rs` implementation instead of the obsolete `opc_da` crate.
> * **New Constraints:** Any new examples must demonstrate COM initialization via `ComGuard` and use `OpcDaClient::default()` unless explicitly demonstrating the mock backend.
> * **Pruned:** References to the library being powered by the external `opc_da` crate.

## 2026-02-22: VT_ERROR and Resource Leak Fixes 
> 📝 **Context Update:**
> * **Feature:** VT_ERROR parsing, tag array constraint fix, and resource leak prevention
> * **Changes:** Fixed `variant_to_string` to properly parse `VT_ERROR` containing HRESULTs. Enforced 1-to-1 array sizes for `read_tag_values` using `TagValue { value: "Error", quality: "Bad", timestamp: "" }` for failed items. Ensured `remove_group` executes unconditionally in `read_tag_values` and `write_tag_value` via RAII-like scope drops. Extracted `format_hresult` to standardize `0xHHHHHHHH: <hint>` output. Updated `spec.md` and `architecture.md` with these invariants.
> * **New Constraints:** `read_tag_values` MUST always return the exact same number of `TagValue`s as requested IDs. OPC groups must be dynamically removed using `remove_group` regardless of failure states.
> * **Pruned:** Old console warnings from missing VT_ERROR handlers. Raw HRESULT error messages that skip `format_hresult()`.

## 2026-02-22: Published opc-da-client v0.1.0 to crates.io
> 📝 **Context Update:**
> * **Feature:** Prepared and published `opc-da-client` v0.1.0, making the OPC DA abstraction layer publicly available.
> * **Changes:** Bumped version to 0.1.0, addressed 18 latent `clippy` lints (`useless-conversion`, `undocumented-unsafe-blocks`, `field-reassign-with-default`, `needless-range-loop`), added `try_from_native!` missing docs, enhanced crate-level docs and `format_hresult` with doctests, and established `exclude`/`license-file` crate metadata.
> * **New Constraints:** None.
> * **Pruned:** The `opc-da-client` crate is now officially v0.1.0 on `crates.io`. `opc-cli` crate version also bumped to 0.1.0 to match.

## 2026-02-22: Fix OPC-BUG-001 — StringIterator E_POINTER Flood
> 📝 **Context Update:**
> * **Feature:** Eliminated phantom `E_POINTER` errors from `StringIterator` at the source.
> * **Changes:** Added cache zeroing before each `IEnumString::Next()` call, null-PWSTR skip loop with `debug!` logging, and diagnostic tracing (HRESULT, celt, count). Removed `is_known_iterator_bug()` function and its caller-side workaround from `browse_recursive`. Added 2 regression tests (`test_string_iterator_null_entries_skipped`, `test_string_iterator_empty`). Updated `architecture.md` and `spec.md`.
> * **New Constraints:** `StringIterator` now self-heals null entries. Callers no longer need to filter `E_POINTER`. Any future iterator changes must preserve the cache-zeroing and null-skip logic.
> * **Pruned:** `is_known_iterator_bug()` function and its 2 tests. `trace!`-level E_POINTER downgrade in `browse_recursive`.

## 2026-02-22: TARS Summary — Mainline Merge
> 📝 **Context Update:**
> * **Feature:** Merged `feature/merge-opc-da` into `main` (Fast-Forward).
> * **Changes:** 16 commits (+15k/-600 lines) bringing the vendored `opc_da` components intimately into `opc-da-client`, adding testability/mocking, releasing v0.1.0 on crates.io, fixing OPC-BUG-001 (E_POINTER flood) at the source in `iterator.rs`, and enhancing global log observability.
> * **New Constraints:** Any future developments to COM iterator consumption MUST observe the new `StringIterator` behavior (self-healing null skip, zeroed cache).
> * **Pruned:** All prior intermediate implementation logs for these features can be dropped from active memory. The `feature/merge-opc-da` branch has been deleted.

## 2026-02-22: TARS Summary — Released opc-da-client v0.1.1
> 📝 **Context Update:**
> * **Feature:** Released `opc-da-client` v0.1.1 to Crates.io.
> * **Changes:** Bumped version. Cleaned up stale documentation references to `is_known_iterator_bug` in `spec.md` and `architecture.md` (OPC-BUG-001 is fixed at the source). Added strict `#![allow]` attributes for `clippy` macro-expansions. Updated CHANGELOG.
> * **New Constraints:** None.
> * **Pruned:** Old `is_known_iterator_bug` context is completely removed. v0.1.1 is the new active baseline.

## 2026-02-22: TARS Summary — Documentation Alignment
> 📝 **Context Update:**
> * **Feature:** Realigned crate docs (`spec.md`, `architecture.md`, `README.md`) and codebase variables with the recent v0.1.1 changes.
> * **Changes:** Fixed broken crates.io links in README. Added missing HRESULT hint codes to `spec.md`, removed stale `is_known_iterator_bug` rows, and corrected stale `E_POINTER` hint blame text.
> * **New Constraints:** None.
> * **Pruned:** The issue track `/issue update crate spec.md and architecture.md` is complete and can be archived.

## 2026-02-22: TARS Summary — Published opc-da-client v0.1.2
> 📝 **Context Update:**
> * **Feature:** Published v0.1.2 to crates.io to push updated README and hint text.
> * **Changes:** Version bump, CHANGELOG entry, corrected crates.io README links and E_POINTER hint text.
> * **New Constraints:** None.
> * **Pruned:** v0.1.2 is the new active baseline on crates.io.

## 2026-02-22: OPC_FLAT Browse Performance Optimization
📝 **Context Update:**
* **Feature:** OPC DA V2 Browse Performance Optimization (OPC_FLAT Try-First)
* **Changes:** 
    * Implemented `OPC_FLAT` try-first hierarchy traversal in `opc_da::client::browse_tags` to eliminate ~90% of COM calls during namespace browsing, smoothly falling back to recursive enumeration on error or empty results.
    * Increased `StringIterator` batch fetched size `STRING_CACHE_SIZE` to 256 for a 16x reduction in `IEnumString::Next` COM round-trips.
    * Added comprehensive `MockHierarchicalServer` TDD tests inside `opc_da.rs` to validate all fast-path and fallback execution flows.
* **New Constraints:** 
    * Future `ServerConnector` mock additions for `browse_tags` must now account for `OpcFlatBehavior` to verify fast-path interactions.
* **Pruned:** 
    * `OPC-BUG-001` (null PWSTR entries) is permanently fixed in `StringIterator` and no longer needs manual tracking as an active constraint.

## 2026-02-22: Documentation Audit & Remediation
> 📝 **Context Update:**
> * **Feature:** Document Audit (Reflect & Summarize)
> * **Changes:** Performed comprehensive audit of `spec.md`, `architecture.md` (repo/crate), and `context.md`. Remediated 12 findings including stale version numbers, missing OPC_FLAT behavioral contracts, stale path references (`opc_impl.rs`), and inconsistent test counts.
> * **New Constraints:** Maintain `architecture.md` and `spec.md` in sync when modifying the `OPC_FLAT` or `StringIterator` logic.
> * **Pruned:** References to `opc_impl.rs` are eliminated. Stale test count (20+) updated to 80+.

## 2026-02-22: TARS Summary — Published opc-da-client v0.1.3
> 📝 **Context Update:**
> * **Feature:** Published v0.1.3 to fix docs.rs build failure.
> * **Changes:** Added `[package.metadata.docs.rs]` with `default-target = "x86_64-pc-windows-msvc"` and `all-features = true`. Bumped version, updated CHANGELOG, README, and architecture.md.
> * **New Constraints:** None.
> * **Pruned:** v0.1.3 is the new active baseline on crates.io.
## 2026-02-22: TARS Summary — Documentation Staleness Audit
> 📝 **Context Update:**
> * **Feature:** Exhaustive documentation staleness audit.
> * **Changes:** Scanned code base and markdown for stale references to Phase 2/3 vendor crates. Updated 1 rustdoc in `opc_da.rs` ("uses the `opc_da` crate" -> "uses the internal `opc_da` module") and 1 phrase in `spec.md`.
> * **New Constraints:** None.
> * **Pruned:** The conceptual barrier of "vendored" code is fully eliminated; `opc_da` is treated strictly as an internal module.

## 2026-02-22: TARS Summary — Audit Remediation
> 📝 **Context Update:**
> * **Feature:** Pre-implementation Audit Remediation
> * **Changes:** Fixed 6 conformance findings (F1-F6). Added `#[non_exhaustive]` to `OpcError`, scrubbed redundant empty lines, scoped `clippy::missing_errors_doc` to the `client` module, added `//!` module doc to `opc_da/mod.rs`, replaced manual `as i32` casting with `.cast_signed()`, and replaced `ComGroup` initialization with `Self`. Workspace `cargo fmt`, `clippy`, and `test` execution is completely clean.
> * **New Constraints:** The `clippy::missing_errors_doc` allowance is strictly localized to the COM bindings wrapping layer (client module). Other opc-da-client library modules must continue documenting `# Errors`.
> * **Pruned:** The audit findings are resolved and the code holds a stable zero-exit verification state.

## 2026-02-22: TARS Summary — Codebase Security Audit
> 📝 **Context Update:**
> * **Feature:** Baseline Security & Compliance Audit
> * **Changes:** Ran Narsil scans (OWASP Top 10, CWE Top 25) and Cargo checks. 0 actionable security findings in `opc-cli`; 1 false positive SQL-i detected on UI keystroke logic.
> * **New Constraints:** None.
> * **Pruned:** The codebase holds a high-fidelity state against the architecture specification.

## 2026-02-23: TARS Summary — Documentation Staleness Audit & Remediation
> 📝 **Context Update:**
> * **Feature:** Documentation Staleness Audit & Remediation
> * **Changes:** Performed a codebase-wide audit catching 7 lingering references to `OpcDaWrapper`. Replaced all instances with the active struct name `OpcDaClient` across architectural diagrams, spec tables, changelogs, and rustdoc safety comments. Verified zero-exit status with `cargo doc` and standard tests.
> * **New Constraints:** None.
> * **Pruned:** The `OpcDaWrapper` identifier is universally excised from the repositories.

## 2026-02-23: TARS Summary — Documentation Issue Remediation
> 📝 **Context Update:**
> * **Feature:** Remediation of Issue Check on `opc-da-client` Documentation.
> * **Changes:** Modernized stale rustdoc claims across `README.md`, `architecture.md`, `spec.md`, and `connector.rs` pointing to `anyhow` instead of the crate's unified `OpcError`/`OpcResult` type hierarchy. Implemented strict `#![doc = include_str!("../README.md")]` static checks, alongside `no_run` attributes to prohibit live CI invocation of OPC DA integration doc-tests without environment dependencies.
> * **New Constraints:** Any new examples added to `README.md` must be valid rust logic and bear the `no_run` attribute so they do not crash standard test suites relying on `OpcDaClient<ComConnector>`.
> * **Pruned:** The `opc-da-client/README.md` issue stands resolved.

## 2026-02-23: TARS Summary — Observability Audit
> 📝 **Context Update:**
> * **Feature:** `/audit` execution inspecting `opc-da-client` observability and tracing compliance. 
> * **Changes:** Evaluated the entire `opc-da-client` library against the explicit constraints established by `coding_standard.md`. Verified that `println!` logging is correctly absent from all production code. Verified that `backend::opc_da::OpcDaClient` structurally enforces `tracing::info_span!` mapping across facade entry-points. Reconciled `tracing::info!` occurrences on all success paths and `tracing::error!` / `tracing::warn!` statements on failure modes, fallback loops, and COM teardown contexts.
> * **New Constraints:** None. All functions cleanly comply with the observability mandate.
> * **Pruned:** The task represents an observational snapshot and required zero codebase modifications. 

## 2026-02-23: TARS Summary — Publication Readiness Audit
> 📝 **Context Update:**
> * **Feature:** Pre-publication quality control and security `/audit` for `opc-da-client`.
> * **Changes:** Evaluated the operational readiness of `opc-da-client` for publishing to `crates.io`. Confirmed `Cargo.toml` structural completeness. Re-ran Narsil security scans across the repository resolving 0 findings and 0 vulnerable dependency maps. Fired a `cargo publish --dry-run` to assert the proper compression, exclusion mapping (`spec.md`, `.winmd`), and MSVC docs.rs target resolution. 
> * **Pruned:** The crate holds a secure and technically verified baseline to initiate the official crates.io distribution.

## 2026-02-23: TARS Summary — Verification Script Audit & Modernization
> 📝 **Context Update:**
> * **Feature:** Pre-execution `/audit` of `verify.sh` for correctness and `pwsh` efficiency.
> * **Changes:** Replaced the legacy split verification sequence with a hyper-efficient `pwsh`-native pipeline hosted at the repository root (`verify.ps1`). The new gate implements strict $LASTEXITCODE evaluation (`$ErrorActionPreference = 'Stop'`) handling zero-exit architecture cleanly. Appended `--all-targets --all-features` to the `cargo clippy` pass to abolish blindspots seen in prior mocks. Injected `cargo test --doc` explicitly to block stale documentation errors structurally. `verify.sh` was retained simply to bridge unix executions strictly back into the `pwsh -File verify.ps1` master process. Old scattered scripts (`scripts/verify.ps1`) were deleted.
> * **New Constraints:** Any integration gating script must pass through `verify.ps1` invoking `$ErrorActionPreference = 'Stop'`.
> * **Pruned:** `scripts/verify.ps1` deleted. Outdated partial `verify.sh` checks deleted.

## 2026-02-23: TARS Summary — Automated Git Pipeline Construction
> 📝 **Context Update:**
> * **Feature:** Crafted an end-to-end `pwsh` script (`commit.ps1`) automating the verification, staging, commit, and remote push mechanics.
> * **Changes:** Built `commit.ps1` at the repository root. This orchestrator accepts a mandatory conventional `$Message` parameter. It forces a synchronized evaluation of `.\verify.ps1`, strictly halting all git actions if the CI gate encounters *any* formatting, clippy, unit, or doc testing errors (via `$LASTEXITCODE`). Upon successful gate verification, it sequentially manages `git add .`, `git commit -m`, configures the dynamic tracking branch via `git branch --show-current`, and commits an automated `git push --set-upstream`. 
> * **New Constraints:** Development changes should be staged via `.\commit.ps1 -Message "conventional commit string"` to guarantee no unverified code infiltrates the deployment lineage.
> * **Pruned:** Manual `git status`, `git commit`, `git push` overheads are now compressed into a single, safely gated command.

## 2026-02-23: TARS Summary — CHANGELOG.md Backfill (v0.2.0)
> 📝 **Context Update:**
> * **Feature:** Formalized backfill of missing `v0.2.0` release notes resolving the issue report.
> * **Changes:** Injected the `## [0.2.0]` release node natively into `opc-da-client/CHANGELOG.md`. Documented the pivotal architectural leap representing the version bump: purging external `opc_da*` bindings dependencies in favor of native workspace inclusion to drastically boost build velocity, safety, and testing agility (`MockServerConnector`). Cataloged the migration from `anyhow` to the strongly-typed `OpcResult` (`thiserror`) while preserving structural application compatibility. Logged the injection of `no_run` onto `README.md` examples.
> * **New Constraints:** None.
> * **Pruned:** The `/issue` surrounding missing changelog data for v0.2.0 is closed. No further ambiguities surround the `v0.1.3 -> v0.2.0` evolution.

## 2026-02-23: TARS Summary — `/prepublish` Workflow Architecture
> 📝 **Context Update:**
> * **Feature:** Constructed `.agent/workflows/prepublish.md` — a 9-step AI workflow automating pre-publication QA/QC for `crates.io` releases.
> * **Changes:** Created the workflow with: Context Init, Version Sync (README/Cargo/rustdocs/CHANGELOG), Docs Consistency, Cargo Manifest QC, Narsil Security Scan, Verification Gate (`verify.ps1`), Simulated `cargo publish --dry-run`, structured Report (`prepublish_report.md` with Pass/Fail matrix, Action Items, Recommendations), and Completion. Follows the same structural conventions as `/audit` and `/plan-making`.
> * **New Constraints:** Invoke `/prepublish` before every `cargo publish` to guarantee documentation, versioning, and security alignment.
> * **Pruned:** Ad-hoc pre-publish manual checks are superseded by the formalized workflow.

## 2026-02-23: Refactor OPC DA COM Threading & Pooling
> 📝 **Context Update:**
> * **Feature:** Replace task-based COM threading with long-lived ComWorker pool
> * **Changes:** Replaced spawn_blocking/ComGuard per-request pattern with a dedicated ComWorker thread using mpsc/oneshot messaging. Added connection cache to the worker to fix COM connection churn and ephemeral port exhaustion. Verified all 51 tests across both crates.
> * **New Constraints:** Any modifications to COM logic MUST occur via ComRequest messages executed inside the single ComWorker thread. Tests needing COM execution must wrap their worker spawn in 	okio::task::spawn_blocking.
> * **Pruned:** ComGuard references and per-request spawn_blocking from opc_da.rs are obsolete. Previous audit recommendations around short-lived connections are superseded by the worker pool.


> 📝 **Context Update:**
> * **Feature:** Resolve tokio runtime panic in ComWorker::start()
> * **Changes:** Switched ComWorker initialization signal from tokio::sync::oneshot to std::sync::mpsc. The OS-level synchronization prevents Tokio from detecting a blocking call on the async runtime thread, safely avoiding a deadlock panic. Enhanced tracing visibility by adding bookend tracing::info! milestones in OpcDaClient::new() and reordering main.rs to initialize OPC *before* taking over the terminal with the raw UI, ensuring any future startup errors write to standard terminal out instead of being swallowed by the alternate screen.
> * **New Constraints:** Only use tokio::sync primitives if waiting inside async methods via .await. Use std::sync::mpsc for purely blocking initialization synchronization between a standard thread and an async tokio thread context.
> * **Pruned:** The panic.txt log output can be completely ignored.

## 2026-07-15: Author Attribution & Sync-TaskList Fix
> 📝 **Context Update:**
> * **Feature:** Added developer attribution and fixed PowerShell 5.1 compatibility.
> * **Changes:**
>   - Updated [LICENSE](file:///c:/Users/WSALIGAN/code/opc-cli/LICENSE) copyright notice to attribute ownership to `Wendell Saligan <saliganw@gmail.com>`.
>   - Added `authors` array containing `Wendell Saligan <saliganw@gmail.com>` in both workspace manifests ([opc-cli/Cargo.toml](file:///c:/Users/WSALIGAN/code/opc-cli/opc-cli/Cargo.toml) and [opc-da-client/Cargo.toml](file:///c:/Users/WSALIGAN/code/opc-cli/opc-da-client/Cargo.toml)).
>   - Replaced multibyte Unicode emojis (`✅`, `❌`, `🟢`, `🟡`, `🔴`) and em-dashes (`—`) with standard ASCII strings and hyphens inside [.agent/scripts/Sync-TaskList.ps1](file:///c:/Users/WSALIGAN/code/opc-cli/.agent/scripts/Sync-TaskList.ps1) to resolve parse and syntax errors in Windows PowerShell 5.1.
>   - Remediated a cargo clippy warning in `opc-da-client/src/helpers.rs` concerning a redundant borrow in `format!`.
> * **New Constraints:**
>   - Maintain ASCII-only strings in repository automation scripts to prevent parsing issues on Windows PowerShell 5.1 environments.
> * **Pruned:** The encoding/parse error in `Sync-TaskList.ps1` is resolved.

## 2026-07-15: Migration of Agent Configuration to `.agents/`
> 📝 **Context Update:**
> * **Feature:** Migrated project agent configuration to the unified `.agents/` layout.
> * **Changes:**
>   - Created the `.agents/` directory structure containing `rules/`, `workflows/`, and `scripts/`.
>   - Copied all standard rules and workflows from the central rules repository `c:\Users\WSALIGAN\code\rules\.agents\`.
>   - Migrated project-specific workflows ([log-audit.md](file:///c:/Users/WSALIGAN/code/opc-cli/.agents/workflows/log-audit.md) and [prepublish.md](file:///c:/Users/WSALIGAN/code/opc-cli/.agents/workflows/prepublish.md)) to the new layout.
>   - Copied and sanitized all 7 helper PowerShell scripts in `.agents/scripts/`, replacing multibyte Unicode emojis with ASCII tags (`[OK]`, `[FAIL]`, `[WARN]`, `[SKIP]`, `[NEW]`, `[MODIFY]`, `[DELETE]`, `[AUTO]`, `[MANUAL]`, `[BUG]`, `[TOOL]`) and correcting multiple-argument `Join-Path` calls for Windows PowerShell 5.1 compatibility.
>   - Updated [.gitignore](file:///c:/Users/WSALIGAN/code/opc-cli/.gitignore) to use directory-level ignores for `.agents/rules/`, `.agents/scripts/`, and `.agents/workflows/*` while tracking our project-specific workflows.
>   - Deleted the obsolete `.agent/` directory recursively.
> * **New Constraints:**
>   - Use the new `.agents/` folder path for all agent workflows and codebase validation scripts.
> * **Pruned:** The old `.agent/` configuration directory is deleted.

## 2026-07-15: Fix Join-Path & Shell Invocations in Git-Checkpoint
> 📝 **Context Update:**
> * **Feature:** Fixed PowerShell 5.1 compatibility and dynamic shell resolution in `Git-Checkpoint.ps1`.
> * **Changes:**
>   - Corrected the three-argument `Join-Path` call in [.agents/scripts/Git-Checkpoint.ps1](file:///c:/Users/WSALIGAN/code/opc-cli/.agents/scripts/Git-Checkpoint.ps1) line 114 to use nested two-argument calls compatible with Windows PowerShell 5.1.
>   - Replaced the hardcoded `pwsh` task validation invocation in `Git-Checkpoint.ps1` line 125 with a dynamic shell selection (`pwsh` if available, falling back to standard `powershell`).
>   - Updated [scratch/sanitize.ps1](file:///C:/Users/WSALIGAN/.gemini/antigravity/brain/ba1a478e-271c-4b53-97ee-9753ed0f9b69/scratch/sanitize.ps1) replacements to match and clean both three-argument and four-argument `Join-Path` formats in future synchronization passes.
> * **New Constraints:** None.
> * **Pruned:** Invalid parameter binding parser crash in `Git-Checkpoint.ps1`.

## 2026-07-15: Divergent Branches & Clean Main Release Merges
> 📝 **Context Update:**
> * **Feature:** Divergent branches architecture with clean main branch releases.
> * **Changes:**
>   - Created [scripts/Merge-ToMain.ps1](file:///c:/Users/WSALIGAN/code/opc-cli/scripts/Merge-ToMain.ps1) to automate clean release merges from development/feature branches into the `main` branch.
>   - Implemented an `Invoke-Git` execution wrapper in the script to bypass PowerShell native command standard error traps (e.g. `git checkout` logging progress to stderr) under `$ErrorActionPreference = 'Stop'`.
>   - Programmed the merge utility to strip agent workflows (`.agents/`), session logs (`context.md`), dev-only documentation (`architecture.md`, `TODO.md`, `long_term_todo.md`), and build artifacts (`clippy_output.json`) from `main` during merge.
>   - Automated stripping of agent-specific ignore rules from `.gitignore` on the `main` branch.
>   - Renamed the local development branch from `refactor/opc-da-integration` to `dev` to act as the primary branch for all active development.
>   - Successfully executed the first clean merge from `dev` to `main`, validating that the release branch is free of all agent-related files, metadata, and dev-only rules.
> * **New Constraints:**
>   - All active development and agent usage occurs on the `dev` branch.
>   - Use the `scripts/Merge-ToMain.ps1` script to propagate changes to `main` for release tags. Do not merge `dev` directly into `main` using standard Git merge commands, as this will bleed agent metadata into the release branch.

## 2026-07-15: Documentation Sync & Verification Hash Tracking
> 📝 **Context Update:**
> * **Feature:** Documented clean release merge utility and branch strategy; synced newline formatting workspace-wide.
> * **Changes:**
>   - Updated [architecture.md](file:///c:/Users/WSALIGAN/code/opc-cli/architecture.md) (root level) to document the Branch Strategy & Release Workflow (`dev` vs. `main`) and the release merge utility (`Merge-ToMain.ps1`).
>   - Updated [opc-da-client/architecture.md](file:///c:/Users/WSALIGAN/code/opc-cli/opc-da-client/architecture.md) to add the `Merge-ToMain.ps1` script to the Toolchain inventory.
>   - Added a verification reference commit hash (`e768239`) to [opc-da-client/spec.md](file:///c:/Users/WSALIGAN/code/opc-cli/opc-da-client/spec.md) for drift detection tracking.
>   - Normalized newline endings to Unix-style LF across all source code files using `cargo fmt` to resolve formatting linter warnings.
>   - Successfully executed the clean release merge from `dev` to `main`, auto-resolving modify/delete conflicts on stripped files.
> * **New Constraints:** None.
> * **Pruned:** Redundant CR character carriage returns in source files.

## 2026-07-15: TARS Summary — Hardening opc-da-client Core Logic
> 📝 **Context Update:**
> * **Feature:** Hardened the core logic of `opc-da-client` COM communications and pool caching.
> * **Changes:**
>   - Added defensive array length validation checks on the return values of `group.add_items` in `ComWorker::handle_read`, comparing COM-allocated array sizes with the requested `tag_ids` to block silent zip truncation.
>   - Added group destruction cleanup to prevent resource leaks during length mismatch failures.
>   - Replaced the stub `test_worker_read_tag_values` in `com_worker.rs` with `test_worker_read_tag_values_mismatched_lengths` unit test.
>   - Refactored server cache lookup logic in `dispatch_with_retry` to use Rust's `Entry` API, removing double lookup hash penalties and unsafe `.unwrap()` calls.
>   - Documented the panicking behavior of `OpcDaClient::default()` in `opc_da.rs` rustdocs.
> * **New Constraints:** None.
> * **Pruned:** The risk of silent zip-truncation data misalignment on array mismatches is resolved.

## 2026-07-15: TARS Summary — Prepublish QA & Release Merge to Main
> 📝 **Context Update:**
> * **Feature:** Prepublish QA sweep, package release, and clean release branch merge.
> * **Changes:**
>   - Performed full `/prepublish` QA check on `opc-da-client` v0.2.0 (version verification, documentation consistency, license attributions check, security scans, verify.ps1 compilation and tests).
>   - Updated `opc-da-client/CHANGELOG.md` for v0.2.0 to detail the ComWorker hardening and `Default::default()` panic behavior documentation.
>   - Added `rewrite.py` to the `exclude` list in `opc-da-client/Cargo.toml` to prevent build scripts from packaging.
>   - Executed `scripts/Merge-ToMain.ps1` to cleanly merge the `dev` branch changes to `main` while stripping all agent metadata and workflows.
>   - Committed and pushed `dev` and `main` branches to remote repository.
>   - Performed a simulated registry dry-run publish, which was successful.
> * **New Constraints:**
>   - Official crates.io publish is prepared and validated on the clean `main` branch, but requires the user's cargo authentication token to publish live.
> * **Pruned:** Outdated CHANGELOG v0.2.0 omissions.


## 2026-07-15: Internalize ComGuard & API Simplification
> 📝 **Context Update:**
> * **Feature:** Internalize ComGuard and document transparent COM management.
> * **Changes:**
>   - Modified `opc-da-client/src/com_guard.rs` to keep `ComGuard` and its constructor internal-only (crate-private) to the library, resolving public API noise.
>   - Re-exported `ComGuard` inside `opc-da-client/src/lib.rs` using `pub(crate) use` to allow crate modules (like `com_worker.rs`) to continue using `crate::ComGuard`.
>   - Updated all four quickstart examples in `opc-da-client/README.md` and module-level docs in `lib.rs` to remove the redundant `ComGuard` initialization lines.
>   - Rewrote the features list and added a detailed "COM Threading Model" architectural overview to the library `README.md` and root `README.md` to clarify the background MTA threading pool.
>   - Switched the doc-test in `com_guard.rs` to `ignore` and updated `spec.md` and `architecture.md` (in both crate and workspace levels) to reflect the new internal API classification.
> * **New Constraints:**
>   - Downstream consumers do not need to call `ComGuard` or initialize COM. COM MTA lifecycles are completely self-contained within `OpcDaClient`.
> * **Pruned:** The public API exposure of `ComGuard` and all associated developer-facing manual COM initialization steps.

## 2026-07-15: TARS Summary — Sync opc-cli with Transparent COM API
> 📝 **Context Update:**
> * **Feature:** Synchronized the `opc-cli` TUI/CLI crate with the new simplified transparent COM API of `opc-da-client`.
> * **Changes:**
>   - Replaced a stale 3-line comment block in `opc-cli/src/main.rs` that referenced the removal of `ComGuard` as a recent transition, establishing it as settled architecture.
>   - Verified that all `opc-cli` CLI and TUI source code contains zero imports or references to the now crate-private `ComGuard` struct.
>   - Executed the workspace validation pipeline to verify clean compilation, zero lints, and passing tests across the entire workspace.
> * **New Constraints:** None.
> * **Pruned:** The stale `ComGuard` removal comment block inside `opc-cli/src/main.rs`.
## 2026-07-15: TARS Summary — Documentation Sync & Codebase Alignment
> 📝 **Context Update:**
> * **Feature:** Executed `/update-doc` workflow to ensure complete compliance with codebase standards.
> * **Changes:**
>   - Added comprehensive `//!` module-level doc comments to all binary crate source files in `opc-cli`: `main.rs`, `app.rs`, and `ui.rs`.
>   - Synchronized description fields in `Cargo.toml` and documentation across the workspace.
>   - Updated the `Last verified against` reference commit hash in `opc-da-client/spec.md` to `91632d6` (matching the current functional codebase state) for drift tracking.
>   - Validated formatting, clippy lints, unit/doc tests, and drift boundaries.
> * **New Constraints:** None.
> * **Pruned:** Lacking module-level doc comments in `opc-cli` TUI modules.

## 2026-07-16: TARS Summary — Address Example Review Findings in README.md
> 📝 **Context Update:**
> * **Feature:** Remediated code examples in library README.
> * **Changes:**
>   - Corrected the Write example in `opc-da-client/README.md` to use `as_deref().unwrap_or("Unknown error")` on `result.error` instead of `unwrap_or_default()`, avoiding empty strings on failure.
>   - Added clarifying comments to the Browse example in `README.md` to guide users on cloning `Arc` pointers (`progress` and `sink`) when performing concurrent tracking or timeout harvesting.
>   - Validated that all library doc-tests compile and pass successfully under the `verify.ps1` pipeline.
> * **New Constraints:** None.
> * **Pruned:** Outdated/incomplete API usage patterns in library documentation examples.

## 2026-07-26: TARS Summary — Win7 / Server 2008 R2 Compatibility Layer
> 📝 **Context Update:**
> * **Feature:** Windows 7 / Server 2008 R2 (NT 6.1) Compatibility Build & Packaging Layer
> * **Changes:**
>   - Implemented 3 `#![no_std]` standalone polyfill crates under `compat/`: `synch-polyfill` (`api-ms-win-core-synch-l1-2-0.dll`), `winrt-error-polyfill` (`api-ms-win-core-winrt-error-l1-1-0.dll`), and `bcrypt-polyfill` (`bcryptprimitives.dll`).
>   - Excluded `compat/*` from root `Cargo.toml` workspace members (`workspace.exclude = ["compat/*"]`) to prevent breaking `verify.ps1`/`commit.ps1` quality gates which invoke `--workspace`.
>   - Created `scripts/package-win7.ps1` automated legacy release pipeline (static CRT linking `+crt-static`, polyfill compilation via `--manifest-path`, PE binary patching `GetSystemTimePreciseAsFileTime` -> `GetSystemTimeAsFileTime`, redistributables bundling, and zip archiving).
>   - Upgraded `scripts/package.ps1` and `Makefile` with modern (`dist/opc-cli-x64.zip`) vs legacy (`dist/opc-cli-win7-x64.zip`) symmetric packaging targets.
>   - Updated `.gitignore` to allow tracking distribution outputs in `dist/` while adding `compat/` and `dist/` to `scripts/Merge-ToMain.ps1` strip list for clean production releases.
>   - Added `vendor/redist/README.md`, updated root `README.md` and `architecture.md` with legacy deployment guidance and system specifications.
> * **New Constraints:**
>   - Polyfill crates in `compat/` must remain standalone (`[workspace]` header in their `Cargo.toml` and listed in parent `workspace.exclude`) so they do not link into workspace `--workspace` test runs.
>   - Build legacy releases using `make package-win7` or `pwsh scripts/package-win7.ps1`.
> * **Pruned:** Manual PE patching and ad-hoc DLL copying.

## 2026-07-26: TARS Summary — Testing Infrastructure Architecture Compliance
> 📝 **Context Update:**
> * **Feature:** Remediated 7 qualitative review findings across testing infrastructure and verification pipeline.
> * **Changes:**
>   - Added `#[ignore = "TODO: ..."]` attributes to 6 empty test stubs in `opc-da-client/src/com_worker.rs` so `cargo test` honestly reports them as ignored rather than false passes.
>   - Deleted dead, un-linked legacy test file `opc-da-client/src/opc_da/client/tests.rs` and removed commented `// mod tests;` declaration from `mod.rs`.
>   - Added Gate 5 (`Polyfill Build: <crate>`) to `scripts/verify.ps1` to independently compile all polyfill crates in `compat/`, preventing silent breakage of `#![no_std]` crates.
>   - Added PE patch post-validation scan and polyfill DLL minimum file-size sanity check (4KB threshold) to `scripts/package-win7.ps1`.
>   - Added `make verify` target and clarifying comments to `Makefile`.
> * **New Constraints:**
>   - `verify.ps1` now validates both workspace crates and `compat/*` polyfill crates.
> * **Pruned:** Silent passing of empty test stubs and dead legacy test code.

## 2026-07-26: TARS Summary — ComWorker Unit Test Suite Implementation
> 📝 **Context Update:**
> * **Feature:** Implemented 100% active test coverage for the 6 previously ignored `ComWorker` unit tests.
> * **Changes:**
>   - Built `ConfigurableMockConnector`, `ConfigurableMockServer`, and `ConfigurableMockGroup` in `opc-da-client/src/com_worker.rs` using atomic state counters and configurable error/panic triggers.
>   - Implemented `test_worker_write_tag_value` verifying tag write dispatch and `WriteResult` output.
>   - Implemented `test_connection_cache_reuse` verifying server connection pooling across requests (`connect_count == 1`).
>   - Implemented `test_stale_connection_eviction` verifying automatic cache eviction and reconnection upon COM/RPC error (`connect_count == 2`).
>   - Implemented `test_worker_panic_propagation` verifying worker thread panic propagation to caller.
>   - Implemented `test_drop_during_active_request` verifying graceful worker shutdown.
>   - Implemented `test_worker_init_failure` verifying worker initialization error handling.
>   - Re-enabled all 6 test functions (0 ignored tests in `opc-da-client`, 37/37 unit tests passing).
> * **New Constraints:** None.
> * **Pruned:** `#[ignore]` attributes on `com_worker.rs` unit tests.

## 2026-07-26: TARS Summary — Unified Build & Automation Infrastructure
> 📝 **Context Update:**
> * **Feature:** Integrated and unified `Makefile` and `scripts/` ecosystem into a single delegated build pipeline.
> * **Changes:**
>   - Refactored `scripts/package.ps1` into a single task dispatcher with strict mode, `$RepoRoot` navigation, and full task coverage (`debug`, `release`, `build`, `test`, `verify`, `package`, `package-win7`, `logs`, `commit`, `release-merge`).
>   - Updated `Makefile` to delegate `package`, `package-win7`, `verify`, `logs`, `commit`, and `release-merge` directly to PowerShell scripts, eliminating divergent POSIX inline commands.
>   - Updated `architecture.md § Build System` to document the unified dual-interface build system.
>   - Validated that `make package` / `pwsh scripts/package.ps1 -Task package` produces `dist/opc-cli-x64.zip` cleanly and all 5 verification gates pass.
> * **New Constraints:**
>   - `scripts/package.ps1` is the single source of truth for task dispatching. `Makefile` delegates to it.
> * **Pruned:** Divergent POSIX `cp`/`tar` inline commands in `Makefile`.

## 2026-07-26: TARS Summary — Documentation Sync (`/update-doc`)
> 📝 **Context Update:**
> * **Feature:** Synchronized code documentation, rustdoc comments, and `spec.md` behavioral contracts.
> * **Changes:**
>   - Added rustdoc comments for `ComRequest` enum, `ComWorker` struct, and `ComWorker::start` method in `opc-da-client/src/com_worker.rs`.
>   - Updated verification hash in `opc-da-client/spec.md` to `e74ee22`.
>   - Updated test status for `quality_to_string` helper tests to `[x]` and added `ComWorker` thread dispatch unit test checklist entries to `opc-da-client/spec.md`.
>   - Verified alignment between `Cargo.toml` description, `lib.rs` / `main.rs` crate-level comments, and `README.md` files.
> * **New Constraints:**
>   - `opc-da-client/spec.md` verification hash recorded at commit `e74ee22`.
> * **Pruned:** Outdated verification hash and pending test checklist items in `spec.md`.

## 2026-07-26: TARS Summary — Architecture Specification Alignment (`/architecture`)
> 📝 **Context Update:**
> * **Feature:** Refactored `architecture.md` to achieve 100% compliance with `.agents/rules/architecture-rules.md`.
> * **Changes:**
>   - Restructured `Project Objectives & Key Features` into explicit `Primary Objectives`, `Key Features`, `Target Users / Audience`, and `Non-Goals` subsections.
>   - Added `Project Layout` directory tree mapping `opc-cli/`, `opc-da-client/`, `compat/`, `scripts/`, `.agents/`.
>   - Restructured `Module Boundaries` into explicit `Owns`, `Does NOT own`, `Trait Interfaces`, and `Mock Availability` declarations for all 4 key components (`opc-cli`, `opc-da-client`, `ComWorker`, `compat/*`).
>   - Added `Dependency Direction Rules` matrix table (§4).
>   - Added Mermaid `Error Propagation Flow` sequence diagram (§5).
>   - Added `Documentation Conventions` section (§1.11).
>   - Verified all 16 required sections are present and fully populated.
> * **New Constraints:**
>   - `architecture.md` fully satisfies all 16 governance section requirements.
> * **Pruned:** Informal module boundary descriptions in `architecture.md`.

## 2026-07-26: TARS Summary — License & Attribution Hygiene (`/review` + `/plan-making`)
> 📝 **Context Update:**
> * **Feature:** Established complete license compliance, upstream attribution, and package bundle license distribution across all crates.
> * **Changes:**
>   - Added provenance comment headers to `opc-da-client/src/bindings/da/mod.rs` and `comn/mod.rs` documenting origin from `Ronbb/rust_opc` (MIT, © 2025 Wang Ruobiao) and OPC Foundation IDLs.
>   - Created root-level `THIRD_PARTY_LICENSES.md` consolidating upstream MIT license text, OPC Foundation IDL credits, and dependency license references.
>   - Pruned stale `vendor/opc_classic_utils/` crate and `vendor/LICENSE`, updated `vendor/NOTICE` with correct frozen binding paths.
>   - Added `authors`, `license = "MIT"`, and `repository` metadata to all 3 polyfill `Cargo.toml` files in `compat/`.
>   - Added `[workspace.package]` to root `Cargo.toml` and inherited metadata in `opc-cli` and `opc-da-client` package declarations.
>   - Added `## 🙏 Acknowledgments` section to `README.md`.
>   - Updated `scripts/package.ps1` and `scripts/package-win7.ps1` to copy `LICENSE` and `THIRD_PARTY_LICENSES.md` into release ZIP bundles.
> * **New Constraints:**
>   - Release packages now automatically include `LICENSE` and `THIRD_PARTY_LICENSES.md`.
> * **Pruned:** Stale `vendor/opc_classic_utils/` directory and outdated `vendor/NOTICE` paths.

## 2026-07-26: TARS Summary — Unused Vendor Cleanup (`/plan-making`)
> 📝 **Context Update:**
> * **Feature:** Confirmed removal of all legacy vendored packages (`opc_da`, `opc_da_bindings`, `opc_comn_bindings`, `opc_classic_utils`), deleted obsolete `vendor/NOTICE`, and cleaned up `.gitignore`.
> * **Changes:**
>   - Deleted redundant `vendor/NOTICE` (superseded by `THIRD_PARTY_LICENSES.md` at root).
>   - Cleaned up duplicate typo rule `!.vendor/redist/*.msi` from `.gitignore`.
>   - Verified `vendor/redist/` remains active as the dedicated drop folder for Win7 redistributable MSIs.
> * **New Constraints:**
>   - Root `THIRD_PARTY_LICENSES.md` is the sole source of third-party notice data.
> * **Pruned:** `vendor/NOTICE` file.

## 2026-07-26: TARS Summary — Mechanical Verification Hardening (`/review` + `/plan-making` + `/build` + `/audit`)
> 📝 **Context Update:**
> * **Feature:** Hardened mechanical quality checks (`ast-grep` rules, 8-gate `verify.ps1` pipeline, safety rationale comments, and test suites).
> * **Changes:**
>   - Fixed `require-safety-comment.yml` `stopBy` semantics (`expression_statement`, `let_declaration`, `return_expression`) to eliminate distant `// SAFETY:` comment false negatives.
>   - Added `two_unsafe_blocks` invalid test case to `require-safety-comment-test.yml` proving distant comments are rejected.
>   - Extended `verify.ps1` Gate 7 ripgrep scan to iterate both `opc-da-client/src/` and `opc-cli/src/`.
>   - Added `sg test` execution to `verify.ps1` Gate 6 prior to `sg scan`.
>   - Added `unimplemented!` to `no-panic-or-unwrap.yml` AST rule, Gate 7 ripgrep pattern, and test suites.
>   - Refactored `verify.ps1` `Invoke-Gate` to use `[scriptblock]$Command` and `& $Command`, eliminating `Invoke-Expression` (PSScriptAnalyzer anti-pattern).
>   - Updated all `verify.ps1` call sites to pass script blocks.
> * **New Constraints:**
>   - `verify.ps1` Gate 6 automatically verifies AST rule unit tests before scanning code.
>   - `verify.ps1` Gate 7 checks both workspace member `src/` directories for forbidden macros (`println!`, `dbg!`, `todo!`, `unimplemented!`).
> * **Pruned:** `Invoke-Expression` string-eval in `verify.ps1`.

## 2026-07-26: TARS Summary — Fine-Grained Dev-Build Logging (`/brainstorm` + `/grill-me` + `/plan-making` + `/build`)
> 📝 **Context Update:**
> * **Feature:** Fine-grained dev-build logging with two-tier diagnostics (dev & field), compile-time `dev-diagnostics` feature flag, CLI `--verbose`/`-v`/`-vv` verbosity flag, structured error forensics, centralized state transition audit trail, `#[instrument]` adoption, and `check-logs.ps1` deep analysis modes.
> * **Changes:**
>   - Added `dev-diagnostics` feature to `opc-da-client/Cargo.toml` and passthrough to `opc-cli/Cargo.toml`.
>   - Added `log_opc_error(error, operation)` in `opc_da/errors.rs` emitting structured `tracing::error!` with named fields (`operation`, `hresult`, `hint`, `chain`). Re-exported in `lib.rs` and `helpers.rs`.
>   - Decorated `ComWorker::start()` and `send_request()` with `#[tracing::instrument]`.
>   - Added `#[cfg(feature = "dev-diagnostics")]` TRACE-level operation argument dumps to `com_worker.rs` handlers.
>   - Implemented `Display` for `CurrentScreen` enum in `app.rs`.
>   - Added centralized `log_transition(to, trigger)` helper on `App` and instrumented all 20 screen transition sites across `app.rs` and `main.rs`.
>   - Added `clap`-derived `Args` struct with `--verbose`/`-v` count flag in `main.rs`, mapping verbosity to `EnvFilter` levels (`info` -> `debug` -> `trace`).
>   - Enhanced `scripts/check-logs.ps1` with §E (HRESULT aggregation top 10) and §F (State Transition sequence anomaly detector).
> * **New Constraints:**
>   - Release builds default to `INFO` level logging unless activated via `-v`/`-vv` CLI flag or `RUST_LOG`.
>   - Screen transitions must go through `app.log_transition()` to ensure auditability.
>   - `check-logs.ps1` validates state transition sequence integrity during deep analysis.

## 2026-07-29: TARS Summary — Documentation Update (`/update-doc`)
> 📝 **Context Update:**
> * **Feature:** Documentation sync for fine-grained logging and CLI verbosity features.
> * **Changes:**
>   - Updated `opc-da-client/spec.md` with a recorded verification hash (`586a9d2`) corresponding to the latest source code commit.
>   - Added [`log_opc_error`](file:///c:/Users/WSALIGAN/code/opc-cli/opc-da-client/spec.md#L158-L170) to the public helpers API contract in `opc-da-client/spec.md`.
>   - Added the `dev-diagnostics` feature to the Feature Flags table in `opc-da-client/spec.md`.
>   - Updated workspace `README.md` **Build & Run** section to document TUI execution under `-v` and `-vv` logging verbosity arguments.

## 2026-07-29: TARS Summary — Architecture Synchronization (`/architecture` + `/plan-making` + `/build`)
> 📝 **Context Update:**
> * **Feature:** Synchronized `architecture.md` technical source of truth with fine-grained logging infrastructure, two-tier diagnostics, and verification pipeline hardening.
> * **Changes:**
>   - Updated Section 4 (Project Layout) tree to include `backend/` (`connector.rs`, `opc_da.rs`) and full `opc_da/` subtree (`errors.rs`, `com_utils.rs`, `typedefs.rs`, `client/` version subdirs).
>   - Updated Section 9 (Observability & Logging) to detail two-tier diagnostics (dynamic field `-v`/`-vv` vs. compile-time `dev-diagnostics`), `log_opc_error` structured logging, `App::log_transition()` state audits, `#[tracing::instrument]` timing, and `check-logs.ps1` §E & §F analysis modes.
>   - Updated Section 10 (Testing Strategy) to document AST-grep rule unit testing in Gate 6.
>   - Updated Section 12 (Dependencies & External Systems) with `dev-diagnostics` Cargo feature documentation.











