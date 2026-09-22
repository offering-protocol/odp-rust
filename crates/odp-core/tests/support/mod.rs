//! Fixtures every odp-core conformance suite builds on.
//!
//! Each builder returns a document the spec accepts, so a test says only what it changes and a
//! rejection can be read as a consequence of that one change.

#![allow(dead_code)]

use odp_core::{ParseError, ValidationIssue};
use serde_json::{Value, json};

/// Merges `changes` into `base`, where a null member removes it. This is what lets a test spell
/// out only its own departure from a conformant document.
#[must_use]
pub fn amend(mut base: Value, changes: Value) -> Value {
    let target = base.as_object_mut().expect("fixture is an object");
    for (key, value) in changes.as_object().expect("changes are an object") {
        if value.is_null() {
            target.remove(key);
        } else {
            target.insert(key.clone(), value.clone());
        }
    }
    base
}

#[must_use]
pub fn encode(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("fixture encodes")
}

/// A conformant Service Document.
#[must_use]
pub fn service_document() -> Value {
    json!({
        "odp_version": "1.0",
        "name": "Plants",
        "description": "A shop that sells plants.",
        "language": "en",
        "localizations": ["en"],
        "operations": [
            {"authentication": "not-required", "name": "list-offerings"},
            {"authentication": "not-required", "name": "get-offering"}
        ],
        "http": {"endpoint_base": "/odp"}
    })
}

/// A conformant Offering.
#[must_use]
pub fn offering() -> Value {
    json!({"odp_version": "1.0", "id": "plant-1", "name": "Monstera"})
}

/// A conformant Collection.
#[must_use]
pub fn collection() -> Value {
    json!({"odp_version": "1.0", "id": "plants", "name": "Plants"})
}

/// A conformant Action, which needs an identifier of its own to sit beside another.
#[must_use]
pub fn action(id: &str) -> Value {
    json!({
        "id": id,
        "rel": "purchase",
        "authentication": "not-required",
        "http": {"href": "https://plants.example/checkout", "method": "POST"}
    })
}

/// A conformant Filter Definition of the named type.
#[must_use]
pub fn filter_definition(filter_type: &str) -> Value {
    json!({
        "id": "weight",
        "title": "Weight",
        "description": "How heavy the plant is.",
        "type": filter_type,
        "operators": ["eq"]
    })
}

/// A conformant Sort Definition.
#[must_use]
pub fn sort_definition() -> Value {
    json!({
        "id": "cheapest",
        "title": "Cheapest first",
        "description": "Orders plants by price.",
        "keys": [{"filter_id": "price", "direction": "ascending", "missing": "last"}]
    })
}

/// A conformant page envelope carrying `items`.
#[must_use]
pub fn page(items: Value) -> Value {
    json!({"odp_version": "1.0", "items": items})
}

/// A conformant Problem Details document.
#[must_use]
pub fn problem() -> Value {
    json!({
        "type": "https://offeringprotocol.org/problems/invalid-request",
        "title": "Invalid request",
        "status": 400,
        "code": "INVALID_REQUEST"
    })
}

/// The issues a rejection reported, or a panic naming what was accepted instead.
#[must_use]
pub fn issues<T>(outcome: Result<T, ParseError>) -> Vec<ValidationIssue> {
    match outcome {
        Ok(_) => panic!("the document was accepted"),
        Err(ParseError::Validation(error)) => error.issues,
        Err(error) => panic!("{error}"),
    }
}

/// Asserts the rejection names this rule, so a test cannot pass on an unrelated defect.
pub fn assert_rejected_for<T>(outcome: Result<T, ParseError>, keyword: &str) {
    let issues = issues(outcome);
    assert!(
        issues.iter().any(|issue| issue.keyword == keyword),
        "expected {keyword}, got {:?}",
        issues
            .iter()
            .map(|issue| format!("{} at {}", issue.keyword, issue.path))
            .collect::<Vec<_>>()
    );
}
