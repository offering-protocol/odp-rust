//! ERR-21 and SEC-05 as they apply to Attribute Schemas and the supporting documents around them.

mod support;

use std::sync::{Arc, Mutex};

use odp_agent::{CacheFallbacks, OfferingIssueScope, ServiceClient};
use odp_directory::{HttpRequest, HttpResponse, TransportError};
use support::{
    ODP_JSON, SCHEMA_JSON, SERVICE_DOCUMENT, Stub, bare, client, response, with_headers,
};

const DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";
const CORE_VOCABULARY: &str = "https://json-schema.org/draft/2020-12/vocab/core";

#[tokio::test]
async fn example_values_are_not_interpreted_as_schema_keywords() {
    let stub = serving(|_| {
        response(200, &serde_json::to_vec(&serde_json::json!({
        "$schema":DIALECT,"type":"object","examples":[{"$ref":"https://unrelated.example/value","$dynamicRef":"not-a-schema-reference","$vocabulary":{"unknown":true}}]
    })).unwrap(), SCHEMA_JSON)
    });
    let details = client(&stub).get_offering_details("plant-1").await.unwrap();
    assert!(details.issues.is_empty(), "{:?}", details.issues);
    assert_eq!(
        stub.requests()
            .iter()
            .filter(|r| r.url.contains("schemas.example"))
            .count(),
        1
    );
}

#[tokio::test]
async fn unsupported_standard_namespace_vocabulary_is_still_unsupported() {
    let stub = serving(|_| {
        response(200, &serde_json::to_vec(&serde_json::json!({"$schema":DIALECT,"$vocabulary":{"https://json-schema.org/draft/2020-12/vocab/not-implemented":true}})).unwrap(), SCHEMA_JSON)
    });
    assert!(schema_issue(&stub).await.contains("not-implemented"));
}

#[tokio::test]
async fn malformed_identifiers_are_not_repaired_during_bundling() {
    for id in [
        serde_json::json!(42),
        serde_json::json!("https://schemas.example/root#fragment"),
    ] {
        let stub = serving(move |_| {
            response(
                200,
                &serde_json::to_vec(&serde_json::json!({"$schema":DIALECT,"$id":id})).unwrap(),
                SCHEMA_JSON,
            )
        });
        assert!(schema_issue(&stub).await.contains("$id"));
    }
}

#[tokio::test]
async fn revalidates_a_redirected_schema_at_its_final_url() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let calls = AtomicUsize::new(0);
    let stub = serving(move |url| match url {
        "https://schemas.example/root.json" => bare(302, &[("location", "/moved/root.json")]),
        "https://schemas.example/moved/root.json" => {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                with_headers(
                    200,
                    &leaf(),
                    SCHEMA_JSON,
                    &[("cache-control", "max-age=0"), ("etag", "schema-v1")],
                )
            } else {
                bare(304, &[])
            }
        }
        other => panic!("Unexpected schema request {other}"),
    });
    let client = client(&stub);
    let first = client.get_offering_details("plant-1").await.unwrap();
    let second = client.get_offering_details("plant-1").await.unwrap();
    assert!(first.issues.is_empty());
    assert!(second.issues.is_empty());
    assert_eq!(first.attribute_schema, second.attribute_schema);
    assert_eq!(
        stub.requests()
            .iter()
            .filter(|r| r.url == "https://schemas.example/root.json")
            .count(),
        1
    );
}

#[tokio::test]
async fn resolves_from_final_url_and_returns_a_self_contained_schema() {
    let stub = serving(|url| {
        match url {
        "https://schemas.example/root.json" => bare(302, &[("location", "/nested/root.json")]),
        "https://schemas.example/nested/root.json" => response(200, &serde_json::to_vec(&serde_json::json!({"$schema":DIALECT,"$ref":"child.json"})).unwrap(), SCHEMA_JSON),
        "https://schemas.example/nested/child.json" => response(200, &serde_json::to_vec(&serde_json::json!({"$schema":DIALECT,"type":"object","properties":{"size":{"type":"integer"}}})).unwrap(), SCHEMA_JSON),
        other => panic!("Unexpected schema request {other}"),
    }
    });
    let details = client(&stub).get_offering_details("plant-1").await.unwrap();
    assert!(details.issues.is_empty(), "{:?}", details.issues);
    let bundled = details.attribute_schema.unwrap();
    // jsonschema has no HTTP retrieval feature enabled in this workspace.
    let validator = jsonschema::validator_for(&bundled).unwrap();
    assert!(validator.is_valid(&serde_json::json!({"size":1})));
    assert!(!validator.is_valid(&serde_json::json!({"size":"wrong"})));
}

#[tokio::test]
async fn bundled_references_preserve_fragments_across_redirects() {
    let stub = serving(|url| {
        match url {
        "https://schemas.example/root.json" => response(200, &serde_json::to_vec(&serde_json::json!({"$schema":DIALECT,"type":"object","properties":{"size":{"$ref":"child.json#/$defs/size"}}})).unwrap(), SCHEMA_JSON),
        "https://schemas.example/child.json" => bare(302, &[("location", "/moved/child.json")]),
        "https://schemas.example/moved/child.json" => response(200, &serde_json::to_vec(&serde_json::json!({"$schema":DIALECT,"$defs":{"size":{"type":"integer"}}})).unwrap(), SCHEMA_JSON),
        other => panic!("Unexpected schema request {other}"),
    }
    });
    let details = client(&stub).get_offering_details("plant-1").await.unwrap();
    assert!(details.issues.is_empty(), "{:?}", details.issues);
    let validator = jsonschema::validator_for(&details.attribute_schema.unwrap()).unwrap();
    assert!(validator.is_valid(&serde_json::json!({"size":1})));
    assert!(!validator.is_valid(&serde_json::json!({"size":"wrong"})));
}

#[tokio::test]
async fn supporting_redirects_cannot_change_origin() {
    let stub = serving(|_| bare(302, &[("location", "https://another.example/schema")]));
    assert!(schema_issue(&stub).await.contains("origin"));
    assert!(
        !stub
            .requests()
            .iter()
            .any(|r| r.url.contains("another.example"))
    );
}

/// An Offering whose attributes are described by a schema at `https://schemas.example/root.json`.
const OFFERING: &[u8] = br#"{"attributes":{"size":1},"id":"plant-1","name":"Plant","odp_version":"1.0","schema":{"url":"https://schemas.example/root.json"}}"#;

fn schema(body: String) -> Vec<u8> {
    body.into_bytes()
}

/// A leaf schema that references nothing.
fn leaf() -> Vec<u8> {
    schema(format!(r#"{{"$schema":"{DIALECT}","type":"object"}}"#))
}

/// A Service answering the Service Document, the Offering, and schemas from a reply function.
fn serving(schemas: impl Fn(&str) -> HttpResponse + Send + Sync + 'static) -> Arc<Stub> {
    Stub::new(move |request| {
        Ok(if request.url.ends_with("/.well-known/odp") {
            response(200, SERVICE_DOCUMENT, ODP_JSON)
        } else if request.url.contains("schemas.example") {
            schemas(&request.url)
        } else {
            response(200, OFFERING, ODP_JSON)
        })
    })
}

/// The single issue an Offering reports about its Attribute Schema.
async fn schema_issue(stub: &Arc<Stub>) -> String {
    let details = client(stub).get_offering_details("plant-1").await.unwrap();
    assert_eq!(
        details.issues.len(),
        1,
        "expected one schema issue: {:?}",
        details.issues
    );
    assert_eq!(details.issues[0].scope, OfferingIssueScope::AttributeSchema);
    details.issues[0].message.clone()
}

// -- the shape of a schema ----------------------------------------------------------------

/// A document that does not declare the 2020-12 dialect is not a schema this Agent can apply.
#[tokio::test]
async fn refuses_a_document_that_declares_no_dialect() {
    let stub = serving(|_| response(200, br#"{"type":"object"}"#, SCHEMA_JSON));
    assert!(
        schema_issue(&stub).await.contains("Draft 2020-12"),
        "the dialect is what is missing"
    );
}

/// A schema requiring a vocabulary this Agent does not implement cannot be evaluated correctly.
#[tokio::test]
async fn refuses_a_schema_requiring_an_unsupported_vocabulary() {
    let body = schema(format!(
        r#"{{"$schema":"{DIALECT}","$vocabulary":{{"{CORE_VOCABULARY}":true,"https://vocab.example/custom":true}},"type":"object"}}"#
    ));
    let stub = serving(move |_| response(200, &body, SCHEMA_JSON));
    assert!(
        schema_issue(&stub).await.contains("vocab.example/custom"),
        "the unsupported vocabulary is named"
    );
}

/// A vocabulary the schema only prefers is not a reason to refuse it.
#[tokio::test]
async fn accepts_a_vocabulary_the_schema_does_not_require() {
    let body = schema(format!(
        r#"{{"$schema":"{DIALECT}","$vocabulary":{{"{CORE_VOCABULARY}":true,"https://vocab.example/custom":false}},"properties":{{"size":{{"type":"integer"}}}},"type":"object"}}"#
    ));
    let stub = serving(move |_| response(200, &body, SCHEMA_JSON));

    let details = client(&stub).get_offering_details("plant-1").await.unwrap();
    assert!(details.issues.is_empty(), "{:?}", details.issues);
    assert_eq!(details.offering.attributes.len(), 1);
}

// -- the bounds of a schema graph ---------------------------------------------------------

/// A graph of more than 16 documents is more than an Agent will assemble.
#[tokio::test]
async fn refuses_a_schema_graph_of_more_than_sixteen_documents() {
    let references = (0..16)
        .map(|index| format!(r#"{{"$ref":"https://schemas.example/leaf-{index}.json"}}"#))
        .collect::<Vec<_>>()
        .join(",");
    let root = schema(format!(
        r#"{{"$schema":"{DIALECT}","allOf":[{references}],"type":"object"}}"#
    ));
    let other = leaf();
    let stub = serving(move |url| {
        response(
            200,
            if url.contains("root") { &root } else { &other },
            SCHEMA_JSON,
        )
    });

    assert!(schema_issue(&stub).await.contains("16 documents"));
}

/// A chain of references deeper than eight levels is refused at the level that crosses the bound.
#[tokio::test]
async fn refuses_a_schema_graph_deeper_than_eight_levels() {
    let stub = serving(|url| {
        let level: usize = url
            .split("level-")
            .nth(1)
            .and_then(|value| value.split('.').next())
            .and_then(|value| value.parse().ok())
            .unwrap_or_default();
        let body = schema(format!(
            r#"{{"$schema":"{DIALECT}","allOf":[{{"$ref":"https://schemas.example/level-{}.json"}}],"type":"object"}}"#,
            level + 1
        ));
        response(200, &body, SCHEMA_JSON)
    });

    assert!(schema_issue(&stub).await.contains("eight reference levels"));
}

/// The graph as a whole is bounded too, however few documents carry it.
#[tokio::test]
async fn refuses_a_schema_graph_over_its_byte_limit() {
    let padding = "x".repeat(200_000);
    let references = (0..6)
        .map(|index| format!(r#"{{"$ref":"https://schemas.example/leaf-{index}.json"}}"#))
        .collect::<Vec<_>>()
        .join(",");
    let root = schema(format!(
        r#"{{"$schema":"{DIALECT}","allOf":[{references}],"description":"{padding}","type":"object"}}"#
    ));
    let big = schema(format!(
        r#"{{"$schema":"{DIALECT}","description":"{padding}","type":"object"}}"#
    ));
    let stub = serving(move |url| {
        response(
            200,
            if url.contains("root") { &root } else { &big },
            SCHEMA_JSON,
        )
    });

    assert!(schema_issue(&stub).await.contains("byte limit"));
}

/// A single document over its own limit is refused before the graph is even assembled.
#[tokio::test]
async fn refuses_a_schema_document_over_its_byte_limit() {
    let body = schema(format!(
        r#"{{"$schema":"{DIALECT}","description":"{}","type":"object"}}"#,
        "x".repeat(262_145)
    ));
    let stub = serving(move |_| response(200, &body, SCHEMA_JSON));

    assert!(schema_issue(&stub).await.contains("byte limit"));
}

/// ERR-20: a declared Content-Length over the limit is refused without reading the body.
#[tokio::test]
async fn refuses_a_schema_that_declares_more_than_it_may_send() {
    let stub =
        serving(|_| with_headers(200, &leaf(), SCHEMA_JSON, &[("content-length", "262145")]));

    assert!(schema_issue(&stub).await.contains("byte limit"));
}

/// A schema served as something other than a schema is not read as one.
#[tokio::test]
async fn refuses_a_schema_of_an_unsupported_media_type() {
    let stub = serving(|_| response(200, &leaf(), "text/html"));
    assert!(schema_issue(&stub).await.contains("media type"));
}

/// The same document reached twice in one graph is fetched once.
#[tokio::test]
async fn fetches_a_shared_schema_document_once() {
    let root = schema(format!(
        r#"{{"$schema":"{DIALECT}","allOf":[{{"$ref":"https://schemas.example/shared.json"}},{{"$ref":"https://schemas.example/shared.json#/$defs/sized"}}],"type":"object"}}"#
    ));
    let shared = schema(format!(
        r#"{{"$schema":"{DIALECT}","$defs":{{"sized":{{"properties":{{"size":{{"type":"integer"}}}},"type":"object"}}}},"type":"object"}}"#
    ));
    let stub = serving(move |url| {
        response(
            200,
            if url.contains("root") { &root } else { &shared },
            SCHEMA_JSON,
        )
    });

    let details = client(&stub).get_offering_details("plant-1").await.unwrap();
    assert!(details.issues.is_empty(), "{:?}", details.issues);
    assert_eq!(
        stub.requests()
            .iter()
            .filter(|request| request.url.contains("shared.json"))
            .count(),
        1
    );
}

/// SEC-05: a schema reference over plain HTTP is not followed.
#[tokio::test]
async fn refuses_a_schema_reference_that_is_not_https() {
    let root = schema(format!(
        r#"{{"$schema":"{DIALECT}","allOf":[{{"$ref":"http://schemas.example/leaf.json"}}],"type":"object"}}"#
    ));
    let stub = serving(move |_| response(200, &root, SCHEMA_JSON));

    assert!(schema_issue(&stub).await.contains("HTTPS"));
}

// -- caching a supporting document ---------------------------------------------------------

/// CCH-02: a schema stays fresh for as long as the Service says, and is not re-fetched.
#[tokio::test]
async fn serves_a_fresh_schema_from_the_cache() {
    let stub = serving(|_| {
        with_headers(
            200,
            &leaf(),
            SCHEMA_JSON,
            &[("cache-control", "max-age=3600")],
        )
    });
    let client = client(&stub);

    client.get_offering_details("plant-1").await.unwrap();
    client.get_offering_details("plant-1").await.unwrap();

    assert_eq!(
        stub.requests()
            .iter()
            .filter(|request| request.url.contains("schemas.example"))
            .count(),
        1,
        "the second read came from the cache"
    );
}

/// CCH-05: a stale schema is revalidated, and a 304 refreshes what is already held.
#[tokio::test]
async fn revalidates_a_stale_schema_with_its_entity_tag() {
    let seen = Arc::new(Mutex::new(Vec::<HttpRequest>::new()));
    let recorder = seen.clone();
    let stub = Stub::new(move |request| {
        if request.url.ends_with("/.well-known/odp") {
            return Ok(response(200, SERVICE_DOCUMENT, ODP_JSON));
        }
        if !request.url.contains("schemas.example") {
            return Ok(response(200, OFFERING, ODP_JSON));
        }
        recorder.lock().unwrap().push(request.clone());
        Ok(if request.headers.contains_key("if-none-match") {
            bare(304, &[("cache-control", "max-age=60")])
        } else {
            with_headers(
                200,
                &leaf(),
                SCHEMA_JSON,
                &[("cache-control", "max-age=0"), ("etag", "\"v1\"")],
            )
        })
    });
    let client = client(&stub);

    client.get_offering_details("plant-1").await.unwrap();
    let details = client.get_offering_details("plant-1").await.unwrap();

    assert!(details.issues.is_empty(), "{:?}", details.issues);
    let requests = seen.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].headers.get("if-none-match").map(String::as_str),
        Some("\"v1\"")
    );
}

/// CCH-05: with no entity tag to offer, a stale schema is revalidated by its modification date.
#[tokio::test]
async fn revalidates_a_stale_schema_by_its_modification_date() {
    let seen = Arc::new(Mutex::new(Vec::<HttpRequest>::new()));
    let recorder = seen.clone();
    let stub = Stub::new(move |request| {
        if request.url.ends_with("/.well-known/odp") {
            return Ok(response(200, SERVICE_DOCUMENT, ODP_JSON));
        }
        if !request.url.contains("schemas.example") {
            return Ok(response(200, OFFERING, ODP_JSON));
        }
        recorder.lock().unwrap().push(request.clone());
        Ok(if request.headers.contains_key("if-modified-since") {
            bare(304, &[("cache-control", "max-age=60")])
        } else {
            with_headers(
                200,
                &leaf(),
                SCHEMA_JSON,
                &[
                    ("cache-control", "max-age=0"),
                    ("last-modified", "Tue, 08 Sep 2026 00:00:00 GMT"),
                ],
            )
        })
    });
    let client = client(&stub);

    client.get_offering_details("plant-1").await.unwrap();
    let details = client.get_offering_details("plant-1").await.unwrap();

    assert!(details.issues.is_empty(), "{:?}", details.issues);
    let requests = seen.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    assert!(!requests[1].headers.contains_key("if-none-match"));
    assert_eq!(
        requests[1]
            .headers
            .get("if-modified-since")
            .map(String::as_str),
        Some("Tue, 08 Sep 2026 00:00:00 GMT")
    );
}

/// A 304 with nothing held is a Service mistake the Agent will not guess around.
#[tokio::test]
async fn refuses_a_schema_revalidation_it_never_asked_for() {
    let stub = serving(|_| bare(304, &[]));
    assert!(schema_issue(&stub).await.contains("304"));
}

/// CCH-06: `no-store` on a revalidation drops what was held rather than extending it.
#[tokio::test]
async fn drops_a_held_schema_the_service_asks_it_to_forget() {
    let stub = Stub::new(move |request| {
        if request.url.ends_with("/.well-known/odp") {
            return Ok(response(200, SERVICE_DOCUMENT, ODP_JSON));
        }
        if !request.url.contains("schemas.example") {
            return Ok(response(200, OFFERING, ODP_JSON));
        }
        Ok(if request.headers.contains_key("if-none-match") {
            bare(304, &[("cache-control", "no-store")])
        } else {
            with_headers(
                200,
                &leaf(),
                SCHEMA_JSON,
                &[("cache-control", "max-age=0"), ("etag", "\"v1\"")],
            )
        })
    });
    let client = client(&stub);

    client.get_offering_details("plant-1").await.unwrap();
    client.get_offering_details("plant-1").await.unwrap();
    // The third read has nothing to revalidate against, so it is a plain request again.
    client.get_offering_details("plant-1").await.unwrap();

    let conditional = stub
        .requests()
        .iter()
        .filter(|request| {
            request.url.contains("schemas.example") && request.headers.contains_key("if-none-match")
        })
        .count();
    assert_eq!(conditional, 1, "the held representation was forgotten");
}

/// CCH-06: `no-store` on the response itself keeps nothing at all.
#[tokio::test]
async fn keeps_nothing_of_a_schema_marked_no_store() {
    let stub =
        serving(|_| with_headers(200, &leaf(), SCHEMA_JSON, &[("cache-control", "no-store")]));
    let client = client(&stub);

    client.get_offering_details("plant-1").await.unwrap();
    client.get_offering_details("plant-1").await.unwrap();

    assert!(
        stub.requests()
            .iter()
            .filter(|request| request.url.contains("schemas.example"))
            .all(|request| !request.headers.contains_key("if-none-match")),
        "nothing was held to revalidate"
    );
}

/// ERR-24: a supporting document follows redirects, and only over HTTPS.
#[tokio::test]
async fn follows_a_schema_redirect_to_its_target() {
    let stub = serving(|url| {
        if url.contains("root") {
            bare(
                308,
                &[("location", "https://schemas.example/moved/leaf.json")],
            )
        } else {
            response(200, &leaf(), SCHEMA_JSON)
        }
    });

    let details = client(&stub).get_offering_details("plant-1").await.unwrap();
    assert!(details.issues.is_empty(), "{:?}", details.issues);
    assert!(
        stub.requests()
            .iter()
            .any(|request| request.url.contains("/moved/"))
    );
}

#[tokio::test]
async fn refuses_a_schema_redirect_that_leaves_https() {
    let stub = serving(|_| bare(302, &[("location", "http://schemas.example/root.json")]));
    assert!(schema_issue(&stub).await.contains("origin"));
}

#[tokio::test]
async fn refuses_a_schema_redirect_that_names_no_target() {
    let stub = serving(|_| bare(307, &[]));
    assert!(schema_issue(&stub).await.contains("Location"));
}

/// ERR-25: the sixth redirect is one too many.
#[tokio::test]
async fn refuses_a_sixth_schema_redirect() {
    let stub = serving(|url| {
        let step: usize = url
            .split("step=")
            .nth(1)
            .and_then(|value| value.parse().ok())
            .unwrap_or_default();
        bare(
            308,
            &[(
                "location",
                Box::leak(
                    format!("https://schemas.example/root.json?step={}", step + 1).into_boxed_str(),
                ),
            )],
        )
    });

    assert!(schema_issue(&stub).await.contains("five redirects"));
}

/// A supporting document is fetched over HTTPS, so a client on loopback cannot reach one.
#[tokio::test]
async fn refuses_a_supporting_document_url_that_is_not_https() {
    let offering = br#"{"actions":[{"authentication":"not-required","http":{"href":"/buy","method":"POST","request":{"content_type":"application/json","schema":{"url":"https://schemas.example/buy.json"}}},"id":"buy","rel":"purchase"}],"id":"plant-1","name":"Plant","odp_version":"1.0"}"#;
    let stub = Stub::new(move |request| {
        Ok(if request.url.ends_with("/.well-known/odp") {
            response(200, SERVICE_DOCUMENT, ODP_JSON)
        } else {
            response(200, offering, ODP_JSON)
        })
    });
    // A transport that refuses everything proves the URL check runs before anything is sent.
    let client = ServiceClient::with_transport("https://plants.example", stub.clone())
        .unwrap()
        .with_supporting_transport(Arc::new(Refusing))
        .with_cache_fallbacks(CacheFallbacks::default());

    let error = client
        .resolve_action("plant-1", "buy")
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("refused"), "{error}");
}

struct Refusing;

#[async_trait::async_trait]
impl odp_directory::Transport for Refusing {
    async fn send(&self, _request: HttpRequest) -> Result<HttpResponse, TransportError> {
        Err(TransportError {
            message: "refused".to_owned(),
        })
    }
}
