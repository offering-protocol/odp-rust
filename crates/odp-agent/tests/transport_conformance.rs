//! What the Agent puts on the wire, and what it refuses to read back.

mod support;

use std::sync::Arc;

use odp_agent::{AgentError, ServiceClient};
use odp_core::Representation;
use support::{
    COLLECTION, COLLECTION_PAGE, ODP_JSON, OFFERING, OFFERING_PAGE, ORIGIN, PROBLEM_JSON,
    SERVICE_DOCUMENT, Stub, bare, client, response, scripted, with_headers,
};

/// MED-02: an Agent says what it will take, and MED-05 what it is sending.
#[tokio::test]
async fn states_the_media_type_it_accepts() {
    let stub = Stub::serving(OFFERING_PAGE);
    client(&stub)
        .list_offerings(Representation::Terse, 10)
        .await
        .unwrap();

    let request = stub.last();
    assert_eq!(
        request.headers.get("accept").map(String::as_str),
        Some(ODP_JSON)
    );
    assert_eq!(request.method, "GET");
    assert!(!request.headers.contains_key("content-type"));
}

#[tokio::test]
async fn declares_the_media_type_of_a_search_body() {
    let stub = Stub::serving(OFFERING_PAGE);
    client(&stub)
        .search_offerings(&search("plants"), Representation::Terse)
        .await
        .unwrap();

    let request = stub.last();
    assert_eq!(request.method, "POST");
    assert_eq!(
        request.headers.get("content-type").map(String::as_str),
        Some(ODP_JSON)
    );
    assert!(!request.body.is_empty());
}

#[tokio::test]
async fn sends_the_language_the_caller_asked_for() {
    let stub = Stub::serving(OFFERING_PAGE);
    client(&stub)
        .with_accept_language("fr-CA")
        .list_offerings(Representation::Terse, 0)
        .await
        .unwrap();

    assert_eq!(
        stub.last()
            .headers
            .get("accept-language")
            .map(String::as_str),
        Some("fr-CA")
    );
}

/// MED-08: a successful response with a missing or different media type is not an ODP response.
#[tokio::test]
async fn refuses_a_successful_response_of_another_media_type() {
    for content_type in ["application/json", "text/html", "", "application/odp+jsonx"] {
        let stub = Stub::new(move |_| Ok(response(200, SERVICE_DOCUMENT, content_type)));
        let error = client(&stub).inspect().await.unwrap_err();
        assert!(
            matches!(&error, AgentError::InvalidResponse(message) if message.contains(ODP_JSON)),
            "{content_type}: {error}"
        );
    }
}

/// MED-10: a parameter is syntactically valid and carries no meaning of its own.
#[tokio::test]
async fn ignores_media_type_parameters() {
    let stub = Stub::new(|_| {
        Ok(response(
            200,
            SERVICE_DOCUMENT,
            "APPLICATION/ODP+JSON; charset=utf-8",
        ))
    });
    assert_eq!(
        client(&stub).inspect().await.unwrap().document.name,
        "Plants"
    );
}

/// SVC-73: the representation the caller asked for is the one the request states.
#[tokio::test]
async fn states_the_representation_and_limit_it_was_asked_for() {
    let stub = Stub::serving(OFFERING_PAGE);
    client(&stub)
        .list_offerings(Representation::Full, 25)
        .await
        .unwrap();
    assert!(stub.last().url.contains("representation=full"));
    assert!(stub.last().url.contains("limit=25"));

    let stub = Stub::serving(OFFERING_PAGE);
    client(&stub)
        .list_offerings(Representation::Terse, 0)
        .await
        .unwrap();
    assert!(stub.last().url.contains("representation=terse"));
    assert!(
        !stub.last().url.contains("limit="),
        "an absent limit is not sent"
    );
}

/// ROLE-04: an Agent asks only for what the Service Document advertises.
#[tokio::test]
async fn refuses_an_operation_the_service_does_not_advertise() {
    let document = br#"{"description":"Plants","http":{"endpoint_base":"/odp"},"language":"en","localizations":["en"],"name":"Plants","odp_version":"1.0","operations":[{"authentication":"not-required","name":"get-offering"},{"authentication":"not-required","name":"list-offerings"}]}"#;
    let stub = Stub::new(move |request| {
        Ok(if request.url.ends_with("/.well-known/odp") {
            response(200, document, ODP_JSON)
        } else {
            response(200, COLLECTION_PAGE, ODP_JSON)
        })
    });

    let error = client(&stub)
        .list_collections(Representation::Terse, 0)
        .await
        .unwrap_err();
    assert!(
        matches!(error, AgentError::UnsupportedOperation(_)),
        "{error}"
    );
    assert_eq!(stub.catalog_requests().len(), 0, "nothing was requested");
}

// -- redirects ---------------------------------------------------------------------------

/// RFC 9110 15.4: a 303, and a 301 or 302 answering a POST, continue as a GET with no body.
#[tokio::test]
async fn continues_a_redirected_search_as_a_get() {
    for status in [301, 302, 303] {
        let stub = scripted(vec![
            response(200, SERVICE_DOCUMENT, ODP_JSON),
            bare(status, &[("location", "/odp/offerings/search/v2")]),
            response(200, OFFERING_PAGE, ODP_JSON),
        ]);
        client(&stub)
            .search_offerings(&search("plants"), Representation::Terse)
            .await
            .unwrap();

        let last = stub.last();
        assert_eq!(last.method, "GET", "{status}");
        assert!(last.body.is_empty(), "{status}");
    }
}

/// A 307 or 308 keeps the method and the body it was answering.
#[tokio::test]
async fn keeps_the_method_across_a_preserving_redirect() {
    for status in [307, 308] {
        let stub = scripted(vec![
            response(200, SERVICE_DOCUMENT, ODP_JSON),
            bare(status, &[("location", "/odp/offerings/search/v2")]),
            response(200, OFFERING_PAGE, ODP_JSON),
        ]);
        client(&stub)
            .search_offerings(&search("plants"), Representation::Terse)
            .await
            .unwrap();

        let last = stub.last();
        assert_eq!(last.method, "POST", "{status}");
        assert!(!last.body.is_empty(), "{status}");
    }
}

/// ERR-24: a redirect keeps the scheme, host and port of the request it answers.
#[tokio::test]
async fn refuses_a_redirect_that_leaves_the_service_origin() {
    for location in [
        "https://elsewhere.example/odp/offerings",
        "https://plants.example:8443/odp/offerings",
        "http://plants.example/odp/offerings",
    ] {
        let stub = scripted(vec![
            response(200, SERVICE_DOCUMENT, ODP_JSON),
            bare(302, &[("location", location)]),
            response(200, OFFERING_PAGE, ODP_JSON),
        ]);
        let error = client(&stub)
            .list_offerings(Representation::Terse, 0)
            .await
            .unwrap_err();
        assert!(
            matches!(&error, AgentError::InvalidResponse(message) if message.contains("origin")),
            "{location}: {error}"
        );
    }
}

/// ERR-25: the sixth redirect is refused.
#[tokio::test]
async fn refuses_a_sixth_redirect() {
    let stub = scripted(vec![
        response(200, SERVICE_DOCUMENT, ODP_JSON),
        bare(302, &[("location", "/odp/offerings?page=next")]),
    ]);
    let error = client(&stub)
        .list_offerings(Representation::Terse, 0)
        .await
        .unwrap_err();
    assert!(
        matches!(&error, AgentError::InvalidResponse(message) if message.contains("redirect")),
        "{error}"
    );
}

#[tokio::test]
async fn refuses_a_redirect_that_names_nowhere() {
    let stub = scripted(vec![
        response(200, SERVICE_DOCUMENT, ODP_JSON),
        bare(302, &[]),
    ]);
    let error = client(&stub)
        .list_offerings(Representation::Terse, 0)
        .await
        .unwrap_err();
    assert!(
        matches!(&error, AgentError::InvalidResponse(message) if message.contains("Location")),
        "{error}"
    );
}

// -- limits ------------------------------------------------------------------------------

/// ERR-21: a Service Document is read to 65,536 bytes and a page to 524,288.
#[tokio::test]
async fn refuses_a_body_past_its_limit() {
    let padded = format!(
        r#"{{"description":"{}","http":{{"endpoint_base":"/odp"}},"language":"en","localizations":["en"],"name":"Plants","odp_version":"1.0","operations":[{{"authentication":"not-required","name":"get-offering"}},{{"authentication":"not-required","name":"list-offerings"}}]}}"#,
        "d".repeat(70_000)
    );
    let stub = Stub::new(move |_| Ok(response(200, padded.as_bytes(), ODP_JSON)));
    let error = client(&stub).inspect().await.unwrap_err();
    assert!(
        matches!(&error, AgentError::InvalidResponse(message) if message.contains("byte limit")),
        "{error}"
    );
}

/// ERR-20: a declared length past the limit is refused before the body is read at all.
#[tokio::test]
async fn refuses_a_declared_length_past_its_limit() {
    let stub = Stub::new(|_| {
        Ok(with_headers(
            200,
            SERVICE_DOCUMENT,
            ODP_JSON,
            &[("content-length", "99999999")],
        ))
    });
    let error = client(&stub).inspect().await.unwrap_err();
    assert!(
        matches!(&error, AgentError::InvalidResponse(message) if message.contains("byte limit")),
        "{error}"
    );
}

#[tokio::test]
async fn accepts_a_declared_length_within_its_limit() {
    let stub = Stub::new(|_| {
        Ok(with_headers(
            200,
            SERVICE_DOCUMENT,
            ODP_JSON,
            &[("content-length", &SERVICE_DOCUMENT.len().to_string())],
        ))
    });
    assert_eq!(
        client(&stub).inspect().await.unwrap().document.name,
        "Plants"
    );
}

/// ERR-21 gives a Problem Details response its own, much smaller limit.
#[tokio::test]
async fn refuses_a_problem_body_past_its_limit() {
    let body = format!(
        r#"{{"code":"X","detail":"{}","status":500,"title":"T","type":"https://offeringprotocol.org/problems/x"}}"#,
        "d".repeat(20_000)
    );
    let stub = Stub::new(move |_| Ok(response(500, body.as_bytes(), PROBLEM_JSON)));
    let error = client(&stub).inspect().await.unwrap_err();
    assert!(
        matches!(&error, AgentError::InvalidResponse(message) if message.contains("byte limit")),
        "{error}"
    );
}

// -- failures ----------------------------------------------------------------------------

/// A failure quotes a structured field of an RFC 9457 document, and nothing else.
#[tokio::test]
async fn quotes_only_the_problem_details_of_a_failure() {
    let stub = Stub::new(|_| {
        Ok(response(
            404,
            br#"{"code":"NOT_FOUND","detail":"Offering plant-9 does not exist","status":404,"title":"Not found","type":"https://offeringprotocol.org/problems/not-found"}"#,
            PROBLEM_JSON,
        ))
    });
    let error = client(&stub).inspect().await.unwrap_err();
    assert!(
        matches!(&error, AgentError::Request { message, status }
            if message == "Offering plant-9 does not exist" && *status == 404),
        "{error}"
    );
}

#[tokio::test]
async fn falls_back_to_the_problem_title() {
    let stub = Stub::new(|_| {
        Ok(response(
            500,
            br#"{"code":"INTERNAL_ERROR","status":500,"title":"Internal error","type":"https://offeringprotocol.org/problems/internal-error"}"#,
            PROBLEM_JSON,
        ))
    });
    let error = client(&stub).inspect().await.unwrap_err();
    assert!(
        matches!(&error, AgentError::Request { message, .. } if message == "Internal error"),
        "{error}"
    );
}

/// A body of any other media type says nothing this Agent should carry into its own errors.
#[tokio::test]
async fn repeats_nothing_of_a_body_that_is_not_problem_details() {
    for (body, content_type) in [
        (
            &b"<html><body>upstream failed\nGET /admin HTTP/1.1 token=secret</body></html>"[..],
            "text/html",
        ),
        (&b"not json at all"[..], PROBLEM_JSON),
        (&b"{}"[..], PROBLEM_JSON),
        (&b""[..], ""),
    ] {
        let stub = Stub::new(move |_| Ok(response(503, body, content_type)));
        let error = client(&stub).inspect().await.unwrap_err();
        let message = error.to_string();
        assert!(message.contains("503"), "{message}");
        assert!(!message.contains("secret"), "{message}");
        assert!(!message.contains("admin"), "{message}");
        assert!(!message.contains("<html>"), "{message}");
    }
}

/// A quoted field cannot forge a log line, however the Service wrote it.
#[tokio::test]
async fn flattens_the_text_it_quotes() {
    let stub = Stub::new(|_| {
        Ok(response(
            400,
            br#"{"code":"INVALID_REQUEST","detail":"forged\n\tINFO   ok","status":400,"title":"T","type":"https://offeringprotocol.org/problems/invalid-request"}"#,
            PROBLEM_JSON,
        ))
    });
    let error = client(&stub).inspect().await.unwrap_err();
    assert!(
        matches!(&error, AgentError::Request { message, .. } if message == "forged INFO ok"),
        "{error}"
    );
}

/// ERR-04 bounds what a Service may write, so a longer detail is not a Problem Details document
/// and nothing of it is quoted.
#[tokio::test]
async fn quotes_a_detail_up_to_its_limit_and_nothing_longer() {
    let at_limit = format!(
        r#"{{"code":"X","detail":"{}","status":400,"title":"T","type":"https://offeringprotocol.org/problems/x"}}"#,
        "d".repeat(2_048)
    );
    let stub = Stub::new(move |_| Ok(response(400, at_limit.as_bytes(), PROBLEM_JSON)));
    let error = client(&stub).inspect().await.unwrap_err();
    let AgentError::Request { message, .. } = error else {
        panic!("expected a request failure");
    };
    assert_eq!(message.chars().count(), 2_048);

    let past_limit = format!(
        r#"{{"code":"X","detail":"{}","status":400,"title":"T","type":"https://offeringprotocol.org/problems/x"}}"#,
        "d".repeat(2_049)
    );
    let stub = Stub::new(move |_| Ok(response(400, past_limit.as_bytes(), PROBLEM_JSON)));
    let error = client(&stub).inspect().await.unwrap_err();
    assert!(
        matches!(&error, AgentError::Request { message, .. } if !message.contains("dddd")),
        "{error}"
    );
}

#[tokio::test]
async fn reports_a_transport_failure_as_such() {
    let stub = Stub::new(|_| {
        Err(odp_directory::TransportError {
            message: "no route to host".to_owned(),
        })
    });
    assert!(matches!(
        client(&stub).inspect().await.unwrap_err(),
        AgentError::Transport(_)
    ));
}

// -- construction ------------------------------------------------------------------------

#[tokio::test]
async fn refuses_a_service_url_it_cannot_use() {
    for url in [
        "http://plants.example",
        "not a url",
        "https://user@plants.example",
        "ftp://plants.example",
    ] {
        assert!(ServiceClient::new(url).is_err(), "{url}");
    }
}

#[tokio::test]
async fn derives_the_service_origin_from_the_document_url() {
    let stub = Stub::serving(OFFERING);
    let client =
        ServiceClient::with_transport("https://PLANTS.example:443/.well-known/odp", stub.clone())
            .unwrap();
    let inspection = client.inspect().await.unwrap();

    assert_eq!(inspection.service_origin, ORIGIN);
    assert_eq!(
        inspection.requested_url,
        format!("{ORIGIN}/.well-known/odp")
    );
}

#[tokio::test]
async fn reaches_every_advertised_operation() {
    let stub = Stub::new(|request| {
        Ok(if request.url.ends_with("/.well-known/odp") {
            response(200, SERVICE_DOCUMENT, ODP_JSON)
        } else if request.url.contains("/collections/plants/offerings") {
            response(200, OFFERING_PAGE, ODP_JSON)
        } else if request.url.contains("/collections/plants") {
            response(200, COLLECTION, ODP_JSON)
        } else if request.url.contains("/collections") {
            response(200, COLLECTION_PAGE, ODP_JSON)
        } else if request.url.contains("/offerings/plant-1") {
            response(200, OFFERING, ODP_JSON)
        } else {
            response(200, OFFERING_PAGE, ODP_JSON)
        })
    });
    let client = Arc::new(client(&stub));

    assert_eq!(client.get_offering("plant-1").await.unwrap().id, "plant-1");
    assert_eq!(client.get_collection("plants").await.unwrap().id, "plants");
    assert_eq!(
        client
            .list_offerings(Representation::Terse, 0)
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(
        client
            .list_collections(Representation::Terse, 0)
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(
        client
            .list_collection_offerings("plants", Representation::Terse, 0)
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(
        client
            .search_collections(
                &odp_core::CollectionSearchRequest {
                    query: "plants".to_owned(),
                    ..odp_core::CollectionSearchRequest::default()
                },
                Representation::Terse
            )
            .await
            .unwrap()
            .items
            .len(),
        1
    );
}

/// IDN-08: an identifier the protocol could not carry is refused before a request is sent.
#[tokio::test]
async fn refuses_an_identifier_that_is_not_local() {
    let stub = Stub::serving(OFFERING);
    for id in ["", ".", "..", "a b", "a/b", "a?b", &"a".repeat(129)] {
        let error = client(&stub).get_offering(id).await.unwrap_err();
        assert!(
            matches!(error, AgentError::InvalidRequest(_)),
            "{id}: {error}"
        );
    }
    assert_eq!(stub.catalog_requests().len(), 0);
}

pub fn search(query: &str) -> odp_core::OfferingSearchRequest {
    odp_core::OfferingSearchRequest {
        odp_version: "1.0".to_owned(),
        query: query.to_owned(),
        ..odp_core::OfferingSearchRequest::default()
    }
}
