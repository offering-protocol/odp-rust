//! FLT-52/53/54/64: the bounds an Agent puts on a Service's declared search capabilities.

mod support;

use std::sync::Arc;

use support::{ODP_JSON, OFFERING_PAGE, Stub, client, response};

const OPERATIONS: &str = r#""operations":[{"authentication":"not-required","name":"get-collection"},{"authentication":"not-required","name":"get-offering"},{"authentication":"not-required","name":"list-offerings"},{"authentication":"not-required","name":"search-offerings"}]"#;

/// A Service Document carrying the given `search_capabilities` object.
fn document(capabilities: &str) -> String {
    format!(
        r#"{{"description":"Plants","http":{{"endpoint_base":"/odp"}},"language":"en","localizations":["en"],"name":"Plants","odp_version":"1.0",{OPERATIONS},"search_capabilities":{capabilities}}}"#
    )
}

/// A Service answering the Service Document, then `pages` by URL substring, then an Offering page.
fn serving(document: String, pages: Vec<(&'static str, String)>) -> Arc<Stub> {
    Stub::new(move |request| {
        if request.url.ends_with("/.well-known/odp") {
            return Ok(response(200, document.as_bytes(), ODP_JSON));
        }
        for (marker, body) in &pages {
            if request.url.contains(marker) {
                return Ok(response(200, body.as_bytes(), ODP_JSON));
            }
        }
        Ok(response(200, OFFERING_PAGE, ODP_JSON))
    })
}

fn filter(id: &str) -> String {
    format!(
        r#"{{"description":"d","id":"{id}","operators":["eq"],"title":"{id}","type":"string"}}"#
    )
}

fn sort(id: &str, filter_id: &str) -> String {
    format!(
        r#"{{"description":"d","id":"{id}","keys":[{{"direction":"ascending","filter_id":"{filter_id}","missing":"last"}}],"title":"{id}"}}"#
    )
}

fn joined(values: impl Iterator<Item = String>) -> String {
    values.collect::<Vec<_>>().join(",")
}

// -- linked sources -----------------------------------------------------------------------

/// FLT-53: a linked filter source that points back at itself never terminates, so it is refused.
#[tokio::test]
async fn refuses_a_linked_filter_source_that_loops() {
    let page = format!(
        r#"{{"items":[{}],"next":"/odp/filters","odp_version":"1.0"}}"#,
        filter("colour")
    );
    let stub = serving(
        document(r#"{"filters":{"linked":{"href":"/odp/filters"}}}"#),
        vec![("/odp/filters", page)],
    );

    let catalog = client(&stub)
        .get_offering_search_capabilities(None)
        .await
        .unwrap();
    assert!(catalog.filters.is_empty());
    assert_eq!(catalog.issues.len(), 1);
    assert!(
        catalog.issues[0].message.contains("loop"),
        "{:?}",
        catalog.issues
    );
}

/// FLT-52: a linked source is read for at most 16 pages; a seventeenth is not followed.
#[tokio::test]
async fn stops_a_linked_filter_source_at_sixteen_pages() {
    let stub = Stub::new(move |request| {
        if request.url.ends_with("/.well-known/odp") {
            return Ok(response(
                200,
                document(r#"{"filters":{"linked":{"href":"/odp/filters?page=0"}}}"#).as_bytes(),
                ODP_JSON,
            ));
        }
        let page: usize = request
            .url
            .split("page=")
            .nth(1)
            .and_then(|value| value.parse().ok())
            .unwrap_or_default();
        let body = format!(
            r#"{{"items":[{}],"next":"/odp/filters?page={}","odp_version":"1.0"}}"#,
            filter(&format!("filter-{page}")),
            page + 1
        );
        Ok(response(200, body.as_bytes(), ODP_JSON))
    });

    let catalog = client(&stub)
        .get_offering_search_capabilities(None)
        .await
        .unwrap();
    assert!(catalog.filters.is_empty(), "the source was never complete");
    assert_eq!(catalog.issues.len(), 1);
    assert!(
        catalog.issues[0].message.contains("16 pages"),
        "{:?}",
        catalog.issues
    );
    assert_eq!(
        stub.catalog_requests().len(),
        16,
        "a seventeenth page was not fetched"
    );
}

#[tokio::test]
async fn stops_a_linked_sort_source_at_sixteen_pages() {
    let stub = Stub::new(move |request| {
        if request.url.ends_with("/.well-known/odp") {
            return Ok(response(
                200,
                document(r#"{"sorts":{"linked":{"href":"/odp/sorts?page=0"}}}"#).as_bytes(),
                ODP_JSON,
            ));
        }
        let page: usize = request
            .url
            .split("page=")
            .nth(1)
            .and_then(|value| value.parse().ok())
            .unwrap_or_default();
        let body = format!(
            r#"{{"items":[{}],"next":"/odp/sorts?page={}","odp_version":"1.0"}}"#,
            sort(&format!("sort-{page}"), "colour"),
            page + 1
        );
        Ok(response(200, body.as_bytes(), ODP_JSON))
    });

    let catalog = client(&stub)
        .get_offering_search_capabilities(None)
        .await
        .unwrap();
    assert!(catalog.sorts.is_empty());
    assert!(
        catalog
            .issues
            .iter()
            .any(|issue| issue.message.contains("16 pages")),
        "{:?}",
        catalog.issues
    );
}

/// A linked sort source that ends is read to its end and no further.
#[tokio::test]
async fn follows_a_linked_sort_source_to_its_last_page() {
    let first = format!(
        r#"{{"items":[{}],"next":"/odp/sorts?cursor=c2","odp_version":"1.0"}}"#,
        sort("cheapest", "colour")
    );
    let last = format!(
        r#"{{"items":[{}],"odp_version":"1.0"}}"#,
        sort("newest", "colour")
    );
    let capabilities = format!(
        r#"{{"filters":{{"inline":[{}]}},"sorts":{{"linked":{{"href":"/odp/sorts"}}}}}}"#,
        filter("colour")
    );
    let stub = serving(
        document(&capabilities),
        vec![("cursor=c2", last), ("/odp/sorts", first)],
    );

    let catalog = client(&stub)
        .get_offering_search_capabilities(None)
        .await
        .unwrap();
    assert_eq!(catalog.sorts.len(), 2, "{:?}", catalog.issues);
    assert!(catalog.issues.is_empty(), "{:?}", catalog.issues);
    assert_eq!(stub.catalog_requests().len(), 2);
}

/// A linked source the Service cannot serve is an issue about that source, not a failed catalog.
#[tokio::test]
async fn reports_a_linked_filter_source_the_service_cannot_serve() {
    let stub = Stub::new(move |request| {
        Ok(if request.url.ends_with("/.well-known/odp") {
            response(
                200,
                document(r#"{"filters":{"linked":{"href":"/odp/filters"}}}"#).as_bytes(),
                ODP_JSON,
            )
        } else {
            response(
                503,
                br#"{"status":503,"title":"Unavailable"}"#,
                "application/problem+json",
            )
        })
    });

    let catalog = client(&stub)
        .get_offering_search_capabilities(None)
        .await
        .unwrap();
    assert!(catalog.filters.is_empty());
    assert_eq!(catalog.issues.len(), 1);
}

// -- effective bounds ---------------------------------------------------------------------

/// FLT-54: more than 1024 effective filters is more than an Agent will hold.
#[tokio::test]
async fn refuses_more_filters_than_the_effective_limit_allows() {
    let stub = paged_source(
        r#"{"filters":{"linked":{"href":"/odp/filters?page=0"}}}"#,
        11,
        &|page| joined((0..100).map(move |index| filter(&format!("filter-{page}-{index}")))),
    );

    let catalog = client(&stub)
        .get_offering_search_capabilities(None)
        .await
        .unwrap();
    assert!(catalog.filters.is_empty());
    assert!(
        catalog
            .issues
            .iter()
            .any(|issue| issue.message.contains("1024")),
        "{:?}",
        catalog.issues
    );
}

/// FLT-54: and more than 128 effective sorts likewise.
#[tokio::test]
async fn refuses_more_sorts_than_the_effective_limit_allows() {
    let stub = paged_source(
        r#"{"sorts":{"linked":{"href":"/odp/sorts?page=0"}}}"#,
        2,
        &|page| joined((0..100).map(move |index| sort(&format!("sort-{page}-{index}"), "colour"))),
    );

    let catalog = client(&stub)
        .get_offering_search_capabilities(None)
        .await
        .unwrap();
    assert!(catalog.sorts.is_empty());
    assert!(
        catalog
            .issues
            .iter()
            .any(|issue| issue.message.contains("128")),
        "{:?}",
        catalog.issues
    );
}

/// A Service serving `pages` pages of a linked capability source, the last of them final.
fn paged_source(
    capabilities: &str,
    pages: usize,
    items: &'static (dyn Fn(usize) -> String + Send + Sync),
) -> Arc<Stub> {
    let document = document(capabilities);
    Stub::new(move |request| {
        if request.url.ends_with("/.well-known/odp") {
            return Ok(response(200, document.as_bytes(), ODP_JSON));
        }
        let page: usize = request
            .url
            .split("page=")
            .nth(1)
            .and_then(|value| value.parse().ok())
            .unwrap_or_default();
        let next = if page + 1 < pages {
            let path = request.url.split('?').next().unwrap_or_default().to_owned();
            format!(r#""next":"{path}?page={}","#, page + 1)
        } else {
            String::new()
        };
        let body = format!(r#"{{"items":[{}],{next}"odp_version":"1.0"}}"#, items(page));
        Ok(response(200, body.as_bytes(), ODP_JSON))
    })
}

/// FLT-64: a sort identifier two effective sources both claim is available from neither.
#[tokio::test]
async fn drops_a_sort_two_sources_both_claim() {
    let capabilities = format!(
        r#"{{"filters":{{"inline":[{}]}},"sorts":{{"inline":[{}]}}}}"#,
        filter("colour"),
        sort("cheapest", "colour")
    );
    let collection = format!(
        r#"{{"id":"plants","name":"Plants","odp_version":"1.0","search_capabilities":{{"sorts":{{"inline":[{},{}]}}}}}}"#,
        sort("cheapest", "colour"),
        sort("newest", "colour")
    );
    let stub = serving(
        document(&capabilities),
        vec![("/odp/collections/plants", collection)],
    );

    let catalog = client(&stub)
        .get_offering_search_capabilities(Some("plants"))
        .await
        .unwrap();
    assert!(!catalog.sorts.contains_key("cheapest"), "claimed twice");
    assert!(catalog.sorts.contains_key("newest"));
    assert!(
        catalog
            .issues
            .iter()
            .any(|issue| issue.message.contains("Duplicate sorts: cheapest")),
        "{:?}",
        catalog.issues
    );
}

/// A linked source that ends exactly at the sixteenth page is complete, not over its bound.
#[tokio::test]
async fn reads_a_linked_filter_source_of_exactly_sixteen_pages() {
    let stub = paged_source(
        r#"{"filters":{"linked":{"href":"/odp/filters?page=0"}}}"#,
        16,
        &|page| filter(&format!("filter-{page}")),
    );

    let catalog = client(&stub)
        .get_offering_search_capabilities(None)
        .await
        .unwrap();
    assert_eq!(catalog.filters.len(), 16);
    assert!(catalog.issues.is_empty(), "{:?}", catalog.issues);
}

/// The same bound applies to a linked sort source.
#[tokio::test]
async fn reads_a_linked_sort_source_of_exactly_sixteen_pages() {
    let stub = paged_source(
        r#"{"filters":{"inline":[{"description":"d","id":"colour","operators":["eq"],"title":"Colour","type":"string"}]},"sorts":{"linked":{"href":"/odp/sorts?page=0"}}}"#,
        16,
        &|page| sort(&format!("sort-{page}"), "colour"),
    );

    let catalog = client(&stub)
        .get_offering_search_capabilities(None)
        .await
        .unwrap();
    assert_eq!(catalog.sorts.len(), 16);
    assert!(catalog.issues.is_empty(), "{:?}", catalog.issues);
}
