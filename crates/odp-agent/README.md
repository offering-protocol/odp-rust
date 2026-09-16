# odp-agent

Agent-side discovery and catalog navigation for the Offering Discovery Protocol.

This crate composes Directory discovery with validated Service inspection, Collection and Offering
navigation, resource-class caching, bounded concurrency, and non-invoking Action resolution. Its
asynchronous client provides a Rustls-backed HTTP transport and permits callers to inject a
compatible transport.

## Inspect and navigate one Service

```rust,no_run
use odp_agent::{ServiceClient, TraversalOptions};
use odp_core::Representation;

# #[tokio::main(flavor = "current_thread")]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {
let client = ServiceClient::new("https://demo.inflowpay.ai")?;
let inspection = client.inspect().await?;
println!("{}", inspection.document.name);
if let Some(protocols) = &inspection.document.protocols {
    println!("Trust protocols: {:?}", protocols.trust);
}

let offerings = client
    .list_all_offerings(
        Representation::Terse,
        50,
        TraversalOptions {
            max_items: 100,
            max_pages: 4,
        },
    )
    .await?;

let details = client.get_offering_details(&offerings[0].id).await?;
for action in &details.actions {
    println!("{}: {:?}", action.id, action.rel);
}
# Ok(())
# }
```

The client checks that a Service advertises an operation before calling it. Every returned resource
is validated. `ServiceClient` uses an in-memory cache by default; callers can inject a shared `Cache`,
set an authentication-aware cache partition, or override the Service Document, Collection, and
Offering fallback lifetimes.

Each client has an isolated cache partition, including when sharing a `Cache`. Use
`with_cache_partition` only to share responses between clients with the same authentication context.
Manual `continue_offerings` and `continue_collections` calls use HTTP freshness headers without a
fallback lifetime: an opaque continuation URL does not identify whether it originated in a search.
The automatic traversal methods retain the originating operation's fallback policy.

`ServiceClient::new` permits public destinations only. Use `ServiceClient::for_local_development`
for localhost examples. The default transport pins validated addresses for each request, disables
proxies, and checks the connected peer. An injected transport owns its network policy; wrapping it
in `SecureTransport` requires implementing `Transport::send_to` with equivalent address pinning and
peer verification. Its default implementation refuses the request rather than silently bypassing
those checks. Implement `send_limited` to enforce response limits while streaming; its default
implementation can only check the completed response.

`get_offering_details` bundles an Offering with its validated Attribute Schema, validates the
Offering attributes, and normalizes usable Action targets. `resolve_action` resolves an Action's
request schema or unique OpenAPI 3.1 operation without invoking the target. Supporting documents
are fetched anonymously over HTTPS with independent byte limits and cache entries. Attribute Schema
resolution accepts JSON Schema Draft 2020-12 and is limited to 256 KiB per document, 16 documents,
eight reference levels, and one MiB for the complete graph. OpenAPI documents are limited to one
MiB. These are fixed SDK safety ceilings. Cross-document schema composition uses `$ref`;
`$dynamicRef` accepts only a fragment reference such as `#node`.
Returned schemas include their referenced resources in `$defs` with absolute identifiers, so a
caller can use them without additional network access.

## Search across Services

Federated discovery uses `DirectoryClient::collect_services` and remains Service-only. For mixed
discovery, use `DirectoryClient::search`, inspect each Collection result's owning Service, then
call `ServiceClient::get_collection` with its Collection ID. Collection results are not separate
Services. See the [Directory guide](../odp-directory/README.md#search-services-and-collections).

```rust,no_run
use odp_agent::{Agent, FederatedSearchRequest};
use odp_core::{OfferingSearchRequest, VERSION};
use odp_directory::{Environment, SearchRequest};

# #[tokio::main(flavor = "current_thread")]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {
let agent = Agent::new(Environment::Production)?;
let events = agent
    .search_offerings_across_services(&FederatedSearchRequest {
        concurrency: 4,
        max_offerings_per_service: 25,
        max_services: 10,
        offerings: OfferingSearchRequest {
            odp_version: VERSION.to_owned(),
            query: "indoor plant".to_owned(),
            ..OfferingSearchRequest::default()
        },
        services: SearchRequest {
            query: "plants".to_owned(),
            ..SearchRequest::default()
        },
    })
    .await?;

for event in events {
    match (event.offering, event.issue) {
        (Some(offering), _) => println!("{}: {}", event.service.name, offering.name),
        (_, Some(issue)) => eprintln!("{}: {issue}", event.service.name),
        _ => {}
    }
}
# Ok(())
# }
```

The Agent searches the canonical Directory, then queries the selected Services with bounded
concurrency. Result order remains deterministic. Each `DiscoveryEvent` contains either an Offering
or a Service-specific issue, allowing one unavailable Service to be reported without failing every
successful Service. When the Offering request has no query, filters, refinements, sort, descendant
selection, or Collection identifier, the Agent lists Offerings instead of requiring Service-side
search support.

## Actions and protocol composition

`get_offering_details` returns normalized, usable Action targets and structured issues separately
from the Offering. `resolve_action` can resolve a request schema or a unique OpenAPI 3.1 operation,
but it never invokes the target. The application uses each Action's authentication requirement and
the Service Document's enrollment, payment, and trust advertisements to compose the necessary
protocol clients before making the resolved HTTP request.

Inspection filters unrecognized enrollment, payment, and trust descriptors for compatible Agent
processing. Recognized descriptors remain subject to current-version validation.

See the [runnable Agent example](../../examples/odp-agent-discovery) for Directory composition,
Collection navigation, full Offering details, and Action resolution.

See the [workspace guide](../../README.md) and the
[ODP specification](https://www.offeringprotocol.org/).
