# Offering Discovery Protocol for Rust

[![CI](https://github.com/offering-protocol/odp-rust/actions/workflows/ci.yml/badge.svg)](https://github.com/offering-protocol/odp-rust/actions/workflows/ci.yml)
[![Rust](https://img.shields.io/badge/Rust-1.85%2B-000000?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](./LICENSE)

Official Rust software development kit for the
[Offering Discovery Protocol](https://www.offeringprotocol.org/), the open protocol for discovering
Services and navigating their Offerings.

ODP separates Service discovery from catalog discovery. An Agent searches the canonical directory
for candidate Services, inspects each Service's live ODP document, and then navigates or searches
that Service's Collections and Offerings.

`DirectoryClient::search` discovers indexed Services and submitted Collections. Use
`service.source` to distinguish native ODP from imported OpenAPI documents and retain the exact
discovery URL. Search and suggestions accept source filters. Imported Collections are Directory
groups and must not be passed to ODP operations. Use
`search_services` for a native ODP Service-only response or `collect_services` for bounded Service-only
aggregation. `suggest` returns mixed target names; `suggest_services` returns Service-only
keyword suggestions. See the [Directory guide](./crates/odp-directory/README.md) for result types,
the 100-result cap, and migration from the earlier method names.

## Workspace

| Goal                                               | Crate           | Guide                                      |
| -------------------------------------------------- | --------------- | ------------------------------------------ |
| Work with protocol models and validation           | `odp-core`      | [Protocol core](./crates/odp-core)         |
| Search the canonical directory                     | `odp-directory` | [Directory client](./crates/odp-directory) |
| Discover Services and navigate their catalogs      | `odp-agent`     | [Agent integration](./crates/odp-agent)    |
| Publish an ODP Service                             | `odp-service`   | [Service integration](./crates/odp-service) |

The dependency direction is intentionally narrow: Core remains transport-independent, Directory
depends toward Core, Agent composes Core and Directory, and Service depends toward Core without
depending on Agent behavior.

Agent and Directory networking is asynchronous. The role crates provide an ergonomic Rustls-backed
HTTP transport while preserving an injectable transport boundary. Service integration remains
independent of a particular Rust web framework.

## Installation

An Agent that searches the Directory and navigates Service catalogs uses:

```toml
[dependencies]
odp-agent = "0.1.1"
odp-core = "0.1.1"
odp-directory = "0.1.1"
```

A Service that publishes its own catalog uses:

```toml
[dependencies]
odp-core = "0.1.1"
odp-service = "0.1.1"
```

Declare only the crates whose public types the application names. The individual crate guides
contain executable API examples.

## Choose an integration path

An Agent normally begins with the canonical Directory, then connects directly to the selected
Services. [`odp-agent`](./crates/odp-agent) demonstrates federated search, direct Collection and
Offering navigation, full Offering details, and Action resolution.

A Service implements the required Offering operations through a `Catalog`. Small catalogs can use
the validated in-memory `StaticCatalog`; larger integrations implement the same framework-neutral
trait over their existing storage. [`odp-service`](./crates/odp-service) demonstrates both paths.

## Protocol composition

ODP discovers what a Service offers and how an Agent can act on an Offering. A Service Document and
its Actions can advertise AEP enrollment, MPP or x402 payment requirements, and TAP trust support,
but ODP does not create credentials, invoke Actions, submit payments, or implement trust protocols.
The application composes the appropriate protocol clients around the resolved ODP Action target.

## Protocol behavior

- The Directory client has fixed production and sandbox origins. Callers cannot supply an arbitrary
  Directory endpoint.
- Agent responses are schema-validated before being returned. Service Documents, Collections, and
  Offerings use separate default cache lifetimes and support HTTP revalidation.
- Directory and Agent traversal follow opaque `next` references with explicit bounds of 16
  pages and 10,000 resources.
- Federated Agent discovery searches Services concurrently and returns results in Directory
  order. A failure from one Service becomes an issue event instead of discarding other results.
- The Service crate validates configuration and responses but does not impose a web framework or
  storage model. `StaticCatalog` provides the small-catalog integration path.

## Development

Rust 1.85 or newer is required. The repository toolchain follows current stable Rust while continuous
integration also verifies the minimum supported compiler.

Run the complete merge gate with:

```sh
make verify
```

Format source files with:

```sh
make format
```

The merge gate checks formatting, Clippy with warnings denied, tests, Rust documentation, and the
contents and metadata of every publishable crate.

See [`odp-specs`](https://github.com/offering-protocol/odp-specs) for the normative draft, schemas,
examples, and test vectors.

Runnable Agent and Service integrations are available in [`examples`](examples/). The shared ODP
conformance harness and the real Node Service interoperability check can be run with:

```sh
make conformance
make interoperability
```

These development checks use sibling `odp-specs` and `odp-node` clones by default. Set
`ODP_SPECS_DIR` or `ODP_NODE_DIR` when those repositories live elsewhere.

## Security

See [SECURITY.md](./SECURITY.md) for vulnerability reporting.

## Releases

All four crates use one workspace version. Maintainers run the `Release` workflow from `main`; it
verifies the workspace, clean consumption, shared conformance, and Node.js interoperability before
publishing crates in dependency order. Crates.io Trusted Publishing supplies a temporary workflow
credential, and the resulting archives receive GitHub build-provenance attestations before the
workflow creates the matching tag and GitHub release.

## License

MIT.
