//! What the Agent makes of the documents a Service returns: versions, pages, Actions, schemas.

mod support;

use std::sync::Arc;

use odp_agent::{AgentError, OfferingIssueScope, ServiceClient, TraversalOptions};
use odp_core::Representation;
use support::{
    COLLECTION_PAGE, ODP_JSON, OFFERING_PAGE, ORIGIN, SCHEMA_JSON, SERVICE_DOCUMENT, Stub, client,
    response, scripted,
};

// -- versions and representations ---------------------------------------------------------

/// VER-04: an item inherits the version of the document carrying it.
#[tokio::test]
async fn reads_an_item_that_inherits_the_page_version() {
    let stub = Stub::serving(OFFERING_PAGE);
    let page = client(&stub)
        .list_offerings(Representation::Terse, 0)
        .await
        .unwrap();

    assert_eq!(page.odp_version, "1.0");
    assert_eq!(page.items[0].id, "plant-1");
}

/// VER-05: a document declaring a version this Agent does not support is rejected.
#[tokio::test]
async fn refuses_a_version_it_does_not_support() {
    for version in ["2.0", "0.9", "", "1"] {
        let body = format!(
            r#"{{"items":[{{"id":"plant-1","name":"Rubber Plant"}}],"odp_version":"{version}"}}"#
        );
        let stub = Stub::new(move |request| {
            Ok(if request.url.ends_with("/.well-known/odp") {
                response(200, SERVICE_DOCUMENT, ODP_JSON)
            } else {
                response(200, body.as_bytes(), ODP_JSON)
            })
        });
        assert!(
            client(&stub)
                .list_offerings(Representation::Terse, 0)
                .await
                .is_err(),
            "{version}"
        );
    }
}

/// A page whose items do not describe the resource they claim to is not a page this Agent reads.
#[tokio::test]
async fn refuses_a_page_whose_items_are_not_the_resource() {
    let stub = Stub::serving(br#"{"items":[{"name":"No identifier"}],"odp_version":"1.0"}"#);
    assert!(
        client(&stub)
            .list_offerings(Representation::Terse, 0)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn refuses_a_collection_page_whose_items_are_not_collections() {
    let stub = Stub::serving(br#"{"items":[{"id":"plants"}],"odp_version":"1.0"}"#);
    assert!(
        client(&stub)
            .list_collections(Representation::Terse, 0)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn refuses_a_body_that_is_not_json_at_all() {
    let stub = Stub::new(|_| Ok(response(200, b"not json", ODP_JSON)));
    assert!(client(&stub).inspect().await.is_err());
}

// -- pagination ---------------------------------------------------------------------------

/// PAG-07: a continuation stays on the origin the operation started from.
#[tokio::test]
async fn refuses_a_continuation_that_leaves_the_service_origin() {
    let stub = Stub::serving(OFFERING_PAGE);
    for next in [
        "https://elsewhere.example/odp/offerings?c=2",
        "//elsewhere.example/x",
        "http://plants.example/odp/offerings",
    ] {
        let error = client(&stub).continue_offerings(next).await.unwrap_err();
        assert!(
            matches!(error, AgentError::InvalidRequest(_)),
            "{next}: {error}"
        );
    }
}

#[tokio::test]
async fn follows_a_continuation_to_the_end_of_the_sequence() {
    let stub = scripted(vec![
        response(200, SERVICE_DOCUMENT, ODP_JSON),
        response(
            200,
            br#"{"items":[{"id":"plant-1","name":"One"}],"next":"/odp/offerings?cursor=c2","odp_version":"1.0"}"#,
            ODP_JSON,
        ),
        response(
            200,
            br#"{"items":[{"id":"plant-2","name":"Two"}],"odp_version":"1.0"}"#,
            ODP_JSON,
        ),
    ]);
    let offerings = client(&stub)
        .list_all_offerings(Representation::Terse, 0, TraversalOptions::default())
        .await
        .unwrap();

    assert_eq!(
        offerings
            .iter()
            .map(|value| value.id.as_str())
            .collect::<Vec<_>>(),
        ["plant-1", "plant-2"]
    );
}

#[tokio::test]
async fn follows_a_collection_continuation_too() {
    let stub = scripted(vec![
        response(200, SERVICE_DOCUMENT, ODP_JSON),
        response(
            200,
            br#"{"items":[{"id":"plants","name":"Plants"}],"next":"/odp/collections?cursor=c2","odp_version":"1.0"}"#,
            ODP_JSON,
        ),
        response(
            200,
            br#"{"items":[{"id":"trees","name":"Trees"}],"odp_version":"1.0"}"#,
            ODP_JSON,
        ),
    ]);
    let collections = client(&stub)
        .list_all_collections(Representation::Terse, 0, TraversalOptions::default())
        .await
        .unwrap();

    assert_eq!(collections.len(), 2);
}

/// A traversal stops at the bounds the caller set, however much the Service offers.
#[tokio::test]
async fn stops_a_traversal_at_the_bounds_it_was_given() {
    let stub = Stub::new(|request| {
        Ok(if request.url.ends_with("/.well-known/odp") {
            response(200, SERVICE_DOCUMENT, ODP_JSON)
        } else {
            response(
                200,
                br#"{"items":[{"id":"plant-1","name":"One"},{"id":"plant-2","name":"Two"}],"next":"/odp/offerings?cursor=more","odp_version":"1.0"}"#,
                ODP_JSON,
            )
        })
    });

    let offerings = client(&stub)
        .list_all_offerings(
            Representation::Terse,
            0,
            TraversalOptions {
                max_items: 3,
                max_pages: 0,
            },
        )
        .await
        .unwrap();
    assert_eq!(offerings.len(), 3);

    let stub = Stub::new(|request| {
        Ok(if request.url.ends_with("/.well-known/odp") {
            response(200, SERVICE_DOCUMENT, ODP_JSON)
        } else {
            response(
                200,
                br#"{"items":[{"id":"plant-1","name":"One"}],"next":"/odp/offerings?cursor=more","odp_version":"1.0"}"#,
                ODP_JSON,
            )
        })
    });
    let offerings = client(&stub)
        .list_all_offerings(
            Representation::Terse,
            0,
            TraversalOptions {
                max_items: 0,
                max_pages: 2,
            },
        )
        .await
        .unwrap();
    assert_eq!(offerings.len(), 2, "two pages of one item each");
}

#[tokio::test]
async fn refuses_a_traversal_past_the_bounds_the_protocol_allows() {
    let stub = Stub::serving(OFFERING_PAGE);
    for options in [
        TraversalOptions {
            max_items: 10_001,
            max_pages: 0,
        },
        TraversalOptions {
            max_items: 0,
            max_pages: 17,
        },
    ] {
        let error = client(&stub)
            .list_all_offerings(Representation::Terse, 0, options)
            .await
            .unwrap_err();
        assert!(matches!(error, AgentError::InvalidRequest(_)), "{error}");
    }
}

#[tokio::test]
async fn searches_every_page_of_a_result() {
    let stub = scripted(vec![
        response(200, SERVICE_DOCUMENT, ODP_JSON),
        response(
            200,
            br#"{"items":[{"id":"plant-1","name":"One"}],"next":"/odp/offerings/search?cursor=c2","odp_version":"1.0"}"#,
            ODP_JSON,
        ),
        response(
            200,
            br#"{"items":[{"id":"plant-2","name":"Two"}],"odp_version":"1.0"}"#,
            ODP_JSON,
        ),
    ]);
    let offerings = client(&stub)
        .search_all_offerings(
            &odp_core::OfferingSearchRequest {
                odp_version: "1.0".to_owned(),
                query: "plants".to_owned(),
                ..odp_core::OfferingSearchRequest::default()
            },
            Representation::Terse,
            TraversalOptions::default(),
        )
        .await
        .unwrap();

    assert_eq!(offerings.len(), 2);
}

// -- Offering details ---------------------------------------------------------------------

/// OFR-62: an Action target is resolved against the Service Origin without being invoked.
#[tokio::test]
async fn resolves_an_action_target_without_invoking_it() {
    let offering = br#"{"actions":[{"authentication":"not-required","description":"Download","http":{"href":"/downloads/plant.pdf","method":"GET","response_content_types":["application/pdf"]},"id":"download","rel":"download"}],"id":"plant-1","name":"Plant","odp_version":"1.0"}"#;
    let stub = Stub::serving(offering);
    let details = client(&stub).get_offering_details("plant-1").await.unwrap();

    assert!(details.issues.is_empty(), "{:?}", details.issues);
    assert_eq!(
        details.actions[0]
            .http
            .as_ref()
            .map(|http| http.url.as_str()),
        Some("https://plants.example/downloads/plant.pdf")
    );
    assert!(
        !stub
            .requests()
            .iter()
            .any(|request| request.url.contains("/downloads/")),
        "the Action was described, not performed"
    );
}

/// OFR-57: an Action identifier is unique within its Offering, so a repeat is reported, not used.
#[tokio::test]
async fn reports_a_duplicate_action_identifier_once() {
    let offering = br#"{"actions":[{"authentication":"not-required","http":{"href":"/a","method":"GET"},"id":"download","rel":"download"},{"authentication":"not-required","http":{"href":"/b","method":"GET"},"id":"download","rel":"download"}],"id":"plant-1","name":"Plant","odp_version":"1.0"}"#;
    let stub = Stub::serving(offering);
    let details = client(&stub).get_offering_details("plant-1").await.unwrap();

    assert!(details.actions.is_empty());
    assert_eq!(details.issues.len(), 1);
    assert_eq!(details.issues[0].scope, OfferingIssueScope::Action);
    assert_eq!(details.issues[0].action_id.as_deref(), Some("download"));
}

/// OFR-68: an OpenAPI Action with no URL of its own uses the Service-wide document.
#[tokio::test]
async fn falls_back_to_the_service_openapi_document() {
    let document = br#"{"description":"Plants","http":{"endpoint_base":"/odp","openapi":{"url":"https://plants.example/openapi.json"}},"language":"en","localizations":["en"],"name":"Plants","odp_version":"1.0","operations":[{"authentication":"not-required","name":"get-offering"},{"authentication":"not-required","name":"list-offerings"}]}"#;
    let offering = br#"{"actions":[{"authentication":"not-required","id":"buy","openapi":{"operation_id":"purchasePlant"},"rel":"purchase"}],"id":"plant-1","name":"Plant","odp_version":"1.0"}"#;
    let stub = Stub::new(move |request| {
        Ok(if request.url.ends_with("/.well-known/odp") {
            response(200, document, ODP_JSON)
        } else {
            response(200, offering, ODP_JSON)
        })
    });

    let details = client(&stub).get_offering_details("plant-1").await.unwrap();
    assert_eq!(
        details.actions[0]
            .openapi
            .as_ref()
            .map(|value| value.url.as_str()),
        Some("https://plants.example/openapi.json")
    );
}

/// An Action with no target at all, and no document to fall back to, is reported as an issue.
#[tokio::test]
async fn reports_an_openapi_action_with_no_document() {
    let offering = br#"{"actions":[{"authentication":"not-required","id":"buy","openapi":{"operation_id":"purchasePlant"},"rel":"purchase"}],"id":"plant-1","name":"Plant","odp_version":"1.0"}"#;
    let stub = Stub::serving(offering);
    let details = client(&stub).get_offering_details("plant-1").await.unwrap();

    assert!(details.actions.is_empty());
    assert_eq!(details.issues.len(), 1);
}

/// OFR-41: attributes that do not match their schema are withheld and reported.
#[tokio::test]
async fn withholds_attributes_that_do_not_match_their_schema() {
    let schema = br#"{"$schema":"https://json-schema.org/draft/2020-12/schema","properties":{"size":{"type":"integer"}},"required":["size"],"type":"object"}"#;
    let offering = br#"{"attributes":{"size":"large"},"id":"plant-1","name":"Plant","odp_version":"1.0","schema":{"url":"https://schemas.example/plant.json"}}"#;
    let stub = Stub::new(move |request| {
        Ok(if request.url.ends_with("/.well-known/odp") {
            response(200, SERVICE_DOCUMENT, ODP_JSON)
        } else if request.url.contains("schemas.example") {
            response(200, schema, SCHEMA_JSON)
        } else {
            response(200, offering, ODP_JSON)
        })
    });

    let details = client(&stub).get_offering_details("plant-1").await.unwrap();
    assert!(details.offering.attributes.is_empty());
    assert_eq!(details.issues[0].scope, OfferingIssueScope::Attributes);
    assert!(details.attribute_schema.is_some());
}

/// ERR-30: a schema the Agent cannot retrieve is a scoped issue, not a failed Offering.
#[tokio::test]
async fn reports_an_unreachable_schema_without_failing_the_offering() {
    let offering = br#"{"attributes":{"size":"L"},"id":"plant-1","name":"Plant","odp_version":"1.0","schema":{"url":"https://schemas.example/plant.json"}}"#;
    let stub = Stub::new(move |request| {
        Ok(if request.url.ends_with("/.well-known/odp") {
            response(200, SERVICE_DOCUMENT, ODP_JSON)
        } else if request.url.contains("schemas.example") {
            response(404, b"{}", "application/problem+json")
        } else {
            response(200, offering, ODP_JSON)
        })
    });

    let details = client(&stub).get_offering_details("plant-1").await.unwrap();
    assert_eq!(details.offering.id, "plant-1");
    assert!(details.offering.attributes.is_empty());
    assert_eq!(details.issues[0].scope, OfferingIssueScope::AttributeSchema);
}

#[tokio::test]
async fn resolves_an_action_a_caller_names() {
    let offering = br#"{"actions":[{"authentication":"not-required","http":{"href":"/downloads/plant.pdf","method":"GET"},"id":"download","rel":"download"}],"id":"plant-1","name":"Plant","odp_version":"1.0"}"#;
    let stub = Stub::serving(offering);
    let client = client(&stub);

    let resolved = client.resolve_action("plant-1", "download").await.unwrap();
    assert_eq!(resolved.action.id, "download");
    assert!(resolved.openapi_document.is_none());

    let error = client
        .resolve_action("plant-1", "absent")
        .await
        .unwrap_err();
    assert!(matches!(error, AgentError::InvalidRequest(_)), "{error}");
}

// -- search capabilities ------------------------------------------------------------------

/// FLT-49: capabilities belong to a Service that advertises the search operation.
#[tokio::test]
async fn reports_collection_capabilities_the_service_cannot_honour() {
    let document = br#"{"description":"Plants","http":{"endpoint_base":"/odp"},"language":"en","localizations":["en"],"name":"Plants","odp_version":"1.0","operations":[{"authentication":"not-required","name":"get-collection"},{"authentication":"not-required","name":"get-offering"},{"authentication":"not-required","name":"list-offerings"}]}"#;
    let collection = br#"{"id":"plants","name":"Plants","odp_version":"1.0","search_capabilities":{"filters":{"inline":[{"description":"d","id":"colour","operators":["eq"],"title":"Colour","type":"string"}]}}}"#;
    let stub = Stub::new(move |request| {
        Ok(if request.url.ends_with("/.well-known/odp") {
            response(200, document, ODP_JSON)
        } else {
            response(200, collection, ODP_JSON)
        })
    });

    let catalog = client(&stub)
        .get_collection_search_capabilities("plants")
        .await
        .unwrap();
    assert!(catalog.filters.is_empty());
    assert_eq!(catalog.issues.len(), 1);
}

/// FLT-64: an identifier published by two effective sources is available from neither.
#[tokio::test]
async fn drops_a_definition_two_sources_both_claim() {
    let document = br#"{"description":"Plants","http":{"endpoint_base":"/odp"},"language":"en","localizations":["en"],"name":"Plants","odp_version":"1.0","operations":[{"authentication":"not-required","name":"get-collection"},{"authentication":"not-required","name":"get-offering"},{"authentication":"not-required","name":"list-offerings"},{"authentication":"not-required","name":"search-offerings"}],"search_capabilities":{"filters":{"inline":[{"description":"d","id":"colour","operators":["eq"],"title":"Colour","type":"string"}]}}}"#;
    let collection = br#"{"id":"plants","name":"Plants","odp_version":"1.0","search_capabilities":{"filters":{"inline":[{"description":"d","id":"colour","operators":["eq"],"title":"Colour","type":"string"},{"description":"d","id":"height","operators":["gte"],"title":"Height","type":"integer"}]}}}"#;
    let stub = Stub::new(move |request| {
        Ok(if request.url.ends_with("/.well-known/odp") {
            response(200, document, ODP_JSON)
        } else {
            response(200, collection, ODP_JSON)
        })
    });

    let catalog = client(&stub)
        .get_offering_search_capabilities(Some("plants"))
        .await
        .unwrap();
    assert!(!catalog.filters.contains_key("colour"), "claimed twice");
    assert!(catalog.filters.contains_key("height"));
    assert_eq!(catalog.issues.len(), 1);
}

/// FLT-41: a sort key resolves to a filter the Agent actually has.
#[tokio::test]
async fn reports_a_sort_whose_filter_is_unavailable() {
    let document = br#"{"description":"Plants","http":{"endpoint_base":"/odp"},"language":"en","localizations":["en"],"name":"Plants","odp_version":"1.0","operations":[{"authentication":"not-required","name":"get-offering"},{"authentication":"not-required","name":"list-offerings"},{"authentication":"not-required","name":"search-offerings"}],"search_capabilities":{"sorts":{"inline":[{"description":"d","id":"tallest","keys":[{"direction":"descending","filter_id":"height","missing":"last"}],"title":"Tallest"}]}}}"#;
    let stub = Stub::new(move |request| {
        Ok(if request.url.ends_with("/.well-known/odp") {
            response(200, document, ODP_JSON)
        } else {
            response(200, OFFERING_PAGE, ODP_JSON)
        })
    });

    let catalog = client(&stub)
        .get_offering_search_capabilities(None)
        .await
        .unwrap();
    assert!(catalog.sorts.is_empty());
    assert_eq!(catalog.issues.len(), 1);
}

#[tokio::test]
async fn resolves_a_sort_against_the_filters_it_names() {
    let document = br#"{"description":"Plants","http":{"endpoint_base":"/odp"},"language":"en","localizations":["en"],"name":"Plants","odp_version":"1.0","operations":[{"authentication":"not-required","name":"get-offering"},{"authentication":"not-required","name":"list-offerings"},{"authentication":"not-required","name":"search-offerings"}],"search_capabilities":{"filters":{"inline":[{"description":"d","id":"height","operators":["gte"],"title":"Height","type":"integer"}]},"sorts":{"inline":[{"description":"d","id":"tallest","keys":[{"direction":"descending","filter_id":"height","missing":"last"}],"title":"Tallest"}]}}}"#;
    let stub = Stub::new(move |request| {
        Ok(if request.url.ends_with("/.well-known/odp") {
            response(200, document, ODP_JSON)
        } else {
            response(200, OFFERING_PAGE, ODP_JSON)
        })
    });

    let catalog = client(&stub)
        .get_offering_search_capabilities(None)
        .await
        .unwrap();
    assert_eq!(catalog.sorts["tallest"].filters.len(), 1);
    assert!(catalog.issues.is_empty());
}

/// FLT-52/53: a linked source is followed page by page, and a loop in it is refused.
#[tokio::test]
async fn follows_a_linked_capability_source() {
    let document = br#"{"description":"Plants","http":{"endpoint_base":"/odp"},"language":"en","localizations":["en"],"name":"Plants","odp_version":"1.0","operations":[{"authentication":"not-required","name":"get-offering"},{"authentication":"not-required","name":"list-offerings"},{"authentication":"not-required","name":"search-offerings"}],"search_capabilities":{"filters":{"linked":{"href":"/odp/filters"}}}}"#;
    let stub = Stub::new(move |request| {
        Ok(if request.url.ends_with("/.well-known/odp") {
            response(200, document, ODP_JSON)
        } else if request.url.contains("cursor=c2") {
            response(
                200,
                br#"{"items":[{"description":"d","id":"height","operators":["gte"],"title":"Height","type":"integer"}],"odp_version":"1.0"}"#,
                ODP_JSON,
            )
        } else {
            response(
                200,
                br#"{"items":[{"description":"d","id":"colour","operators":["eq"],"title":"Colour","type":"string"}],"next":"/odp/filters?cursor=c2","odp_version":"1.0"}"#,
                ODP_JSON,
            )
        })
    });

    let catalog = client(&stub)
        .get_offering_search_capabilities(None)
        .await
        .unwrap();
    assert_eq!(catalog.filters.len(), 2);
    assert!(catalog.issues.is_empty());
}

#[tokio::test]
async fn refuses_a_linked_source_that_loops() {
    let document = br#"{"description":"Plants","http":{"endpoint_base":"/odp"},"language":"en","localizations":["en"],"name":"Plants","odp_version":"1.0","operations":[{"authentication":"not-required","name":"get-offering"},{"authentication":"not-required","name":"list-offerings"},{"authentication":"not-required","name":"search-offerings"}],"search_capabilities":{"sorts":{"linked":{"href":"/odp/sorts"}}}}"#;
    let stub = Stub::new(move |request| {
        Ok(if request.url.ends_with("/.well-known/odp") {
            response(200, document, ODP_JSON)
        } else {
            response(
                200,
                br#"{"items":[{"description":"d","id":"tallest","keys":[{"direction":"descending","filter_id":"height","missing":"last"}],"title":"Tallest"}],"next":"/odp/sorts","odp_version":"1.0"}"#,
                ODP_JSON,
            )
        })
    });

    let catalog = client(&stub)
        .get_offering_search_capabilities(None)
        .await
        .unwrap();
    assert_eq!(catalog.issues.len(), 1);
    assert!(catalog.issues[0].message.contains("loop"));
}

/// A Service advertising no capabilities at all reports none, and no issues either.
#[tokio::test]
async fn reports_no_capabilities_when_the_service_advertises_none() {
    let stub = Stub::serving(COLLECTION_PAGE);
    let catalog = client(&stub)
        .get_offering_search_capabilities(None)
        .await
        .unwrap();

    assert!(catalog.filters.is_empty());
    assert!(catalog.sorts.is_empty());
    assert!(catalog.issues.is_empty());
}

#[tokio::test]
async fn keeps_the_client_shareable_across_tasks() {
    let stub = Stub::serving(OFFERING_PAGE);
    let client = Arc::new(ServiceClient::with_transport(ORIGIN, stub.clone()).unwrap());
    let second = Arc::clone(&client);

    let task = tokio::spawn(async move { second.inspect().await.map(|value| value.document.name) });
    client.inspect().await.unwrap();

    assert_eq!(task.await.unwrap().unwrap(), "Plants");
}

// -- collections --------------------------------------------------------------------------

/// PAG-07: a Collection continuation is followed the same way an Offering one is.
#[tokio::test]
async fn continues_a_collection_sequence_to_its_end() {
    let stub = Stub::new(|request| {
        Ok(if request.url.ends_with("/.well-known/odp") {
            response(200, SERVICE_DOCUMENT, ODP_JSON)
        } else if request.url.contains("cursor=c2") {
            response(
                200,
                br#"{"items":[{"id":"shrubs","name":"Shrubs"}],"odp_version":"1.0"}"#,
                ODP_JSON,
            )
        } else {
            response(
                200,
                br#"{"items":[{"id":"plants","name":"Plants"}],"next":"/odp/collections?cursor=c2","odp_version":"1.0"}"#,
                ODP_JSON,
            )
        })
    });
    let client = client(&stub);

    let first = client
        .list_collections(Representation::Terse, 0)
        .await
        .unwrap();
    assert_eq!(first.items.len(), 1);

    let second = client.continue_collections(&first.next).await.unwrap();
    assert_eq!(second.items[0].id, "shrubs");
    assert!(second.next.is_empty());
}

/// A whole Collection sequence is gathered in one call, within the bounds it was given.
#[tokio::test]
async fn gathers_every_collection_across_pages() {
    let stub = Stub::new(|request| {
        Ok(if request.url.ends_with("/.well-known/odp") {
            response(200, SERVICE_DOCUMENT, ODP_JSON)
        } else if request.url.contains("cursor=c3") {
            response(
                200,
                br#"{"items":[{"id":"trees","name":"Trees"}],"odp_version":"1.0"}"#,
                ODP_JSON,
            )
        } else if request.url.contains("cursor=c2") {
            response(
                200,
                br#"{"items":[{"id":"shrubs","name":"Shrubs"}],"next":"/odp/collections?cursor=c3","odp_version":"1.0"}"#,
                ODP_JSON,
            )
        } else {
            response(
                200,
                br#"{"items":[{"id":"plants","name":"Plants"}],"next":"/odp/collections?cursor=c2","odp_version":"1.0"}"#,
                ODP_JSON,
            )
        })
    });

    let all = client(&stub)
        .list_all_collections(Representation::Terse, 0, TraversalOptions::default())
        .await
        .unwrap();
    assert_eq!(all.len(), 3);

    let bounded = client(&stub)
        .list_all_collections(
            Representation::Terse,
            0,
            TraversalOptions {
                max_items: 10,
                max_pages: 2,
            },
        )
        .await
        .unwrap();
    assert_eq!(bounded.len(), 2, "the third page was outside the bound");
}

/// SEC-05: a schema URL on loopback HTTP is a reference this Agent will not follow.
#[tokio::test]
async fn reports_an_attribute_schema_url_it_will_not_follow() {
    let offering = br#"{"attributes":{"size":"L"},"id":"plant-1","name":"Plant","odp_version":"1.0","schema":{"url":"http://localhost:8080/plant.json"}}"#;
    let stub = Stub::serving(offering);

    let details = client(&stub).get_offering_details("plant-1").await.unwrap();
    assert_eq!(details.offering.id, "plant-1");
    assert!(details.offering.attributes.is_empty());
    assert_eq!(details.issues.len(), 1);
    assert_eq!(details.issues[0].scope, OfferingIssueScope::AttributeSchema);
    assert!(
        details.issues[0].message.contains("HTTPS"),
        "{:?}",
        details.issues
    );
}

/// The same applies to an Action's request schema.
#[tokio::test]
async fn refuses_a_request_schema_url_it_will_not_follow() {
    let offering = br#"{"actions":[{"authentication":"not-required","http":{"href":"/buy","method":"POST","request":{"content_type":"application/json","schema":{"url":"http://localhost:8080/buy.json"}}},"id":"buy","rel":"purchase"}],"id":"plant-1","name":"Plant","odp_version":"1.0"}"#;
    let stub = Stub::serving(offering);

    let error = client(&stub)
        .resolve_action("plant-1", "buy")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("HTTPS"), "{error}");
}
