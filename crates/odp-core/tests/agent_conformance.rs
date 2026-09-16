//! ROLE-04 and CNF-09: what an Agent does with a member a later ODP version defined.
//!
//! An Agent that refused every document carrying something it did not recognize would stop working
//! the day a Service adopted a newer ODP. So the Agent entry points drop what they cannot read and
//! keep the rest, and these tests pin down exactly how much gets dropped: the unreadable descriptor,
//! not the document around it.

mod support;

use odp_core::{ParseError, normalize_agent_response, parse_agent_service_document};
use serde_json::{Value, json};
use support::{amend, encode, service_document};

fn normalize(kind: &str, document: &Value) -> Value {
    let encoded = normalize_agent_response(&encode(document), kind).expect("readable JSON");
    serde_json::from_slice(&encoded).expect("normalization returns JSON")
}

fn read(changes: Value) -> Result<odp_core::ServiceDocument, ParseError> {
    parse_agent_service_document(&encode(&amend(service_document(), changes)))
}

// -- Service Document ------------------------------------------------------------------------------

/// An operation ODP does not name is dropped, and the operations beside it are kept.
#[test]
fn drops_an_operation_odp_does_not_name() {
    let document = read(json!({"operations": [
        {"authentication": "not-required", "name": "list-offerings"},
        {"authentication": "not-required", "name": "get-offering"},
        {"authentication": "not-required", "name": "rent-offering"}
    ]}))
    .unwrap();

    assert_eq!(document.operations.len(), 2);
}

/// An authentication requirement ODP does not name leaves the Agent unable to say whether it must
/// sign in, so that descriptor goes rather than being guessed at.
#[test]
fn drops_an_operation_whose_authentication_it_cannot_read() {
    let document = read(json!({"operations": [
        {"authentication": "not-required", "name": "list-offerings"},
        {"authentication": "not-required", "name": "get-offering"},
        {"authentication": "biometric", "name": "list-collections"}
    ]}))
    .unwrap();

    assert_eq!(document.operations.len(), 2);
}

/// A descriptor carrying a member ODP does not define describes something this Agent cannot honour.
#[test]
fn drops_a_descriptor_that_carries_more_than_odp_defines() {
    let normalized = normalize(
        "service-document",
        &amend(
            service_document(),
            json!({"operations": [
                {"authentication": "not-required", "name": "list-offerings"},
                {"authentication": "not-required", "name": "get-offering", "quota": 10}
            ]}),
        ),
    );

    assert_eq!(normalized["operations"].as_array().unwrap().len(), 1);
}

/// With every descriptor dropped the member itself goes, rather than being left empty.
#[test]
fn removes_a_member_whose_every_entry_was_dropped() {
    let normalized = normalize(
        "service-document",
        &amend(
            service_document(),
            json!({"mcp": [{"type": "sse", "url": "https://plants.example/mcp"}]}),
        ),
    );

    assert!(normalized.get("mcp").is_none());
}

/// A protocol ODP does not name is dropped from its category, and an emptied category with it.
#[test]
fn drops_a_protocol_odp_does_not_name() {
    let normalized = normalize(
        "service-document",
        &amend(
            service_document(),
            json!({"protocols": {
                "payments": [
                    {"authentication": "not-required", "name": "mpp", "options": ["card"]},
                    {"authentication": "not-required", "name": "swift"}
                ],
                "trust": [{"name": "web-of-trust"}]
            }}),
        ),
    );

    assert_eq!(
        normalized["protocols"]["payments"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(normalized["protocols"].get("trust").is_none());
}

/// With no category left the whole member goes.
#[test]
fn removes_protocols_whose_every_category_was_dropped() {
    let normalized = normalize(
        "service-document",
        &amend(
            service_document(),
            json!({"protocols": {"enrollment": [{"name": "oauth"}]}}),
        ),
    );

    assert!(normalized.get("protocols").is_none());
}

/// A payment option ODP does not name is dropped from its descriptor, not from the protocol.
#[test]
fn drops_a_payment_option_odp_does_not_name() {
    let document = read(json!({"protocols": {"payments": [
        {"authentication": "not-required", "name": "mpp", "options": ["card", "cheque"]}
    ]}}))
    .unwrap();

    let payments = &document.protocols.unwrap().payments;
    assert_eq!(payments.len(), 1);
    assert_eq!(payments[0].options.len(), 1);
}

/// A branding image in a format the Agent cannot render is dropped; the other one stays.
#[test]
fn drops_branding_it_cannot_render() {
    let normalized = normalize(
        "service-document",
        &amend(
            service_document(),
            json!({"branding": {
                "icon": {"src": "/icon.png", "type": "image/png"},
                "logo": {"src": "/logo.tiff", "type": "image/tiff"},
                "banner": {"src": "/banner.png"}
            }}),
        ),
    );

    assert!(normalized["branding"].get("icon").is_some());
    assert!(normalized["branding"].get("logo").is_none());
    assert!(normalized["branding"].get("banner").is_none());
}

#[test]
fn removes_branding_it_can_render_nothing_of() {
    let normalized = normalize(
        "service-document",
        &amend(
            service_document(),
            json!({"branding": {"icon": {"src": "/icon.tiff", "type": "image/tiff"}}}),
        ),
    );

    assert!(normalized.get("branding").is_none());
}

/// An inline Filter Definition the Agent cannot evaluate is dropped from the advertisement.
#[test]
fn drops_an_inline_definition_it_cannot_evaluate() {
    let capabilities = |filters: Value| {
        normalize(
            "service-document",
            &amend(
                service_document(),
                json!({"search_capabilities": {"filters": filters}}),
            ),
        )
    };

    let kept = capabilities(json!({"inline": [
        support::filter_definition("string"),
        amend(support::filter_definition("string"), json!({"id": "colour", "type": "colour"}))
    ]}));
    assert_eq!(
        kept["search_capabilities"]["filters"]["inline"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let emptied = capabilities(json!({"inline": [
        amend(support::filter_definition("string"), json!({"operators": ["startswith"]}))
    ]}));
    assert!(emptied.get("search_capabilities").is_none());
}

/// A unit system ODP does not name is a dimension the Agent cannot convert, so that definition goes.
#[test]
fn drops_an_inline_definition_measured_in_a_system_it_does_not_know() {
    let normalized = normalize(
        "service-document",
        &amend(
            service_document(),
            json!({"search_capabilities": {"filters": {"inline": [
                amend(
                    support::filter_definition("integer"),
                    json!({"unit": {"system": "imperial", "code": "lb"}})
                )
            ]}}}),
        ),
    );

    assert!(normalized.get("search_capabilities").is_none());
}

/// A Sort Definition that orders in a direction the Agent cannot follow is dropped the same way.
#[test]
fn drops_an_inline_sort_it_cannot_follow() {
    let normalized = normalize(
        "service-document",
        &amend(
            service_document(),
            json!({"search_capabilities": {"sorts": {"inline": [
                amend(
                    support::sort_definition(),
                    json!({"keys": [
                        {"filter_id": "price", "direction": "sideways", "missing": "last"}
                    ]})
                )
            ]}}}),
        ),
    );

    assert!(normalized.get("search_capabilities").is_none());
}

// -- Offering --------------------------------------------------------------------------------------

/// An Action the Agent cannot perform exactly as written is dropped rather than approximated.
#[test]
fn drops_an_action_it_cannot_perform_as_written() {
    for change in [
        json!({"authentication": "biometric"}),
        json!({"quota": 10}),
        json!({"http": {"href": "/buy", "method": "DELETE"}}),
        json!({"http": {"href": "/buy", "method": "POST", "retries": 3}}),
        json!({"http": {
            "href": "/buy", "method": "POST", "request": {"content_type": "text/csv", "encoding": "gzip"}
        }}),
        json!({"http": {
            "href": "/buy",
            "method": "POST",
            "request": {"content_type": "application/json", "schema": {"url": "/s.json", "draft": 7}}
        }}),
        json!({"openapi": {"operation_id": "buy", "server": "https://plants.example"}}),
    ] {
        let offering = amend(
            support::offering(),
            json!({"actions": [amend(support::action("buy"), change.clone())]}),
        );
        let normalized = normalize("offering", &offering);

        assert!(normalized.get("actions").is_none(), "{change}");
    }
}

/// A method ODP does define is left alone.
#[test]
fn keeps_an_action_it_can_perform() {
    for method in ["GET", "POST"] {
        let offering = amend(
            support::offering(),
            json!({"actions": [amend(
                support::action("buy"),
                json!({"http": {"href": "/buy", "method": method}})
            )]}),
        );

        assert_eq!(
            normalize("offering", &offering)["actions"]
                .as_array()
                .unwrap()
                .len(),
            1,
            "{method}"
        );
    }
}

/// An Attribute Schema reference carrying more than a URL points at something else entirely.
#[test]
fn drops_an_attribute_schema_reference_it_cannot_follow() {
    let offering = amend(
        support::offering(),
        json!({"schema": {"url": "https://plants.example/schema.json", "draft": 7}}),
    );

    assert!(normalize("offering", &offering).get("schema").is_none());
}

/// An image in a format the Agent cannot render is dropped, and unknown members are stripped from
/// the ones it keeps.
#[test]
fn drops_an_image_it_cannot_render_and_strips_the_rest() {
    let offering = amend(
        support::offering(),
        json!({"images": [
            {"src": "/a.tiff", "type": "image/tiff"},
            {"src": "/b.png", "type": "image/png", "focal_point": "centre"}
        ]}),
    );
    let normalized = normalize("offering", &offering);

    let images = normalized["images"].as_array().unwrap();
    assert_eq!(images.len(), 1);
    assert!(images[0].get("focal_point").is_none());
    assert_eq!(images[0]["src"], "/b.png");
}

/// A Collection is normalized the same way an Offering is, minus the Offering-only members.
#[test]
fn normalizes_a_collection_the_same_way() {
    let collection = amend(
        support::collection(),
        json!({"images": [{"src": "/a.tiff", "type": "image/tiff"}]}),
    );

    assert!(normalize("collection", &collection).get("images").is_none());
}

// -- pages and problems ------------------------------------------------------------------------------

/// A page is normalized item by item, so one unreadable member does not cost the whole page.
#[test]
fn normalizes_every_item_on_a_page() {
    for (kind, item) in [
        (
            "offering-page",
            amend(support::offering(), json!({"price": {"type": "auction"}})),
        ),
        (
            "collection-page",
            amend(
                support::collection(),
                json!({"images": [{"src": "/a.tiff", "type": "image/tiff"}]}),
            ),
        ),
    ] {
        let normalized = normalize(kind, &support::page(json!([item])));
        let first = &normalized["items"][0];

        assert!(
            first.get("price").is_none() && first.get("images").is_none(),
            "{kind}"
        );
        assert_eq!(first["id"], normalized["items"][0]["id"]);
    }
}

/// A capability page drops the definitions the Agent cannot use and keeps the others.
#[test]
fn drops_definitions_it_cannot_use_from_a_capability_page() {
    let filters = normalize(
        "filter-page",
        &support::page(json!([
            support::filter_definition("string"),
            amend(
                support::filter_definition("string"),
                json!({"type": "colour"})
            )
        ])),
    );
    assert_eq!(filters["items"].as_array().unwrap().len(), 1);

    let sorts = normalize(
        "sort-page",
        &support::page(json!([
            support::sort_definition(),
            amend(
                support::sort_definition(),
                json!({"keys": [{"filter_id": "p", "direction": "ascending", "missing": "middle"}]})
            )
        ])),
    );
    assert_eq!(sorts["items"].as_array().unwrap().len(), 1);
}

/// PRB-14: a parameter located somewhere ODP does not name cannot be pointed at, so it is dropped
/// while the problem itself is still reported.
#[test]
fn drops_an_invalid_parameter_it_cannot_locate() {
    let problem = amend(
        support::problem(),
        json!({"invalid_params": [
            {"in": "query", "name": "limit", "reason": "too large"},
            {"in": "cookie", "name": "session", "reason": "expired"}
        ]}),
    );
    let normalized = normalize("problem", &problem);

    assert_eq!(normalized["invalid_params"].as_array().unwrap().len(), 1);
}

// -- boundaries --------------------------------------------------------------------------------------

/// A body that is not JSON is not something normalization can repair.
#[test]
fn refuses_a_body_that_is_not_json() {
    let error = normalize_agent_response(b"not json", "offering").unwrap_err();
    assert!(matches!(error, ParseError::Validation(_)));
}

/// A kind with nothing to normalize, and a document of the wrong shape, are both left as they are.
#[test]
fn leaves_alone_what_it_has_no_rule_for() {
    assert_eq!(
        normalize("resource-identity", &json!({"id": "plant-1"}))["id"],
        "plant-1"
    );
    assert_eq!(normalize("offering", &json!([1, 2])), json!([1, 2]));
    assert_eq!(normalize("service-document", &json!("text")), json!("text"));
    assert_eq!(
        normalize("offering-page", &json!({"items": "none"}))["items"],
        "none"
    );
}

/// A member of the right name but the wrong type is not a list to filter.
#[test]
fn leaves_a_member_that_is_not_a_list_alone() {
    for document in [
        json!({"operations": "all", "mcp": 3, "branding": "none", "search_capabilities": 1}),
        json!({"protocols": {"payments": "all"}}),
    ] {
        assert_eq!(normalize("service-document", &document), document);
    }

    let offering = json!({"actions": "all", "images": 3, "schema": "none", "price": 1});
    assert_eq!(normalize("offering", &offering), offering);
}

/// A member whose entries the Agent can all read is left exactly as it was, and one whose entries
/// it can read none of goes entirely. These are the two ends of the same filter.
#[test]
fn removes_a_member_it_emptied_and_keeps_one_it_did_not() {
    for (member, entry) in [
        (
            "operations",
            json!({"authentication": "biometric", "name": "list-offerings"}),
        ),
        (
            "mcp",
            json!({"type": "streamable-http", "url": "/mcp", "retries": 3}),
        ),
    ] {
        let emptied = normalize(
            "service-document",
            &amend(service_document(), json!({member: [entry]})),
        );
        assert!(emptied.get(member).is_none(), "{member}");
    }
}

/// A payment descriptor that is not an object, or that lists no options, has no options to filter.
#[test]
fn leaves_a_payment_descriptor_it_cannot_read_alone() {
    for payments in [
        json!(["mpp"]),
        json!([{"authentication": "not-required", "name": "mpp"}]),
        json!([{"authentication": "not-required", "name": "mpp", "options": "all"}]),
    ] {
        let document = amend(
            service_document(),
            json!({"protocols": {"payments": payments}}),
        );
        assert_eq!(
            normalize("service-document", &document)["protocols"]["payments"],
            payments
        );
    }
}

/// An options list the Agent can read nothing in is dropped rather than left empty, because an
/// empty list would claim the protocol supports nothing.
#[test]
fn removes_an_options_list_it_emptied() {
    let document = amend(
        service_document(),
        json!({"protocols": {"payments": [
            {"authentication": "not-required", "name": "mpp", "options": ["cheque"]}
        ]}}),
    );
    let normalized = normalize("service-document", &document);

    assert!(
        normalized["protocols"]["payments"][0]
            .get("options")
            .is_none()
    );
}

/// A definition that is not an object is not one this Agent can find fault with either.
#[test]
fn leaves_a_definition_it_cannot_read_alone() {
    let page = support::page(json!(["weight", 3]));
    assert_eq!(
        normalize("filter-page", &page)["items"],
        json!(["weight", 3])
    );
    assert_eq!(normalize("sort-page", &page)["items"], json!(["weight", 3]));
}

/// A problem whose `invalid_params` is not a list is left as it stands.
#[test]
fn leaves_invalid_parameters_that_are_not_a_list_alone() {
    let problem = amend(support::problem(), json!({"invalid_params": "limit"}));
    assert_eq!(normalize("problem", &problem)["invalid_params"], "limit");
}

/// An image entry that is not an object has no members to strip.
#[test]
fn leaves_an_image_that_is_not_an_object_alone() {
    let offering = amend(support::offering(), json!({"images": ["/a.png"]}));
    assert_eq!(
        normalize("offering", &offering)["images"],
        json!(["/a.png"])
    );
}
