use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use odp_directory::*;
use serde_json::{Value, json};

#[derive(Default)]
struct Stub {
    requests: Mutex<Vec<HttpRequest>>,
    replies: Mutex<VecDeque<HttpResponse>>,
}

impl Stub {
    fn reply(&self, status: u16, body: Value, headers: BTreeMap<String, String>) {
        self.replies.lock().unwrap().push_back(HttpResponse {
            status,
            body: serde_json::to_vec(&body).unwrap(),
            headers: BTreeMap::from([("content-type".to_owned(), "application/json".to_owned())])
                .into_iter()
                .chain(headers)
                .collect(),
        });
    }
    fn ok(&self, body: Value) {
        self.reply(200, body, BTreeMap::new());
    }
}

#[async_trait]
impl Transport for Stub {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        self.requests.lock().unwrap().push(request);
        Ok(self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected request"))
    }
}

fn setup() -> (DirectoryClient, Arc<Stub>) {
    let stub = Arc::new(Stub::default());
    (
        DirectoryClient::with_transport(Environment::Sandbox, stub.clone()),
        stub,
    )
}

fn service() -> Value {
    json!({"service_id":"parent", "service_origin":"https://api.example.com",
        "source":{"type":"odp","url":"https://api.example.com/.well-known/odp","x402_discovery":false},
        "indexed_at":"2026-09-18T11:00:00Z", "name":"Data", "description":"Data services.",
        "language":"en", "localizations":["en"], "operations":[
            {"name":"get-offering","authentication":"not-required"},
            {"name":"list-offerings","authentication":"not-required"}],
        "protocols":{"trust":[{"name":"tap"},{"name":"future"}]}})
}

#[tokio::test]
async fn sends_suggestion_filters_without_mutating_the_request() {
    let (client, stub) = setup();
    stub.ok(json!({"items":["Weather"]}));
    let mut request = SuggestionRequest {
        prefix: " we ".to_owned(),
        filters: Some(ServiceFilters {
            keywords: vec!["weather".to_owned()],
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(client.suggest(&request).await.unwrap(), ["Weather"]);
    assert_eq!(request.prefix, " we ");
    assert!(client.suggest_services(&request).await.is_err());
    request.filters.as_mut().unwrap().keywords = vec![String::new()];
    assert!(client.suggest(&request).await.is_err());
    let requests = stub.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[0].body).unwrap(),
        json!({"prefix":"we", "filters":{"keywords":["weather"]}})
    );
}

fn item(kind: &str) -> Value {
    let mut item = json!({"type":kind, "service":service(), "indexed_at":"2026-09-18T12:00:00Z"});
    if kind == "collection" {
        item["collection"] =
            json!({"id":"Weather","name":"Weather forecasts","description":"Forecasts."});
    }
    item
}

fn imported(kind: &str) -> Value {
    let mut value = item(kind);
    let service = value["service"].as_object_mut().unwrap();
    for key in [
        "description",
        "language",
        "localizations",
        "keywords",
        "operations",
        "protocols",
    ] {
        service.remove(key);
    }
    service.insert("source".to_owned(), json!({
        "type":"openapi", "url":"https://docs.example/specs/api.json?version=3&key=a%2Fb", "x402_discovery":true
    }));
    value
}

#[tokio::test]
async fn preserves_exact_sources_and_optional_metadata_without_odp_capabilities() {
    let (client, stub) = setup();
    let mut first = imported("service");
    first["service"]["source"]["extra"] = json!({"retained":true});
    for field in [
        "operations",
        "http",
        "branding",
        "mcp",
        "odp_version",
        "payment_origins",
        "search_capabilities",
    ] {
        first["service"][field] = json!("not authoritative");
    }
    let mut future = imported("collection");
    future["service"]["source"]["type"] = json!("future-format");
    future["service"]["source"]["url"] = json!("HTTPS://Docs.Example:443/other.json?x=1");
    future["service"]["service_id"] = json!("other-document");
    future["service"]["description"] = json!("");
    future["service"]["language"] = json!("en");
    future["service"]["localizations"] = json!(["en"]);
    future["service"]["keywords"] = json!(["weather"]);
    for field in [
        "documentation_url",
        "status_url",
        "support_url",
        "website_url",
    ] {
        future["service"][field] = json!(format!("https://example.com/{field}"));
    }
    stub.ok(json!({"items":[first,imported("collection"),future]}));
    let response = client
        .continue_search("/v1/directory/search?cursor=opaque")
        .await
        .unwrap();
    assert!(response.issues.is_empty(), "{:?}", response.issues);
    let DirectoryResult::Service(first) = &response.items[0] else {
        panic!("service")
    };
    assert_eq!(first.service.source.source_type, "openapi");
    assert_eq!(
        first.service.source.url,
        "https://docs.example/specs/api.json?version=3&key=a%2Fb"
    );
    assert!(first.service.source.x402_discovery);
    assert_eq!(
        first.service.source.additional["extra"],
        json!({"retained":true})
    );
    assert_eq!(
        serde_json::to_value(&first.service.source).unwrap()["type"],
        "openapi"
    );
    assert!(first.service.description.is_none());
    assert!(first.service.language.is_none());
    assert!(first.service.operations.is_empty());
    assert!(first.service.localizations.is_empty());
    assert!(first.service.keywords.is_empty());
    assert!(first.service.protocols.is_none());
    assert!(first.service.additional.is_empty());
    let DirectoryResult::Collection(second) = &response.items[1] else {
        panic!("collection")
    };
    let DirectoryResult::Collection(third) = &response.items[2] else {
        panic!("collection")
    };
    assert_eq!(second.service.service_origin, third.service.service_origin);
    assert_eq!(second.collection.id, third.collection.id);
    assert_ne!(second.service.service_id, third.service.service_id);
    assert_eq!(third.service.source.source_type, "future-format");
    assert_eq!(
        third.service.source.url,
        "HTTPS://Docs.Example:443/other.json?x=1"
    );
    assert_eq!(third.service.description.as_deref(), Some(""));
    assert_eq!(third.service.localizations, ["en"]);
    assert_eq!(third.service.keywords, ["weather"]);
    assert_eq!(
        third.service.website_url.as_deref(),
        Some("https://example.com/website_url")
    );
    assert_eq!(stub.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn isolates_invalid_sources_imported_metadata_and_native_records() {
    let (client, stub) = setup();
    let mut candidates = Vec::new();
    for field in [
        "source",
        "name",
        "service_id",
        "service_origin",
        "indexed_at",
    ] {
        let mut value = imported("service");
        value["service"].as_object_mut().unwrap().remove(field);
        candidates.push(value);
    }
    for field in ["type", "url", "x402_discovery"] {
        for missing in [true, false] {
            let mut value = imported("service");
            if missing {
                value["service"]["source"]
                    .as_object_mut()
                    .unwrap()
                    .remove(field);
            } else {
                value["service"]["source"][field] = Value::Null;
            }
            candidates.push(value);
        }
    }
    for (field, invalid) in [
        ("source", json!(null)),
        ("source", json!([])),
        ("name", json!(" ")),
        ("indexed_at", json!("yesterday")),
        ("localizations", json!([null])),
        ("keywords", json!([12])),
    ] {
        let mut value = imported("service");
        value["service"][field] = invalid;
        candidates.push(value);
    }
    for field in [
        "description",
        "language",
        "localizations",
        "keywords",
        "documentation_url",
        "status_url",
        "support_url",
        "website_url",
    ] {
        for invalid in [Value::Null, json!(42)] {
            let mut value = imported("service");
            value["service"][field] = invalid;
            candidates.push(value);
        }
    }
    for url in [
        "http://example.com/spec",
        "/openapi.json",
        "https:example.com/spec",
        "https://user:secret@example.com/spec",
        "https://example.com/spec#part",
        "https://localhost/spec",
        "https://127.0.0.1/spec",
        "https://[::1]/spec",
        "https://example.com:70000/spec",
        "https://example.com/a\nb",
        " ",
        "https://[",
    ] {
        let mut value = imported("service");
        value["service"]["source"]["url"] = json!(url);
        candidates.push(value);
    }
    let mut invalid = imported("service");
    invalid["service"]["source"]["x402_discovery"] = json!("false");
    candidates.push(invalid);
    for field in ["operations", "language", "source"] {
        let mut native = item("service");
        native["service"].as_object_mut().unwrap().remove(field);
        candidates.push(native);
    }
    let count = candidates.len();
    candidates.push(imported("service"));
    candidates.push(item("service"));
    stub.ok(json!({"items":candidates}));
    let response = client
        .search(&ResourceSearchRequest::default())
        .await
        .unwrap();
    assert_eq!(response.items.len(), 2, "{:?}", response.issues);
    assert_eq!(
        response
            .issues
            .iter()
            .map(|issue| issue.index)
            .collect::<Vec<_>>(),
        (0..count).collect::<Vec<_>>()
    );
    stub.ok(json!({"items":[imported("service")["service"],service()]}));
    let native = client
        .search_services(&SearchRequest::default())
        .await
        .unwrap();
    assert_eq!(native.items.len(), 1);
    assert_eq!(native.issues.len(), 1);
}

#[tokio::test]
async fn validates_recognized_protocol_evidence_without_synthesizing_enrollment() {
    let (client, stub) = setup();
    let valid = json!({"payments":[{"name":"x402","authentication":"required","options":["base"]},{"name":"future"}], "trust":[{"name":"tap"},{"name":"future"}]});
    let mut candidates = Vec::new();
    for protocols in [
        json!({}),
        valid,
        json!({"enrollment":[{"name":"aep"}]}),
        json!({"payments":[{"name":"future"}]}),
    ] {
        let mut item = imported("service");
        item["service"]["protocols"] = protocols;
        candidates.push(item);
    }
    let valid_count = candidates.len();
    for protocols in [
        Value::Null,
        json!([]),
        json!({"trust":[]}),
        json!({"trust":null}),
        json!({"trust":[null]}),
        json!({"trust":[{}]}),
        json!({"trust":[{"name":"tap"},{"name":"tap"}]}),
        json!({"enrollment":[{"name":"aep","extra":true}]}),
        json!({"payments":[{"name":"x402"}]}),
        json!({"payments":[{"name":"x402","authentication":"optional"}]}),
        json!({"payments":[{"name":"x402","authentication":"required","options":["unknown"]}]}),
    ] {
        let mut item = imported("service");
        item["service"]["protocols"] = protocols;
        candidates.push(item);
    }
    let invalid_count = candidates.len() - valid_count;
    stub.ok(json!({"items":candidates}));
    let response = client
        .search(&ResourceSearchRequest::default())
        .await
        .unwrap();
    assert_eq!(response.items.len(), valid_count, "{:?}", response.issues);
    assert_eq!(response.issues.len(), invalid_count);
    let DirectoryResult::Service(item) = &response.items[1] else {
        panic!("service")
    };
    let protocols = item.service.protocols.as_ref().unwrap();
    assert!(protocols.enrollment.is_empty());
    assert_eq!(protocols.payments.len(), 1);
    assert_eq!(
        protocols.payments[0].options,
        [odp_core::PaymentOption::Base]
    );
    assert_eq!(protocols.trust.len(), 1);
}

#[tokio::test]
async fn sends_source_filters_on_all_supported_routes_and_rejects_empty_or_duplicate_filters() {
    let (client, stub) = setup();
    let filters = ServiceFilters {
        sources: Some(vec![SourceType::Odp, SourceType::Openapi]),
        keywords: vec!["weather".to_owned()],
        ..Default::default()
    };
    let original = filters.clone();
    stub.ok(json!({"items":[]}));
    client
        .search(&ResourceSearchRequest {
            filters: Some(filters.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    stub.ok(json!({"items":[]}));
    client
        .search_services(&SearchRequest {
            filters: Some(filters.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    stub.ok(json!({"items":["Weather"]}));
    assert_eq!(
        client
            .suggest(&SuggestionRequest {
                filters: Some(filters.clone()),
                prefix: "we".to_owned(),
                ..Default::default()
            })
            .await
            .unwrap(),
        ["Weather"]
    );
    assert_eq!(original, filters);
    for sources in [
        vec![],
        vec![SourceType::Odp, SourceType::Odp],
        vec![SourceType::Odp, SourceType::Openapi, SourceType::Odp],
    ] {
        let filters = ServiceFilters {
            sources: Some(sources),
            ..Default::default()
        };
        assert!(
            client
                .search(&ResourceSearchRequest {
                    filters: Some(filters.clone()),
                    ..Default::default()
                })
                .await
                .is_err()
        );
        assert!(
            client
                .search_services(&SearchRequest {
                    filters: Some(filters.clone()),
                    ..Default::default()
                })
                .await
                .is_err()
        );
        assert!(
            client
                .suggest(&SuggestionRequest {
                    filters: Some(filters),
                    prefix: "we".to_owned(),
                    ..Default::default()
                })
                .await
                .is_err()
        );
    }
    for invalid in [json!(["future"]), json!(["ODP"]), json!([null])] {
        assert!(serde_json::from_value::<ServiceFilters>(json!({"sources":invalid})).is_err());
    }
    let requests = stub.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    for (request, path) in requests.iter().zip([
        "/v1/directory/search",
        "/v1/services/search",
        "/v1/directory/suggestions",
    ]) {
        assert_eq!(request.url, format!("https://sandbox.inflowpay.ai{path}"));
        assert_eq!(request.method, "POST");
        assert_eq!(
            serde_json::from_slice::<Value>(&request.body).unwrap()["filters"],
            json!({"sources":["odp","openapi"],"keywords":["weather"]})
        );
    }
    assert_eq!(
        serde_json::to_value(ServiceFilters::default()).unwrap(),
        json!({})
    );
}

#[tokio::test]
async fn decodes_mixed_results_and_preserves_unknown_types() {
    let (client, stub) = setup();
    let mut first = item("service");
    first["publisher"] = json!({"publisher_id":"platform", "website_url":"https://platform.example/catalog", "name":"Platform", "extra":true});
    first["extra"] = json!(true);
    let future = json!({"type":"future","nested":{"data":42}});
    stub.ok(json!({"items":[first,item("collection"),future],"extra":42,
        "facets":{"keywords":[{"value":"weather","count":12}]}}));
    let response = client
        .search(&ResourceSearchRequest::default())
        .await
        .unwrap();
    assert!(response.issues.is_empty());
    assert_eq!(response.additional["extra"], 42);
    assert_eq!(response.facets.unwrap().keywords[0].count, 12);
    let DirectoryResult::Service(service) = &response.items[0] else {
        panic!("service")
    };
    assert_eq!(service.service.service_id, "parent");
    assert_eq!(service.publisher.as_ref().unwrap().name, "Platform");
    assert_eq!(service.additional["extra"], true);
    assert_eq!(
        service.publisher.as_ref().unwrap().additional["extra"],
        true
    );
    assert_eq!(
        service.publisher.as_ref().unwrap().website_url,
        "https://platform.example/catalog"
    );
    assert_eq!(service.service.protocols.as_ref().unwrap().trust.len(), 1);
    let DirectoryResult::Collection(collection) = &response.items[1] else {
        panic!("collection")
    };
    assert_eq!(collection.collection.id, "Weather");
    assert_ne!(collection.indexed_at, collection.service.indexed_at);
    let DirectoryResult::Unknown { kind, raw } = &response.items[2] else {
        panic!("unknown")
    };
    assert_eq!(kind, "future");
    assert_eq!(raw, &future);
}

#[tokio::test]
async fn publisher_metadata_is_optional_and_extensible() {
    let (client, stub) = setup();
    let mut first = item("service");
    first["publisher"] = Value::Null;
    first["available_through"] = json!({"service_id":"legacy"});
    first["future_metadata"] = json!({"arbitrary":true});
    stub.ok(json!({"items":[first,item("service")]}));
    let response = client
        .search(&ResourceSearchRequest::default())
        .await
        .unwrap();
    assert!(response.issues.is_empty());
    assert_eq!(response.items.len(), 2);
    let DirectoryResult::Service(service) = &response.items[0] else {
        panic!("service")
    };
    assert!(service.publisher.is_none());
    assert_eq!(
        service.additional["available_through"],
        json!({"service_id":"legacy"})
    );
    assert_eq!(
        service.additional["future_metadata"],
        json!({"arbitrary":true})
    );
}

#[tokio::test]
async fn validates_publisher_websites() {
    for address in [
        "http://platform.example",
        "https://user@platform.example",
        "https://user:secret@platform.example",
        "://invalid",
    ] {
        let (client, stub) = setup();
        let mut first = item("service");
        first["publisher"] =
            json!({"publisher_id":"gateway","name":"Gateway","website_url":address});
        stub.ok(json!({"items":[first,item("service")]}));
        let response = client
            .search(&ResourceSearchRequest::default())
            .await
            .unwrap();
        assert_eq!(response.issues.len(), 1);
        assert_eq!(response.items.len(), 1);
    }
}

#[tokio::test]
async fn isolates_malformed_known_items_and_normalizes_future_operations() {
    let (client, stub) = setup();
    let mut invalid = Vec::new();
    for (pointer, value) in [
        ("/type", json!(null)),
        ("/service/service_id", json!("")),
        (
            "/service/service_origin",
            json!("https://user@api.example.com"),
        ),
        (
            "/service/service_origin",
            json!("https://api.example.com/path"),
        ),
        ("/service/operations", json!([])),
        ("/indexed_at", json!(false)),
        ("/collection/id", json!("../bad")),
        ("/collection/name", json!("")),
        ("/collection/description", json!(null)),
    ] {
        let mut candidate = item("collection");
        *candidate.pointer_mut(pointer).unwrap() = value;
        invalid.push(candidate);
    }
    let count = invalid.len();
    let mut valid = item("collection");
    valid["collection"]["description"] = json!("");
    valid["service"]["operations"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"future-operation","authentication":"not-required"}));
    valid["service"]["http"] = json!({"endpoint_base":"https://untrusted.example"});
    invalid.push(valid);
    stub.ok(json!({"items":invalid}));
    let response = client
        .search(&ResourceSearchRequest::default())
        .await
        .unwrap();
    assert_eq!(response.issues.len(), count);
    assert_eq!(
        response
            .issues
            .iter()
            .map(|issue| issue.index)
            .collect::<Vec<_>>(),
        (0..count).collect::<Vec<_>>()
    );
    assert_eq!(response.items.len(), 1);
    let DirectoryResult::Collection(item) = &response.items[0] else {
        panic!("collection")
    };
    assert_eq!(item.service.operations.len(), 2);
    assert!(!item.service.additional.contains_key("http"));
}

#[tokio::test]
async fn keeps_routes_bodies_and_suggestions_separate() {
    let (client, stub) = setup();
    stub.ok(json!({"items":[]}));
    client
        .search(&ResourceSearchRequest {
            query: "weather".to_owned(),
            types: Some(vec![ResultType::Collection]),
            ..Default::default()
        })
        .await
        .unwrap();
    stub.ok(json!({"items":[],"next":"/v1/directory/search?cursor=opaque"}));
    assert_eq!(
        client
            .continue_search("/v1/directory/search?cursor=opaque")
            .await
            .unwrap()
            .next
            .as_deref(),
        Some("/v1/directory/search?cursor=opaque")
    );
    stub.ok(json!({"items":[service()]}));
    assert_eq!(
        client
            .search_services(&SearchRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    stub.ok(json!({"items":[]}));
    client
        .continue_search_services("/v1/services/search?cursor=old")
        .await
        .unwrap();
    for mixed in [true, false] {
        stub.ok(json!({"items":["Weather forecasts"]}));
        let request = SuggestionRequest {
            prefix: "we".to_owned(),
            limit: 10,
            ..Default::default()
        };
        let names = if mixed {
            client.suggest(&request).await
        } else {
            client.suggest_services(&request).await
        }
        .unwrap();
        assert_eq!(names, ["Weather forecasts"]);
    }
    let requests = stub.requests.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .map(|request| request.method.as_str())
            .collect::<Vec<_>>(),
        ["POST", "GET", "POST", "GET", "POST", "GET"]
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[0].body).unwrap(),
        json!({"query":"weather","types":["collection"]})
    );
    assert!(requests[1].body.is_empty());
    for (request, path) in requests.iter().zip([
        "/v1/directory/search",
        "/v1/directory/search?cursor=opaque",
        "/v1/services/search",
        "/v1/services/search?cursor=old",
        "/v1/directory/suggestions",
        "/v1/services/suggestions?prefix=we&limit=10",
    ]) {
        assert_eq!(request.url, format!("https://sandbox.inflowpay.ai{path}"));
    }
}

#[tokio::test]
async fn rejects_invalid_requests_before_transport() {
    let (client, stub) = setup();
    for request in [
        ResourceSearchRequest {
            types: Some(vec![]),
            ..Default::default()
        },
        ResourceSearchRequest {
            types: Some(vec![ResultType::Service, ResultType::Service]),
            ..Default::default()
        },
        ResourceSearchRequest {
            limit: 101,
            ..Default::default()
        },
        ResourceSearchRequest {
            query: " ".to_owned(),
            ..Default::default()
        },
    ] {
        assert!(matches!(
            client.search(&request).await,
            Err(DirectoryError::InvalidRequest(_))
        ));
    }
    for next in [
        "",
        " ",
        "https://other.example/",
        "https://user@sandbox.inflowpay.ai/",
    ] {
        assert!(client.continue_search(next).await.is_err());
    }
    assert!(client.suggest(&SuggestionRequest::default()).await.is_err());
    assert!(stub.requests.lock().unwrap().is_empty());
    assert_eq!(
        serde_json::to_value(ResourceSearchRequest::default()).unwrap(),
        json!({})
    );
}

#[tokio::test]
async fn bounds_service_aggregation_without_extra_requests() {
    for options in [
        IterationOptions {
            max_items: 1,
            max_pages: 10,
        },
        IterationOptions {
            max_items: 10,
            max_pages: 1,
        },
    ] {
        let (client, stub) = setup();
        stub.ok(json!({"items":[service()],"next":"/v1/services/search?cursor=more"}));
        assert_eq!(
            client
                .collect_services(&SearchRequest::default(), options)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(stub.requests.lock().unwrap().len(), 1);
    }
    let (client, stub) = setup();
    stub.ok(json!({"items":[service()],"next":"/v1/services/search?cursor=more"}));
    stub.ok(json!({"items":[service()]}));
    assert_eq!(
        client
            .collect_services(&SearchRequest::default(), IterationOptions::default())
            .await
            .unwrap()
            .len(),
        2
    );
    assert_eq!(stub.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn rejects_invalid_envelopes_and_transport_failures() {
    let (client, stub) = setup();
    for body in [
        json!(null),
        json!({}),
        json!({"items":null}),
        json!({"items":vec![item("service");101]}),
        json!({"items":[],"next":false}),
        json!({"items":[],"facets":{"trust":[{"value":{"name":"mpp"},"count":1}]}}),
    ] {
        stub.ok(body);
        assert!(
            client
                .search(&ResourceSearchRequest::default())
                .await
                .is_err()
        );
    }
    stub.reply(
        429,
        json!({"detail":"rate limited"}),
        BTreeMap::from([("retry-after".to_owned(), "5".to_owned())]),
    );
    assert!(matches!(
        client.search(&ResourceSearchRequest::default()).await,
        Err(DirectoryError::Request { status: 429, .. })
    ));
    stub.reply(
        307,
        json!(null),
        BTreeMap::from([("location".to_owned(), "https://other.example/".to_owned())]),
    );
    assert!(
        client
            .search(&ResourceSearchRequest::default())
            .await
            .is_err()
    );
    stub.reply(
        303,
        json!(null),
        BTreeMap::from([("location".to_owned(), "/redirected".to_owned())]),
    );
    stub.ok(json!({"items":[]}));
    client
        .search(&ResourceSearchRequest {
            query: "weather".to_owned(),
            ..Default::default()
        })
        .await
        .unwrap();
    let requests = stub.requests.lock().unwrap();
    let last = requests.last().unwrap();
    assert_eq!(last.method, "GET");
    assert!(last.body.is_empty());
}
