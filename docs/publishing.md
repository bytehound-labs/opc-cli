# Publishing `bytehound-opc-da-client`

`bytehound-opc-da-client` is published to crates.io by the manual workflow
`.github/workflows/publish-client.yml`. The workflow does not run on pull
requests and is not a required status check. The `opc-cli` package is
`publish = false` and is not part of this workflow.

The Actions UI selects the workflow definition. The `ref` input selects the
code that is tested. A real publish checks out the exact commit the verify job
tested, not a branch name that may have moved.

## Dispatch

Trigger **Publish ByteHound OPC DA client** with `workflow_dispatch`.

| Input | Default | Meaning |
| --- | --- | --- |
| `ref` | `main` | Git ref to test. A real publish uses the verified commit SHA. |
| `dry_run` | `true` | Validate only. A real publish requires `dry_run` set to `false`. |

A default dispatch runs `cargo test -p bytehound-opc-da-client --locked` and
`cargo publish -p bytehound-opc-da-client --locked --dry-run` on
`windows-latest`. It does not enter a GitHub Environment and does not read a
secret. Windows is the publish gate: non-Windows package checks do not
exercise the OPC DA backend.

## Real publish

The publish job runs only when `dry_run` is `false` and verification recorded
a commit SHA. That job is the only job that uses the GitHub Environment
`crates-publish`.

This workflow does not create `crates-publish` and does not provision
`CARGO_REGISTRY_TOKEN`. Before the first real publish, a maintainer creates
that environment, protects it with the required reviewers, and stores
`CARGO_REGISTRY_TOKEN` as an environment secret. GitHub creates a missing
environment the first time a job enters it, and that automatic environment has
no required reviewers. Create and protect the environment before any dispatch
with `dry_run` set to `false`.

The verify job stays outside the environment, so a dry run does not wait for
that approval and does not need the token.

crates.io trusted publishing is not configured. The publish step uses the
environment token. Do not grant `id-token: write` unless trusted publishing
replaces that token.

Concurrent dispatches queue. A later run does not cancel an in-flight publish.

The published crate version must not already exist on crates.io. This workflow
does not bump the version.

The cross-repo workflow
`bytehound-labs/opcda-bridge/.github/workflows/publish-opcda-client.yml`
remains until a dry run of this workflow has passed.
