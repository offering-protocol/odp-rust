//! What the client sends, what it will read back, and what it says when a Directory refuses.

mod support;

use odp_directory::{DirectoryError, SearchRequest, SuggestionRequest};
use support::{
    JSON, ORIGIN, PROBLEM_JSON, SEARCH_URL, Stub, bare, client, json, page, service, typed,
    with_headers,
};

// -- the request ---------------------------------------------------------------------------

/// A search is a POST of JSON, and says so.
#[tokio::test]
async fn states_what_it_sends_and_what_it_accepts() {
    let stub = Stub::serving(page(&[], ""));
    client(&stub)
        .search_services(&SearchRequest::default())
        .await
        .unwrap();

    let request = stub.last();
    assert_eq!(request.method, "POST");
    assert_eq!(request.url, SEARCH_URL);
    assert_eq!(
        request.headers.get("accept").map(String::as_str),
        Some(JSON)
    );
    assert_eq!(
        request.headers.get("content-type").map(String::as_str),
        Some(JSON)
    );
}

/// A request with no body describes no content type, because there is no content to describe.
#[tokio::test]
async fn describes_no_content_type_without_a_body() {
    let stub = Stub::serving(r#"{"items":[]}"#);
    client(&stub)
        .suggest_services(&SuggestionRequest {
            filters: None,
            limit: 0,
            prefix: "pl".to_owned(),
        })
        .await
        .unwrap();

    let request = stub.last();
    assert_eq!(request.method, "GET");
    assert!(!request.headers.contains_key("content-type"));
}

/// Only the selected environment is ever contacted.
#[tokio::test]
async fn reaches_only_the_directory_it_was_built_for() {
    use odp_directory::{DirectoryClient, Environment};

    let stub = Stub::serving(page(&[], ""));
    DirectoryClient::with_transport(Environment::Sandbox, stub.clone())
        .search_services(&SearchRequest::default())
        .await
        .unwrap();

    assert_eq!(
        stub.last().url,
        "https://sandbox.inflowpay.ai/v1/services/search"
    );
}

/// A client built without a transport of its own brings the default one.
#[test]
fn builds_a_client_over_the_default_transport() {
    use odp_directory::{DirectoryClient, Environment};

    let client = DirectoryClient::new(Environment::Sandbox).unwrap();
    assert_eq!(client.environment(), Environment::Sandbox);
}

#[test]
fn names_an_origin_for_every_environment() {
    use odp_directory::{DirectoryClient, Environment};

    assert_eq!(Environment::Production.origin(), ORIGIN);
    assert_eq!(
        Environment::Sandbox.origin(),
        "https://sandbox.inflowpay.ai"
    );
    assert_eq!(Environment::default(), Environment::Production);

    let client = DirectoryClient::with_transport(Environment::Sandbox, Stub::serving("{}"));
    assert_eq!(client.environment(), Environment::Sandbox);
}

// -- media types ---------------------------------------------------------------------------

/// A result is JSON. Anything else describes something this client did not ask for.
#[tokio::test]
async fn refuses_a_result_of_another_media_type() {
    for content_type in ["text/html", PROBLEM_JSON, ""] {
        let stub = Stub::new(move |_| Ok(typed(200, page(&[], ""), content_type)));
        let error = client(&stub)
            .search_services(&SearchRequest::default())
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("application/json"),
            "{content_type}: {error}"
        );
    }
}

#[tokio::test]
async fn reads_a_media_type_with_parameters() {
    let stub = Stub::new(|_| Ok(typed(200, page(&[], ""), "application/JSON; charset=utf-8")));
    assert!(
        client(&stub)
            .search_services(&SearchRequest::default())
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn refuses_a_body_that_is_not_json() {
    let stub = Stub::serving("not json");
    assert!(
        client(&stub)
            .search_services(&SearchRequest::default())
            .await
            .is_err()
    );
}

// -- byte limits ---------------------------------------------------------------------------

/// ERR-20: a declared length past the limit is refused without trusting the body.
#[tokio::test]
async fn refuses_a_declared_length_past_the_limit() {
    let stub = Stub::new(|_| {
        Ok(with_headers(
            200,
            page(&[], ""),
            JSON,
            &[("content-length", "524289")],
        ))
    });
    let error = client(&stub)
        .search_services(&SearchRequest::default())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("524288"), "{error}");
}

#[tokio::test]
async fn accepts_a_declared_length_within_the_limit() {
    let body = page(&[service("https://plants.example")], "");
    let length = body.len().to_string();
    let stub = Stub::new(move |_| {
        Ok(with_headers(
            200,
            body.clone(),
            JSON,
            &[("content-length", &length)],
        ))
    });
    assert_eq!(
        client(&stub)
            .search_services(&SearchRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
}

#[tokio::test]
async fn refuses_a_body_past_the_limit() {
    let body = format!(r#"{{"items":[],"padding":"{}"}}"#, "x".repeat(524_300));
    let stub = Stub::serving(body);
    let error = client(&stub)
        .search_services(&SearchRequest::default())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("524288"), "{error}");
}

// -- failures ------------------------------------------------------------------------------

/// ERR-21: a failure is described from Problem Details, briefly, and with the headers kept.
#[tokio::test]
async fn describes_a_failure_from_its_problem_details() {
    let stub = Stub::new(|_| {
        Ok(with_headers(
            429,
            br#"{"detail":"Slow down.","status":429,"title":"Too Many Requests"}"#.to_vec(),
            PROBLEM_JSON,
            &[("retry-after", "30")],
        ))
    });
    let error = client(&stub)
        .search_services(&SearchRequest::default())
        .await
        .unwrap_err();

    let DirectoryError::Request {
        headers,
        message,
        status,
    } = error
    else {
        panic!("expected a request failure");
    };
    assert_eq!(status, 429);
    assert!(message.contains("Slow down."), "{message}");
    assert_eq!(headers.get("retry-after").map(String::as_str), Some("30"));
}

/// With no `detail`, the title says what happened.
#[tokio::test]
async fn falls_back_through_the_members_that_carry_a_reason() {
    for (body, expected) in [
        (r#"{"detail":"d","title":"t"}"#, "d"),
        (r#"{"detail":"","title":"t"}"#, "t"),
        (r#"{"message":"m"}"#, "m"),
    ] {
        let stub = Stub::new(move |_| Ok(typed(500, body.as_bytes().to_vec(), PROBLEM_JSON)));
        let error = client(&stub)
            .search_services(&SearchRequest::default())
            .await
            .unwrap_err()
            .to_string();
        assert!(error.ends_with(expected), "{body}: {error}");
    }
}

/// A failure body this client cannot read is not repeated at all.
#[tokio::test]
async fn repeats_nothing_of_a_failure_it_cannot_read() {
    let noisy = format!("<html>\u{0007}{}</html>", "A".repeat(64));
    for (body, content_type) in [
        (noisy.clone(), "text/html"),
        ("x".repeat(20_000), PROBLEM_JSON),
    ] {
        let stub = Stub::new(move |_| Ok(typed(503, body.clone().into_bytes(), content_type)));
        let error = client(&stub)
            .search_services(&SearchRequest::default())
            .await
            .unwrap_err()
            .to_string();
        assert!(error.ends_with("HTTP 503: "), "{content_type}: {error}");
    }
}

/// A body that is not Problem Details at all is quoted, but only as printable text.
#[tokio::test]
async fn flattens_and_shortens_whatever_it_does_quote() {
    let stub = Stub::new(|_| {
        Ok(typed(
            500,
            format!("first\u{0007}line\n\n{}", "b".repeat(4_000)).into_bytes(),
            JSON,
        ))
    });
    let error = client(&stub)
        .search_services(&SearchRequest::default())
        .await
        .unwrap_err()
        .to_string();

    assert!(
        !error.contains('\u{0007}'),
        "control characters are flattened"
    );
    assert!(error.contains("first line"), "{}", &error[..40]);
    assert!(error.ends_with('…'), "the excerpt is cut short");
    let detail = error.rsplit(": ").next().unwrap_or_default();
    assert!(
        detail.chars().count() <= 2_049,
        "{}",
        detail.chars().count()
    );
}

/// A failure body declaring more than the limit is not read either.
#[tokio::test]
async fn repeats_nothing_of_a_failure_that_declares_too_much() {
    let stub = Stub::new(|_| {
        Ok(with_headers(
            500,
            br#"{"detail":"short"}"#.to_vec(),
            PROBLEM_JSON,
            &[("content-length", "99999")],
        ))
    });
    let error = client(&stub)
        .search_services(&SearchRequest::default())
        .await
        .unwrap_err()
        .to_string();
    assert!(error.ends_with("HTTP 500: "), "{error}");
}

/// A transport that cannot reach the Directory says so as itself.
#[tokio::test]
async fn reports_a_transport_failure_as_such() {
    use odp_directory::TransportError;

    let stub = Stub::new(|_| {
        Err(TransportError {
            message: "connection reset".to_owned(),
        })
    });
    let error = client(&stub)
        .search_services(&SearchRequest::default())
        .await
        .unwrap_err();
    assert!(matches!(error, DirectoryError::Transport(_)), "{error}");
}

// -- redirects -----------------------------------------------------------------------------

/// ERR-24: a redirect is followed, and a POST that is told to look elsewhere becomes a GET.
#[tokio::test]
async fn continues_a_redirected_search_as_a_get() {
    for status in [301, 302, 303] {
        let stub = Stub::scripted(vec![
            bare(status, &[("location", "/v1/services/search/v2")]),
            json(200, page(&[], "")),
        ]);
        client(&stub)
            .search_services(&SearchRequest::default())
            .await
            .unwrap();

        let last = stub.last();
        assert_eq!(last.method, "GET", "{status}");
        assert!(last.body.is_empty(), "{status}");
        assert_eq!(last.url, "https://api.inflowpay.ai/v1/services/search/v2");
    }
}

/// 307 and 308 preserve the method and the body they were given.
#[tokio::test]
async fn keeps_the_method_across_a_preserving_redirect() {
    for status in [307, 308] {
        let stub = Stub::scripted(vec![
            bare(status, &[("location", "/v1/services/search/v2")]),
            json(200, page(&[], "")),
        ]);
        client(&stub)
            .search_services(&SearchRequest {
                query: "plants".to_owned(),
                ..SearchRequest::default()
            })
            .await
            .unwrap();

        let last = stub.last();
        assert_eq!(last.method, "POST", "{status}");
        assert!(!last.body.is_empty(), "{status}");
    }
}

/// A redirect off the Directory's own origin is somebody else's answer.
#[tokio::test]
async fn refuses_a_redirect_that_leaves_the_directory() {
    for location in [
        "https://elsewhere.example/v1/services/search",
        "http://api.inflowpay.ai/v1/services/search",
    ] {
        let stub = Stub::new(move |_| Ok(bare(302, &[("location", location)])));
        let error = client(&stub)
            .search_services(&SearchRequest::default())
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("changed origin"),
            "{location}: {error}"
        );
    }
}

#[tokio::test]
async fn refuses_a_redirect_that_names_nowhere() {
    let stub = Stub::new(|_| Ok(bare(308, &[])));
    let error = client(&stub)
        .search_services(&SearchRequest::default())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Location"), "{error}");
}

/// ERR-25: five redirects are followed; the sixth is one too many.
#[tokio::test]
async fn refuses_a_sixth_redirect() {
    let stub = Stub::new(|request| {
        let step: usize = request
            .url
            .split("step=")
            .nth(1)
            .and_then(|value| value.parse().ok())
            .unwrap_or_default();
        Ok(bare(
            308,
            &[(
                "location",
                &format!("/v1/services/search?step={}", step + 1),
            )],
        ))
    });
    let error = client(&stub)
        .search_services(&SearchRequest::default())
        .await
        .unwrap_err();

    assert!(error.to_string().contains("five redirects"), "{error}");
    assert_eq!(stub.count(), 6, "five were followed, the sixth was not");
}

#[tokio::test]
async fn follows_five_redirects_to_an_answer() {
    let mut steps = (0..5)
        .map(|step| bare(308, &[("location", &format!("/v1/services/search/{step}"))]))
        .collect::<Vec<_>>();
    steps.push(json(200, page(&[service("https://plants.example")], "")));
    let stub = Stub::scripted(steps);

    let result = client(&stub)
        .search_services(&SearchRequest::default())
        .await
        .unwrap();
    assert_eq!(result.items.len(), 1);
    assert_eq!(stub.count(), 6);
}
