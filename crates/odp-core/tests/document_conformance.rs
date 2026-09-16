//! The Service Document rules a JSON Schema cannot state, and the schema boundaries around them.

mod support;

use odp_core::{
    Operation, ParseError, Representation, VERSION, parse_resource_identity,
    parse_service_document, validate_value,
};
use serde_json::json;
use support::{amend, assert_rejected_for, encode, service_document};

fn parse(changes: serde_json::Value) -> Result<odp_core::ServiceDocument, ParseError> {
    parse_service_document(&encode(&amend(service_document(), changes)))
}

#[test]
fn reads_a_conformant_service_document() {
    let document = parse(json!({})).unwrap();

    assert_eq!(document.odp_version, VERSION);
    assert_eq!(document.name, "Plants");
    assert_eq!(document.language, "en");
    assert_eq!(document.http.endpoint_base, "/odp");
    assert_eq!(document.operations.len(), 2);
    assert_eq!(document.operations[0].name, Operation::ListOfferings);
}

/// SVC-11: a Service Document describes a Service, not a resource, so it carries no identifier.
#[test]
fn refuses_an_identifier() {
    assert_rejected_for(parse(json!({"id": "plants"})), "prohibited");
}

/// SVC-11: nor a `web_url`, which belongs to a Collection or an Offering.
#[test]
fn refuses_a_resource_web_url() {
    assert_rejected_for(
        parse(json!({"web_url": "https://plants.example"})),
        "prohibited",
    );
}

/// Both prohibited members are reported together rather than one per round trip.
#[test]
fn reports_every_prohibited_member_at_once() {
    let issues = support::issues(parse(json!({"id": "x", "web_url": "https://a.example"})));
    assert_eq!(issues.len(), 2);
    assert_eq!(issues[0].path, "/id");
    assert_eq!(issues[1].path, "/web_url");
}

// -- language and localizations -----------------------------------------------------------------

/// SVC-55: the default language is a language tag.
#[test]
fn refuses_a_default_language_that_is_not_a_tag() {
    assert_rejected_for(
        parse(json!({"language": "not a tag", "localizations": ["not a tag"]})),
        "language-tag",
    );
}

/// A tag that repeats a variant subtag is not well-formed, whatever its length.
#[test]
fn refuses_a_tag_that_repeats_a_subtag() {
    for tag in ["de-1901-1901", "en-a-bbb-a-ccc"] {
        assert_rejected_for(
            parse(json!({"language": tag, "localizations": [tag]})),
            "language-tag",
        );
    }
}

/// SVC-56: the default language is one of the localizations the Service advertises.
#[test]
fn refuses_localizations_that_omit_the_default_language() {
    assert_rejected_for(
        parse(json!({"language": "en", "localizations": ["fr", "de"]})),
        "contains-default-language",
    );
}

/// Language tags are case-insensitive, so the default language may be spelled either way.
#[test]
fn matches_the_default_language_without_regard_to_case() {
    assert!(parse(json!({"language": "en-GB", "localizations": ["EN-gb"]})).is_ok());
}

/// Two spellings of one tag advertise one localization twice.
#[test]
fn refuses_localizations_that_repeat_a_tag() {
    assert_rejected_for(
        parse(json!({"language": "en", "localizations": ["en", "EN"]})),
        "unique-language-tag",
    );
}

/// A malformed tag in the list is reported on its own; the membership rule needs a readable list.
#[test]
fn reports_a_malformed_localization_before_membership() {
    let issues = support::issues(parse(json!({"localizations": ["not a tag"]})));
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].keyword, "language-tag");
    assert_eq!(issues[0].path, "/localizations");
}

// -- keywords -----------------------------------------------------------------------------------

/// SVC-21: keywords are budgeted together, so a Service cannot spend the budget on one long list.
#[test]
fn refuses_keywords_beyond_the_shared_budget() {
    // 32 distinct keywords of 64 code points each is 2048, twice what the Service may spend.
    let keywords: Vec<String> = (0..32).map(|index| format!("{index:0>64}")).collect();
    assert_rejected_for(parse(json!({"keywords": keywords})), "max-code-points");
}

/// The budget counts code points, not bytes, so a multi-byte keyword is not charged twice.
#[test]
fn counts_keyword_code_points_rather_than_bytes() {
    // 16 distinct keywords of 64 code points each is the whole budget, and four times as many
    // bytes.
    let keywords: Vec<String> = (b'a'..b'q')
        .map(|letter| format!("{}{}", char::from(letter), "\u{1f33f}".repeat(63)))
        .collect();
    assert!(parse(json!({"keywords": keywords})).is_ok());
}

// -- search capabilities ------------------------------------------------------------------------

/// SVC-70: advertised search capabilities describe an operation the Service actually offers.
#[test]
fn refuses_search_capabilities_without_the_search_operation() {
    assert_rejected_for(
        parse(json!({
            "search_capabilities": {"filters": {"inline": [support::filter_definition("string")]}}
        })),
        "operation-support",
    );
}

#[test]
fn accepts_search_capabilities_beside_the_search_operation() {
    assert!(
        parse(json!({
            "operations": [
                {"authentication": "not-required", "name": "list-offerings"},
                {"authentication": "not-required", "name": "get-offering"},
                {"authentication": "not-required", "name": "search-offerings"}
            ],
            "search_capabilities": {"filters": {"inline": [support::filter_definition("string")]}}
        }))
        .is_ok()
    );
}

// -- schema boundaries ---------------------------------------------------------------------------

/// A Service Document that is not JSON at all is refused before any rule is applied.
#[test]
fn refuses_a_body_that_is_not_json() {
    let issues = support::issues(parse_service_document(b"not json"));
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].keyword, "json");
}

/// SVC-04: the version member names the version of ODP this document speaks.
#[test]
fn refuses_another_protocol_version() {
    assert_rejected_for(parse(json!({"odp_version": "2.0"})), "const");
}

/// SVC-64: the endpoint base is an origin-relative absolute path, so it carries no query.
#[test]
fn refuses_an_endpoint_base_that_is_not_a_path() {
    for base in ["odp", "//elsewhere.example/odp", "/odp?x=1", "/odp#a"] {
        assert!(
            parse(json!({"http": {"endpoint_base": base}})).is_err(),
            "{base}"
        );
    }
}

/// SVC-45: a Service advertises each payment protocol once.
#[test]
fn refuses_a_repeated_payment_protocol() {
    assert!(
        parse(json!({"protocols": {"payments": [
            {"name": "mpp", "options": ["card"]},
            {"name": "mpp", "options": ["solana"]}
        ]}}))
        .is_err()
    );
}

/// SVC-30: an MCP endpoint descriptor names a transport ODP defines.
#[test]
fn refuses_an_unknown_mcp_transport() {
    assert!(parse(json!({"mcp": [{"type": "sse", "url": "https://plants.example/mcp"}]})).is_err());
    assert!(
        parse(json!({
            "mcp": [{"type": "streamable-http", "url": "https://plants.example/mcp"}]
        }))
        .is_ok()
    );
}

// -- resource identity ---------------------------------------------------------------------------

/// A Resource Identity names the Service, the kind of resource, and the resource.
#[test]
fn reads_a_resource_identity() {
    let identity = parse_resource_identity(
        br#"{"service":"https://plants.example","type":"offering","id":"plant-1"}"#,
    )
    .unwrap();

    assert_eq!(identity.id, "plant-1");
    assert_eq!(identity.service, "https://plants.example");
}

#[test]
fn refuses_a_resource_identity_of_an_unknown_kind() {
    assert!(
        parse_resource_identity(
            br#"{"service":"https://plants.example","type":"basket","id":"plant-1"}"#
        )
        .is_err()
    );
}

// -- the shared validator -------------------------------------------------------------------------

/// `validate_value` is the same check without the decode step, for a document already in hand.
#[test]
fn validates_a_value_against_a_bundled_schema() {
    let document = service_document();
    assert!(
        validate_value(
            &document,
            "service-document.schema.json",
            "Service Document"
        )
        .is_ok()
    );

    let broken = amend(document, json!({"odp_version": "0.9"}));
    let issues = support::issues(validate_value(
        &broken,
        "service-document.schema.json",
        "Service Document",
    ));
    assert_eq!(issues[0].path, "/odp_version");
}

/// Asking for a schema that was never bundled is a programming error, not a document defect.
#[test]
fn reports_a_schema_that_was_never_bundled() {
    let error = validate_value(&json!({}), "basket.schema.json", "Basket").unwrap_err();
    assert!(matches!(error, ParseError::SchemaInitialization(_)));
    assert!(error.to_string().contains("basket.schema.json"));
}

/// The representation enumeration is the one the query parameter accepts.
#[test]
fn round_trips_the_representation_enumeration() {
    assert_eq!(
        serde_json::from_str::<Representation>("\"full\"").unwrap(),
        Representation::Full
    );
    assert_eq!(
        serde_json::to_string(&Representation::Terse).unwrap(),
        "\"terse\""
    );
}
