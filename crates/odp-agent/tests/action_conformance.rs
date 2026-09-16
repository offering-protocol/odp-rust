//! Resolving an Action: the target an Agent would call, and the documents describing it.

mod support;

use odp_agent::AgentError;
use support::{ODP_JSON, SCHEMA_JSON, SERVICE_DOCUMENT, Stub, client, response};

const OPENAPI_JSON: &str = "application/vnd.oai.openapi+json";

/// A Service Document naming a Service-wide OpenAPI document.
const DOCUMENT_WITH_OPENAPI: &[u8] = br#"{"description":"Plants","http":{"endpoint_base":"/odp","openapi":{"url":"https://plants.example/openapi.json"}},"language":"en","localizations":["en"],"name":"Plants","odp_version":"1.0","operations":[{"authentication":"not-required","name":"get-offering"},{"authentication":"not-required","name":"list-offerings"}]}"#;

const OPENAPI_OFFERING: &[u8] = br#"{"actions":[{"authentication":"not-required","id":"buy","openapi":{"operation_id":"purchasePlant"},"rel":"purchase"}],"id":"plant-1","name":"Plant","odp_version":"1.0"}"#;

/// A Service serving the Service Document, one Offering, and one supporting document.
fn serving(
    document: &'static [u8],
    offering: &'static [u8],
    supporting: Vec<u8>,
    media: &'static str,
) -> std::sync::Arc<Stub> {
    Stub::new(move |request| {
        Ok(if request.url.ends_with("/.well-known/odp") {
            response(200, document, ODP_JSON)
        } else if request.url.contains("/odp/") {
            response(200, offering, ODP_JSON)
        } else {
            response(200, &supporting, media)
        })
    })
}

// -- OpenAPI Actions ----------------------------------------------------------------------

/// OFR-66: the named operation is read out of the OpenAPI document, and nothing is invoked.
#[tokio::test]
async fn resolves_an_openapi_operation_by_its_identifier() {
    let openapi = br#"{"openapi":"3.1.0","info":{"title":"Plants","version":"1"},"paths":{"/buy":{"post":{"operationId":"purchasePlant","summary":"Buy"},"get":{"operationId":"listPlants"}}}}"#;
    let stub = serving(
        DOCUMENT_WITH_OPENAPI,
        OPENAPI_OFFERING,
        openapi.to_vec(),
        OPENAPI_JSON,
    );

    let resolved = client(&stub)
        .resolve_action("plant-1", "buy")
        .await
        .unwrap();
    assert_eq!(
        resolved
            .operation
            .as_ref()
            .and_then(|value| value.get("summary"))
            .and_then(|value| value.as_str()),
        Some("Buy")
    );
    assert!(resolved.openapi_document.is_some());
    assert!(resolved.request_schema.is_none());
}

/// ODP names OpenAPI 3.1, so a 3.0 document describes a contract this Agent cannot read.
#[tokio::test]
async fn refuses_an_openapi_document_of_another_version() {
    let openapi = br#"{"openapi":"3.0.3","info":{"title":"Plants","version":"1"},"paths":{"/buy":{"post":{"operationId":"purchasePlant"}}}}"#;
    let stub = serving(
        DOCUMENT_WITH_OPENAPI,
        OPENAPI_OFFERING,
        openapi.to_vec(),
        OPENAPI_JSON,
    );

    let error = client(&stub)
        .resolve_action("plant-1", "buy")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("OpenAPI 3.1"), "{error}");
}

#[tokio::test]
async fn refuses_an_openapi_document_with_no_paths() {
    let openapi = br#"{"openapi":"3.1.0","info":{"title":"Plants","version":"1"}}"#;
    let stub = serving(
        DOCUMENT_WITH_OPENAPI,
        OPENAPI_OFFERING,
        openapi.to_vec(),
        OPENAPI_JSON,
    );

    let error = client(&stub)
        .resolve_action("plant-1", "buy")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("paths"), "{error}");
}

/// An `operationId` is unique across an OpenAPI document, so zero or two matches is unusable.
#[tokio::test]
async fn refuses_an_operation_identifier_that_does_not_resolve_exactly_once() {
    for paths in [
        r#"{"/other":{"post":{"operationId":"somethingElse"}}}"#,
        r#"{"/buy":{"post":{"operationId":"purchasePlant"}},"/buy-again":{"put":{"operationId":"purchasePlant"}}}"#,
    ] {
        let openapi = format!(
            r#"{{"openapi":"3.1.0","info":{{"title":"P","version":"1"}},"paths":{paths}}}"#
        );
        let stub = serving(
            DOCUMENT_WITH_OPENAPI,
            OPENAPI_OFFERING,
            openapi.into_bytes(),
            OPENAPI_JSON,
        );

        let error = client(&stub)
            .resolve_action("plant-1", "buy")
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("exactly once"),
            "{paths}: {error}"
        );
    }
}

/// An Action naming its own OpenAPI document uses that one, not the Service-wide document.
#[tokio::test]
async fn prefers_the_document_the_action_names() {
    let offering = br#"{"actions":[{"authentication":"not-required","id":"buy","openapi":{"operation_id":"purchasePlant","url":"https://plants.example/buy-api.json"},"rel":"purchase"}],"id":"plant-1","name":"Plant","odp_version":"1.0"}"#;
    let openapi = br#"{"openapi":"3.1.0","info":{"title":"Plants","version":"1"},"paths":{"/buy":{"post":{"operationId":"purchasePlant"}}}}"#;
    let stub = serving(
        DOCUMENT_WITH_OPENAPI,
        offering,
        openapi.to_vec(),
        OPENAPI_JSON,
    );

    client(&stub)
        .resolve_action("plant-1", "buy")
        .await
        .unwrap();
    assert!(
        stub.requests()
            .iter()
            .any(|request| request.url == "https://plants.example/buy-api.json"),
        "{:?}",
        stub.requests()
    );
}

// -- HTTP Actions -------------------------------------------------------------------------

/// An HTTP Action naming a request schema has it fetched, so a caller can build the body.
#[tokio::test]
async fn resolves_the_request_schema_of_an_http_action() {
    let offering = br#"{"actions":[{"authentication":"not-required","http":{"href":"/buy","method":"POST","request":{"content_type":"application/json","schema":{"url":"https://plants.example/buy.json"}}},"id":"buy","rel":"purchase"}],"id":"plant-1","name":"Plant","odp_version":"1.0"}"#;
    let schema = br#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object"}"#;
    let stub = serving(SERVICE_DOCUMENT, offering, schema.to_vec(), SCHEMA_JSON);

    let resolved = client(&stub)
        .resolve_action("plant-1", "buy")
        .await
        .unwrap();
    assert!(resolved.request_schema.is_some());
    assert!(resolved.openapi_document.is_none());
    assert!(resolved.operation.is_none());
}

/// Naming an Action the Offering does not describe is the caller's mistake, not the Service's.
#[tokio::test]
async fn refuses_to_resolve_an_action_the_offering_does_not_describe() {
    let stub = Stub::serving(br#"{"id":"plant-1","name":"Plant","odp_version":"1.0"}"#);

    let error = client(&stub)
        .resolve_action("plant-1", "buy")
        .await
        .unwrap_err();
    assert!(matches!(error, AgentError::InvalidRequest(_)), "{error}");
}
