# odp-service

Service-side integration for the Offering Discovery Protocol.

This crate owns validated Service Documents, catalog operations, static catalogs for small
Services, and storage-backed operation boundaries for large catalogs. Its request and response
surface remains independent of Axum, Actix Web, and other application frameworks.

Implement `Catalog` for a database-backed or remote catalog. Small Services can use `StaticCatalog`,
which validates all resources during construction and supplies bounded, expiring, stateless
continuations protected against tampering.

```rust,no_run
use std::sync::Arc;

use odp_core::{Protocol, TrustProtocol, parse_offering};
use odp_service::{ServiceBuilder, StaticCatalog, StaticCatalogOptions};

let offering = parse_offering(
    br#"{"id":"rubber-plant","name":"Rubber Plant","odp_version":"1.0"}"#,
)?;
let catalog = StaticCatalog::new(StaticCatalogOptions {
    collections: Vec::new(),
    offerings: vec![offering],
})?;
let service = ServiceBuilder::new(
    "Indica Flowers",
    "An AI-enabled plant store.",
    "en",
    "/odp",
)
.keywords(["plants", "flowers"])
.trust(vec![TrustProtocol {
    name: Protocol::Tap,
}])
.build(Arc::new(catalog))?;
assert_eq!(service.document().name, "Indica Flowers");
# Ok::<(), Box<dyn std::error::Error>>(())
```

`ServiceBuilder` supplies the current ODP version, defaults the localizations to the Service
language, and derives advertised operations from the `Catalog`. Use
`operation_authentication` when a catalog operation requires or optionally accepts authentication.
ODP validation also requires the builder's `protocols` metadata to advertise enrollment whenever
an operation accepts or requires authentication. Use `trust` to advertise supported trust
protocols; the host application remains responsible for their implementation.

Service Document construction uses strict current-version validation. Unknown enrollment, payment,
and trust protocol names are rejected.

## Catalog contract

Every `Catalog` implements `list_offerings` and `get_offering`, and reports its supported operations
through `operations`. The remaining methods have an unsupported default and are implemented only
when the Service advertises the corresponding capability:

| Operation | Catalog method |
| --- | --- |
| Search Offerings | `search_offerings` |
| List Collections | `list_collections` |
| Get Collection | `get_collection` |
| Search Collections | `search_collections` |
| List Collection Offerings | `list_collection_offerings` |

The `CatalogRequest` provides the requested representation, page limit, continuation cursor,
language preference, and canonical request path. A storage-backed implementation applies those
values in its own query layer and returns typed ODP pages. `StaticCatalog` instead validates all
resources during construction, defaults pages to 50 resources, and issues stateless continuations
protected against tampering. They expire after one hour. The Service request boundary caps every
requested page at 100 resources.

Individual Offering and Collection retrieval defaults to full representation; lists and searches
default to terse. Search body limits are passed to the catalog and checked against its response.
For search continuations on `/offerings/search` or `/collections/search`, implement
`continue_offering_search` or `continue_collection_search`. These receive the opaque cursor in
`CatalogRequest` without a fabricated search query. Their defaults return `CONTINUATION_EXPIRED`.

`CatalogRequest.language` is the selected localization; `accept_language` retains the original
header. Use `ServiceError::retry_after` for failures with retry timing, or `ServiceError::request`
when no retry timing applies.

The storage boundary can wrap an existing asynchronous repository without coupling ODP to its
database:

```rust,no_run
use std::sync::Arc;

use async_trait::async_trait;
use odp_core::{Offering, OfferingPage, Operation};
use odp_service::{Catalog, CatalogRequest, ServiceError};

#[async_trait]
trait OfferingRepository: Send + Sync {
    async fn list(
        &self,
        request: CatalogRequest,
    ) -> Result<OfferingPage<Offering>, ServiceError>;

    async fn get(
        &self,
        id: &str,
        request: CatalogRequest,
    ) -> Result<Option<Offering>, ServiceError>;
}

struct RepositoryCatalog {
    repository: Arc<dyn OfferingRepository>,
}

#[async_trait]
impl Catalog for RepositoryCatalog {
    fn operations(&self) -> Vec<Operation> {
        vec![Operation::GetOffering, Operation::ListOfferings]
    }

    async fn list_offerings(
        &self,
        request: CatalogRequest,
    ) -> Result<OfferingPage<Offering>, ServiceError> {
        self.repository.list(request).await
    }

    async fn get_offering(
        &self,
        id: &str,
        request: CatalogRequest,
    ) -> Result<Option<Offering>, ServiceError> {
        self.repository.get(id, request).await
    }
}
```

Add optional methods and their corresponding `Operation` values together. This prevents the
Service Document from advertising a route that the repository adapter does not implement.

Adapt the framework-neutral `Request` and `Response` values at the application boundary. The
Service owns fixed methods, paths, media types, request bounds, response validation, and structured
problem responses; the host application retains authentication, payment, routing, and persistence.
Advertising an authentication or payment requirement does not enforce it. Apply the Service's AEP,
MPP, or x402 middleware before forwarding an authorized ODP request to `Service::handle`.

See the [runnable small Service](../../examples/odp-service-small) for an HTTP adapter, Collection
navigation, and a free download Action.

See the [workspace guide](../../README.md) and the
[ODP specification](https://www.offeringprotocol.org/).
