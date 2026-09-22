//! MED-03/04/06/07, SVC-73, ERR-01..07 and ERR-31..33: the HTTP contract a Service owes an Agent.

mod support;

use odp_core::parse_problem_details;
use odp_service::{MEDIA_TYPE, PROBLEM_MEDIA_TYPE, Request};
use support::{Answer, Stub, get, header, post, query, service, with_header};

/// The smallest body each search operation accepts.
const SEARCH: &[u8] = br#"{"odp_version":"1.0","query":"plants"}"#;
const COLLECTION_SEARCH: &[u8] = br#"{"odp_version":"1.0","query":"plants"}"#;

#[tokio::test]
async fn search_limits_are_forwarded_and_enforced() {
    for path in ["/odp/offerings/search", "/odp/collections/search"] {
        let catalog = Stub::serving(2);
        let response = service(catalog.clone())
            .handle(post(
                path,
                br#"{"odp_version":"1.0","query":"plants","limit":1}"#,
            ))
            .await;
        assert_eq!(catalog.last().limit, 1);
        assert_eq!(response.status, 500);
    }
}

#[tokio::test]
async fn search_continuations_receive_the_opaque_cursor() {
    for path in ["/odp/offerings/search", "/odp/collections/search"] {
        let catalog = Stub::serving(1);
        let response = service(catalog.clone())
            .handle(query(path, "cursor=opaque-token"))
            .await;
        assert_eq!(response.status, 200);
        assert_eq!(catalog.last().cursor.as_deref(), Some("opaque-token"));
    }
}

#[tokio::test]
async fn post_preconditions_do_not_return_not_modified() {
    let service = service(Stub::serving(1));
    let response = service.handle(post("/odp/offerings/search", SEARCH)).await;
    let etag = header(&response, "etag").unwrap();
    let response = service
        .handle(with_header(
            post("/odp/offerings/search", SEARCH),
            "if-none-match",
            etag,
        ))
        .await;
    assert_eq!(response.status, 412);
}

// -- media types ----------------------------------------------------------------------------

/// MED-07: a successful ODP document is served as one.
#[tokio::test]
async fn serves_an_odp_document_as_odp() {
    let response = service(Stub::serving(1))
        .handle(get("/.well-known/odp"))
        .await;

    assert_eq!(response.status, 200);
    assert_eq!(header(&response, "content-type"), Some(MEDIA_TYPE));
}

/// MED-03: an absent or wildcard `Accept` permits an ODP representation.
#[tokio::test]
async fn answers_an_accept_that_permits_odp() {
    for accept in [
        None,
        Some(""),
        Some("*/*"),
        Some("application/*"),
        Some(MEDIA_TYPE),
        Some("text/html, application/odp+json;q=0.9"),
        Some("APPLICATION/ODP+JSON"),
        Some("application/odp+json; charset=utf-8"),
    ] {
        let mut request = get("/.well-known/odp");
        match accept {
            Some(value) => {
                request
                    .headers
                    .insert("accept".to_owned(), value.to_owned());
            }
            None => {
                request.headers.remove("accept");
            }
        }
        let response = service(Stub::serving(1)).handle(request).await;
        assert_eq!(response.status, 200, "{accept:?}");
    }
}

/// MED-04: an `Accept` that excludes the media type is refused, however it says so.
#[tokio::test]
async fn refuses_an_accept_that_excludes_odp() {
    for accept in [
        "text/html",
        "application/json",
        "application/odp+json;q=0",
        "*/*;q=0",
        "application/*;q=0",
        "text/html, application/odp+json;q=0",
    ] {
        let response = service(Stub::serving(1))
            .handle(with_header(get("/.well-known/odp"), "accept", accept))
            .await;
        assert_eq!(response.status, 406, "{accept}");
        assert_eq!(header(&response, "content-type"), Some(PROBLEM_MEDIA_TYPE));
    }
}

/// A more specific range decides, so a wildcard refusal does not override an explicit welcome.
#[tokio::test]
async fn lets_the_most_specific_range_decide() {
    let response = service(Stub::serving(1))
        .handle(with_header(
            get("/.well-known/odp"),
            "accept",
            "*/*;q=0, application/odp+json",
        ))
        .await;
    assert_eq!(response.status, 200);
}

/// MED-06: a search body is ODP JSON, compared by media-type essence.
#[tokio::test]
async fn reads_a_body_whose_media_type_essence_matches() {
    for content_type in [
        MEDIA_TYPE,
        "APPLICATION/ODP+JSON",
        " application/odp+json ",
        "application/odp+json ; charset=utf-8",
    ] {
        let response = service(Stub::serving(1))
            .handle(with_header(
                post("/odp/offerings/search", SEARCH),
                "content-type",
                content_type,
            ))
            .await;
        assert_eq!(response.status, 200, "{content_type}");
    }
}

#[tokio::test]
async fn refuses_a_body_of_another_media_type() {
    for content_type in ["application/json", "text/plain", ""] {
        let response = service(Stub::serving(1))
            .handle(with_header(
                post("/odp/offerings/search", SEARCH),
                "content-type",
                content_type,
            ))
            .await;
        assert_eq!(response.status, 415, "{content_type}");
    }

    let mut request = post("/odp/offerings/search", SEARCH);
    request.headers.remove("content-type");
    let response = service(Stub::serving(1)).handle(request).await;
    assert_eq!(response.status, 415, "a missing Content-Type is not ODP");
}

/// ERR-31: an oversized request is refused rather than read.
#[tokio::test]
async fn refuses_a_request_body_past_the_limit() {
    let response = service(Stub::serving(1))
        .handle(post("/odp/offerings/search", &vec![b' '; 65_537]))
        .await;

    assert_eq!(response.status, 413);
    assert_eq!(
        parse_problem_details(&response.body).unwrap().code,
        "REQUEST_TOO_LARGE"
    );
}

// -- methods --------------------------------------------------------------------------------

/// A resource that answers GET answers HEAD, with the same headers and no body.
#[tokio::test]
async fn answers_head_wherever_it_answers_get() {
    for path in [
        "/.well-known/odp",
        "/odp/offerings",
        "/odp/offerings/plant-1",
        "/odp/collections",
        "/odp/collections/plants",
        "/odp/collections/plants/offerings",
    ] {
        let body = service(Stub::serving(2)).handle(get(path)).await;
        let mut head = get(path);
        head.method = "HEAD".to_owned();
        let headless = service(Stub::serving(2)).handle(head).await;

        assert_eq!(headless.status, body.status, "{path}");
        assert!(headless.body.is_empty(), "{path}");
        assert_eq!(
            header(&headless, "content-type"),
            header(&body, "content-type"),
            "{path}"
        );
        assert_eq!(header(&headless, "etag"), header(&body, "etag"), "{path}");
        assert_eq!(
            header(&headless, "content-length").map(str::to_owned),
            Some(body.body.len().to_string()),
            "{path}"
        );
    }
}

/// A 405 states what the resource does allow.
#[tokio::test]
async fn states_what_a_refused_method_could_have_used() {
    for (method, path, allow) in [
        ("DELETE", "/odp/offerings", "GET, HEAD"),
        ("PUT", "/odp/offerings/plant-1", "GET, HEAD"),
        ("PUT", "/odp/offerings/search", "POST, GET, HEAD"),
        ("POST", "/.well-known/odp", "GET, HEAD"),
    ] {
        let mut request = get(path);
        request.method = method.to_owned();
        let response = service(Stub::serving(1)).handle(request).await;

        assert_eq!(response.status, 405, "{method} {path}");
        assert_eq!(header(&response, "allow"), Some(allow), "{method} {path}");
    }
}

// -- query parameters -----------------------------------------------------------------------

/// SVC-73: a repeated `representation` describes two requests, and neither is answered.
#[tokio::test]
async fn refuses_a_repeated_single_valued_parameter() {
    for value in [
        "representation=terse&representation=full",
        "representation=full&representation=full",
        "limit=1&limit=2",
        "cursor=a&cursor=b",
    ] {
        let response = service(Stub::serving(1))
            .handle(query("/odp/offerings", value))
            .await;
        assert_eq!(response.status, 400, "{value}");
        assert_eq!(
            parse_problem_details(&response.body).unwrap().code,
            "INVALID_REQUEST"
        );
    }
}

/// SVC-73: and an unsupported value is refused the same way.
#[tokio::test]
async fn refuses_an_unsupported_representation() {
    let response = service(Stub::serving(1))
        .handle(query("/odp/offerings", "representation=summary"))
        .await;
    assert_eq!(response.status, 400);
}

#[tokio::test]
async fn refuses_a_limit_it_cannot_serve() {
    for value in ["limit=0", "limit=101", "limit=-1", "limit=many", "limit="] {
        let response = service(Stub::serving(1))
            .handle(query("/odp/offerings", value))
            .await;
        assert_eq!(response.status, 400, "{value}");
    }
}

#[tokio::test]
async fn passes_the_request_through_to_the_catalog() {
    let stub = Stub::serving(1);
    service(stub.clone())
        .handle(query(
            "/odp/offerings",
            "cursor=abc&limit=7&representation=full",
        ))
        .await;

    let request = stub.last();
    assert_eq!(request.cursor.as_deref(), Some("abc"));
    assert_eq!(request.limit, 7);
    assert_eq!(request.representation, odp_core::Representation::Full);
    assert_eq!(request.path, "/odp/offerings");
}

// -- problem details ------------------------------------------------------------------------

/// ERR-02, ERR-03 and ERR-07: a problem describes itself consistently.
#[tokio::test]
async fn describes_every_failure_as_problem_details() {
    for (request, status, code) in [
        (get("/odp/nowhere"), 404, "NOT_FOUND"),
        (get("/odp/offerings/not a valid id"), 400, "INVALID_REQUEST"),
        (get("/odp/offerings/absent"), 404, "NOT_FOUND"),
    ] {
        let response = service(Stub::serving(1)).handle(request).await;
        let problem = parse_problem_details(&response.body).unwrap();

        assert_eq!(response.status, status);
        assert_eq!(problem.status, status);
        assert_eq!(problem.code, code);
        assert_eq!(
            problem.problem_type,
            format!(
                "https://offeringprotocol.org/problems/{}",
                code.to_ascii_lowercase().replace('_', "-")
            )
        );
        assert_eq!(header(&response, "content-type"), Some(PROBLEM_MEDIA_TYPE));
    }
}

/// ERR-04: a Problem Details object that breaks its own limits describes nothing, so a long
/// failure is cut to fit rather than serialized whole.
#[tokio::test]
async fn keeps_a_long_failure_within_the_problem_limits() {
    let response = service(Stub::new(Answer::LongFailure(5_000)))
        .handle(get("/odp/offerings"))
        .await;

    assert_eq!(response.status, 500);
    let problem = parse_problem_details(&response.body).expect("a valid Problem Details object");
    assert!(problem.title.chars().count() <= 128);
    assert!(problem.detail.chars().count() <= 2_048);
    assert!(problem.title.ends_with('…'));
}

/// ERR-32: a 429 says when to come back, whether or not the catalog named an interval.
#[tokio::test]
async fn names_a_retry_interval_where_one_is_required() {
    for (status, seconds, expected) in [
        (429_u16, None, "1"),
        (429, Some(30), "30"),
        (503, None, "1"),
        (503, Some(120), "120"),
    ] {
        let response = service(Stub::new(Answer::Fail(status, "UNAVAILABLE", seconds)))
            .handle(get("/odp/offerings"))
            .await;

        assert_eq!(response.status, status);
        assert_eq!(header(&response, "retry-after"), Some(expected), "{status}");
    }
}

/// A failure that is nobody's fault to wait on carries no interval.
#[tokio::test]
async fn names_no_retry_interval_where_none_applies() {
    let response = service(Stub::new(Answer::Fail(404, "NOT_FOUND", None)))
        .handle(get("/odp/offerings"))
        .await;

    assert_eq!(response.status, 404);
    assert_eq!(header(&response, "retry-after"), None);
}

/// A catalog may still name one on another status when it knows something useful.
#[tokio::test]
async fn carries_an_interval_a_catalog_names_on_any_status() {
    let response = service(Stub::new(Answer::Fail(409, "CONFLICT", Some(5))))
        .handle(get("/odp/offerings"))
        .await;
    assert_eq!(header(&response, "retry-after"), Some("5"));
}

// -- routing --------------------------------------------------------------------------------

/// ROLE-04: a Service answers only for operations it advertises.
#[tokio::test]
async fn refuses_an_operation_it_does_not_advertise() {
    use odp_core::Operation;

    let stub = Stub::with_operations(
        Answer::Items(1, String::new()),
        vec![Operation::GetOffering, Operation::ListOfferings],
    );
    let service = odp_service::Service::new(support::document(&["en"]), stub).unwrap();

    for request in [
        get("/odp/collections"),
        get("/odp/collections/plants"),
        get("/odp/collections/plants/offerings"),
        post("/odp/offerings/search", SEARCH),
        post("/odp/collections/search", COLLECTION_SEARCH),
    ] {
        let path = request.path.clone();
        let response = service.handle(request).await;
        assert_eq!(response.status, 404, "{path}");
    }
}

#[tokio::test]
async fn answers_nothing_outside_its_endpoint_base() {
    for path in ["/offerings", "/other/offerings", "/", "/odp"] {
        let response = service(Stub::serving(1)).handle(get(path)).await;
        assert_eq!(response.status, 404, "{path}");
    }
}

/// The Service Document is served from the well-known path whatever the endpoint base is.
#[tokio::test]
async fn serves_the_service_document_from_the_well_known_path() {
    let response = service(Stub::serving(1))
        .handle(get("/.well-known/odp"))
        .await;
    let document = odp_core::parse_service_document(&response.body).unwrap();

    assert_eq!(document.name, "Plants");
    assert_eq!(document.http.endpoint_base, "/odp");
}

#[tokio::test]
async fn reports_a_body_it_cannot_read_as_the_agent_s_mistake() {
    let response = service(Stub::serving(1))
        .handle(post("/odp/offerings/search", b"not json"))
        .await;

    assert_eq!(response.status, 400);
    assert_eq!(
        parse_problem_details(&response.body).unwrap().code,
        "INVALID_REQUEST"
    );
}

#[tokio::test]
async fn serves_a_collection_search() {
    let response = service(Stub::serving(2))
        .handle(post("/odp/collections/search", COLLECTION_SEARCH))
        .await;
    assert_eq!(response.status, 200);
    assert_eq!(
        odp_core::parse_page::<serde_json::Value>(&response.body)
            .unwrap()
            .items
            .len(),
        2
    );
}

#[tokio::test]
async fn serves_a_request_with_no_accept_at_all() {
    let response = service(Stub::serving(1))
        .handle(Request {
            method: "GET".to_owned(),
            path: "/.well-known/odp".to_owned(),
            ..Request::default()
        })
        .await;
    assert_eq!(response.status, 200);
}

/// A parameter with no value at all is not a quality value.
#[tokio::test]
async fn reads_a_media_range_whose_parameters_carry_no_value() {
    let response = service(Stub::serving(1))
        .handle(with_header(
            get("/.well-known/odp"),
            "accept",
            "application/odp+json;charset;q=0.8",
        ))
        .await;
    assert_eq!(response.status, 200);
}

/// A language range that is nothing but an extension prefix matches nothing, and says so
/// by falling back rather than looping.
#[tokio::test]
async fn falls_back_from_a_range_that_is_only_a_prefix() {
    for accept in ["a-b", "x-private", "i-default"] {
        let response = service(Stub::serving(1))
            .handle(with_header(
                get("/.well-known/odp"),
                "accept-language",
                accept,
            ))
            .await;
        assert_eq!(response.status, 200, "{accept}");
        assert_eq!(
            header(&response, "content-language"),
            Some("en"),
            "{accept}"
        );
    }
}

/// ERR-19: a Service produces documents within the applicable limits, or serves none.
#[tokio::test]
async fn refuses_to_publish_a_response_past_its_limit() {
    use odp_core::{AdditionalMembers, Offering};

    let mut offering = support::offering("plant-1");
    offering.additional = AdditionalMembers::from([(
        "padding".to_owned(),
        serde_json::Value::String("x".repeat(600_000)),
    )]);
    let page = support::offering_page(vec![offering.clone()], "");

    struct Huge(odp_core::OfferingPage<Offering>);

    #[async_trait::async_trait]
    impl odp_service::Catalog for Huge {
        fn operations(&self) -> Vec<odp_core::Operation> {
            support::ALL_OPERATIONS.to_vec()
        }

        async fn list_offerings(
            &self,
            _request: odp_service::CatalogRequest,
        ) -> Result<odp_core::OfferingPage<Offering>, odp_service::ServiceError> {
            Ok(self.0.clone())
        }

        async fn get_offering(
            &self,
            _id: &str,
            _request: odp_service::CatalogRequest,
        ) -> Result<Option<Offering>, odp_service::ServiceError> {
            Ok(None)
        }
    }

    let service =
        odp_service::Service::new(support::document(&["en"]), std::sync::Arc::new(Huge(page)))
            .unwrap();
    let response = service.handle(get("/odp/offerings")).await;
    assert_eq!(response.status, 500);
}
