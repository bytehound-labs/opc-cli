# Provenance

## Repository lineage

This repository is a GitHub fork of
[`wends155/opc-cli`](https://github.com/wends155/opc-cli). It retains the
upstream project's Windows OPC DA TUI and workspace history. The upstream
copyright notice remains in `LICENSE` alongside the notice for ByteHound
contributions.

## OPC DA bindings

The frozen generated OPC DA and OPC Common COM bindings are checked into
`opc-da-client/src/bindings/`. They originate from
[`Ronbb/rust_opc`](https://github.com/Ronbb/rust_opc), maintained by Wang
Ruobiao, and were generated using `windows-bindgen` 0.62.1 from the OPC
Foundation `opcda.idl` and `OPCComn.idl` interface definitions. The generated
bindings are vendored source, rather than a build-time dependency on
`rust_opc`; their MIT license and attribution are recorded in
`THIRD_PARTY_LICENSES.md`.

## Optional redistributable

`vendor/redist/` documents the optional OPC Core Components redistributable
used by the legacy Windows packaging flow. The repository contains the
instructions but not the MSI itself. The packaging script includes MSI files
placed in that directory; any such file must be sourced separately and
redistributed under its own terms.

## Fork-specific maintenance

The fork keeps the TUI package in the workspace, but `opc-cli/Cargo.toml`
sets `publish = false` because the crates.io package name `opc-cli` belongs to
the upstream project. The client library uses the separate Cargo package name
`bytehound-opc-da-client`, while its Rust library target remains
`opc_da_client`.

ByteHound-maintained client changes focus on scalable native OPC browsing and
inventory: bounded DA 2.x/3.0 traversal, exact ItemIDs, root-scoped inventory,
runtime pacing, and typed operation telemetry. The COM backend remains
Windows-specific, while the library can be checked on non-Windows systems
without compiling that backend. Release details are recorded in the root and
library `CHANGELOG.md` files.
