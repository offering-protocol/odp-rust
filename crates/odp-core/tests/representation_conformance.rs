//! The Offering and Collection rules that compare one member of a representation against another.

mod support;

use odp_core::{
    ActionRelation, ParseError, PriceType, parse_agent_collection, parse_agent_offering,
    parse_collection, parse_offering,
};
use serde_json::json;
use support::{action, amend, assert_rejected_for, collection, encode, offering};

fn read_offering(changes: serde_json::Value) -> Result<odp_core::Offering, ParseError> {
    parse_offering(&encode(&amend(offering(), changes)))
}

fn read_collection(changes: serde_json::Value) -> Result<odp_core::Collection, ParseError> {
    parse_collection(&encode(&amend(collection(), changes)))
}

#[test]
fn reads_a_conformant_offering() {
    let value = read_offering(json!({
        "description": "A big green plant.",
        "actions": [action("buy")],
        "price": {"type": "fixed", "amount": "39.00", "currency": "USD"}
    }))
    .unwrap();

    assert_eq!(value.id, "plant-1");
    assert_eq!(value.name, "Monstera");
    assert_eq!(value.actions[0].rel, ActionRelation::Purchase);
    assert_eq!(value.price.unwrap().price_type, PriceType::Fixed);
}

#[test]
fn reads_a_conformant_collection() {
    let value = read_collection(json!({"parent_ids": ["garden"]})).unwrap();

    assert_eq!(value.id, "plants");
    assert_eq!(value.parent_ids, ["garden"]);
}

// -- Action identifiers ---------------------------------------------------------------------------

/// OFR-57: an Action identifier is unique within its Offering, so a repeat leaves a caller unable
/// to say which Action it meant.
#[test]
fn refuses_a_repeated_action_identifier() {
    assert_rejected_for(
        read_offering(json!({"actions": [action("buy"), action("buy")]})),
        "unique-action-id",
    );
}

#[test]
fn accepts_actions_that_are_told_apart() {
    assert!(read_offering(json!({"actions": [action("buy"), action("rent")]})).is_ok());
}

// -- price ----------------------------------------------------------------------------------------

/// OFR-49: a range whose minimum is above its maximum describes no price at all.
#[test]
fn refuses_an_inverted_price_range() {
    let range = |minimum, maximum| {
        read_offering(json!({"price": {
            "type": "range", "currency": "USD", "minimum": minimum, "maximum": maximum
        }}))
    };

    assert_rejected_for(range("99.00", "5.00"), "price-range");
    assert_rejected_for(range("10", "9.99"), "price-range");
    assert!(range("5.00", "99.00").is_ok());
}

/// OFR-48: a price is a decimal string, so the bounds are ordered numerically, not lexically.
#[test]
fn orders_a_price_range_numerically() {
    let range = |minimum, maximum| {
        read_offering(json!({"price": {
            "type": "range", "currency": "USD", "minimum": minimum, "maximum": maximum
        }}))
        .is_ok()
    };

    // Lexically "9.00" sorts after "10.00"; numerically it does not.
    assert!(range("9.00", "10.00"));
    // Trailing and leading zeros do not change a value, so these bounds are equal.
    assert!(range("5.0", "5.00"));
    assert!(range("0", "0.000"));
    assert!(range("1.5", "1.50"));
    assert!(!range("1.51", "1.5"));
}

/// A price type that carries no bounds has nothing to order.
#[test]
fn leaves_a_price_without_bounds_alone() {
    assert!(read_offering(json!({"price": {"type": "free"}})).is_ok());
    assert!(read_offering(json!({"price": {"type": "quote"}})).is_ok());
    assert!(
        read_offering(json!({
            "price": {"type": "metered", "amount": "0.10", "currency": "USD", "unit": "litre"}
        }))
        .is_ok()
    );
}

// -- hierarchy -------------------------------------------------------------------------------------

/// COL-20: a Collection naming itself as a parent is a one-node cycle, and nothing walking the
/// hierarchy upwards from it would reach a root.
#[test]
fn refuses_a_collection_that_parents_itself() {
    assert_rejected_for(
        read_collection(json!({"parent_ids": ["plants"]})),
        "self-parent",
    );
    assert_rejected_for(
        read_collection(json!({"parent_ids": ["garden", "plants"]})),
        "self-parent",
    );
}

/// COL-19: a Collection names each parent once.
#[test]
fn refuses_a_repeated_parent() {
    assert!(read_collection(json!({"parent_ids": ["garden", "garden"]})).is_err());
}

// -- language and images ----------------------------------------------------------------------------

/// REP-20: a representation that says which language it is in says it with a language tag.
#[test]
fn refuses_a_representation_language_that_is_not_a_tag() {
    assert_rejected_for(
        read_offering(json!({"language": "not a tag"})),
        "language-tag",
    );
    assert_rejected_for(
        read_collection(json!({"language": "not a tag"})),
        "language-tag",
    );
}

/// REP-21: a representation that lists its localizations lists the one it is written in.
#[test]
fn refuses_localizations_that_omit_the_representation_language() {
    assert_rejected_for(
        read_offering(json!({"language": "en", "localizations": ["fr"]})),
        "contains-language",
    );
}

/// A representation naming no language of its own has no membership to check.
#[test]
fn leaves_localizations_alone_when_no_language_was_named() {
    assert!(read_offering(json!({"localizations": ["fr", "de"]})).is_ok());
}

/// A language with no localizations beside it is not a contradiction either.
#[test]
fn accepts_a_language_without_localizations() {
    assert!(read_offering(json!({"language": "fr"})).is_ok());
}

/// REP-25: one image listed twice is one image, so a repeat is a defect rather than two pictures.
#[test]
fn refuses_a_repeated_image_source() {
    let image = |src| json!({"src": src, "type": "image/png"});

    assert_rejected_for(
        read_offering(json!({"images": [image("/a.png"), image("/a.png")]})),
        "unique-image-source",
    );
    assert!(read_offering(json!({"images": [image("/a.png"), image("/b.png")]})).is_ok());
}

// -- what an Agent tolerates -------------------------------------------------------------------------

/// ROLE-03: an Agent describes a defect to its caller rather than discarding an Offering it can
/// still use, so the invariants a Service must satisfy do not refuse the document on that side.
#[test]
fn hands_an_agent_an_offering_a_service_must_not_publish() {
    let data = encode(&amend(
        offering(),
        json!({"actions": [action("buy"), action("buy")]}),
    ));

    assert!(parse_offering(&data).is_err());
    let tolerated = parse_agent_offering(&data).unwrap();
    assert_eq!(tolerated.actions.len(), 2, "the repeat is left to report");
}

#[test]
fn hands_an_agent_a_collection_a_service_must_not_publish() {
    let data = encode(&amend(collection(), json!({"parent_ids": ["plants"]})));

    assert!(parse_collection(&data).is_err());
    assert_eq!(
        parse_agent_collection(&data).unwrap().parent_ids,
        ["plants"]
    );
}

/// Tolerance stops at defects that leave nothing to use: a representation an Agent cannot read at
/// all is still refused.
#[test]
fn refuses_an_agent_document_that_is_not_readable() {
    assert!(parse_agent_offering(b"not json").is_err());
    assert!(parse_agent_collection(b"{}").is_err());
    assert!(
        parse_agent_offering(&encode(&amend(
            offering(),
            json!({"language": "not a tag"})
        )))
        .is_err()
    );
}

/// The Agent entry points normalize first, so a member ODP does not define never reaches the
/// schema.
#[test]
fn filters_what_a_later_odp_version_added_before_reading_it() {
    let data = encode(&amend(
        offering(),
        json!({"price": {"type": "auction", "reserve": "10.00"}}),
    ));

    assert!(parse_offering(&data).is_err());
    assert!(parse_agent_offering(&data).unwrap().price.is_none());
}

// -- Action relations --------------------------------------------------------------------------------

/// OFR-55: an Agent reads a relation ODP does not define rather than refusing the Action, because
/// the relation is a hint about the Action, not the Action itself.
#[test]
fn reads_a_relation_a_later_odp_version_may_define() {
    let other = amend(action("swap"), json!({"rel": "barter"}));
    let value = read_offering(json!({"actions": [other]})).unwrap();

    assert_eq!(
        value.actions[0].rel,
        ActionRelation::Other("barter".to_owned())
    );
}

#[test]
fn round_trips_every_relation_odp_defines() {
    for (wire, relation) in [
        ("download", ActionRelation::Download),
        ("invoke", ActionRelation::Invoke),
        ("purchase", ActionRelation::Purchase),
        ("quote", ActionRelation::Quote),
        ("reserve", ActionRelation::Reserve),
        ("barter", ActionRelation::Other("barter".to_owned())),
    ] {
        let encoded = format!("\"{wire}\"");
        assert_eq!(
            serde_json::from_str::<ActionRelation>(&encoded).unwrap(),
            relation
        );
        assert_eq!(serde_json::to_string(&relation).unwrap(), encoded);
    }
}
