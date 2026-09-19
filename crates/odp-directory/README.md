# odp-directory

Canonical directory client for the Offering Discovery Protocol.

This crate owns fixed production and sandbox Directory access, validated Service search results,
facets, suggestions, and opaque continuation traversal. Its asynchronous client provides a
Rustls-backed HTTP transport and permits callers to inject a compatible transport.

Use `Environment::Production` for `https://api.inflowpay.ai` or `Environment::Sandbox` for
`https://sandbox.inflowpay.ai`. These are the only Directory origins accepted by the client.

```rust,no_run
use odp_core::{PaymentOption, Protocol};
use odp_directory::{
    DirectoryClient, Environment, IterationOptions, PaymentFilter, SearchRequest, ServiceFilters,
    SuggestionRequest,
};

# #[tokio::main(flavor = "current_thread")]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {
let directory = DirectoryClient::new(Environment::Production)?;
let services = directory
    .collect_services(
        &SearchRequest {
            filters: Some(ServiceFilters {
                payments: vec![PaymentFilter {
                    authentication: None,
                    name: Protocol::Mpp,
                    options: vec![PaymentOption::Solana],
                }],
                ..ServiceFilters::default()
            }),
            query: "plants".to_owned(),
            ..SearchRequest::default()
        },
        IterationOptions {
            max_items: 10,
            max_pages: 2,
        },
    )
    .await?;

for service in services {
    println!("{}: {}", service.name, service.service_origin);
}

let suggestions = directory
    .suggest_services(&SuggestionRequest {
        limit: 5,
        prefix: "pla".to_owned(),
        ..Default::default()
    })
    .await?;
# Ok(())
# }
```

`search_services` returns one Service-only response. `continue_search_services` follows one opaque
`next` reference. `collect_services` performs bounded Service-only traversal, stopping at the
response or item limit without fetching another response.
Search filters cover keywords, ODP operations, enrollment protocols, payment protocols, payment
options, trust protocols, and the authentication requirements attached to operations and payments.
Search responses can also carry facets for building data-driven filters without packaging the
Directory's current vocabulary into the Agent.
Each returned Service preserves its enrollment, payment, and trust protocol advertisements.
Unrecognized protocol descriptors are filtered while recognized descriptors remain strictly
validated.

See the [workspace guide](../../README.md) and the
[ODP specification](https://www.offeringprotocol.org/).

## Search Services and Collections

```rust,no_run
use odp_directory::{DirectoryClient, DirectoryResult, Environment, ResourceSearchRequest};

# #[tokio::main(flavor = "current_thread")]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {
let directory = DirectoryClient::new(Environment::Production)?;
let response = directory.search(&ResourceSearchRequest {
    query: "weather forecast".to_owned(),
    limit: 25,
    ..ResourceSearchRequest::default()
}).await?;
for item in response.items {
    match item {
        DirectoryResult::Service(item) => println!("Service: {}", item.service.name),
        DirectoryResult::Collection(item) => println!("Collection: {} ({}, through {})",
            item.collection.name, item.collection.id, item.service.service_origin),
        DirectoryResult::Unknown { kind, .. } => println!("Unsupported result type: {kind}"),
    }
}
for issue in response.issues {
    eprintln!("Skipped result {}: {}", issue.index, issue.message);
}
# Ok(())
# }
```

`ResourceSearchRequest.types` can restrict results to `ResultType::Service` or
`ResultType::Collection`. `None` selects both; explicit lists must be nonempty and distinct.
Filters apply to the owning Service. An empty query is omitted, allowing browsing.

Collection identity is its owning Service origin plus its case-sensitive Collection ID.
The result's `indexed_at` describes the Collection's freshness; `service.indexed_at` describes
the parent's freshness. Both are timestamp strings. `service.service_id()` identifies the
Directory's Service record. Service results may include `available_through` platform attribution;
Collection attribution is the owning `service` itself.

Inspect the owning Service's live ODP document, then use the Agent client's `get_collection` to
retrieve current details. Directory metadata is not authority to execute an Action or send
credentials. Unknown future result types retain their full raw JSON and are not interpreted as
Services. Malformed known results become indexed `issues` without discarding valid results.
Additional fields are retained in `additional` maps.

Mixed search returns at most 100 results. `limit: 0` omits the limit, using the server's default
of 100. The server does not currently offer continuation: absent `next` does not mean every match
was returned. `continue_search` accepts an opaque same-origin continuation if one is supplied.
Each call returns one response. Facets count all matching targets, not just returned items;
a Service and two Collections count as three. Collection search does not depend on permission
to display its card on the Directory landing page.

`suggest` sends POST `/v1/directory/suggestions`. `SuggestionRequest.filters` accepts the same
`ServiceFilters` as search, including AEP, keywords, ODP operations, payments and trust.
Collection filters apply to their owning Service. Matching spans names, descriptions and keywords,
but output contains deduplicated **names of matching Services and Collections**. Despite the
argument name `prefix`, matching uses substrings and whitespace-separated alternative terms.
`suggest_services` uses GET `/v1/services/suggestions` for Service-only keyword-prefix suggestions
and does not accept filters.
Both return strings from the server's `items` array; the default and maximum limit are 25.

See the [canonical Directory example](../../examples/README.md#canonical-directory-discovery).

## Migration

- Service-only `search` calls become `search_services`; `continue_search` calls become
  `continue_search_services`.
- Aggregating `search_services(request, options)` calls become `collect_services(request, options)`.
- `search_pages` is removed. To retain individual responses, call `search_services` followed by
  `continue_search_services` with an explicit application limit.
- Service-only `suggest` calls become `suggest_services`.
- `search`, `continue_search`, and `suggest` select mixed discovery.
