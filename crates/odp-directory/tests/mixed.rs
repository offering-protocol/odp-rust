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
        "indexed_at":"2026-09-18T11:00:00Z", "name":"Data", "description":"Data services.",
        "language":"en", "localizations":["en"], "operations":[
            {"name":"get-offering","authentication":"not-required"},
            {"name":"list-offerings","authentication":"not-required"}],
        "protocols":{"trust":[{"name":"tap"},{"name":"future"}]}})
}

fn item(kind: &str) -> Value {
    let mut item = json!({"type":kind, "service":service(), "indexed_at":"2026-09-18T12:00:00Z"});
    if kind == "collection" {
        item["collection"] =
            json!({"id":"Weather","name":"Weather forecasts","description":"Forecasts."});
    }
    item
}

#[tokio::test]
async fn decodes_mixed_results_and_preserves_unknown_types() {
    let (client, stub) = setup();
    let mut first = item("service");
    first["available_through"] = json!({"service_id":"platform", "service_origin":"https://platform.example", "name":"Platform"});
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
    assert_eq!(service.service.service_id(), Some("parent"));
    assert_eq!(
        service.available_through.as_ref().unwrap().name.as_deref(),
        Some("Platform")
    );
    assert_eq!(service.additional["extra"], true);
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
        ["POST", "GET", "POST", "GET", "GET", "GET"]
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
        "/v1/directory/suggestions?prefix=we&limit=10",
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
