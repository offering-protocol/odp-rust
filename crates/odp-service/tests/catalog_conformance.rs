//! PAG-11/14, REP-12, OFR-55 and IDN-08: what a Service will publish on a catalog's behalf.

mod support;

use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use odp_core::{
    Action, ActionRelation, AdditionalMembers, AuthenticationRequirement, Collection,
    HttpActionTarget, Offering, OfferingPage, Operation, Page, Representation, VERSION,
    parse_collection, parse_offering,
};
use odp_service::{
    Catalog, CatalogRequest, Request, Service, ServiceBuilder, ServiceError, StaticCatalog,
    StaticCatalogOptions,
};
use support::{
    Stub, collection, collection_page, document, get, offering, offering_page, query, service,
};

/// A catalog that answers with exactly what a test hands it.
struct Fixed {
    collections: Page<Collection>,
    offerings: OfferingPage<Offering>,
    single: Option<Offering>,
}

impl Fixed {
    fn offerings(page: OfferingPage<Offering>) -> Arc<Self> {
        Arc::new(Self {
            collections: collection_page(Vec::new(), ""),
            offerings: page,
            single: None,
        })
    }

    fn offering(value: Offering) -> Arc<Self> {
        Arc::new(Self {
            collections: collection_page(Vec::new(), ""),
            offerings: offering_page(Vec::new(), ""),
            single: Some(value),
        })
    }

    fn collections(page: Page<Collection>) -> Arc<Self> {
        Arc::new(Self {
            collections: page,
            offerings: offering_page(Vec::new(), ""),
            single: None,
        })
    }
}

#[async_trait]
impl Catalog for Fixed {
    fn operations(&self) -> Vec<Operation> {
        support::ALL_OPERATIONS.to_vec()
    }

    async fn list_offerings(
        &self,
        _request: CatalogRequest,
    ) -> Result<OfferingPage<Offering>, ServiceError> {
        Ok(self.offerings.clone())
    }

    async fn get_offering(
        &self,
        _id: &str,
        _request: CatalogRequest,
    ) -> Result<Option<Offering>, ServiceError> {
        Ok(self.single.clone())
    }

    async fn list_collections(
        &self,
        _request: CatalogRequest,
    ) -> Result<Page<Collection>, ServiceError> {
        Ok(self.collections.clone())
    }

    async fn get_collection(
        &self,
        _id: &str,
        _request: CatalogRequest,
    ) -> Result<Option<Collection>, ServiceError> {
        Ok(self.collections.items.first().cloned())
    }

    async fn list_collection_offerings(
        &self,
        _collection_id: &str,
        _request: CatalogRequest,
    ) -> Result<OfferingPage<Offering>, ServiceError> {
        Ok(self.offerings.clone())
    }
}

fn serving(catalog: Arc<Fixed>) -> Service {
    Service::new(document(&["en"]), catalog).unwrap()
}

// -- page contracts -------------------------------------------------------------------------

/// PAG-14: a Service may answer with fewer items than asked for, never with more.
#[tokio::test]
async fn refuses_a_page_larger_than_the_request_allows() {
    let page = offering_page(
        (0..5).map(|index| offering(&format!("p{index}"))).collect(),
        "",
    );
    let response = serving(Fixed::offerings(page))
        .handle(query("/odp/offerings", "limit=2"))
        .await;

    assert_eq!(response.status, 500, "the Service does not publish it");
}

#[tokio::test]
async fn serves_a_page_within_the_request() {
    let page = offering_page(
        (0..2).map(|index| offering(&format!("p{index}"))).collect(),
        "",
    );
    let response = serving(Fixed::offerings(page))
        .handle(query("/odp/offerings", "limit=2"))
        .await;

    assert_eq!(response.status, 200);
}

/// With no limit asked for, the Service chooses its own page size (PAG-14).
#[tokio::test]
async fn serves_whatever_the_catalog_chose_when_no_limit_was_asked_for() {
    let page = offering_page(
        (0..30)
            .map(|index| offering(&format!("p{index}")))
            .collect(),
        "",
    );
    let response = serving(Fixed::offerings(page))
        .handle(get("/odp/offerings"))
        .await;
    assert_eq!(response.status, 200);
}

/// PAG-11: a continuation that points at the request it answers never advances.
#[tokio::test]
async fn refuses_a_continuation_that_does_not_advance() {
    let page = offering_page(vec![offering("p0")], "/odp/offerings?limit=2");
    let response = serving(Fixed::offerings(page))
        .handle(query("/odp/offerings", "limit=2"))
        .await;

    assert_eq!(response.status, 500);
}

#[tokio::test]
async fn serves_a_continuation_that_advances() {
    let page = offering_page(vec![offering("p0")], "/odp/offerings?cursor=c2&limit=2");
    let response = serving(Fixed::offerings(page))
        .handle(query("/odp/offerings", "limit=2"))
        .await;

    assert_eq!(response.status, 200);
    assert_eq!(
        odp_core::parse_page::<serde_json::Value>(&response.body)
            .unwrap()
            .next,
        "/odp/offerings?cursor=c2&limit=2"
    );
}

/// The same reasoning applies to a Collection page.
#[tokio::test]
async fn holds_a_collection_page_to_the_same_contract() {
    let page = collection_page(
        (0..5)
            .map(|index| collection(&format!("c{index}")))
            .collect(),
        "",
    );
    let response = serving(Fixed::collections(page))
        .handle(query("/odp/collections", "limit=1"))
        .await;
    assert_eq!(response.status, 500);

    let looping = collection_page(vec![collection("c0")], "/odp/collections");
    let response = serving(Fixed::collections(looping))
        .handle(get("/odp/collections"))
        .await;
    assert_eq!(response.status, 500);
}

/// PAG-01: a page envelope that is not one is not published either.
#[tokio::test]
async fn refuses_a_page_envelope_that_is_not_one() {
    let mut page = offering_page(vec![offering("p0")], "");
    page.odp_version = String::new();
    let response = serving(Fixed::offerings(page))
        .handle(get("/odp/offerings"))
        .await;
    assert_eq!(response.status, 500);
}

// -- representation contracts ---------------------------------------------------------------

/// OFR-55: a Terse Offering carries no Actions.
#[tokio::test]
async fn refuses_actions_in_a_terse_offering() {
    let mut value = offering("plant-1");
    value.actions.push(Action {
        authentication: AuthenticationRequirement::NotRequired,
        description: String::new(),
        http: Some(HttpActionTarget {
            href: "/purchase".to_owned(),
            method: "POST".to_owned(),
            request: None,
            response_content_types: Vec::new(),
        }),
        id: "purchase".to_owned(),
        openapi: None,
        rel: ActionRelation::Purchase,
    });

    let response = serving(Fixed::offering(value.clone()))
        .handle(query("/odp/offerings/plant-1", "representation=terse"))
        .await;
    assert_eq!(response.status, 500);

    let full = serving(Fixed::offering(value))
        .handle(query("/odp/offerings/plant-1", "representation=full"))
        .await;
    assert_eq!(full.status, 200, "a Full Offering may carry them");
}

#[tokio::test]
async fn get_resources_default_to_full_while_lists_default_to_terse() {
    let catalog = Stub::serving(1);
    let service = service(catalog.clone());
    for path in ["/odp/offerings/plant-1", "/odp/collections/plants"] {
        assert_eq!(service.handle(get(path)).await.status, 200);
        assert_eq!(catalog.last().representation, Representation::Full);
    }
    assert_eq!(service.handle(get("/odp/offerings")).await.status, 200);
    assert_eq!(catalog.last().representation, Representation::Terse);
}

#[tokio::test]
async fn serves_supported_version_without_changing_catalog_values() {
    let mut value = offering("plant-1");
    value.odp_version = "1.7".to_owned();
    let catalog = Fixed::offering(value);
    let response = serving(catalog.clone())
        .handle(get("/odp/offerings/plant-1"))
        .await;
    assert_eq!(response.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
    assert_eq!(body["odp_version"], "1.0");
    assert_eq!(catalog.single.as_ref().unwrap().odp_version, "1.7");
}

#[test]
fn static_catalog_checks_the_complete_parent_graph() {
    let make = |id: &str, parents: Vec<String>| {
        let mut value = collection(id);
        value.parent_ids = parents;
        value
    };
    for collections in [
        vec![make("a", vec!["missing".to_owned()])],
        vec![
            make("a", vec!["b".to_owned()]),
            make("b", vec!["a".to_owned()]),
        ],
        (0..34)
            .map(|index| {
                make(
                    &format!("c{index}"),
                    if index == 0 {
                        vec![]
                    } else {
                        vec![format!("c{}", index - 1)]
                    },
                )
            })
            .collect(),
    ] {
        assert!(
            StaticCatalog::new(StaticCatalogOptions {
                collections,
                ..Default::default()
            })
            .is_err()
        );
    }
    let collections = (0..33)
        .map(|index| {
            make(
                &format!("c{index}"),
                if index == 0 {
                    vec![]
                } else {
                    vec![format!("c{}", index - 1)]
                },
            )
        })
        .collect();
    assert!(
        StaticCatalog::new(StaticCatalogOptions {
            collections,
            ..Default::default()
        })
        .is_ok()
    );
}

/// REP-12: a Full Representation omits nothing, so it has nothing to point at.
#[tokio::test]
async fn refuses_detail_fields_in_a_full_representation() {
    let mut value = offering("plant-1");
    value.detail_fields.push("/description".to_owned());

    let response = serving(Fixed::offering(value.clone()))
        .handle(query("/odp/offerings/plant-1", "representation=full"))
        .await;
    assert_eq!(response.status, 500);

    let terse = serving(Fixed::offering(value))
        .handle(query("/odp/offerings/plant-1", "representation=terse"))
        .await;
    assert_eq!(terse.status, 200, "a Terse Offering may point at them");
}

/// A catalog that answers with a different resource than the one asked for is not published.
#[tokio::test]
async fn refuses_a_resource_that_does_not_match_the_path() {
    let response = serving(Fixed::offering(offering("somebody-else")))
        .handle(get("/odp/offerings/plant-1"))
        .await;
    assert_eq!(response.status, 500);
}

// -- identifiers ----------------------------------------------------------------------------

/// IDN-08: a Collection identifier is checked the way an Offering identifier is.
#[tokio::test]
async fn refuses_an_identifier_that_could_never_name_a_resource() {
    for path in [
        "/odp/offerings/not a valid id",
        "/odp/collections/not a valid id",
        "/odp/collections/not a valid id/offerings",
        "/odp/collections//offerings",
    ] {
        let response = service(Stub::serving(1)).handle(get(path)).await;
        assert_eq!(response.status, 400, "{path}");
        assert_eq!(
            odp_core::parse_problem_details(&response.body)
                .unwrap()
                .code,
            "INVALID_REQUEST",
            "{path}"
        );
    }
}

/// And a valid identifier that names nothing is a plain 404.
#[tokio::test]
async fn reports_a_resource_that_is_simply_absent() {
    for path in ["/odp/offerings/absent", "/odp/collections/absent"] {
        let response = service(Stub::serving(1)).handle(get(path)).await;
        assert_eq!(response.status, 404, "{path}");
    }
}

/// `/offerings/search` names an operation, so it is never read as an Offering called `search`.
#[tokio::test]
async fn keeps_the_search_paths_for_search() {
    for path in ["/odp/offerings/search", "/odp/collections/search"] {
        let response = service(Stub::serving(1)).handle(get(path)).await;
        assert_eq!(
            response.status, 400,
            "a search GET needs its continuation cursor: {path}"
        );
    }
}

// -- the Service's own configuration --------------------------------------------------------

/// SVC-82: a Service that cannot list and get Offerings is not a Service.
#[test]
fn refuses_a_catalog_that_cannot_meet_the_baseline() {
    struct Thin;

    #[async_trait]
    impl Catalog for Thin {
        fn operations(&self) -> Vec<Operation> {
            vec![Operation::ListOfferings]
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

    let Err(error) = Service::new(document(&["en"]), Arc::new(Thin)) else {
        panic!("a Service without the baseline operations is not conformant");
    };
    assert!(matches!(error, ServiceError::InvalidConfiguration(_)));
}

/// ROLE-04: the advertised operations are the catalog's, not whatever the document claimed.
#[tokio::test]
async fn advertises_exactly_what_the_catalog_supports() {
    let stub = Stub::with_operations(
        support::Answer::Items(1, String::new()),
        vec![Operation::GetOffering, Operation::ListOfferings],
    );
    let service = Service::new(document(&["en"]), stub).unwrap();
    let document = service.document();

    assert_eq!(document.operations.len(), 2);
    assert!(document.operations.iter().all(|descriptor| matches!(
        descriptor.name,
        Operation::GetOffering | Operation::ListOfferings
    )));
}

/// A builder produces a Service Document its own parser accepts.
#[tokio::test]
async fn builds_a_service_from_its_parts() {
    let service = ServiceBuilder::new("Plants", "Plants for agents.", "en", "/odp")
        .keywords(["plants"])
        .documentation_url("https://plants.example/docs")
        .status_url("https://plants.example/status")
        .support_url("https://plants.example/support")
        .website_url("https://plants.example")
        .localizations(["en", "fr"])
        .payment_origins(["https://pay.plants.example"])
        .operation_authentication(
            Operation::GetOffering,
            AuthenticationRequirement::NotRequired,
        )
        .build(Stub::serving(1))
        .unwrap();

    let response = service.handle(get("/.well-known/odp")).await;
    assert_eq!(response.status, 200);

    let document = odp_core::parse_service_document(&response.body).unwrap();
    assert_eq!(document.name, "Plants");
    assert_eq!(document.localizations, ["en", "fr"]);
    assert_eq!(document.odp_version, VERSION);
}

/// A Service whose catalog fails outright reports it without leaking how.
#[tokio::test]
async fn reports_an_unsupported_operation_the_catalog_advertised() {
    struct Lying;

    #[async_trait]
    impl Catalog for Lying {
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

    // The default trait methods report the operations this catalog never implemented.
    let service = Service::new(document(&["en"]), Arc::new(Lying)).unwrap();
    for path in ["/odp/collections", "/odp/collections/plants"] {
        let response = service.handle(get(path)).await;
        assert_eq!(response.status, 500, "{path}");
    }
}

// -- the static catalog ---------------------------------------------------------------------

/// REP-11 and REP-12: detail fields belong to a Terse representation and only to one.
#[tokio::test]
async fn keeps_detail_fields_on_the_representation_that_can_carry_them() {
    let offering = parse_offering(
        br#"{"actions":[{"authentication":"not-required","http":{"href":"/buy","method":"POST"},"id":"buy","rel":"purchase"}],"detail_fields":["/actions"],"id":"plant-1","name":"Plant","odp_version":"1.0"}"#,
    )
    .unwrap();
    let catalog = Arc::new(
        StaticCatalog::new(StaticCatalogOptions {
            collections: Vec::new(),
            offerings: vec![offering],
        })
        .unwrap(),
    );

    let terse = catalog
        .get_offering("plant-1", CatalogRequest::default())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(terse.detail_fields, ["/actions"]);
    assert!(terse.actions.is_empty(), "OFR-55");

    let full = catalog
        .get_offering(
            "plant-1",
            CatalogRequest {
                representation: Representation::Full,
                ..CatalogRequest::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert!(full.detail_fields.is_empty(), "REP-12");
    assert_eq!(full.actions.len(), 1);
}

/// And the Service serves both without complaint.
#[tokio::test]
async fn serves_both_representations_of_a_static_offering() {
    let offering = parse_offering(
        br#"{"actions":[{"authentication":"not-required","http":{"href":"/buy","method":"POST"},"id":"buy","rel":"purchase"}],"detail_fields":["/actions"],"id":"plant-1","name":"Plant","odp_version":"1.0"}"#,
    )
    .unwrap();
    let service = Service::new(
        document(&["en"]),
        Arc::new(
            StaticCatalog::new(StaticCatalogOptions {
                collections: vec![
                    parse_collection(br#"{"id":"plants","name":"Plants","odp_version":"1.0"}"#)
                        .unwrap(),
                ],
                offerings: vec![offering],
            })
            .unwrap(),
        ),
    )
    .unwrap();

    for request in [
        get("/odp/offerings/plant-1"),
        query("/odp/offerings/plant-1", "representation=full"),
        get("/odp/offerings"),
        query("/odp/offerings", "representation=full"),
        get("/odp/collections"),
        get("/odp/collections/plants"),
        get("/odp/collections/plants/offerings"),
    ] {
        let path = format!("{} {}", request.path, request.query);
        let response = service.handle(request).await;
        assert_eq!(response.status, 200, "{path}");
    }
}

/// A Collection a static catalog does not hold is absent, not an error.
#[tokio::test]
async fn reports_an_absent_static_collection() {
    let service = Service::new(
        document(&["en"]),
        Arc::new(
            StaticCatalog::new(StaticCatalogOptions {
                collections: vec![
                    parse_collection(br#"{"id":"plants","name":"Plants","odp_version":"1.0"}"#)
                        .unwrap(),
                ],
                offerings: vec![offering("plant-1")],
            })
            .unwrap(),
        ),
    )
    .unwrap();

    assert_eq!(
        service.handle(get("/odp/collections/absent")).await.status,
        404
    );
    assert_eq!(
        service
            .handle(get("/odp/collections/absent/offerings"))
            .await
            .status,
        404
    );
}

/// A request carrying no headers at all is still a request.
#[tokio::test]
async fn serves_a_bare_request() {
    let response = service(Stub::serving(1))
        .handle(Request {
            headers: BTreeMap::new(),
            method: "GET".to_owned(),
            path: "/odp/offerings".to_owned(),
            ..Request::default()
        })
        .await;
    assert_eq!(response.status, 200);
}

/// A Service Document larger than its own limit is not published.
#[test]
fn refuses_a_service_document_past_its_limit() {
    let mut document = document(&["en"]);
    document.additional = AdditionalMembers::from([(
        "padding".to_owned(),
        serde_json::Value::String("x".repeat(70_000)),
    )]);
    assert!(Service::new(document, Stub::serving(1)).is_err());
}
