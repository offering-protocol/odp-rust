//! PRB-*: the Problem Details a Service answers a refused request with, and what a Service that
//! serializes a document has to put on the wire.

mod support;

use odp_core::{
    Collection, CollectionSearchRequest, Offering, Operation, OperationDescriptor, Page,
    ResourceImage, parse_problem_details, parse_problem_response,
};
use serde_json::{Value, json};
use support::{amend, assert_rejected_for, encode, problem};

fn read(changes: Value) -> Result<odp_core::ProblemDetails, odp_core::ParseError> {
    parse_problem_details(&encode(&amend(problem(), changes)))
}

#[test]
fn reads_a_conformant_problem() {
    let value = read(json!({
        "detail": "The limit was above 100.",
        "instance": "/odp/offerings",
        "invalid_params": [{"in": "query", "name": "limit", "reason": "too large"}]
    }))
    .unwrap();

    assert_eq!(value.code, "INVALID_REQUEST");
    assert_eq!(value.status, 400);
    assert_eq!(value.invalid_params[0].name, "limit");
}

/// PRB-04: the type URI and the code name one problem, so a document whose two disagree tells the
/// Agent two different things.
#[test]
fn refuses_a_type_that_does_not_match_the_code() {
    assert_rejected_for(
        read(json!({"type": "https://offeringprotocol.org/problems/not-found"})),
        "problem-type",
    );
}

/// The code is spelled in capitals with underscores and the URI in lower case with hyphens, and one
/// is derived from the other.
#[test]
fn derives_the_type_from_the_code() {
    for (code, problem_type) in [
        (
            "NOT_FOUND",
            "https://offeringprotocol.org/problems/not-found",
        ),
        (
            "UNSUPPORTED_OPERATION",
            "https://offeringprotocol.org/problems/unsupported-operation",
        ),
        (
            "STALE_CONTINUATION",
            "https://offeringprotocol.org/problems/stale-continuation",
        ),
    ] {
        assert!(
            read(json!({"code": code, "type": problem_type})).is_ok(),
            "{code}"
        );
    }
}

/// PRB-11: the status inside the document is the status the response carried, so an Agent reading
/// one need not reconcile two answers.
#[test]
fn refuses_a_status_that_contradicts_the_response() {
    let data = encode(&problem());

    assert_eq!(parse_problem_response(&data, 400).unwrap().status, 400);
    assert_rejected_for(parse_problem_response(&data, 404), "http-status");
}

/// A body that is not a Problem Details document at all is reported as such rather than as a
/// mismatched status.
#[test]
fn refuses_a_response_body_that_is_not_a_problem() {
    assert!(parse_problem_response(b"not json", 400).is_err());
    assert!(parse_problem_response(br#"{"title":"Oops"}"#, 400).is_err());
}

/// PRB-13: a code is an upper-case identifier, and a status is an HTTP status.
#[test]
fn refuses_a_code_or_status_that_is_neither() {
    for changes in [
        json!({"code": "invalid_request"}),
        json!({"code": "1_INVALID"}),
        json!({"status": 42}),
        json!({"status": "400"}),
    ] {
        assert!(read(changes.clone()).is_err(), "{changes}");
    }
}

/// PRB-14: an invalid parameter says where it was found, what it was called, and what was wrong.
#[test]
fn refuses_an_invalid_parameter_that_points_nowhere() {
    for parameter in [
        json!({"in": "query", "name": "limit"}),
        json!({"in": "query", "reason": "too large"}),
        json!({"name": "limit", "reason": "too large"}),
        json!({"in": "cookie", "name": "session", "reason": "expired"}),
        // Outside a body the name is a parameter name, so it cannot be empty.
        json!({"in": "query", "name": "", "reason": "too large"}),
    ] {
        assert!(
            read(json!({"invalid_params": [parameter.clone()]})).is_err(),
            "{parameter}"
        );
    }
}

/// In a body the name is a JSON Pointer, and the pointer to the whole body is the empty string.
#[test]
fn reads_a_body_parameter_named_by_pointer() {
    for name in ["", "/filters/0/value", "/a~0b", "/a~1b"] {
        assert!(
            read(json!({"invalid_params": [
                {"in": "body", "name": name, "reason": "wrong"}
            ]}))
            .is_ok(),
            "{name:?}"
        );
    }
    assert!(
        read(json!({"invalid_params": [
            {"in": "body", "name": "filters", "reason": "wrong"}
        ]}))
        .is_err()
    );
}

// -- serialization -----------------------------------------------------------------------------------

/// A Service builds these documents rather than reading them, so what the models leave off the wire
/// matters as much as what they read from it: a member that was absent stays absent, rather than
/// being written back as the default that stood in for it.
#[test]
fn leaves_an_absent_member_off_the_wire() {
    for document in [
        json!({"odp_version": "1.0", "id": "plant-1", "name": "Monstera"}),
        json!({"odp_version": "1.0", "id": "plant-1", "name": "Monstera", "images": [{"src": "/a.png"}]}),
    ] {
        let read: Offering = serde_json::from_value(document.clone()).unwrap();
        assert_eq!(serde_json::to_value(&read).unwrap(), document);
    }

    let collection = json!({"odp_version": "1.0", "id": "plants", "name": "Plants"});
    let read: Collection = serde_json::from_value(collection.clone()).unwrap();
    assert_eq!(serde_json::to_value(&read).unwrap(), collection);
}

/// A page says which ODP it speaks and what it carries, and says nothing about a continuation it
/// does not offer.
#[test]
fn writes_a_page_that_offers_no_continuation() {
    let document = json!({"odp_version": "1.0", "items": []});
    let read: Page<Collection> = serde_json::from_value(document.clone()).unwrap();
    let encoded = serde_json::to_value(&read).unwrap();

    assert_eq!(encoded, document);
    assert!(encoded.get("next").is_none());
    assert!(encoded.get("auth_expands").is_none());
}

/// COL-06: a null parent survives the round trip as a null, because it asks a question an absent
/// member does not, while an absent one is written back absent.
#[test]
fn keeps_a_null_parent_on_the_wire() {
    for document in [
        json!({"odp_version": "1.0", "parent_id": null}),
        json!({"odp_version": "1.0", "parent_id": "garden"}),
        json!({"odp_version": "1.0", "query": "plants"}),
    ] {
        let read: CollectionSearchRequest = serde_json::from_value(document.clone()).unwrap();
        assert_eq!(serde_json::to_value(&read).unwrap(), document);
    }
}

/// An Operation Descriptor is written with the wire spelling ODP fixes, not the Rust one.
#[test]
fn writes_an_operation_with_its_wire_spelling() {
    let descriptor = OperationDescriptor {
        authentication: odp_core::AuthenticationRequirement::NotRequired,
        name: Operation::ListCollectionOfferings,
    };

    assert_eq!(
        serde_json::to_value(&descriptor).unwrap(),
        json!({"authentication": "not-required", "name": "list-collection-offerings"})
    );
}

/// A dimension nobody measured is not a dimension of zero, so it is left off.
#[test]
fn leaves_an_unmeasured_image_dimension_off_the_wire() {
    let document = json!({"src": "/a.png", "type": "image/png"});
    let read: ResourceImage = serde_json::from_value(document.clone()).unwrap();

    assert_eq!(serde_json::to_value(&read).unwrap(), document);

    let measured = json!({"src": "/a.png", "width": 800, "height": 600});
    let read: ResourceImage = serde_json::from_value(measured.clone()).unwrap();
    assert_eq!(serde_json::to_value(&read).unwrap(), measured);
}
