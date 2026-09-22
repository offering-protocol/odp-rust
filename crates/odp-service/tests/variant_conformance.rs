//! SVC-58/59/60/61 and PAG-31: which variant a Service serves, and how an Agent revalidates it.

mod support;

use support::{Stub, get, header, localized, service, with_header};

/// The localizations a multilingual Service in these tests publishes.
const TAGS: &[&str] = &["en", "en-GB", "fr", "de-CH", "zh-Hant"];

/// SVC-58: RFC 4647 Lookup, which walks a range down its own subtags and never sideways.
#[tokio::test]
async fn selects_a_language_by_lookup() {
    for (accept, expected) in [
        ("fr", "fr"),
        ("FR", "fr"),
        ("en-GB", "en-GB"),
        // Lookup truncates: en-GB-oed has no match, en-GB does.
        ("en-GB-oed", "en-GB"),
        // de-CH-1901 falls back to de-CH, never sideways to another de-* tag.
        ("de-CH-1901", "de-CH"),
        ("zh-Hant-TW", "zh-Hant"),
        // A range with no match at all falls back to the default representation.
        ("de-DE", "en"),
        ("ja", "en"),
        ("*", "en"),
    ] {
        let response = localized(Stub::serving(1), TAGS)
            .handle(with_header(
                get("/.well-known/odp"),
                "accept-language",
                accept,
            ))
            .await;

        assert_eq!(response.status, 200, "{accept}");
        assert_eq!(
            header(&response, "content-language"),
            Some(expected),
            "{accept}"
        );
    }
}

/// Weight orders the ranges, and a range weighted zero is not wanted at all.
#[tokio::test]
async fn honours_the_order_the_agent_asked_in() {
    for (accept, expected) in [
        ("fr;q=0.5, de-CH;q=0.9", "de-CH"),
        ("de-CH;q=0.1, fr", "fr"),
        ("fr, de-CH", "fr"),
        ("fr;q=0, de-CH", "de-CH"),
        // Every range refused leaves only the default.
        ("fr;q=0, de-CH;q=0", "en"),
    ] {
        let response = localized(Stub::serving(1), TAGS)
            .handle(with_header(
                get("/.well-known/odp"),
                "accept-language",
                accept,
            ))
            .await;
        assert_eq!(
            header(&response, "content-language"),
            Some(expected),
            "{accept}"
        );
    }
}

/// SVC-59: nothing matching is not a reason to refuse the request.
#[tokio::test]
async fn never_refuses_a_request_over_language() {
    let response = localized(Stub::serving(1), TAGS)
        .handle(with_header(
            get("/odp/offerings"),
            "accept-language",
            "ja, ko",
        ))
        .await;

    assert_eq!(response.status, 200);
    assert_eq!(header(&response, "content-language"), Some("en"));
}

/// SVC-60: a localized response says which variant it is and what the choice depended on.
#[tokio::test]
async fn describes_the_variant_it_served() {
    for path in [
        "/.well-known/odp",
        "/odp/offerings",
        "/odp/offerings/plant-1",
        "/odp/collections",
        "/odp/collections/plants",
        "/odp/collections/plants/offerings",
    ] {
        let response = localized(Stub::serving(1), TAGS)
            .handle(with_header(get(path), "accept-language", "fr"))
            .await;

        assert_eq!(header(&response, "content-language"), Some("fr"), "{path}");
        assert_eq!(header(&response, "vary"), Some("Accept-Language"), "{path}");
    }
}

/// The catalog is told which variant to answer in, not left to parse the field itself.
#[tokio::test]
async fn tells_the_catalog_which_variant_to_answer_in() {
    let stub = Stub::serving(1);
    localized(stub.clone(), TAGS)
        .handle(with_header(
            get("/odp/offerings"),
            "accept-language",
            "de-CH-1901, fr",
        ))
        .await;

    let request = stub.last();
    assert_eq!(request.language, "de-CH");
    assert_eq!(request.accept_language.as_deref(), Some("de-CH-1901, fr"));
}

/// With no field at all the Service serves its own language.
#[tokio::test]
async fn serves_its_own_language_when_none_was_asked_for() {
    let response = localized(Stub::serving(1), TAGS)
        .handle(get("/.well-known/odp"))
        .await;
    assert_eq!(header(&response, "content-language"), Some("en"));
}

// -- entity tags ----------------------------------------------------------------------------

/// SVC-61: every representation carries a validator.
#[tokio::test]
async fn tags_every_representation_it_serves() {
    for path in [
        "/.well-known/odp",
        "/odp/offerings",
        "/odp/offerings/plant-1",
        "/odp/collections",
        "/odp/collections/plants",
        "/odp/collections/plants/offerings",
    ] {
        let response = service(Stub::serving(1)).handle(get(path)).await;
        let etag = header(&response, "etag").unwrap_or_default();

        assert!(
            etag.starts_with('"') && etag.ends_with('"'),
            "{path}: {etag}"
        );
        assert!(etag.len() > 2, "{path}");
    }
}

/// SVC-61: a tag distinguishes variants, so two languages never share one.
#[tokio::test]
async fn gives_each_variant_a_tag_of_its_own() {
    let service = localized(Stub::serving(1), TAGS);
    let english = service
        .handle(with_header(
            get("/.well-known/odp"),
            "accept-language",
            "en",
        ))
        .await;
    let french = service
        .handle(with_header(
            get("/.well-known/odp"),
            "accept-language",
            "fr",
        ))
        .await;

    assert_ne!(header(&english, "etag"), header(&french, "etag"));
}

/// The same representation asked for twice carries the same tag.
#[tokio::test]
async fn gives_one_representation_one_tag() {
    let service = service(Stub::serving(3));
    let first = service.handle(get("/odp/offerings")).await;
    let second = service.handle(get("/odp/offerings")).await;

    assert_eq!(header(&first, "etag"), header(&second, "etag"));
}

/// Two different representations of one resource are two variants.
#[tokio::test]
async fn distinguishes_terse_from_full() {
    let service = service(Stub::serving(1));
    let terse = service
        .handle(support::query(
            "/odp/offerings/plant-1",
            "representation=terse",
        ))
        .await;
    let full = service
        .handle(support::query(
            "/odp/offerings/plant-1",
            "representation=full",
        ))
        .await;

    assert_ne!(header(&terse, "etag"), header(&full, "etag"));
}

/// PAG-31: a validator that still matches means the Agent already holds this representation.
#[tokio::test]
async fn answers_a_conditional_request_that_still_matches() {
    let service = service(Stub::serving(2));
    let first = service.handle(get("/odp/offerings")).await;
    let etag = header(&first, "etag").unwrap().to_owned();

    let second = service
        .handle(with_header(get("/odp/offerings"), "if-none-match", &etag))
        .await;

    assert_eq!(second.status, 304);
    assert!(second.body.is_empty());
    assert_eq!(header(&second, "etag"), Some(etag.as_str()));
    assert_eq!(header(&second, "content-type"), None);
}

/// A field listing several tags matches when any of them is this one, weakly compared.
#[tokio::test]
async fn reads_every_form_a_conditional_field_takes() {
    let service = service(Stub::serving(2));
    let etag = header(&service.handle(get("/odp/offerings")).await, "etag")
        .unwrap()
        .to_owned();
    let weak = format!("W/{etag}");
    let listed = format!("\"other\", {etag}");

    for value in [etag.as_str(), weak.as_str(), listed.as_str(), "*"] {
        let response = service
            .handle(with_header(get("/odp/offerings"), "if-none-match", value))
            .await;
        assert_eq!(response.status, 304, "{value}");
    }
}

/// A validator for something else is no reason to withhold the representation.
#[tokio::test]
async fn serves_a_conditional_request_that_no_longer_matches() {
    let response = service(Stub::serving(2))
        .handle(with_header(
            get("/odp/offerings"),
            "if-none-match",
            "\"something-else\"",
        ))
        .await;

    assert_eq!(response.status, 200);
    assert!(!response.body.is_empty());
}

/// A conditional HEAD is answered like a conditional GET.
#[tokio::test]
async fn answers_a_conditional_head() {
    let service = service(Stub::serving(2));
    let etag = header(&service.handle(get("/odp/offerings")).await, "etag")
        .unwrap()
        .to_owned();

    let mut request = with_header(get("/odp/offerings"), "if-none-match", &etag);
    request.method = "HEAD".to_owned();
    let response = service.handle(request).await;

    assert_eq!(response.status, 304);
    assert!(response.body.is_empty());
}

/// A conditional request that fails for another reason is not turned into a 304.
#[tokio::test]
async fn does_not_confuse_a_failure_with_an_unchanged_representation() {
    let response = service(Stub::serving(1))
        .handle(with_header(
            get("/odp/offerings/absent"),
            "if-none-match",
            "*",
        ))
        .await;
    assert_eq!(response.status, 404);
}

/// A Service advertising one language still describes the variant it serves.
#[tokio::test]
async fn describes_the_variant_of_a_monolingual_service() {
    let response = service(Stub::serving(1))
        .handle(get("/.well-known/odp"))
        .await;

    assert_eq!(header(&response, "content-language"), Some("en"));
    assert_eq!(header(&response, "vary"), Some("Accept-Language"));
}
