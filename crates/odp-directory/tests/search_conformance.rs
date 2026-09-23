//! ROLE-03, SVC-17 and SVC-27: what a Directory record is worth once this client has read it.

mod support;

use odp_core::{EnrollmentProtocol, Operation, PaymentOption, Protocol, TrustProtocol};
use odp_directory::{
    OperationFilter, PaymentFilter, SearchRequest, ServiceFilters, SuggestionRequest,
};
use support::{JSON, Stub, client, named_service, page, service, typed, with_headers};

/// A page carrying exactly one record built from the given members.
fn one(members: &str) -> String {
    format!(r#"{{"items":[{{{members}}}]}}"#)
}

const WHOLE: &str = r#""description":"Plants for agents.","indexed_at":"2026-08-25T00:00:00Z","language":"en","localizations":["en"],"name":"Plants","operations":[],"service_origin":"https://plants.example""#;

// -- what the client sends -----------------------------------------------------------------

/// A Directory refuses a request it cannot satisfy, so the caller hears it here, not as a 400.
#[tokio::test]
async fn refuses_a_request_the_directory_would_refuse() {
    let stub = Stub::serving(page(&[], ""));
    let cases: Vec<(SearchRequest, &str)> = vec![
        (
            SearchRequest {
                limit: 101,
                ..SearchRequest::default()
            },
            "limit",
        ),
        (
            SearchRequest {
                query: " plants".to_owned(),
                ..SearchRequest::default()
            },
            "query",
        ),
        (
            SearchRequest {
                query: "x".repeat(513),
                ..SearchRequest::default()
            },
            "query",
        ),
        (
            filtered(ServiceFilters {
                keywords: (0..33).map(|index| index.to_string()).collect(),
                ..ServiceFilters::default()
            }),
            "keywords",
        ),
        (
            filtered(ServiceFilters {
                keywords: vec!["  ".to_owned()],
                ..ServiceFilters::default()
            }),
            "keywords",
        ),
        (
            filtered(ServiceFilters {
                keywords: vec!["x".repeat(65)],
                ..ServiceFilters::default()
            }),
            "keywords",
        ),
        (
            filtered(ServiceFilters {
                keywords: vec!["plants".to_owned(), "plants".to_owned()],
                ..ServiceFilters::default()
            }),
            "keywords must not repeat",
        ),
        (
            filtered(ServiceFilters {
                enrollment: vec![
                    EnrollmentProtocol {
                        name: Protocol::Aep,
                    },
                    EnrollmentProtocol {
                        name: Protocol::Aep,
                    },
                ],
                ..ServiceFilters::default()
            }),
            "enrollment",
        ),
        (
            filtered(ServiceFilters {
                operations: (0..22).map(|_| operation(Operation::GetOffering)).collect(),
                ..ServiceFilters::default()
            }),
            "operations must contain",
        ),
        (
            filtered(ServiceFilters {
                operations: vec![
                    operation(Operation::GetOffering),
                    operation(Operation::GetOffering),
                ],
                ..ServiceFilters::default()
            }),
            "operations must not repeat",
        ),
        (
            filtered(ServiceFilters {
                payments: (0..33).map(|_| payment(Vec::new())).collect(),
                ..ServiceFilters::default()
            }),
            "payments must contain",
        ),
        (
            filtered(ServiceFilters {
                payments: vec![payment(Vec::new()), payment(Vec::new())],
                ..ServiceFilters::default()
            }),
            "payments must not repeat",
        ),
        (
            filtered(ServiceFilters {
                payments: vec![payment(vec![PaymentOption::Card, PaymentOption::Card])],
                ..ServiceFilters::default()
            }),
            "payment options must not repeat",
        ),
    ];

    for (request, expected) in cases {
        let error = client(&stub)
            .search_services(&request)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains(expected), "expected {expected}: {error}");
    }
    assert_eq!(stub.count(), 0, "nothing was sent");
}

/// A request within every bound reaches the Directory as it was written.
#[tokio::test]
async fn sends_a_request_it_accepts() {
    let stub = Stub::serving(page(&[], ""));
    client(&stub)
        .search_services(&SearchRequest {
            filters: Some(ServiceFilters {
                enrollment: vec![EnrollmentProtocol {
                    name: Protocol::Aep,
                }],
                keywords: vec!["plants".to_owned(), "seeds".to_owned()],
                operations: vec![operation(Operation::GetOffering)],
                payments: vec![payment(vec![PaymentOption::Card])],
                sources: None,
                trust: vec![TrustProtocol {
                    name: Protocol::Tap,
                }],
            }),
            limit: 25,
            query: "rubber plant".to_owned(),
        })
        .await
        .unwrap();

    let body = String::from_utf8(stub.last().body).unwrap();
    assert!(body.contains(r#""query":"rubber plant""#), "{body}");
    assert!(body.contains(r#""limit":25"#), "{body}");
    assert!(body.contains(r#""keywords":["plants","seeds"]"#), "{body}");
}

/// An empty request carries nothing it does not mean.
#[tokio::test]
async fn omits_what_the_caller_did_not_ask_for() {
    let stub = Stub::serving(page(&[], ""));
    client(&stub)
        .search_services(&SearchRequest::default())
        .await
        .unwrap();
    assert_eq!(String::from_utf8(stub.last().body).unwrap(), "{}");
}

fn filtered(filters: ServiceFilters) -> SearchRequest {
    SearchRequest {
        filters: Some(filters),
        ..SearchRequest::default()
    }
}

fn operation(name: Operation) -> OperationFilter {
    OperationFilter {
        authentication: None,
        name,
    }
}

fn payment(options: Vec<PaymentOption>) -> PaymentFilter {
    PaymentFilter {
        authentication: None,
        name: Protocol::Mpp,
        options,
    }
}

// -- what the client accepts back ----------------------------------------------------------

#[tokio::test]
async fn reads_a_page_of_records() {
    let stub = Stub::serving(page(
        &[
            named_service("https://plants.example", "Plants"),
            named_service("https://seeds.example", "Seeds"),
        ],
        "/v1/services/search?cursor=c2",
    ));
    let result = client(&stub)
        .search_services(&SearchRequest::default())
        .await
        .unwrap();

    assert_eq!(result.items.len(), 2);
    assert_eq!(result.items[0].name, "Plants");
    assert_eq!(result.next, "/v1/services/search?cursor=c2");
    assert!(result.issues.is_empty());
}

/// PAG: a page no larger than the Directory promises.
#[tokio::test]
async fn refuses_a_page_of_more_than_a_hundred_records() {
    let items = (0..101)
        .map(|index| service(&format!("https://plant-{index}.example")))
        .collect::<Vec<_>>();
    let stub = Stub::serving(page(&items, ""));
    let error = client(&stub)
        .search_services(&SearchRequest::default())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("100 Services"), "{error}");
}

#[tokio::test]
async fn reads_a_page_of_exactly_a_hundred_records() {
    let items = (0..100)
        .map(|index| service(&format!("https://plant-{index}.example")))
        .collect::<Vec<_>>();
    let stub = Stub::serving(page(&items, ""));
    assert_eq!(
        client(&stub)
            .search_services(&SearchRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        100
    );
}

/// A page that is not a page at all.
#[tokio::test]
async fn refuses_an_envelope_it_cannot_read() {
    for body in [r#"["a"]"#, r#"{"items":{}}"#, r#"{}"#, r#""text""#] {
        let stub = Stub::serving(body);
        assert!(
            client(&stub)
                .search_services(&SearchRequest::default())
                .await
                .is_err(),
            "{body}"
        );
    }
}

/// ROLE-03: one record this client cannot read is a note about that record, not a lost page.
#[tokio::test]
async fn withholds_only_the_record_it_cannot_read() {
    let body = format!(
        r#"{{"items":[{},{{"name":"broken"}},{},"not an object"]}}"#,
        named_service("https://a.example", "A"),
        named_service("https://b.example", "B")
    );
    let stub = Stub::serving(body);
    let result = client(&stub)
        .search_services(&SearchRequest::default())
        .await
        .unwrap();

    assert_eq!(result.items.len(), 2);
    assert_eq!(result.items[1].name, "B");
    assert_eq!(result.issues.len(), 2);
    assert_eq!(result.issues[0].index, 1, "the position in the page sent");
    assert_eq!(result.issues[1].index, 3);
    assert!(!result.issues[0].message.is_empty());
}

// -- Service origins -----------------------------------------------------------------------

/// SEC-08: an origin the public internet does not route is not a Service a caller can reach.
#[tokio::test]
async fn withholds_a_record_naming_a_non_public_origin() {
    for origin in [
        "https://127.0.0.1",
        "https://10.0.0.1",
        "https://192.168.1.1",
        "https://169.254.169.254",
        "https://[::1]",
        "https://[fd00::1]",
        "https://[64:ff9b::a9fe:a9fe]",
        "https://localhost",
        "https://api.localhost",
    ] {
        let body = format!(r#"{{"items":[{}]}}"#, service(origin));
        let stub = Stub::serving(body);
        let result = client(&stub)
            .search_services(&SearchRequest::default())
            .await
            .unwrap();
        assert!(result.items.is_empty(), "{origin} was handed back");
        assert_eq!(result.issues.len(), 1, "{origin}");
        assert!(
            result.issues[0].message.contains("non-public"),
            "{origin}: {}",
            result.issues[0].message
        );
    }
}

#[tokio::test]
async fn keeps_a_record_naming_a_public_address() {
    let body = format!(r#"{{"items":[{}]}}"#, service("https://93.184.216.34"));
    let stub = Stub::serving(body);
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

/// SVC-17: an origin is the origin, not an origin with something appended to it.
#[tokio::test]
async fn withholds_a_record_whose_origin_is_not_canonical() {
    for origin in [
        "https://plants.example/",
        "https://plants.example/odp",
        "https://user:secret@plants.example",
        "http://plants.example",
        "not a url",
    ] {
        let body = format!(r#"{{"items":[{}]}}"#, service(origin));
        let stub = Stub::serving(body);
        let result = client(&stub)
            .search_services(&SearchRequest::default())
            .await
            .unwrap();
        assert!(result.items.is_empty(), "{origin} was handed back");
        assert_eq!(result.issues.len(), 1, "{origin}");
    }
}

#[tokio::test]
async fn withholds_a_record_with_no_origin_at_all() {
    let stub = Stub::serving(one(
        r#""description":"d","indexed_at":"2026-08-25T00:00:00Z","language":"en","localizations":["en"],"name":"n","operations":[]"#,
    ));
    let result = client(&stub)
        .search_services(&SearchRequest::default())
        .await
        .unwrap();
    assert_eq!(result.issues.len(), 1);
    assert!(result.issues[0].message.contains("origin is missing"));
}

// -- indexing time -------------------------------------------------------------------------

/// An indexing time is an RFC 3339 timestamp, not whatever a date parser happens to accept.
#[tokio::test]
async fn withholds_a_record_whose_indexing_time_is_not_rfc_3339() {
    for indexed_at in [
        "December 17, 1995 03:24:00",
        "2026-08-25",
        "2026-08-25 00:00:00Z",
        "2026-08-25T00:00Z",
        "2026-08-25T00:00:00",
        "2026-08-25T00x00:00Z",
        "2026-08-25T00:00x00Z",
        "2026-08-25T00:00:0aZ",
        "2026-08-25T0a:00:00Z",
        "2026-08-25T00:00:00+0000",
        "2026-08-25T00:00:00.Z",
        "20260825T000000Z",
        "",
    ] {
        let stub = Stub::serving(one(&format!(
            r#""description":"d","indexed_at":"{indexed_at}","language":"en","localizations":["en"],"name":"n","operations":[],"service_origin":"https://plants.example""#
        )));
        let result = client(&stub)
            .search_services(&SearchRequest::default())
            .await
            .unwrap();
        assert!(result.items.is_empty(), "{indexed_at} was accepted");
        assert!(
            result.issues[0].message.contains("RFC 3339"),
            "{indexed_at}: {}",
            result.issues[0].message
        );
    }
}

#[tokio::test]
async fn accepts_every_shape_rfc_3339_allows() {
    for indexed_at in [
        "2026-08-25T00:00:00Z",
        "2026-08-25t00:00:00z",
        "2026-08-25T00:00:00.123456Z",
        "2026-08-25T00:00:00+02:00",
        "2026-08-25T00:00:00.5-07:00",
    ] {
        let stub = Stub::serving(one(&format!(
            r#""description":"d","indexed_at":"{indexed_at}","language":"en","localizations":["en"],"name":"n","operations":[],"service_origin":"https://plants.example""#
        )));
        let result = client(&stub)
            .search_services(&SearchRequest::default())
            .await
            .unwrap();
        assert_eq!(result.items.len(), 1, "{indexed_at}: {:?}", result.issues);
    }
}

#[tokio::test]
async fn withholds_a_record_with_no_indexing_time() {
    let stub = Stub::serving(one(
        r#""description":"d","language":"en","localizations":["en"],"name":"n","operations":[],"service_origin":"https://plants.example""#,
    ));
    let result = client(&stub)
        .search_services(&SearchRequest::default())
        .await
        .unwrap();
    assert!(
        result.issues[0]
            .message
            .contains("indexing time is missing")
    );
}

// -- what a Directory cannot vouch for -----------------------------------------------------

#[tokio::test]
async fn preserves_directory_metadata_without_dereferencing_it() {
    let stub = Stub::serving(one(&format!(
        r#"{WHOLE},"branding":{{"logo":"x"}},"http":{{"endpoint_base":"/odp"}},"mcp":{{"url":"https://evil.example/mcp"}},"odp_version":"1.0","payment_origins":["https://pay.example"],"search_capabilities":{{"filters":{{"inline":[]}}}},"future_member":true"#
    )));
    let result = client(&stub)
        .search_services(&SearchRequest::default())
        .await
        .unwrap();

    let carried = result.items[0].additional.keys().collect::<Vec<_>>();
    assert_eq!(
        carried,
        [
            "branding",
            "future_member",
            "http",
            "mcp",
            "odp_version",
            "payment_origins",
            "search_capabilities"
        ],
        "metadata is preserved without treating it as execution authority"
    );
    assert_eq!(stub.count(), 1, "nothing in the record was dereferenced");
}

/// EXT: a protocol this version does not define is dropped, not a reason to lose the record.
#[tokio::test]
async fn drops_a_protocol_it_does_not_know() {
    let stub = Stub::serving(one(&format!(
        r#"{WHOLE},"protocols":{{"payments":[{{"authentication":"not-required","name":"future"}},{{"authentication":"not-required","name":"mpp"}}],"trust":[{{"name":"future"}},{{"name":"tap"}}]}}"#
    )));
    let protocols = client(&stub)
        .search_services(&SearchRequest::default())
        .await
        .unwrap()
        .items
        .swap_remove(0)
        .protocols
        .unwrap();

    assert_eq!(protocols.payments.len(), 1);
    assert_eq!(protocols.trust.len(), 1);
}

/// A category left with nothing in it is removed rather than reported as empty.
#[tokio::test]
async fn removes_a_protocol_block_nothing_survived() {
    let stub = Stub::serving(one(&format!(
        r#"{WHOLE},"protocols":{{"trust":[{{"name":"future"}}]}}"#
    )));
    let service = client(&stub)
        .search_services(&SearchRequest::default())
        .await
        .unwrap()
        .items
        .swap_remove(0);
    assert!(service.protocols.is_none_or(|value| value.trust.is_empty()));
}

/// A record with no protocols at all is complete as it stands.
#[tokio::test]
async fn keeps_a_record_that_declares_no_protocols() {
    let stub = Stub::serving(one(WHOLE));
    let result = client(&stub)
        .search_services(&SearchRequest::default())
        .await
        .unwrap();
    assert!(result.items[0].protocols.is_none());
}

#[tokio::test]
async fn withholds_a_record_whose_protocols_are_not_a_block() {
    let stub = Stub::serving(one(&format!(r#"{WHOLE},"protocols":5"#)));
    let result = client(&stub)
        .search_services(&SearchRequest::default())
        .await
        .unwrap();
    assert_eq!(result.issues.len(), 1);
}

// -- facets --------------------------------------------------------------------------------

#[tokio::test]
async fn reads_the_facets_a_directory_publishes() {
    let stub =
        Stub::serving(r#"{"facets":{"keywords":[{"count":12,"value":"plants"}]},"items":[]}"#);
    let facets = client(&stub)
        .search_services(&SearchRequest::default())
        .await
        .unwrap()
        .facets
        .unwrap();
    assert_eq!(facets.keywords[0].count, 12);
    assert_eq!(facets.keywords[0].value, "plants");
}

/// A facet describes how many Services share a value, so the count has to be countable.
#[tokio::test]
async fn refuses_a_facet_it_cannot_count() {
    for facets in [
        r#"{"keywords":[{"value":"plants"}]}"#,
        r#"{"keywords":[{"count":-1,"value":"plants"}]}"#,
        r#"{"keywords":[{"count":"many","value":"plants"}]}"#,
        r#"{"keywords":{}}"#,
    ] {
        let stub = Stub::serving(format!(r#"{{"facets":{facets},"items":[]}}"#));
        assert!(
            client(&stub)
                .search_services(&SearchRequest::default())
                .await
                .is_err(),
            "{facets}"
        );
    }
}

#[tokio::test]
async fn refuses_a_facet_of_more_than_a_hundred_entries() {
    let entries = (0..101)
        .map(|index| format!(r#"{{"count":1,"value":"k{index}"}}"#))
        .collect::<Vec<_>>()
        .join(",");
    let stub = Stub::serving(format!(
        r#"{{"facets":{{"keywords":[{entries}]}},"items":[]}}"#
    ));
    let error = client(&stub)
        .search_services(&SearchRequest::default())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("100 entries"), "{error}");
}

/// Facets are optional, and a page without them is not missing anything.
#[tokio::test]
async fn reads_a_page_that_publishes_no_facets() {
    for body in [r#"{"items":[]}"#, r#"{"facets":null,"items":[]}"#] {
        let stub = Stub::serving(body);
        assert!(
            client(&stub)
                .search_services(&SearchRequest::default())
                .await
                .unwrap()
                .facets
                .is_none(),
            "{body}"
        );
    }
}

// -- continuations -------------------------------------------------------------------------

/// A continuation this client could not use is not one it will carry.
#[tokio::test]
async fn refuses_a_continuation_it_could_not_use() {
    for next in [
        r#"" /v1/services/search""#,
        "5",
        "{}",
        &format!(r#""{}""#, "x".repeat(2_049)),
    ] {
        let stub = Stub::serving(format!(r#"{{"items":[],"next":{next}}}"#));
        assert!(
            client(&stub)
                .search_services(&SearchRequest::default())
                .await
                .is_err(),
            "{next}"
        );
    }
}

#[tokio::test]
async fn reads_a_page_that_offers_no_continuation() {
    for body in [r#"{"items":[]}"#, r#"{"items":[],"next":null}"#] {
        let stub = Stub::serving(body);
        assert!(
            client(&stub)
                .search_services(&SearchRequest::default())
                .await
                .unwrap()
                .next
                .is_empty(),
            "{body}"
        );
    }
}

// -- suggestions ---------------------------------------------------------------------------

/// ROLE-06: a suggestion is a prefix completion, and arrives in its own envelope.
#[tokio::test]
async fn reads_the_suggestions_a_directory_offers() {
    let stub = Stub::serving(r#"{"items":["plants","planters","plant food"]}"#);
    let suggestions = client(&stub)
        .suggest_services(&SuggestionRequest {
            filters: None,
            limit: 10,
            prefix: " plan ".to_owned(),
        })
        .await
        .unwrap();

    assert_eq!(suggestions, ["plants", "planters", "plant food"]);
    let url = stub.last().url;
    assert!(url.contains("prefix=plan"), "the prefix is trimmed: {url}");
    assert!(url.contains("limit=10"), "{url}");
}

#[tokio::test]
async fn asks_for_no_limit_it_was_not_given() {
    let stub = Stub::serving(r#"{"items":[]}"#);
    client(&stub)
        .suggest_services(&SuggestionRequest {
            filters: None,
            limit: 0,
            prefix: "pl".to_owned(),
        })
        .await
        .unwrap();
    assert!(!stub.last().url.contains("limit="), "{}", stub.last().url);
}

/// A suggestion repeated is still one suggestion, and twenty-five is all a caller gets.
#[tokio::test]
async fn keeps_each_suggestion_once_and_no_more_than_twenty_five() {
    let items = (0..40)
        .map(|index| format!(r#""s{}""#, index % 30))
        .collect::<Vec<_>>()
        .join(",");
    let stub = Stub::serving(format!(r#"{{"items":[{items}]}}"#));
    let suggestions = client(&stub)
        .suggest_services(&SuggestionRequest {
            filters: None,
            limit: 25,
            prefix: "s".to_owned(),
        })
        .await
        .unwrap();

    assert_eq!(suggestions.len(), 25);
    assert_eq!(suggestions[0], "s0");
    let mut unique = suggestions.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), suggestions.len());
}

#[tokio::test]
async fn refuses_a_prefix_the_directory_would_refuse() {
    let stub = Stub::serving(r#"{"items":[]}"#);
    for request in [
        SuggestionRequest {
            filters: None,
            limit: 0,
            prefix: "   ".to_owned(),
        },
        SuggestionRequest {
            filters: None,
            limit: 0,
            prefix: "x".repeat(129),
        },
        SuggestionRequest {
            filters: None,
            limit: 26,
            prefix: "pl".to_owned(),
        },
    ] {
        assert!(client(&stub).suggest_services(&request).await.is_err());
    }
    assert_eq!(stub.count(), 0);
}

#[tokio::test]
async fn refuses_suggestions_it_cannot_read() {
    for body in [
        r#"["plants"]"#,
        r#"{"suggestions":["plants"]}"#,
        r#"{"items":[5]}"#,
        r#"{"items":[" plants"]}"#,
        r#"{"items":["   "]}"#,
        r#"{"items":[""]}"#,
        "not json",
    ] {
        let stub = Stub::serving(body);
        let long = format!(r#"{{"items":["{}"]}}"#, "x".repeat(129));
        assert!(
            client(&stub)
                .suggest_services(&SuggestionRequest {
                    filters: None,
                    limit: 0,
                    prefix: "pl".to_owned(),
                })
                .await
                .is_err(),
            "{body}"
        );
        let stub = Stub::serving(long);
        assert!(
            client(&stub)
                .suggest_services(&SuggestionRequest {
                    filters: None,
                    limit: 0,
                    prefix: "pl".to_owned(),
                })
                .await
                .is_err()
        );
    }
}

/// A failed suggestion request reads like any other failure.
#[tokio::test]
async fn reports_a_failed_suggestion_request() {
    let stub = Stub::new(|_| Ok(with_headers(503, b"{}".to_vec(), JSON, &[])));
    assert!(
        client(&stub)
            .suggest_services(&SuggestionRequest {
                filters: None,
                limit: 0,
                prefix: "pl".to_owned(),
            })
            .await
            .is_err()
    );
}

#[tokio::test]
async fn refuses_suggestions_of_another_media_type() {
    let stub = Stub::new(|_| Ok(typed(200, br#"{"items":[]}"#.to_vec(), "text/plain")));
    assert!(
        client(&stub)
            .suggest_services(&SuggestionRequest {
                filters: None,
                limit: 0,
                prefix: "pl".to_owned(),
            })
            .await
            .is_err()
    );
}

// -- trust -----------------------------------------------------------------------------------

/// A trust filter names `tap` or nothing: it is the only trust protocol this ODP version defines,
/// and the Directory refuses any other.
#[tokio::test]
async fn refuses_a_trust_filter_this_version_does_not_define() {
    let stub = Stub::serving(page(&[], ""));
    for trust in [
        vec![TrustProtocol {
            name: Protocol::Mpp,
        }],
        vec![TrustProtocol {
            name: Protocol::Aep,
        }],
        // Exactly one descriptor is allowed, so even a repeated `tap` is one too many.
        vec![
            TrustProtocol {
                name: Protocol::Tap,
            },
            TrustProtocol {
                name: Protocol::Tap,
            },
        ],
    ] {
        let error = client(&stub)
            .search_services(&filtered(ServiceFilters {
                trust: trust.clone(),
                ..ServiceFilters::default()
            }))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("trust"), "{trust:?}: {error}");
    }
    assert_eq!(stub.count(), 0, "nothing was sent");
}

/// The one shape the Directory does accept travels as written.
#[tokio::test]
async fn sends_the_only_trust_filter_this_version_defines() {
    let stub = Stub::serving(page(&[], ""));
    client(&stub)
        .search_services(&filtered(ServiceFilters {
            trust: vec![TrustProtocol {
                name: Protocol::Tap,
            }],
            ..ServiceFilters::default()
        }))
        .await
        .unwrap();

    let body = String::from_utf8(stub.last().body).unwrap();
    assert!(body.contains(r#""trust":[{"name":"tap"}]"#), "{body}");
}

/// A request naming no trust filter carries none.
#[tokio::test]
async fn omits_a_trust_filter_the_caller_did_not_ask_for() {
    let stub = Stub::serving(page(&[], ""));
    client(&stub)
        .search_services(&SearchRequest::default())
        .await
        .unwrap();
    assert!(
        !String::from_utf8(stub.last().body)
            .unwrap()
            .contains("trust")
    );
}

/// A trust facet counting anything but a bare `tap` descriptor is one this client cannot read.
#[tokio::test]
async fn refuses_a_trust_facet_this_version_does_not_define() {
    for facets in [
        r#"{"trust":[{"count":3,"value":{"name":"mpp"}}]}"#,
        r#"{"trust":[{"count":3,"value":{"name":"tap"}},{"count":1,"value":{"name":"aep"}}]}"#,
        r#"{"trust":[{"count":3,"value":{}}]}"#,
        // A descriptor carries `name` and nothing else.
        r#"{"trust":[{"count":3,"value":{"name":"tap","extra":1}}]}"#,
    ] {
        let stub = Stub::serving(format!(r#"{{"facets":{facets},"items":[]}}"#));
        let error = client(&stub)
            .search_services(&SearchRequest::default())
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("trust facets are invalid"),
            "{facets}: {error}"
        );
    }
}

#[tokio::test]
async fn reads_the_trust_facet_this_version_defines() {
    let stub =
        Stub::serving(r#"{"facets":{"trust":[{"count":12,"value":{"name":"tap"}}]},"items":[]}"#);
    let facets = client(&stub)
        .search_services(&SearchRequest::default())
        .await
        .unwrap()
        .facets
        .unwrap();

    assert_eq!(facets.trust.len(), 1);
    assert_eq!(facets.trust[0].count, 12);
    assert_eq!(facets.trust[0].value.name, Protocol::Tap);
}

/// A page publishing no trust facet is not missing anything.
#[tokio::test]
async fn reads_a_page_that_publishes_no_trust_facet() {
    let stub =
        Stub::serving(r#"{"facets":{"keywords":[{"count":1,"value":"plants"}]},"items":[]}"#);
    let facets = client(&stub)
        .search_services(&SearchRequest::default())
        .await
        .unwrap()
        .facets
        .unwrap();
    assert!(facets.trust.is_empty());
}
