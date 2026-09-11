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
    .search_services(
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
    .suggest(&SuggestionRequest {
        limit: 5,
        prefix: "pla".to_owned(),
    })
    .await?;
# Ok(())
# }
```

`search` returns one page. `continue_search` follows one opaque `next` reference.
`search_pages` and `search_services` perform bounded traversal for callers that want aggregation.
Search filters cover keywords, ODP operations, enrollment protocols, payment protocols, payment
options, trust protocols, and the authentication requirements attached to operations and payments.
Search responses can also carry facets for building data-driven filters without packaging the
Directory's current vocabulary into the Agent.
Each returned Service preserves its enrollment, payment, and trust protocol advertisements.
Unrecognized protocol descriptors are filtered while recognized descriptors remain strictly
validated.

See the [workspace guide](../../README.md) and the
[ODP specification](https://www.offeringprotocol.org/).
