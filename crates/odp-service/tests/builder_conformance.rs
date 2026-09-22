//! SVC-06/21/29/41/65, PAG-19/20/23/26: the Service Document a builder produces, and the
//! continuations a static catalog signs.

mod support;

use std::sync::Arc;

use async_trait::async_trait;
use odp_core::{
    AuthenticationRequirement, Collection, EnrollmentProtocol, McpEndpoint, McpEndpointType,
    Offering, OfferingPage, Operation, Page, Protocol, Representation, ServiceBranding,
    ServiceBrandingImage, ServiceBrandingImageType, ServiceOpenApi, TrustProtocol,
    parse_collection, parse_offering, parse_service_document,
};
use odp_service::{
    Catalog, CatalogRequest, MEDIA_TYPE, Request, Service, ServiceBuilder, ServiceError,
    StaticCatalog, StaticCatalogOptions,
};
use support::{Stub, get, offering_page, query, service, with_header};

/// SVC-06: a Service Document carries every optional member the builder was given.
#[tokio::test]
async fn publishes_every_optional_member_it_was_given() {
    let service = ServiceBuilder::new("Plants", "Plants for agents.", "en", "/odp")
        .branding(ServiceBranding {
            icon: ServiceBrandingImage {
                media_type: Some(ServiceBrandingImageType::Svg),
                src: "https://plants.example/icon.svg".to_owned(),
            },
            logo: ServiceBrandingImage {
                media_type: Some(ServiceBrandingImageType::Png),
                src: "https://plants.example/logo.png".to_owned(),
            },
        })
        .mcp(vec![McpEndpoint {
            description: "Plants over MCP".to_owned(),
            endpoint_type: McpEndpointType::StreamableHttp,
            name: "Plants".to_owned(),
            url: "https://plants.example/mcp".to_owned(),
        }])
        .openapi(ServiceOpenApi {
            url: "https://plants.example/openapi.json".to_owned(),
        })
        .protocols(
            vec![EnrollmentProtocol {
                name: Protocol::Aep,
            }],
            Vec::new(),
        )
        .trust(vec![TrustProtocol {
            name: Protocol::Tap,
        }])
        .build(Stub::serving(1))
        .unwrap();

    let response = service.handle(get("/.well-known/odp")).await;
    assert_eq!(response.status, 200);

    let document = parse_service_document(&response.body).unwrap();
    assert!(document.branding.is_some());
    assert_eq!(document.mcp.len(), 1);
    assert_eq!(
        document
            .http
            .openapi
            .as_ref()
            .map(|value| value.url.as_str()),
        Some("https://plants.example/openapi.json")
    );
    let protocols = document.protocols.as_ref().unwrap();
    assert_eq!(protocols.enrollment.len(), 1);
    assert_eq!(protocols.trust.len(), 1);
}

/// FLT: search capabilities travel with the document when the Service declares them.
#[tokio::test]
async fn publishes_the_search_capabilities_it_declares() {
    let capabilities = serde_json::from_str(
        r#"{"filters":{"inline":[{"description":"The colour","id":"colour","operators":["eq"],"title":"Colour","type":"string"}]}}"#,
    )
    .unwrap();
    let service = ServiceBuilder::new("Plants", "Plants for agents.", "en", "/odp")
        .search_capabilities(capabilities)
        .build(Stub::serving(1))
        .unwrap();

    let document =
        parse_service_document(&service.handle(get("/.well-known/odp")).await.body).unwrap();
    assert!(document.search_capabilities.is_some());
}

/// SVC-77: the builder sets an operation's authentication once, however often it is named.
#[test]
fn keeps_one_descriptor_per_operation() {
    let service = ServiceBuilder::new("Plants", "Plants for agents.", "en", "/odp")
        .operation_authentication(Operation::GetOffering, AuthenticationRequirement::Optional)
        .operation_authentication(Operation::GetOffering, AuthenticationRequirement::Required)
        .protocols(
            vec![EnrollmentProtocol {
                name: Protocol::Aep,
            }],
            Vec::new(),
        )
        .build(Stub::serving(1))
        .unwrap();

    let document = service.document();
    let descriptors = document
        .operations
        .iter()
        .filter(|descriptor| descriptor.name == Operation::GetOffering)
        .collect::<Vec<_>>();
    assert_eq!(descriptors.len(), 1);
    assert_eq!(
        descriptors[0].authentication,
        AuthenticationRequirement::Required
    );
}

/// A Service Document the parser refuses is a configuration error, not a served document.
#[test]
fn refuses_a_document_its_own_parser_would_not_accept() {
    let Err(error) = ServiceBuilder::new("Plants", "Plants for agents.", "en", "not-a-path")
        .build(Stub::serving(1))
    else {
        panic!("an endpoint base that is not a path is not configurable");
    };
    assert!(matches!(error, ServiceError::InvalidConfiguration(_)));
}

// -- the operations a catalog declines ------------------------------------------------------

/// A catalog advertising an operation it never implemented fails that request, not the Service.
#[tokio::test]
async fn reports_each_operation_the_catalog_left_unimplemented() {
    struct Baseline;

    #[async_trait]
    impl Catalog for Baseline {
        fn operations(&self) -> Vec<Operation> {
            support::ALL_OPERATIONS.to_vec()
        }

        async fn list_offerings(
            &self,
            _request: CatalogRequest,
        ) -> Result<OfferingPage<Offering>, ServiceError> {
            Ok(offering_page(Vec::new(), ""))
        }

        async fn get_offering(
            &self,
            _id: &str,
            _request: CatalogRequest,
        ) -> Result<Option<Offering>, ServiceError> {
            Ok(None)
        }
    }

    let service = Service::new(support::document(&["en"]), Arc::new(Baseline)).unwrap();
    let search = Request {
        body: br#"{"odp_version":"1.0","query":"plants"}"#.to_vec(),
        headers: std::collections::BTreeMap::from([
            ("accept".to_owned(), MEDIA_TYPE.to_owned()),
            ("content-type".to_owned(), MEDIA_TYPE.to_owned()),
        ]),
        method: "POST".to_owned(),
        path: "/odp/offerings/search".to_owned(),
        ..Request::default()
    };

    for request in [
        search.clone(),
        Request {
            path: "/odp/collections/search".to_owned(),
            ..search
        },
        get("/odp/collections"),
        get("/odp/collections/plants"),
        get("/odp/collections/plants/offerings"),
    ] {
        let path = request.path.clone();
        let response = service.handle(request).await;
        assert_eq!(response.status, 500, "{path}");
    }
}

/// A catalog answering with a Collection other than the one asked for is not published.
#[tokio::test]
async fn refuses_a_collection_that_does_not_match_the_path() {
    struct Wrong;

    #[async_trait]
    impl Catalog for Wrong {
        fn operations(&self) -> Vec<Operation> {
            support::ALL_OPERATIONS.to_vec()
        }

        async fn list_offerings(
            &self,
            _request: CatalogRequest,
        ) -> Result<OfferingPage<Offering>, ServiceError> {
            Ok(offering_page(Vec::new(), ""))
        }

        async fn get_offering(
            &self,
            _id: &str,
            _request: CatalogRequest,
        ) -> Result<Option<Offering>, ServiceError> {
            Ok(None)
        }

        async fn get_collection(
            &self,
            _id: &str,
            _request: CatalogRequest,
        ) -> Result<Option<Collection>, ServiceError> {
            Ok(Some(support::collection("somebody-else")))
        }

        async fn list_collections(
            &self,
            _request: CatalogRequest,
        ) -> Result<Page<Collection>, ServiceError> {
            Ok(support::collection_page(Vec::new(), ""))
        }
    }

    let service = Service::new(support::document(&["en"]), Arc::new(Wrong)).unwrap();
    let response = service.handle(get("/odp/collections/plants")).await;
    assert_eq!(response.status, 500);
}

// -- language edge cases --------------------------------------------------------------------

/// Lookup skips a single-character subtag, which is an extension prefix rather than a tag.
#[tokio::test]
async fn skips_an_extension_prefix_while_truncating() {
    let service = support::localized(Stub::serving(1), &["en", "zh-Hant"]);
    let response = service
        .handle(with_header(
            get("/.well-known/odp"),
            "accept-language",
            "zh-Hant-a-myext-x-private",
        ))
        .await;
    assert_eq!(
        response.headers.get("content-language").map(String::as_str),
        Some("zh-Hant")
    );
}

/// An empty range in the field is ignored rather than matched.
#[tokio::test]
async fn ignores_an_empty_language_range() {
    let service = support::localized(Stub::serving(1), &["en", "fr"]);
    for accept in [", fr", "  ,fr", ",,"] {
        let response = service
            .handle(with_header(
                get("/.well-known/odp"),
                "accept-language",
                accept,
            ))
            .await;
        assert_eq!(response.status, 200, "{accept}");
    }
}

// -- the static catalog's configuration and cursors ------------------------------------------

/// IDN-15: one identifier names one resource of a type, so a repeat is a configuration error.
#[test]
fn refuses_a_static_catalog_that_repeats_an_identifier() {
    let offering =
        parse_offering(br#"{"id":"plant-1","name":"Plant","odp_version":"1.0"}"#).unwrap();
    let Err(error) = StaticCatalog::new(StaticCatalogOptions {
        collections: Vec::new(),
        offerings: vec![offering.clone(), offering],
    }) else {
        panic!("one identifier names one Offering");
    };
    assert!(matches!(error, ServiceError::InvalidConfiguration(_)));

    let collection =
        parse_collection(br#"{"id":"plants","name":"Plants","odp_version":"1.0"}"#).unwrap();
    assert!(
        StaticCatalog::new(StaticCatalogOptions {
            collections: vec![collection.clone(), collection],
            offerings: Vec::new(),
        })
        .is_err()
    );
}

/// COL-31: every Collection an Offering names has to exist.
#[test]
fn refuses_an_offering_naming_a_collection_that_is_not_there() {
    let Err(error) = StaticCatalog::new(StaticCatalogOptions {
        collections: Vec::new(),
        offerings: vec![
            parse_offering(
                br#"{"collection_ids":["absent"],"id":"plant-1","name":"Plant","odp_version":"1.0"}"#,
            )
            .unwrap(),
        ],
    }) else {
        panic!("a Collection an Offering names has to exist");
    };
    assert!(matches!(error, ServiceError::InvalidConfiguration(_)));
}

/// COL-29: the inverse query answers only for a Collection that exists.
#[tokio::test]
async fn reports_an_inverse_query_for_a_collection_that_is_not_there() {
    let catalog = StaticCatalog::new(StaticCatalogOptions {
        collections: vec![
            parse_collection(br#"{"id":"plants","name":"Plants","odp_version":"1.0"}"#).unwrap(),
        ],
        offerings: Vec::new(),
    })
    .unwrap();

    let error = catalog
        .list_collection_offerings("absent", CatalogRequest::default())
        .await
        .unwrap_err();
    assert!(
        matches!(error, ServiceError::Request { status: 404, .. }),
        "{error}"
    );
}

/// PAG-26: a cursor is untrusted input, and is bound to the traversal that issued it.
#[tokio::test]
async fn refuses_a_cursor_from_another_traversal() {
    let catalog = StaticCatalog::new(StaticCatalogOptions {
        collections: Vec::new(),
        offerings: (0..3)
            .map(|index| {
                parse_offering(
                    format!(r#"{{"id":"p{index}","name":"Plant","odp_version":"1.0"}}"#).as_bytes(),
                )
                .unwrap()
            })
            .collect(),
    })
    .unwrap();

    let first = catalog
        .list_offerings(CatalogRequest {
            limit: 1,
            path: "/odp/offerings".to_owned(),
            ..CatalogRequest::default()
        })
        .await
        .unwrap();
    let cursor = url::form_urlencoded::parse(first.next.split_once('?').unwrap().1.as_bytes())
        .find_map(|(name, value)| (name == "cursor").then(|| value.into_owned()))
        .unwrap();

    // A cursor carries the limit, path and representation it was issued for, and only answers
    // for that traversal.
    for request in [
        CatalogRequest {
            cursor: Some(cursor.clone()),
            limit: 2,
            path: "/odp/offerings".to_owned(),
            ..CatalogRequest::default()
        },
        CatalogRequest {
            cursor: Some(cursor.clone()),
            limit: 1,
            path: "/odp/collections".to_owned(),
            ..CatalogRequest::default()
        },
        CatalogRequest {
            cursor: Some(cursor.clone()),
            limit: 1,
            path: "/odp/offerings".to_owned(),
            representation: Representation::Full,
            ..CatalogRequest::default()
        },
        // PAG-23: the signature is what makes the cursor usable at all.
        CatalogRequest {
            cursor: Some("not-a-cursor".to_owned()),
            limit: 1,
            path: "/odp/offerings".to_owned(),
            ..CatalogRequest::default()
        },
        CatalogRequest {
            cursor: Some("payload.c2lnbmF0dXJl".to_owned()),
            limit: 1,
            path: "/odp/offerings".to_owned(),
            ..CatalogRequest::default()
        },
    ] {
        assert!(catalog.list_offerings(request).await.is_err());
    }
}

/// A cursor another catalog signed is not this catalog's cursor.
#[tokio::test]
async fn refuses_a_cursor_another_catalog_signed() {
    let options = || StaticCatalogOptions {
        collections: Vec::new(),
        offerings: (0..3)
            .map(|index| {
                parse_offering(
                    format!(r#"{{"id":"p{index}","name":"Plant","odp_version":"1.0"}}"#).as_bytes(),
                )
                .unwrap()
            })
            .collect(),
    };
    let first = StaticCatalog::new(options()).unwrap();
    let second = StaticCatalog::new(options()).unwrap();

    let page = first
        .list_offerings(CatalogRequest {
            limit: 1,
            path: "/odp/offerings".to_owned(),
            ..CatalogRequest::default()
        })
        .await
        .unwrap();
    let cursor = url::form_urlencoded::parse(page.next.split_once('?').unwrap().1.as_bytes())
        .find_map(|(name, value)| (name == "cursor").then(|| value.into_owned()))
        .unwrap();

    assert!(
        second
            .list_offerings(CatalogRequest {
                cursor: Some(cursor),
                limit: 1,
                path: "/odp/offerings".to_owned(),
                ..CatalogRequest::default()
            })
            .await
            .is_err()
    );
}

/// PAG-12: the continuation preserves everything needed to continue the same operation.
#[tokio::test]
async fn carries_the_traversal_forward_in_its_continuation() {
    let catalog = StaticCatalog::new(StaticCatalogOptions {
        collections: Vec::new(),
        offerings: (0..3)
            .map(|index| {
                parse_offering(
                    format!(r#"{{"id":"p{index}","name":"Plant","odp_version":"1.0"}}"#).as_bytes(),
                )
                .unwrap()
            })
            .collect(),
    })
    .unwrap();

    let page = catalog
        .list_offerings(CatalogRequest {
            limit: 2,
            path: "/odp/offerings".to_owned(),
            representation: Representation::Full,
            ..CatalogRequest::default()
        })
        .await
        .unwrap();

    assert!(page.next.starts_with("/odp/offerings?"));
    assert!(page.next.contains("limit=2"));
    assert!(page.next.contains("representation=full"));
}

/// With no limit asked for, the catalog chooses its own page size.
#[tokio::test]
async fn chooses_its_own_page_size() {
    let catalog = StaticCatalog::new(StaticCatalogOptions {
        collections: Vec::new(),
        offerings: (0..60)
            .map(|index| {
                parse_offering(
                    format!(r#"{{"id":"p{index}","name":"Plant","odp_version":"1.0"}}"#).as_bytes(),
                )
                .unwrap()
            })
            .collect(),
    })
    .unwrap();

    let page = catalog
        .list_offerings(CatalogRequest {
            path: "/odp/offerings".to_owned(),
            ..CatalogRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(page.items.len(), 50);
    assert!(!page.next.is_empty());
}

/// A static catalog with no Collections advertises no Collection operations.
#[test]
fn advertises_collection_operations_only_when_it_has_collections() {
    let bare = StaticCatalog::new(StaticCatalogOptions::default()).unwrap();
    assert_eq!(bare.operations().len(), 2);

    let with_collections = StaticCatalog::new(StaticCatalogOptions {
        collections: vec![
            parse_collection(br#"{"id":"plants","name":"Plants","odp_version":"1.0"}"#).unwrap(),
        ],
        offerings: Vec::new(),
    })
    .unwrap();
    assert_eq!(with_collections.operations().len(), 5);
}

/// A Collection list is paged the same way an Offering list is.
#[tokio::test]
async fn pages_collections_too() {
    let catalog = StaticCatalog::new(StaticCatalogOptions {
        collections: (0..3)
            .map(|index| {
                parse_collection(
                    format!(r#"{{"id":"c{index}","name":"Collection","odp_version":"1.0"}}"#)
                        .as_bytes(),
                )
                .unwrap()
            })
            .collect(),
        offerings: Vec::new(),
    })
    .unwrap();

    let page = catalog
        .list_collections(CatalogRequest {
            limit: 2,
            path: "/odp/collections".to_owned(),
            ..CatalogRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(page.items.len(), 2);
    assert!(!page.next.is_empty());

    assert!(
        catalog
            .get_collection("c0", CatalogRequest::default())
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        catalog
            .get_collection("absent", CatalogRequest::default())
            .await
            .unwrap()
            .is_none()
    );
}

/// The inverse query returns the Offerings that name the Collection, and no others.
#[tokio::test]
async fn answers_the_inverse_query() {
    let catalog = StaticCatalog::new(StaticCatalogOptions {
        collections: vec![
            parse_collection(br#"{"id":"plants","name":"Plants","odp_version":"1.0"}"#).unwrap(),
        ],
        offerings: vec![
            parse_offering(
                br#"{"collection_ids":["plants"],"id":"p0","name":"Plant","odp_version":"1.0"}"#,
            )
            .unwrap(),
            parse_offering(br#"{"id":"p1","name":"Plant","odp_version":"1.0"}"#).unwrap(),
        ],
    })
    .unwrap();

    let page = catalog
        .list_collection_offerings(
            "plants",
            CatalogRequest {
                path: "/odp/collections/plants/offerings".to_owned(),
                ..CatalogRequest::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].id, "p0");
}

/// A Service over a static catalog still answers a search it never advertised.
#[tokio::test]
async fn reports_a_search_a_static_catalog_does_not_support() {
    let response = service(Stub::serving(1))
        .handle(query("/odp/offerings", "representation=terse"))
        .await;
    assert_eq!(response.status, 200);
}
