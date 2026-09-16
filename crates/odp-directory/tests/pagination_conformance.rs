//! SVC-94: walking a Directory search yields candidate Services, within bounds a caller sets.

mod support;

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use odp_directory::{IterationOptions, SearchRequest};
use support::{Stub, client, json, named_service, page, service};

/// A Directory whose pages continue forever, each with a cursor of its own.
fn endless(per_page: usize) -> Arc<Stub> {
    let step = AtomicUsize::new(0);
    Stub::new(move |_| {
        let cursor = step.fetch_add(1, Ordering::SeqCst);
        let items = (0..per_page)
            .map(|index| {
                named_service(
                    &format!("https://plant-{cursor}-{index}.example"),
                    &format!("Plant {cursor}-{index}"),
                )
            })
            .collect::<Vec<_>>();
        Ok(json(
            200,
            page(&items, &format!("/v1/services/search?cursor=c{cursor}")),
        ))
    })
}

/// A Directory of exactly `pages` pages, the last of them final.
fn finite(pages: usize, per_page: usize) -> Arc<Stub> {
    let step = AtomicUsize::new(0);
    Stub::new(move |_| {
        let cursor = step.fetch_add(1, Ordering::SeqCst);
        let items = (0..per_page)
            .map(|index| service(&format!("https://plant-{cursor}-{index}.example")))
            .collect::<Vec<_>>();
        let next = if cursor + 1 < pages {
            format!("/v1/services/search?cursor=c{cursor}")
        } else {
            String::new()
        };
        Ok(json(200, page(&items, &next)))
    })
}

// -- following a sequence ------------------------------------------------------------------

#[tokio::test]
async fn follows_a_sequence_to_its_end() {
    let stub = finite(3, 2);
    let pages = client(&stub)
        .search_pages(&SearchRequest::default(), IterationOptions::default())
        .await
        .unwrap();

    assert_eq!(pages.len(), 3);
    assert_eq!(stub.count(), 3);
    assert!(pages[2].next.is_empty());
}

/// A page is fetched only when something still wants it.
#[tokio::test]
async fn fetches_no_page_it_would_discard() {
    let stub = endless(1);
    let pages = client(&stub)
        .search_pages(
            &SearchRequest::default(),
            IterationOptions {
                max_items: 0,
                max_pages: 3,
            },
        )
        .await
        .unwrap();

    assert_eq!(pages.len(), 3);
    assert_eq!(stub.count(), 3, "no fourth page was asked for");
}

/// Reaching the page bound is not reaching the end, so the last page keeps its continuation.
#[tokio::test]
async fn leaves_a_continuation_a_caller_can_resume_from() {
    let stub = endless(1);
    let pages = client(&stub)
        .search_pages(
            &SearchRequest::default(),
            IterationOptions {
                max_items: 0,
                max_pages: 2,
            },
        )
        .await
        .unwrap();

    let next = pages.last().unwrap().next.clone();
    assert!(!next.is_empty());

    let resumed = client(&stub).continue_search(&next).await.unwrap();
    assert_eq!(resumed.items.len(), 1);
}

/// Asking for ten Services does not pay for a thousand.
#[tokio::test]
async fn stops_once_it_has_the_services_it_was_asked_for() {
    let stub = endless(10);
    let services = client(&stub)
        .search_services(
            &SearchRequest::default(),
            IterationOptions {
                max_items: 10,
                max_pages: 0,
            },
        )
        .await
        .unwrap();

    assert_eq!(services.len(), 10);
    assert_eq!(stub.count(), 1, "one page held everything asked for");
}

#[tokio::test]
async fn reads_only_the_pages_the_item_budget_needs() {
    let stub = endless(10);
    let services = client(&stub)
        .search_services(
            &SearchRequest::default(),
            IterationOptions {
                max_items: 25,
                max_pages: 0,
            },
        )
        .await
        .unwrap();

    assert_eq!(services.len(), 25);
    assert_eq!(stub.count(), 3, "three pages of ten covered twenty-five");
}

#[tokio::test]
async fn gathers_every_service_a_finite_sequence_holds() {
    let stub = finite(4, 10);
    let services = client(&stub)
        .search_services(&SearchRequest::default(), IterationOptions::default())
        .await
        .unwrap();

    assert_eq!(services.len(), 40);
    assert_eq!(stub.count(), 4);
}

/// A sequence longer than the historic sixteen-page cap is still walked in full.
#[tokio::test]
async fn walks_a_sequence_past_sixteen_pages() {
    let stub = finite(40, 10);
    let services = client(&stub)
        .search_services(&SearchRequest::default(), IterationOptions::default())
        .await
        .unwrap();

    assert_eq!(services.len(), 400);
    assert_eq!(stub.count(), 40);
}

/// The page bound stops a traversal a Directory would otherwise never end.
#[tokio::test]
async fn stops_an_endless_sequence_at_the_page_bound() {
    let stub = endless(1);
    let services = client(&stub)
        .search_services(
            &SearchRequest::default(),
            IterationOptions {
                max_items: 0,
                max_pages: 5,
            },
        )
        .await
        .unwrap();

    assert_eq!(services.len(), 5);
    assert_eq!(stub.count(), 5);
}

// -- refusing what cannot be walked --------------------------------------------------------

/// A cursor that comes round again describes a walk that never ends.
#[tokio::test]
async fn refuses_a_continuation_that_repeats() {
    let stub = Stub::serving(page(
        &[service("https://plants.example")],
        "/v1/services/search?cursor=same",
    ));
    let error = client(&stub)
        .search_pages(&SearchRequest::default(), IterationOptions::default())
        .await
        .unwrap_err();

    assert!(error.to_string().contains("loop"), "{error}");
    assert_eq!(stub.count(), 2, "the repeat was recognised, not followed");
}

/// PAG-07: a continuation stays on the Directory that offered it.
#[tokio::test]
async fn refuses_a_continuation_that_leaves_the_directory() {
    let stub = Stub::serving(page(&[], ""));
    for next in [
        "https://elsewhere.example/v1/services/search",
        "//elsewhere.example/v1/services/search",
        "http://api.inflowpay.ai/v1/services/search",
        "https://user:secret@api.inflowpay.ai/v1/services/search",
    ] {
        let error = client(&stub).continue_search(next).await.unwrap_err();
        assert!(
            error.to_string().contains("canonical origin"),
            "{next}: {error}"
        );
    }
    assert_eq!(stub.count(), 0, "nothing was sent");
}

#[tokio::test]
async fn refuses_a_continuation_that_is_not_a_reference() {
    let stub = Stub::serving(page(&[], ""));
    assert!(
        client(&stub)
            .continue_search("https://[not-an-address")
            .await
            .is_err()
    );
}

/// A relative continuation resolves onto the Directory, which is where it came from.
#[tokio::test]
async fn resolves_a_relative_continuation_against_the_directory() {
    let stub = Stub::serving(page(&[], ""));
    client(&stub)
        .continue_search("/v1/services/search?cursor=c2")
        .await
        .unwrap();
    assert_eq!(
        stub.last().url,
        "https://api.inflowpay.ai/v1/services/search?cursor=c2"
    );
}

/// A traversal a Directory could not satisfy is refused before the first request.
#[tokio::test]
async fn refuses_bounds_past_what_a_traversal_allows() {
    let stub = Stub::serving(page(&[], ""));
    for (options, expected) in [
        (
            IterationOptions {
                max_items: 10_001,
                max_pages: 0,
            },
            "max_items",
        ),
        (
            IterationOptions {
                max_items: 0,
                max_pages: 10_001,
            },
            "max_pages",
        ),
    ] {
        let error = client(&stub)
            .search_services(&SearchRequest::default(), options)
            .await
            .unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");

        let error = client(&stub)
            .search_pages(&SearchRequest::default(), options)
            .await
            .unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
    }
    assert_eq!(stub.count(), 0);
}

/// A continuation is fetched as a GET carrying nothing.
#[tokio::test]
async fn continues_a_search_without_repeating_the_request() {
    let stub = finite(2, 1);
    client(&stub)
        .search_services(
            &SearchRequest {
                query: "plants".to_owned(),
                ..SearchRequest::default()
            },
            IterationOptions::default(),
        )
        .await
        .unwrap();

    let requests = stub.requests();
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[1].method, "GET");
    assert!(requests[1].body.is_empty());
    assert!(requests[1].url.contains("cursor=c0"), "{}", requests[1].url);
}

/// A record withheld from a page does not count towards what the caller asked for.
#[tokio::test]
async fn counts_only_the_services_it_hands_back() {
    let stub = Stub::serving(format!(
        r#"{{"items":[{},{{"broken":true}}],"next":""}}"#,
        service("https://plants.example")
    ));
    let page = client(&stub)
        .search_services(&SearchRequest::default(), IterationOptions::default())
        .await
        .unwrap();
    assert_eq!(page.len(), 1);
}
