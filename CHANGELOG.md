# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Added runtime inventory batch-size control for bounded native browse slices.

## [0.2.1] - 2026-08-12

### Fixed
- **docs.rs Build Failure**: Added `cargo-args = ["--bin", "opc-cli"]` to `[package.metadata.docs.rs]` in `opc-cli/Cargo.toml` so docs.rs explicitly targets the binary executable (`src/main.rs`) instead of defaulting to a non-existent `--lib` target.

## [0.2.0] - 2026-08-12

### Added
- **crates.io Publication**: Published `opc-cli` and `opc-da-client` to `crates.io`.
- **Standalone `windows-rs` Integration**: `opc-da-client` fully internalized raw `windows-bindgen` COM definitions into `src/bindings/`, eliminating external unmaintained bindings crates.
- **Windows 7 / Server 2008 R2 Polyfill Bundle**: Built legacy release packaging pipeline (`package-win7`) producing static PE binaries bundled with `#![no_std]` polyfills (`WaitOnAddress`, `RtlGenRandom`, WinRT error stubs).
- **Quality Pipeline & Automation**: Added PowerShell-driven automated quality verification (`verify.ps1`), release packager (`package.ps1`), and clean merge workflow (`Merge-ToMain.ps1`).

### Changed
- Synchronized workspace versions for `opc-cli` and `opc-da-client` at `0.2.0`.
- Internalized `ComGuard` into `opc-da-client` crate-private scope. COM MTA apartment initialization and thread affinity are managed transparently by a background worker thread.
- Enforced LF line endings across Rust source files with `.gitattributes` and `rustfmt.toml`.
- Replaced machine-specific MSVC linker overrides in `.cargo/config.toml` with standard Cargo MSVC toolchain auto-discovery.

### Fixed
- Fixed phantom `E_POINTER` error cascades from null PWSTR values in `StringIterator` (OPC-BUG-001).
- Prevented potential silent array truncation in OPC DA tag read/write handling.
