//! CCH: what the Agent stores, what it revalidates, and what it keeps apart.

mod support;

use std::{sync::Arc, time::Duration};

use odp_agent::{AgentError, CacheFallbacks, Freshness, MemoryCache, ServiceClient};
use odp_core::Representation;
use support::{
    ODP_JSON, OFFERING_PAGE, ORIGIN, SCHEMA_JSON, SERVICE_DOCUMENT, Stub, bare, client, response,
    scripted, with_headers,
};

/// CCH-02: a response supplying no freshness is kept for the class's fallback lifetime.
#[tokio::test]
async fn serves_a_fresh_document_from_the_cache() {
    let stub = Stub::serving(OFFERING_PAGE);
    let client = client(&stub);

    assert_eq!(
        client.inspect().await.unwrap().freshness,
        Freshness::Fetched
    );
    assert_eq!(client.inspect().await.unwrap().freshness, Freshness::Fresh);
    assert_eq!(stub.count(), 1, "the second read never left the Agent");
}

/// CCH-03: every class the draft names is configurable on its own.
#[tokio::test]
async fn keeps_a_fallback_for_every_resource_class() {
    let fallbacks = CacheFallbacks::default();

    assert_eq!(fallbacks.service_document, Duration::from_secs(4 * 60 * 60));
    assert_eq!(fallbacks.collection, Duration::from_secs(60 * 60));
    assert_eq!(fallbacks.offering, Duration::from_secs(5 * 60));
    assert_eq!(fallbacks.capabilities, Duration::from_secs(60 * 60));
    assert_eq!(fallbacks.schema, Duration::from_secs(24 * 60 * 60));
    assert_eq!(
        fallbacks.search,
        Duration::ZERO,
        "a search answers one request and is not reused for the next"
    );
}

/// A fallback of zero keeps nothing, so the next read reaches the Service again.
#[tokio::test]
async fn stores_nothing_for_a_class_whose_fallback_is_zero() {
    let stub = Stub::serving(OFFERING_PAGE);
    let client = client(&stub).with_cache_fallbacks(CacheFallbacks {
        service_document: Duration::ZERO,
        ..CacheFallbacks::default()
    });

    client.inspect().await.unwrap();
    client.inspect().await.unwrap();
    assert_eq!(stub.count(), 2);
}

/// CCH-04: a fallback never overrides what the Service said.
#[tokio::test]
async fn lets_the_service_override_the_fallback() {
    let stub = Stub::new(|_| {
        Ok(with_headers(
            200,
            SERVICE_DOCUMENT,
            ODP_JSON,
            &[("cache-control", "no-store")],
        ))
    });
    let client = client(&stub);

    client.inspect().await.unwrap();
    client.inspect().await.unwrap();
    assert_eq!(
        stub.count(),
        2,
        "no-store is not cached whatever the fallback"
    );
}

#[tokio::test]
async fn reads_freshness_from_the_directives_the_service_sent() {
    for (directive, reached) in [
        ("max-age=600", 1),
        ("max-age=0", 2),
        ("no-cache", 2),
        ("no-store", 2),
    ] {
        let stub = Stub::new(move |_| {
            Ok(with_headers(
                200,
                SERVICE_DOCUMENT,
                ODP_JSON,
                &[("cache-control", directive)],
            ))
        });
        let client = client(&stub);
        client.inspect().await.unwrap();
        let _ = client.inspect().await;
        assert_eq!(stub.count(), reached, "{directive}");
    }
}

/// A stored response ages by the time it already spent in an intermediary.
#[tokio::test]
async fn subtracts_the_age_a_response_arrived_with() {
    let stub = Stub::new(|_| {
        Ok(with_headers(
            200,
            SERVICE_DOCUMENT,
            ODP_JSON,
            &[("cache-control", "max-age=60"), ("age", "120")],
        ))
    });
    let client = client(&stub);

    client.inspect().await.unwrap();
    let _ = client.inspect().await;
    assert_eq!(stub.count(), 2, "it arrived already stale");
}

#[tokio::test]
async fn reads_an_expires_date_when_no_directive_says_otherwise() {
    let stub = Stub::new(|_| {
        Ok(with_headers(
            200,
            SERVICE_DOCUMENT,
            ODP_JSON,
            &[("expires", "Sun, 06 Nov 1994 08:49:37 GMT")],
        ))
    });
    let client = client(&stub);

    client.inspect().await.unwrap();
    let _ = client.inspect().await;
    assert_eq!(stub.count(), 2, "a date in the past is already stale");
}

/// CCH-01: a stale entry is revalidated rather than discarded.
#[tokio::test]
async fn revalidates_a_stale_entry_with_its_validators() {
    let stub = scripted(vec![
        with_headers(
            200,
            SERVICE_DOCUMENT,
            ODP_JSON,
            &[
                ("cache-control", "max-age=0"),
                ("etag", "\"v1\""),
                ("last-modified", "Sun, 06 Nov 1994 08:49:37 GMT"),
            ],
        ),
        bare(304, &[("cache-control", "max-age=600")]),
    ]);
    let client = client(&stub);

    client.inspect().await.unwrap();
    let second = client.inspect().await.unwrap();
    assert_eq!(second.freshness, Freshness::Revalidated);
    assert_eq!(second.document.name, "Plants");

    let conditional = stub.last();
    assert_eq!(
        conditional.headers.get("if-none-match").map(String::as_str),
        Some("\"v1\"")
    );
    assert_eq!(
        conditional
            .headers
            .get("if-modified-since")
            .map(String::as_str),
        Some("Sun, 06 Nov 1994 08:49:37 GMT")
    );

    let third = client.inspect().await.unwrap();
    assert_eq!(third.freshness, Freshness::Fresh, "the 304 refreshed it");
}

/// A 304 telling the Agent to store nothing drops the entry it revalidated.
#[tokio::test]
async fn drops_an_entry_a_revalidation_told_it_not_to_store() {
    let stub = scripted(vec![
        with_headers(
            200,
            SERVICE_DOCUMENT,
            ODP_JSON,
            &[("cache-control", "max-age=0"), ("etag", "\"v1\"")],
        ),
        bare(304, &[("cache-control", "no-store")]),
        with_headers(200, SERVICE_DOCUMENT, ODP_JSON, &[("etag", "\"v2\"")]),
    ]);
    let client = client(&stub);

    client.inspect().await.unwrap();
    assert_eq!(
        client.inspect().await.unwrap().freshness,
        Freshness::Revalidated
    );
    assert_eq!(
        client.inspect().await.unwrap().freshness,
        Freshness::Fetched,
        "the entry was dropped, so the next read fetched"
    );
}

/// A 304 for something the Agent never stored describes nothing it can serve.
#[tokio::test]
async fn refuses_a_revalidation_of_something_it_never_stored() {
    let stub = Stub::new(|_| Ok(bare(304, &[])));
    let error = client(&stub).inspect().await.unwrap_err();
    assert!(
        matches!(&error, AgentError::InvalidResponse(message) if message.contains("304")),
        "{error}"
    );
}

/// A validator-less 304 keeps the lifetime the stored entry already had.
#[tokio::test]
async fn keeps_the_stored_lifetime_when_a_revalidation_states_none() {
    let stub = scripted(vec![
        with_headers(
            200,
            SERVICE_DOCUMENT,
            ODP_JSON,
            &[("cache-control", "max-age=0"), ("etag", "\"v1\"")],
        ),
        bare(304, &[]),
    ]);
    let client = client(&stub);

    client.inspect().await.unwrap();
    assert_eq!(
        client.inspect().await.unwrap().freshness,
        Freshness::Revalidated
    );
    let _ = client.inspect().await;
    assert_eq!(stub.count(), 3, "a zero lifetime stayed zero");
}

/// A Vary the Agent cannot honour means the stored entry could answer the wrong request.
#[tokio::test]
async fn stores_nothing_it_cannot_vary_on() {
    for (vary, reached) in [
        ("Accept-Language", 1),
        ("accept, content-type", 1),
        ("Authorization", 2),
        ("*", 2),
    ] {
        let stub = Stub::new(move |_| {
            Ok(with_headers(
                200,
                SERVICE_DOCUMENT,
                ODP_JSON,
                &[("vary", vary)],
            ))
        });
        let client = client(&stub);
        client.inspect().await.unwrap();
        let _ = client.inspect().await;
        assert_eq!(stub.count(), reached, "{vary}");
    }
}

/// CCH-06: a request in another language is another request.
#[tokio::test]
async fn keeps_each_language_in_its_own_entry() {
    let stub = Stub::serving(OFFERING_PAGE);
    let cache = Arc::new(MemoryCache::new());
    let english = client(&stub).with_cache(cache.clone());
    let french = client(&stub)
        .with_cache(cache.clone())
        .with_accept_language("fr");

    english.inspect().await.unwrap();
    french.inspect().await.unwrap();
    assert_eq!(stub.count(), 2);
    assert_eq!(english.inspect().await.unwrap().freshness, Freshness::Fresh);
}

/// CCH-05/06: an authenticated representation is never reused for another context.
#[tokio::test]
async fn partitions_every_authentication_context() {
    let stub = Stub::serving(OFFERING_PAGE);
    let cache = Arc::new(MemoryCache::new());
    let anonymous = client(&stub).with_cache(cache.clone());
    let authenticated = client(&stub)
        .with_cache(cache.clone())
        .with_cache_partition("bearer-abc");

    anonymous.inspect().await.unwrap();
    assert_eq!(
        authenticated.inspect().await.unwrap().freshness,
        Freshness::Fetched,
        "the anonymous entry did not answer for the authenticated context"
    );
    assert_eq!(
        anonymous.inspect().await.unwrap().freshness,
        Freshness::Fresh
    );
}

/// The same partitioning covers supporting documents, which are fetched on their own path.
#[tokio::test]
async fn partitions_supporting_documents_too() {
    let schema = br#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object"}"#;
    let offering = br#"{"attributes":{"size":"L"},"id":"plant-1","name":"Plant","odp_version":"1.0","schema":{"url":"https://schemas.example/plant.json"}}"#;
    let stub = Stub::new(move |request| {
        Ok(if request.url.ends_with("/.well-known/odp") {
            response(200, SERVICE_DOCUMENT, ODP_JSON)
        } else if request.url.contains("schemas.example") {
            with_headers(
                200,
                schema,
                SCHEMA_JSON,
                &[("cache-control", "max-age=86400")],
            )
        } else {
            response(200, offering, ODP_JSON)
        })
    });
    let cache = Arc::new(MemoryCache::new());
    let anonymous = client(&stub).with_cache(cache.clone());
    let authenticated = client(&stub)
        .with_cache(cache.clone())
        .with_cache_partition("bearer-abc");

    anonymous.get_offering_details("plant-1").await.unwrap();
    let before = schema_fetches(&stub);
    authenticated.get_offering_details("plant-1").await.unwrap();
    assert_eq!(
        schema_fetches(&stub),
        before + 1,
        "the authenticated context fetched the schema for itself"
    );

    let again = schema_fetches(&stub);
    anonymous.get_offering_details("plant-1").await.unwrap();
    assert_eq!(
        schema_fetches(&stub),
        again,
        "its own entry was still fresh"
    );
}

/// An Attribute Schema is long-lived, so the Agent keeps one rather than fetching it each time.
#[tokio::test]
async fn keeps_an_attribute_schema_for_its_fallback_lifetime() {
    let schema = br#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object"}"#;
    let offering = br#"{"attributes":{"size":"L"},"id":"plant-1","name":"Plant","odp_version":"1.0","schema":{"url":"https://schemas.example/plant.json"}}"#;
    let stub = Stub::new(move |request| {
        Ok(if request.url.ends_with("/.well-known/odp") {
            response(200, SERVICE_DOCUMENT, ODP_JSON)
        } else if request.url.contains("schemas.example") {
            response(200, schema, SCHEMA_JSON)
        } else {
            response(200, offering, ODP_JSON)
        })
    });
    let client = client(&stub);

    client.get_offering_details("plant-1").await.unwrap();
    let before = schema_fetches(&stub);
    client.get_offering_details("plant-1").await.unwrap();
    assert_eq!(
        schema_fetches(&stub),
        before,
        "the schema was stored, not refetched"
    );
}

/// A search describes one request, so a second identical search asks the Service again.
#[tokio::test]
async fn stores_no_search_response_of_its_own_accord() {
    let stub = Stub::serving(OFFERING_PAGE);
    let client = client(&stub);
    let request = odp_core::OfferingSearchRequest {
        odp_version: "1.0".to_owned(),
        query: "plants".to_owned(),
        ..odp_core::OfferingSearchRequest::default()
    };

    client
        .search_offerings(&request, Representation::Terse)
        .await
        .unwrap();
    let before = stub.catalog_requests().len();
    client
        .search_offerings(&request, Representation::Terse)
        .await
        .unwrap();
    assert_eq!(stub.catalog_requests().len(), before + 1);
}

// -- the cache itself --------------------------------------------------------------------

/// A cache that grows without bound is a liability in a long-lived Agent.
#[tokio::test]
async fn bounds_what_the_memory_cache_keeps() {
    let cache = Arc::new(MemoryCache::with_capacity(2));
    let client = ServiceClient::with_transport(ORIGIN, Stub::serving(OFFERING_PAGE))
        .unwrap()
        .with_cache(cache.clone());

    for language in ["en", "fr", "de", "es"] {
        client
            .clone()
            .with_accept_language(language)
            .inspect()
            .await
            .unwrap();
    }
    assert_eq!(cache.len(), 2);
    assert!(!cache.is_empty());
}

#[tokio::test]
async fn keeps_nothing_at_all_when_it_has_no_capacity() {
    let cache = Arc::new(MemoryCache::with_capacity(0));
    let stub = Stub::serving(OFFERING_PAGE);
    let client = client(&stub).with_cache(cache.clone());

    client.inspect().await.unwrap();
    client.inspect().await.unwrap();
    assert_eq!(cache.len(), 0);
    assert!(cache.is_empty());
    assert_eq!(stub.count(), 2);
}

#[tokio::test]
async fn replaces_an_entry_it_already_holds() {
    let cache = Arc::new(MemoryCache::with_capacity(1));
    let stub = scripted(vec![
        with_headers(
            200,
            SERVICE_DOCUMENT,
            ODP_JSON,
            &[("cache-control", "max-age=0")],
        ),
        with_headers(
            200,
            SERVICE_DOCUMENT,
            ODP_JSON,
            &[("cache-control", "max-age=600")],
        ),
    ]);
    let client = client(&stub).with_cache(cache.clone());

    client.inspect().await.unwrap();
    client.inspect().await.unwrap();
    assert_eq!(cache.len(), 1);
    assert_eq!(client.inspect().await.unwrap().freshness, Freshness::Fresh);
}

/// CCH-05: a document reached through a redirect is revalidated where it was actually found.
#[tokio::test]
async fn revalidates_a_document_where_the_redirect_left_it() {
    let stub = Stub::new(|request| {
        Ok(if request.url.ends_with("/.well-known/odp") {
            bare(308, &[("location", "https://plants.example/odp/document")])
        } else if request.headers.contains_key("if-none-match") {
            bare(304, &[("cache-control", "max-age=600")])
        } else {
            with_headers(
                200,
                SERVICE_DOCUMENT,
                ODP_JSON,
                &[("cache-control", "max-age=0"), ("etag", "\"v1\"")],
            )
        })
    });
    let client = client(&stub);

    client.inspect().await.unwrap();
    assert_eq!(
        client.inspect().await.unwrap().freshness,
        Freshness::Revalidated
    );

    let last = stub.last();
    assert_eq!(last.url, "https://plants.example/odp/document");
    assert!(
        !stub
            .requests()
            .iter()
            .skip(2)
            .any(|request| request.url.ends_with("/.well-known/odp")),
        "the redirect was not walked a second time"
    );
}

fn schema_fetches(stub: &Arc<Stub>) -> usize {
    stub.requests()
        .into_iter()
        .filter(|request| request.url.contains("schemas.example"))
        .count()
}
