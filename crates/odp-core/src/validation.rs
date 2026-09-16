use std::{borrow::Cow, collections::BTreeMap, sync::OnceLock};

use include_dir::{Dir, include_dir};
use jsonschema::{Registry, Validator};
use language_tags::LanguageTag;
use serde::de::DeserializeOwned;
use serde_json::Value;
use thiserror::Error;

use crate::{
    Collection, CollectionSearchRequest, FilterDefinition, FilterOperator, FilterType, Offering,
    OfferingPage, OfferingSearchRequest, Operation, Page, ProblemDetails, RefinementGroup,
    ResourceIdentity, ServiceDocument, SortDefinition,
};

static SCHEMA_FILES: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/schemas");
static SCHEMAS: OnceLock<Result<SchemaSet, String>> = OnceLock::new();

struct SchemaSet {
    validators: BTreeMap<String, Validator>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ValidationIssue {
    pub keyword: String,
    pub message: String,
    pub params: BTreeMap<String, Value>,
    pub path: String,
}

#[derive(Clone, Debug, Error, PartialEq)]
#[error("invalid ODP {document_type}")]
pub struct ValidationError {
    pub document_type: String,
    pub issues: Vec<ValidationIssue>,
}

#[derive(Debug, Error)]
pub enum ParseError {
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error("failed to initialize ODP schemas: {0}")]
    SchemaInitialization(String),
}

pub fn parse_service_document(data: &[u8]) -> Result<ServiceDocument, ParseError> {
    parse(
        data,
        "service-document.schema.json",
        "Service Document",
        service_document_issues,
    )
}

pub fn parse_agent_service_document(data: &[u8]) -> Result<ServiceDocument, ParseError> {
    let encoded = normalize_agent_response(data, "service-document")?;
    parse_service_document(&encoded)
}

pub fn normalize_agent_response(data: &[u8], kind: &str) -> Result<Vec<u8>, ParseError> {
    let mut raw = serde_json::from_slice::<Value>(data).map_err(|error| ValidationError {
        document_type: "Agent response".to_owned(),
        issues: vec![issue("", "json", &error.to_string())],
    })?;
    validate_json_depth(&raw, if kind == "service-document" { 8 } else { 16 })?;
    normalize_agent_document(&mut raw, kind);
    // Re-encoding cannot fail, so it reports no error to handle: a `Value` holds only what JSON
    // can spell, and its `Display` writes that spelling directly rather than through a `Result`.
    Ok(raw.to_string().into_bytes())
}

fn normalize_agent_document(document: &mut Value, kind: &str) {
    match kind {
        "service-document" => {
            filter_agent_protocols(document);
            if let Some(protocols) = document.get_mut("protocols") {
                filter_unknown_authentication(protocols, "payments");
            }
            filter_list(
                document,
                "operations",
                "name",
                &[
                    "get-collection",
                    "get-offering",
                    "list-collection-offerings",
                    "list-collections",
                    "list-offerings",
                    "search-collections",
                    "search-offerings",
                ],
            );
            filter_unknown_authentication(document, "operations");
            filter_list(document, "mcp", "type", &["streamable-http"]);
            filter_closed_object_list(document, "operations", &["authentication", "name"]);
            filter_closed_object_list(document, "mcp", &["description", "name", "type", "url"]);
            filter_payment_options(document);
            normalize_branding(document);
            normalize_search_capabilities(document);
        }
        "collection" | "offering" => {
            filter_list(
                document,
                "images",
                "type",
                &[
                    "image/avif",
                    "image/jpeg",
                    "image/png",
                    "image/svg+xml",
                    "image/webp",
                ],
            );
            strip_object_list(
                document,
                "images",
                &["alt", "height", "src", "type", "width"],
            );
            normalize_search_capabilities(document);
            if kind == "offering" {
                normalize_offering(document);
            }
        }
        "collection-page" | "offering-page" => {
            let item_kind = if kind == "offering-page" {
                "offering"
            } else {
                "collection"
            };
            if let Some(items) = document
                .as_object_mut()
                .and_then(|value| value.get_mut("items"))
                .and_then(Value::as_array_mut)
            {
                for item in items {
                    normalize_agent_document(item, item_kind);
                }
            }
        }
        "filter-page" => filter_definitions(document, known_filter),
        "sort-page" => filter_definitions(document, known_sort),
        "problem" => filter_problem_parameters(document),
        _ => {}
    }
}

fn filter_list(document: &mut Value, member: &str, discriminator: &str, recognized: &[&str]) {
    let Some(object) = document.as_object_mut() else {
        return;
    };
    let Some(Value::Array(items)) = object.get_mut(member) else {
        return;
    };
    items.retain(|item| {
        item.as_object()
            .and_then(|value| value.get(discriminator))
            .and_then(Value::as_str)
            .is_none_or(|value| recognized.contains(&value))
    });
    if items.is_empty() {
        object.remove(member);
    }
}

fn filter_closed_object_list(document: &mut Value, member: &str, allowed: &[&str]) {
    let Some(object) = document.as_object_mut() else {
        return;
    };
    let Some(Value::Array(items)) = object.get_mut(member) else {
        return;
    };
    items.retain(|item| {
        item.as_object()
            .is_none_or(|value| value.keys().all(|key| allowed.contains(&key.as_str())))
    });
    if items.is_empty() {
        object.remove(member);
    }
}

fn filter_unknown_authentication(document: &mut Value, member: &str) {
    let Some(object) = document.as_object_mut() else {
        return;
    };
    let Some(Value::Array(items)) = object.get_mut(member) else {
        return;
    };
    items.retain(|item| !has_unknown_authentication(item));
    if items.is_empty() {
        object.remove(member);
    }
}

fn has_unknown_authentication(value: &Value) -> bool {
    value
        .get("authentication")
        .and_then(Value::as_str)
        .is_some_and(|authentication| {
            !["not-required", "optional", "required"].contains(&authentication)
        })
}

fn strip_object_list(document: &mut Value, member: &str, allowed: &[&str]) {
    let Some(items) = document.get_mut(member).and_then(Value::as_array_mut) else {
        return;
    };
    for item in items {
        if let Some(object) = item.as_object_mut() {
            object.retain(|key, _value| allowed.contains(&key.as_str()));
        }
    }
}

fn filter_payment_options(document: &mut Value) {
    let Some(payments) = document
        .pointer_mut("/protocols/payments")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    let recognized = [
        "algorand",
        "aptos",
        "arbitrum",
        "avalanche",
        "base",
        "card",
        "ethereum",
        "hedera",
        "inflow",
        "lightning",
        "polygon",
        "solana",
        "stellar",
        "stripe",
        "tempo",
        "ton",
    ];
    for payment in payments {
        let Some(object) = payment.as_object_mut() else {
            continue;
        };
        let Some(Value::Array(options)) = object.get_mut("options") else {
            continue;
        };
        options.retain(|option| {
            option
                .as_str()
                .is_none_or(|value| recognized.contains(&value))
        });
        if options.is_empty() {
            object.remove("options");
        }
    }
}

fn normalize_branding(document: &mut Value) {
    let Some(object) = document.as_object_mut() else {
        return;
    };
    let Some(Value::Object(branding)) = object.get_mut("branding") else {
        return;
    };
    branding.retain(|key, _value| matches!(key.as_str(), "icon" | "logo"));
    let recognized = ["image/png", "image/svg+xml", "image/webp"];
    for member in ["icon", "logo"] {
        let unknown = branding
            .get(member)
            .and_then(Value::as_object)
            .and_then(|image| image.get("type"))
            .and_then(Value::as_str)
            .is_some_and(|image_type| !recognized.contains(&image_type));
        if unknown {
            object.remove("branding");
            return;
        } else if let Some(Value::Object(image)) = branding.get_mut(member) {
            image.retain(|key, _value| matches!(key.as_str(), "src" | "type"));
        }
    }
    if branding.is_empty() {
        object.remove("branding");
    }
}

fn normalize_search_capabilities(document: &mut Value) {
    let Some(object) = document.as_object_mut() else {
        return;
    };
    let Some(Value::Object(capabilities)) = object.get_mut("search_capabilities") else {
        return;
    };
    filter_inline_definitions(capabilities, "filters", known_filter);
    filter_inline_definitions(capabilities, "sorts", known_sort);
    if capabilities.is_empty() {
        object.remove("search_capabilities");
    }
}

fn filter_inline_definitions(
    capabilities: &mut serde_json::Map<String, Value>,
    member: &str,
    recognized: fn(&Value) -> bool,
) {
    let Some(Value::Array(items)) = capabilities
        .get_mut(member)
        .and_then(Value::as_object_mut)
        .and_then(|source| source.get_mut("inline"))
    else {
        return;
    };
    items.retain(recognized);
    if items.is_empty() {
        capabilities.remove(member);
    }
}

fn normalize_offering(document: &mut Value) {
    let Some(object) = document.as_object_mut() else {
        return;
    };
    if object
        .get("schema")
        .and_then(Value::as_object)
        .is_some_and(|schema| schema.keys().any(|key| key != "url"))
    {
        object.remove("schema");
        object.remove("attributes");
    }
    let known_prices = ["fixed", "free", "metered", "quote", "range", "starting_at"];
    let unknown_price = object
        .get("price")
        .and_then(Value::as_object)
        .and_then(|price| price.get("type"))
        .and_then(Value::as_str)
        .is_some_and(|price_type| !known_prices.contains(&price_type));
    if unknown_price {
        object.remove("price");
    }
    let Some(Value::Array(actions)) = object.get_mut("actions") else {
        return;
    };
    actions.retain(|action| {
        if has_unknown_authentication(action) {
            return false;
        }
        if action.as_object().is_some_and(|value| {
            value.keys().any(|key| {
                ![
                    "authentication",
                    "description",
                    "http",
                    "id",
                    "openapi",
                    "rel",
                ]
                .contains(&key.as_str())
            })
        }) {
            return false;
        }
        if action
            .pointer("/http")
            .and_then(Value::as_object)
            .is_some_and(|value| {
                value.keys().any(|key| {
                    !["href", "method", "request", "response_content_types"].contains(&key.as_str())
                })
            })
        {
            return false;
        }
        if action
            .pointer("/http/request")
            .and_then(Value::as_object)
            .is_some_and(|value| {
                value
                    .keys()
                    .any(|key| !["content_type", "schema"].contains(&key.as_str()))
            })
        {
            return false;
        }
        if action
            .pointer("/http/request/schema")
            .and_then(Value::as_object)
            .is_some_and(|value| value.keys().any(|key| key != "url"))
        {
            return false;
        }
        if action
            .get("openapi")
            .and_then(Value::as_object)
            .is_some_and(|value| {
                value
                    .keys()
                    .any(|key| !["operation_id", "url"].contains(&key.as_str()))
            })
        {
            return false;
        }
        action
            .pointer("/http/method")
            .and_then(Value::as_str)
            .is_none_or(|method| method == "GET" || method == "POST")
    });
    if actions.is_empty() {
        object.remove("actions");
    }
}

fn filter_definitions(document: &mut Value, recognized: fn(&Value) -> bool) {
    if let Some(items) = document
        .as_object_mut()
        .and_then(|value| value.get_mut("items"))
        .and_then(Value::as_array_mut)
    {
        items.retain(recognized);
    }
}

fn known_filter(definition: &Value) -> bool {
    let Some(object) = definition.as_object() else {
        return true;
    };
    let types = [
        "boolean",
        "date",
        "date-time",
        "decimal",
        "integer",
        "number",
        "string",
    ];
    if object
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|value| !types.contains(&value))
    {
        return false;
    }
    let operators = ["eq", "exists", "gt", "gte", "in", "lt", "lte"];
    if object
        .get("operators")
        .and_then(Value::as_array)
        .is_some_and(|values| {
            values.iter().any(|value| {
                value
                    .as_str()
                    .is_some_and(|operator| !operators.contains(&operator))
            })
        })
    {
        return false;
    }
    !definition
        .pointer("/unit/system")
        .and_then(Value::as_str)
        .is_some_and(|system| system != "service" && system != "ucum")
}

fn known_sort(definition: &Value) -> bool {
    !definition
        .get("keys")
        .and_then(Value::as_array)
        .is_some_and(|keys| {
            keys.iter().any(|key| {
                key.get("direction")
                    .and_then(Value::as_str)
                    .is_some_and(|value| value != "ascending" && value != "descending")
                    || key
                        .get("missing")
                        .and_then(Value::as_str)
                        .is_some_and(|value| value != "first" && value != "last")
            })
        })
}

fn filter_problem_parameters(document: &mut Value) {
    let Some(parameters) = document
        .get_mut("invalid_params")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    let recognized = ["body", "header", "path", "query"];
    parameters.retain(|parameter| {
        parameter
            .get("in")
            .and_then(Value::as_str)
            .is_none_or(|location| recognized.contains(&location))
    });
}

fn filter_agent_protocols(document: &mut Value) {
    let Some(protocols) = document
        .as_object_mut()
        .and_then(|document| document.get_mut("protocols"))
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    filter_agent_protocol_category(protocols, "enrollment", &["aep"]);
    filter_agent_protocol_category(protocols, "payments", &["mpp", "x402"]);
    filter_agent_protocol_category(protocols, "trust", &["tap"]);
    let remove_protocols = protocols.is_empty();
    if remove_protocols {
        if let Some(document) = document.as_object_mut() {
            document.remove("protocols");
        }
    }
}

fn filter_agent_protocol_category(
    protocols: &mut serde_json::Map<String, Value>,
    category: &str,
    recognized: &[&str],
) {
    let Some(Value::Array(descriptors)) = protocols.get_mut(category) else {
        return;
    };
    let original_length = descriptors.len();
    descriptors.retain(|descriptor| {
        descriptor
            .as_object()
            .and_then(|value| value.get("name"))
            .and_then(Value::as_str)
            .is_none_or(|name| recognized.contains(&name))
    });
    if original_length != 0 && descriptors.is_empty() {
        protocols.remove(category);
    }
}

pub fn parse_collection(data: &[u8]) -> Result<Collection, ParseError> {
    parse(
        data,
        "collection.schema.json",
        "Collection",
        |value: &Collection| {
            let mut issues = collection_representation_issues(value);
            issues.extend(collection_issues(value));
            issues
        },
    )
}

/// Reads a Collection an Agent received.
///
/// ROLE-03: discovery metadata an Agent can still use is not withheld over a defect it can work
/// around, so the invariants `collection_issues` states -- which describe a hierarchy the Agent
/// simply does not walk -- do not refuse the document here. A Service, which MUST NOT publish one,
/// goes through [`parse_collection`].
pub fn parse_agent_collection(data: &[u8]) -> Result<Collection, ParseError> {
    parse(
        &normalize_agent_response(data, "collection")?,
        "collection.schema.json",
        "Collection",
        collection_representation_issues,
    )
}

fn collection_representation_issues(value: &Collection) -> Vec<ValidationIssue> {
    representation_issues(&value.language, &value.localizations, &value.images)
}

/// The Collection rules a JSON Schema cannot state: they compare one member against another.
fn collection_issues(value: &Collection) -> Vec<ValidationIssue> {
    // COL-20: a Collection naming itself as a parent is a one-node cycle, so anything walking the
    // hierarchy upwards from it would never reach a root.
    if value.parent_ids.contains(&value.id) {
        vec![issue(
            "/parent_ids",
            "self-parent",
            "must not name the Collection itself",
        )]
    } else {
        Vec::new()
    }
}

pub fn parse_offering(data: &[u8]) -> Result<Offering, ParseError> {
    parse(
        data,
        "offering.schema.json",
        "Offering",
        |value: &Offering| {
            let mut issues = offering_representation_issues(value);
            issues.extend(offering_issues(value));
            issues
        },
    )
}

/// Reads an Offering an Agent received.
///
/// ROLE-03: a defect an Agent can describe to its caller is a note about that Offering rather than
/// a reason to discard it, so the invariants `offering_issues` states are left for the Agent to
/// report against the Actions or price they concern. A Service, which MUST NOT publish one, goes
/// through [`parse_offering`].
pub fn parse_agent_offering(data: &[u8]) -> Result<Offering, ParseError> {
    parse(
        &normalize_agent_response(data, "offering")?,
        "offering.schema.json",
        "Offering",
        offering_representation_issues,
    )
}

fn offering_representation_issues(value: &Offering) -> Vec<ValidationIssue> {
    representation_issues(&value.language, &value.localizations, &value.images)
}

/// The Offering rules a JSON Schema cannot state: they compare one member against another.
fn offering_issues(value: &Offering) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    // OFR-57: an Action identifier is unique within its Offering, so a repeat leaves an Agent
    // unable to say which Action a caller meant.
    let mut identifiers = std::collections::BTreeSet::new();
    if value
        .actions
        .iter()
        .any(|action| !identifiers.insert(&action.id))
    {
        issues.push(issue(
            "/actions",
            "unique-action-id",
            "must contain unique Action identifiers",
        ));
    }
    // OFR-49: a range whose minimum is above its maximum describes no price at all.
    if let Some(price) = &value.price {
        if price.price_type == crate::PriceType::Range
            && compare_decimals(&price.minimum, &price.maximum).is_gt()
        {
            issues.push(issue(
                "/price/minimum",
                "price-range",
                "must be less than or equal to maximum",
            ));
        }
    }
    issues
}

/// Orders two ODP monetary values, which are decimal strings rather than JSON numbers (OFR-48).
///
/// The schema already fixes the shape as digits with an optional fractional part, so the two are
/// compared digit by digit: the longer whole part is larger, and otherwise the first difference
/// decides. Parsing to a float would lose precision the decimal form exists to keep.
fn compare_decimals(left: &str, right: &str) -> std::cmp::Ordering {
    fn split(value: &str) -> (&str, &str) {
        let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
        (
            whole.trim_start_matches('0'),
            fraction.trim_end_matches('0'),
        )
    }
    let (left_whole, left_fraction) = split(left);
    let (right_whole, right_fraction) = split(right);
    left_whole
        .len()
        .cmp(&right_whole.len())
        .then_with(|| left_whole.cmp(right_whole))
        .then_with(|| left_fraction.cmp(right_fraction))
}

pub fn parse_problem_details(data: &[u8]) -> Result<ProblemDetails, ParseError> {
    parse(
        data,
        "problem-details.schema.json",
        "Problem Details",
        problem_details_issues,
    )
}

pub fn parse_problem_response(data: &[u8], http_status: u16) -> Result<ProblemDetails, ParseError> {
    let value = parse_problem_details(data)?;
    if value.status != http_status {
        return Err(ValidationError {
            document_type: "Problem Details".to_owned(),
            issues: vec![issue(
                "/status",
                "http-status",
                "must match the HTTP response status",
            )],
        }
        .into());
    }
    Ok(value)
}

pub fn parse_resource_identity(data: &[u8]) -> Result<ResourceIdentity, ParseError> {
    parse_without_refinement(data, "resource-identity.schema.json", "resource identity")
}

pub fn parse_page<T: DeserializeOwned>(data: &[u8]) -> Result<Page<T>, ParseError> {
    parse_without_refinement(data, "page-envelope.schema.json", "page envelope")
}

pub fn parse_collection_search_request(data: &[u8]) -> Result<CollectionSearchRequest, ParseError> {
    parse_without_refinement(
        data,
        "collection-search-request.schema.json",
        "Collection search request",
    )
}

pub fn parse_offering_search_request(data: &[u8]) -> Result<OfferingSearchRequest, ParseError> {
    parse_without_refinement(
        data,
        "offering-search-request.schema.json",
        "Offering search request",
    )
}

pub fn parse_offering_search_response(data: &[u8]) -> Result<OfferingPage<Offering>, ParseError> {
    parse(
        data,
        "offering-search-response.schema.json",
        "Offering search response",
        |value: &OfferingPage<Offering>| refinement_issues(&value.refinements),
    )
}

/// Reads an Offering-search response an Agent received.
///
/// A Refinement Group the Agent cannot use is not a reason to discard the Offering results beside
/// it, so the invariants `refinement_issues` states do not refuse the page here. A Service, which
/// MUST NOT publish one, goes through [`parse_offering_search_response`].
pub fn parse_agent_offering_search_response(
    data: &[u8],
) -> Result<OfferingPage<Offering>, ParseError> {
    parse_without_refinement(
        &normalize_agent_response(data, "offering-page")?,
        "offering-search-response.schema.json",
        "Offering search response",
    )
}

/// The Refinement rules a JSON Schema cannot state: they compare one member against another.
fn refinement_issues(groups: &[RefinementGroup]) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    // FLT-30: `filter_id` is unique among the returned groups, so a repeat leaves an Agent unable
    // to say which group belongs to that Filter Definition.
    let mut identifiers = std::collections::BTreeSet::new();
    if groups
        .iter()
        .any(|group| !identifiers.insert(&group.filter_id))
    {
        issues.push(issue(
            "/refinements",
            "unique-filter-id",
            "must contain unique Refinement Group identifiers",
        ));
    }
    // FLT-32: bucket values are unique within a group. The schema's `uniqueItems` compares whole
    // buckets, so it passes two buckets that name one value with differing counts -- exactly the
    // case that leaves an Agent with two counts for the same candidate and no way to choose.
    for (index, group) in groups.iter().enumerate() {
        let mut values = std::collections::BTreeSet::new();
        if group
            .values
            .iter()
            .any(|bucket| !values.insert(bucket_key(&bucket.value)))
        {
            issues.push(issue(
                &format!("/refinements/{index}/values"),
                "unique-bucket-value",
                "must contain unique Refinement Bucket values",
            ));
        }
    }
    issues
}

// String values need their Filter Definition before decimal equality can be applied.
fn bucket_key(value: &Value) -> String {
    match value {
        Value::String(text) => format!("s{text}"),
        Value::Number(number) => number
            .as_f64()
            .map_or_else(|| format!("n{number}"), |float| format!("n{float}")),
        other => format!("o{other}"),
    }
}

pub fn parse_filter_definition(data: &[u8]) -> Result<FilterDefinition, ParseError> {
    parse(
        data,
        "filter-definition.schema.json",
        "Filter Definition",
        filter_definition_issues,
    )
}

pub fn parse_sort_definition(data: &[u8]) -> Result<SortDefinition, ParseError> {
    parse_without_refinement(data, "sort-definition.schema.json", "Sort Definition")
}

pub fn parse_filter_definition_page(data: &[u8]) -> Result<Page<FilterDefinition>, ParseError> {
    parse_without_refinement(
        data,
        "filter-definition-page.schema.json",
        "Filter Definition page",
    )
}

pub fn parse_sort_definition_page(data: &[u8]) -> Result<Page<SortDefinition>, ParseError> {
    parse_without_refinement(
        data,
        "sort-definition-page.schema.json",
        "Sort Definition page",
    )
}

pub fn validate_value(
    value: &Value,
    schema_name: &str,
    document_type: &str,
) -> Result<(), ParseError> {
    validate_json_depth(
        value,
        if schema_name == "service-document.schema.json" {
            8
        } else {
            16
        },
    )?;
    let mut compatible = Cow::Borrowed(value);
    if value
        .get("odp_version")
        .and_then(Value::as_str)
        .is_some_and(|version| {
            version != crate::VERSION
                && version.strip_prefix("1.").is_some_and(|minor| {
                    !minor.is_empty()
                        && (minor == "0" || !minor.starts_with('0'))
                        && minor.bytes().all(|byte| byte.is_ascii_digit())
                })
        })
    {
        compatible.to_mut()["odp_version"] = Value::String(crate::VERSION.to_owned());
    }
    let schemas = schemas()?;
    let validator = schemas.validators.get(schema_name).ok_or_else(|| {
        ParseError::SchemaInitialization(format!("missing bundled schema {schema_name}"))
    })?;
    let issues = validator
        .iter_errors(&compatible)
        .map(|error| {
            let schema_path = error.schema_path().to_string();
            ValidationIssue {
                keyword: schema_path
                    .rsplit('/')
                    .next()
                    .filter(|value| !value.is_empty())
                    .unwrap_or("schema")
                    .to_owned(),
                message: error.to_string(),
                params: BTreeMap::new(),
                path: error.instance_path().to_string(),
            }
        })
        .collect::<Vec<_>>();
    if issues.is_empty() {
        Ok(())
    } else {
        Err(ValidationError {
            document_type: document_type.to_owned(),
            issues,
        }
        .into())
    }
}

/// Checks JSON nesting from a top-level depth of one before protocol data is exposed.
pub fn validate_json_depth(value: &Value, maximum: usize) -> Result<(), ParseError> {
    let mut pending = vec![(value, 1)];
    while let Some((value, depth)) = pending.pop() {
        if !value.is_object() && !value.is_array() {
            continue;
        }
        if depth > maximum {
            return Err(ValidationError {
                document_type: "JSON document".to_owned(),
                issues: vec![issue("", "depth", "exceeds the JSON nesting depth limit")],
            }
            .into());
        }
        match value {
            Value::Object(values) => {
                pending.extend(values.values().map(|value| (value, depth + 1)))
            }
            Value::Array(values) => pending.extend(values.iter().map(|value| (value, depth + 1))),
            _ => {}
        }
    }
    Ok(())
}

fn parse_without_refinement<T: DeserializeOwned>(
    data: &[u8],
    schema_name: &str,
    document_type: &str,
) -> Result<T, ParseError> {
    parse(data, schema_name, document_type, |_| Vec::new())
}

fn problem_details_issues(value: &ProblemDetails) -> Vec<ValidationIssue> {
    let expected_type = format!(
        "https://offeringprotocol.org/problems/{}",
        value.code.to_ascii_lowercase().replace('_', "-")
    );
    if value.problem_type == expected_type {
        Vec::new()
    } else {
        vec![issue(
            "/type",
            "problem-type",
            "must correspond to the problem code",
        )]
    }
}

fn parse<T: DeserializeOwned>(
    data: &[u8],
    schema_name: &str,
    document_type: &str,
    refine: impl FnOnce(&T) -> Vec<ValidationIssue>,
) -> Result<T, ParseError> {
    let raw = serde_json::from_slice(data).map_err(|error| ValidationError {
        document_type: document_type.to_owned(),
        issues: vec![issue("", "json", &error.to_string())],
    })?;
    validate_value(&raw, schema_name, document_type)?;
    let value = serde_json::from_value(raw).map_err(|error| ValidationError {
        document_type: document_type.to_owned(),
        issues: vec![issue("", "decode", &error.to_string())],
    })?;
    let issues = refine(&value);
    if issues.is_empty() {
        Ok(value)
    } else {
        Err(ValidationError {
            document_type: document_type.to_owned(),
            issues,
        }
        .into())
    }
}

fn schemas() -> Result<&'static SchemaSet, ParseError> {
    SCHEMAS
        .get_or_init(initialize_schemas)
        .as_ref()
        .map_err(|error| ParseError::SchemaInitialization(error.clone()))
}

fn initialize_schemas() -> Result<SchemaSet, String> {
    let mut documents = BTreeMap::new();
    for file in SCHEMA_FILES.files() {
        let name = file
            .path()
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "bundled schema has an invalid name".to_owned())?;
        let value = serde_json::from_slice(file.contents())
            .map_err(|error| format!("decode {name}: {error}"))?;
        documents.insert(name.to_owned(), value);
    }
    let mut registry = Registry::new();
    for (name, value) in &documents {
        registry = registry
            .add(
                format!("https://offeringprotocol.org/schemas/{name}"),
                value,
            )
            .map_err(|error| format!("register {name}: {error}"))?;
    }
    let registry = registry
        .prepare()
        .map_err(|error| format!("prepare schema registry: {error}"))?;
    let mut validators = BTreeMap::new();
    for (name, value) in &documents {
        let validator = jsonschema::options()
            .with_registry(&registry)
            .should_validate_formats(true)
            .build(value)
            .map_err(|error| format!("compile {name}: {error}"))?;
        validators.insert(name.clone(), validator);
    }
    Ok(SchemaSet { validators })
}

fn service_document_issues(value: &ServiceDocument) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    if value.additional.contains_key("id") {
        issues.push(issue(
            "/id",
            "prohibited",
            "must not appear in a Service Document",
        ));
    }
    if value.additional.contains_key("web_url") {
        issues.push(issue(
            "/web_url",
            "prohibited",
            "must not appear in a Service Document",
        ));
    }
    if !valid_language_tag(&value.language) {
        issues.push(issue("/language", "language-tag", "must be a language tag"));
    }
    validate_localizations(&value.language, &value.localizations, true, &mut issues);
    if value
        .keywords
        .iter()
        .map(|keyword| keyword.chars().count())
        .sum::<usize>()
        > 1024
    {
        issues.push(issue(
            "/keywords",
            "max-code-points",
            "must contain no more than 1024 code points in total",
        ));
    }
    if value.search_capabilities.is_some()
        && !value
            .operations
            .iter()
            .any(|operation| operation.name == Operation::SearchOfferings)
    {
        issues.push(issue(
            "/search_capabilities",
            "operation-support",
            "requires the search-offerings operation",
        ));
    }
    issues
}

fn representation_issues(
    language: &str,
    localizations: &[String],
    images: &[crate::ResourceImage],
) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    if !language.is_empty() && !valid_language_tag(language) {
        issues.push(issue("/language", "language-tag", "must be a language tag"));
    }
    validate_localizations(language, localizations, false, &mut issues);
    let mut sources = std::collections::BTreeSet::new();
    if images.iter().any(|image| !sources.insert(&image.src)) {
        issues.push(issue(
            "/images",
            "unique-image-source",
            "must contain unique image sources",
        ));
    }
    issues
}

fn validate_localizations(
    language: &str,
    localizations: &[String],
    require_default: bool,
    issues: &mut Vec<ValidationIssue>,
) {
    if localizations.iter().any(|tag| !valid_language_tag(tag)) {
        issues.push(issue(
            "/localizations",
            "language-tag",
            "must contain only language tags",
        ));
        return;
    }
    let folded = localizations
        .iter()
        .map(|tag| tag.to_ascii_lowercase())
        .collect::<std::collections::BTreeSet<_>>();
    if folded.len() != localizations.len() {
        issues.push(issue(
            "/localizations",
            "unique-language-tag",
            "must be unique without regard to case",
        ));
    }
    if (require_default || (!language.is_empty() && !localizations.is_empty()))
        && !folded.contains(&language.to_ascii_lowercase())
    {
        issues.push(issue(
            "/localizations",
            if require_default {
                "contains-default-language"
            } else {
                "contains-language"
            },
            if require_default {
                "must contain the default language"
            } else {
                "must contain the representation language"
            },
        ));
    }
}

fn valid_language_tag(value: &str) -> bool {
    let Ok(tag) = value.parse::<LanguageTag>() else {
        return false;
    };
    let mut variants = std::collections::BTreeSet::new();
    if tag
        .variant_subtags()
        .any(|variant| !variants.insert(variant.to_ascii_lowercase()))
    {
        return false;
    }
    let mut extensions = std::collections::BTreeSet::new();
    !tag.extension_subtags()
        .any(|(singleton, _)| !extensions.insert(singleton.to_ascii_lowercase()))
}

fn filter_definition_issues(value: &FilterDefinition) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    if matches!(value.filter_type, FilterType::String | FilterType::Boolean)
        && value.operators.iter().any(|operator| {
            matches!(
                operator,
                FilterOperator::GreaterThan
                    | FilterOperator::GreaterThanOrEqual
                    | FilterOperator::LessThan
                    | FilterOperator::LessThanOrEqual
            )
        })
    {
        issues.push(issue(
            "/operators",
            "operator-type",
            "contains an operator incompatible with the Filter type",
        ));
    }
    // FLT-10: only a numeric Filter carries a unit. A unit on a string, date, date-time or
    // boolean Filter describes a dimension its values do not have, so an Agent reading it would
    // convert or label values that were never quantities.
    if !matches!(
        value.filter_type,
        FilterType::Decimal | FilterType::Integer | FilterType::Number
    ) && value.unit.is_some()
    {
        issues.push(issue(
            "/unit",
            "unit-type",
            "must not appear on a non-numeric Filter",
        ));
    }
    issues
}

fn issue(path: &str, keyword: &str, message: &str) -> ValidationIssue {
    ValidationIssue {
        keyword: keyword.to_owned(),
        message: message.to_owned(),
        params: BTreeMap::new(),
        path: path.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_normative_service_document() {
        let document = br#"{
            "description":"Plant store",
            "http":{"endpoint_base":"/odp"},
            "language":"en",
            "localizations":["en"],
            "name":"Plants",
            "odp_version":"1.0",
            "operations":[
                {"authentication":"not-required","name":"get-offering"},
                {"authentication":"not-required","name":"list-offerings"}
            ]
        }"#;
        assert_eq!(parse_service_document(document).unwrap().name, "Plants");
    }

    #[test]
    fn rejects_problem_types_that_do_not_correspond_to_the_code() {
        let problem = br#"{
            "code":"NOT_FOUND",
            "status":404,
            "title":"Not found",
            "type":"https://offeringprotocol.org/problems/validation-failed"
        }"#;

        assert!(parse_problem_details(problem).is_err());
    }

    #[test]
    fn parses_tap_trust_protocol_support() {
        let document = br#"{
            "description":"Plant store",
            "http":{"endpoint_base":"/odp"},
            "language":"en",
            "localizations":["en"],
            "name":"Plants",
            "odp_version":"1.0",
            "operations":[
                {"authentication":"not-required","name":"get-offering"},
                {"authentication":"not-required","name":"list-offerings"}
            ],
            "protocols":{"trust":[{"name":"tap"}]}
        }"#;
        let parsed = parse_service_document(document).unwrap();
        assert_eq!(
            parsed.protocols.unwrap().trust,
            [crate::TrustProtocol {
                name: crate::Protocol::Tap
            }]
        );
    }

    #[test]
    fn agent_parser_filters_unknown_protocols_strictly() {
        let document = br#"{
            "description":"Plants","http":{"endpoint_base":"/odp"},"language":"en",
            "localizations":["en"],"name":"Plants","odp_version":"1.0",
            "operations":[{"authentication":"not-required","name":"get-offering"},
            {"authentication":"not-required","name":"list-offerings"}],
            "protocols":{
                "enrollment":[{"name":"future-enrollment"},{"name":"aep"}],
                "payments":[{"authentication":"not-required","name":"future-payment"},
                {"authentication":"not-required","name":"mpp"},
                {"authentication":"not-required","name":"x402"}],
                "trust":[{"name":"future-trust"},{"name":"tap"}]
            }
        }"#;
        assert!(parse_service_document(document).is_err());
        let protocols = parse_agent_service_document(document)
            .unwrap()
            .protocols
            .unwrap();
        assert_eq!(protocols.enrollment[0].name, crate::Protocol::Aep);
        assert_eq!(protocols.payments.len(), 2);
        assert_eq!(protocols.trust[0].name, crate::Protocol::Tap);

        let unknown_only = br#"{
            "description":"Plants","http":{"endpoint_base":"/odp"},"language":"en",
            "localizations":["en"],"name":"Plants","odp_version":"1.0",
            "operations":[{"authentication":"not-required","name":"get-offering"},
            {"authentication":"not-required","name":"list-offerings"}],
            "protocols":{"trust":[{"name":"future-trust"}]}
        }"#;
        assert!(
            parse_agent_service_document(unknown_only)
                .unwrap()
                .protocols
                .is_none()
        );

        let malformed = br#"{
            "description":"Plants","http":{"endpoint_base":"/odp"},"language":"en",
            "localizations":["en"],"name":"Plants","odp_version":"1.0",
            "operations":[{"authentication":"not-required","name":"get-offering"},
            {"authentication":"not-required","name":"list-offerings"}],
            "protocols":{"trust":[{"name":"tap","unexpected":true}]}
        }"#;
        assert!(parse_agent_service_document(malformed).is_err());
        assert!(parse_agent_service_document(b"invalid").is_err());
    }

    #[test]
    fn returns_structured_validation_issues() {
        let error = parse_service_document(br#"{}"#).unwrap_err();
        let ParseError::Validation(error) = error else {
            panic!("expected validation error");
        };
        assert!(!error.issues.is_empty());
    }

    #[test]
    fn rejects_duplicate_language_variants() {
        let document = br#"{
            "description":"An example Service.",
            "http":{"endpoint_base":"/odp"},
            "language":"sl-rozaj-rozaj",
            "localizations":["sl-rozaj-rozaj"],
            "name":"Example",
            "odp_version":"1.0",
            "operations":[
                {"authentication":"not-required","name":"get-offering"},
                {"authentication":"not-required","name":"list-offerings"}
            ]
        }"#;
        assert!(parse_service_document(document).is_err());
    }
}
