use std::{
    collections::BTreeMap,
    io::BufRead,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use odp_agent::{OfferingIssueScope, ServiceClient};
use odp_core::{
    Collection, Offering, OfferingPage, OfferingSearchRequest, Operation, ResourceIdentity,
    VERSION, derive_service_origin, is_local_resource_identifier, normalize_agent_response,
    parse_agent_service_document, parse_collection, parse_collection_search_request,
    parse_filter_definition, parse_filter_definition_page, parse_offering,
    parse_offering_search_request, parse_page, parse_problem_details, parse_problem_response,
    parse_service_document, parse_sort_definition, parse_sort_definition_page,
    resolve_continuation, resolve_resource_reference,
};
use odp_directory::{HttpRequest, HttpResponse, Transport, TransportError};
use odp_service::{Catalog, CatalogRequest, MEDIA_TYPE, Request, Service, ServiceError};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

#[derive(Deserialize)]
struct AdapterRequest {
    case: BTreeMap<String, Value>,
    role: String,
    sequence: u64,
    vector: Vector,
}

#[derive(Deserialize)]
struct Vector {
    subject: String,
}

#[derive(Serialize)]
struct AdapterResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    protocol_version: &'static str,
    sequence: u64,
    status: &'static str,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input = std::io::stdin();
    for line in input.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let request: AdapterRequest = serde_json::from_str(&line)?;
        let response = evaluate(&request).await;
        println!("{}", serde_json::to_string(&response)?);
    }
    Ok(())
}

async fn evaluate(request: &AdapterRequest) -> AdapterResponse {
    match evaluate_case(&request.vector.subject, &request.case, &request.role).await {
        Ok(Some(true)) => response(request.sequence, "passed", None),
        Ok(Some(false)) => response(
            request.sequence,
            "failed",
            Some("Public API result did not match the vector".to_owned()),
        ),
        Ok(None) => response(
            request.sequence,
            "skipped",
            Some("No public Rust operation maps this vector case".to_owned()),
        ),
        Err(error) => response(
            request.sequence,
            "failed",
            Some(error.chars().take(1024).collect()),
        ),
    }
}

fn response(sequence: u64, status: &'static str, message: Option<String>) -> AdapterResponse {
    AdapterResponse {
        message,
        protocol_version: "1",
        sequence,
        status,
    }
}

async fn evaluate_case(
    subject: &str,
    case: &BTreeMap<String, Value>,
    role: &str,
) -> Result<Option<bool>, String> {
    let valid = field::<bool>(case, "valid").unwrap_or(false);
    match subject {
        "protocol-version" => {
            let received: String = required(case, "received")?;
            let compatible: bool = required(case, "compatible")?;
            let document = serde_json::json!({"odp_version": received,"name":"Conformance","description":"Conformance","language":"en","localizations":["en"],"http":{"endpoint_base":"/odp"},"operations":[{"name":"list-offerings","authentication":"not-required"},{"name":"get-offering","authentication":"not-required"}]});
            Ok(Some(
                parse_service_document(
                    &serde_json::to_vec(&document).map_err(|error| error.to_string())?,
                )
                .is_ok()
                    == compatible,
            ))
        }
        "local-identifier" => {
            let value: String = required(case, "value")?;
            Ok(Some(is_local_resource_identifier(&value) == valid))
        }
        "identity-comparison" => {
            let left: ResourceIdentity = required(case, "left")?;
            let right: ResourceIdentity = required(case, "right")?;
            let expected: bool = required(case, "same_identity")?;
            Ok(Some((left == right) == expected))
        }
        "service-origin" => {
            let value: String = required(case, "value")?;
            let canonical = derive_service_origin(&value).is_ok_and(|origin| origin == value);
            Ok(Some(canonical == valid))
        }
        "resource-reference" => {
            let value: String = required(case, "value")?;
            Ok(Some(
                resolve_resource_reference(&value, "https://service.example").is_ok() == valid,
            ))
        }
        "service-document" => parse_result(case, "document", parse_service_document),
        "collection-envelope" => parse_result(case, "document", parse_collection),
        "offering-contract"
            if field::<String>(case, "representation").as_deref() == Some("full") =>
        {
            parse_result(case, "document", parse_offering)
        }
        "offering-contract" => Ok(None),
        "collection-search-contract" if operation(case) == "validate-request" => {
            parse_result(case, "request", parse_collection_search_request)
        }
        "collection-search-contract" => Ok(None),
        "composition-contract" => evaluate_composition(case, role),
        "offering-search-contract" if operation(case) == "validate-request" => {
            parse_result(case, "request", parse_offering_search_request)
        }
        "offering-search-contract" => Ok(None),
        "attribute-schema-retrieval" => evaluate_attribute_schema(case).await,
        "filter-sort-contract" if operation(case) == "validate-definition" => {
            parse_result(case, "definition", parse_filter_definition)
        }
        "filter-sort-contract"
            if operation(case) == "validate-sort" && !case.contains_key("definitions") =>
        {
            parse_result(case, "sort", parse_sort_definition)
        }
        "filter-sort-contract" => Ok(None),
        "pagination-contract" => evaluate_pagination(case).await,
        "errors-limits-contract" => evaluate_errors_and_limits(case).await,
        "role-baseline" => evaluate_baseline(case, role),
        _ => Ok(None),
    }
}

fn evaluate_composition(
    case: &BTreeMap<String, Value>,
    role: &str,
) -> Result<Option<bool>, String> {
    match operation(case).as_str() {
        "normalize-agent-response" if role == "agent" => {
            let document: Value = required(case, "document")?;
            let kind: String = required(case, "kind")?;
            let expected: Value = required(case, "expected")?;
            let encoded = serde_json::to_vec(&document).map_err(|error| error.to_string())?;
            let actual =
                normalize_agent_response(&encoded, &kind).map_err(|error| error.to_string())?;
            validate_agent_response(&actual, &kind)?;
            let actual: Value =
                serde_json::from_slice(&actual).map_err(|error| error.to_string())?;
            Ok(Some(actual == expected))
        }
        "validate-advertisement" => {
            let protocols: Value = required(case, "protocols")?;
            let document = service_document_with_protocols(protocols);
            let encoded = serde_json::to_vec(&document).map_err(|error| error.to_string())?;
            Ok(Some(
                parse_service_document(&encoded).is_ok()
                    == field::<bool>(case, "valid").unwrap_or(false),
            ))
        }
        "filter-advertisement" if role == "agent" => {
            let protocols: Value = required(case, "protocols")?;
            let expected: Value = required(case, "expected")?;
            let document = service_document_with_protocols(protocols);
            let encoded = serde_json::to_vec(&document).map_err(|error| error.to_string())?;
            let parsed =
                parse_agent_service_document(&encoded).map_err(|error| error.to_string())?;
            let actual = parsed
                .protocols
                .map_or_else(|| json!({}), |value| json!(value));
            Ok(Some(actual == expected))
        }
        _ => Ok(None),
    }
}

fn validate_agent_response(document: &[u8], kind: &str) -> Result<(), String> {
    match kind {
        "service-document" => parse_agent_service_document(document).map(|_| ()),
        "collection" => parse_collection(document).map(|_| ()),
        "offering" => parse_offering(document).map(|_| ()),
        "collection-page" => parse_page::<Collection>(document).map(|_| ()),
        "offering-page" => parse_page::<Offering>(document).map(|_| ()),
        "filter-page" => parse_filter_definition_page(document).map(|_| ()),
        "sort-page" => parse_sort_definition_page(document).map(|_| ()),
        "problem" => parse_problem_details(document).map(|_| ()),
        _ => return Err("Unknown Agent response kind".to_owned()),
    }
    .map_err(|error| error.to_string())
}

fn service_document_with_protocols(protocols: Value) -> Value {
    json!({
        "description": "ODP Rust conformance adapter",
        "http": {"endpoint_base": "/odp"},
        "language": "en",
        "localizations": ["en"],
        "name": "Conformance Service",
        "odp_version": "1.0",
        "operations": [
            {"authentication": "not-required", "name": "get-offering"},
            {"authentication": "not-required", "name": "list-offerings"}
        ],
        "protocols": protocols
    })
}

#[derive(Clone)]
struct SchemaResponse {
    body: Vec<u8>,
    content_type: String,
    status: u16,
}

struct ServiceTransport {
    offering: Vec<u8>,
    terse: bool,
}

#[async_trait]
impl Transport for ServiceTransport {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        let body = if request.url.ends_with("/.well-known/odp") {
            br#"{"description":"ODP Rust conformance adapter","http":{"endpoint_base":"/odp"},"language":"en","localizations":["en"],"name":"Conformance Service","odp_version":"1.0","operations":[{"authentication":"not-required","name":"get-offering"},{"authentication":"not-required","name":"list-offerings"}]}"#.to_vec()
        } else if self.terse {
            br#"{"items":[{"id":"item","name":"Item"}],"odp_version":"1.0"}"#.to_vec()
        } else {
            self.offering.clone()
        };
        Ok(HttpResponse {
            body,
            headers: BTreeMap::from([(
                "content-type".to_owned(),
                "application/odp+json".to_owned(),
            )]),
            status: 200,
        })
    }
}

struct SupportingTransport {
    calls: AtomicUsize,
    documents: BTreeMap<String, SchemaResponse>,
}

#[async_trait]
impl Transport for SupportingTransport {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let response = self
            .documents
            .get(&request.url)
            .cloned()
            .unwrap_or_else(|| SchemaResponse {
                body: br#"{"title":"Not Found"}"#.to_vec(),
                content_type: "application/problem+json".to_owned(),
                status: 404,
            });
        Ok(HttpResponse {
            body: response.body,
            headers: BTreeMap::from([("content-type".to_owned(), response.content_type)]),
            status: response.status,
        })
    }
}

async fn evaluate_attribute_schema(case: &BTreeMap<String, Value>) -> Result<Option<bool>, String> {
    let valid = field::<bool>(case, "valid").unwrap_or(false);
    match operation(case).as_str() {
        "validate-reference" => {
            let offering = serde_json::to_vec(&json!({
                "id": "item",
                "name": "Item",
                "odp_version": VERSION,
                "schema": case.get("reference").ok_or("case omitted reference")?
            }))
            .map_err(|error| error.to_string())?;
            Ok(Some(parse_offering(&offering).is_ok() == valid))
        }
        "validate-response" => {
            let response = SchemaResponse {
                body: raw(case, "document")?,
                content_type: required(case, "content_type")?,
                status: required(case, "status")?,
            };
            let (details, _) = attribute_schema_details(
                BTreeMap::from([("https://schemas.example/root.json".to_owned(), response)]),
                "https://schemas.example/root.json",
                json!({"name": "root"}),
                false,
            )
            .await?;
            Ok(Some(details.attribute_schema.is_some() == valid))
        }
        "validate-schema-reference-profile" => {
            let documents: Vec<Value> = required(case, "documents")?;
            let mut responses = BTreeMap::new();
            let mut root_url = String::new();
            for (index, document) in documents.into_iter().enumerate() {
                let url = document
                    .get("$id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("https://schemas.example/document-{index}.json"));
                if root_url.is_empty() {
                    root_url.clone_from(&url);
                }
                responses.insert(
                    url,
                    SchemaResponse {
                        body: serde_json::to_vec(&document).map_err(|error| error.to_string())?,
                        content_type: "application/schema+json".to_owned(),
                        status: 200,
                    },
                );
            }
            let (details, _) = attribute_schema_details(
                responses,
                &root_url,
                json!({"children": [{"name": "child"}], "name": "root"}),
                false,
            )
            .await?;
            Ok(Some(details.attribute_schema.is_some() == valid))
        }
        "validation-scope" => {
            let representation: String = required(case, "representation")?;
            let expected: bool = required(case, "complete_instance_validation")?;
            let terse = representation == "terse";
            let (details, requests) = attribute_schema_details(
                BTreeMap::from([(
                    "https://schemas.example/root.json".to_owned(),
                    SchemaResponse {
                        body: br#"{"$schema":"https://json-schema.org/draft/2020-12/schema","properties":{"memory":{"type":"number"}},"type":"object"}"#.to_vec(),
                        content_type: "application/schema+json".to_owned(),
                        status: 200,
                    },
                )]),
                "https://schemas.example/root.json",
                json!({"memory": "invalid"}),
                terse,
            )
            .await?;
            let complete = requests > 0
                && details.offering.attributes.is_empty()
                && details
                    .issues
                    .iter()
                    .any(|issue| issue.scope == OfferingIssueScope::Attributes);
            Ok(Some(complete == expected))
        }
        "failure-scope" => {
            let expected: BTreeMap<String, bool> = required(case, "expected")?;
            let (details, _) = attribute_schema_details(
                BTreeMap::from([(
                    "https://schemas.example/root.json".to_owned(),
                    SchemaResponse {
                        body: br#"{"title":"Unavailable"}"#.to_vec(),
                        content_type: "application/problem+json".to_owned(),
                        status: 503,
                    },
                )]),
                "https://schemas.example/root.json",
                json!({"name": "root"}),
                false,
            )
            .await?;
            let actual = BTreeMap::from([
                ("offering_usable".to_owned(), details.offering.id == "item"),
                (
                    "attributes_usable".to_owned(),
                    !details.offering.attributes.is_empty(),
                ),
                (
                    "report_issue".to_owned(),
                    details
                        .issues
                        .iter()
                        .any(|issue| issue.scope == OfferingIssueScope::AttributeSchema),
                ),
            ]);
            Ok(Some(actual == expected))
        }
        _ => Ok(None),
    }
}

async fn attribute_schema_details(
    documents: BTreeMap<String, SchemaResponse>,
    root_url: &str,
    attributes: Value,
    terse: bool,
) -> Result<(odp_agent::OfferingDetails, usize), String> {
    let offering = serde_json::to_vec(&json!({
        "attributes": attributes,
        "id": "item",
        "name": "Item",
        "odp_version": VERSION,
        "schema": {"url": root_url}
    }))
    .map_err(|error| error.to_string())?;
    let service_transport = Arc::new(ServiceTransport { offering, terse });
    let supporting_transport = Arc::new(SupportingTransport {
        calls: AtomicUsize::new(0),
        documents,
    });
    let client = ServiceClient::with_transport("https://service.example", service_transport)
        .map_err(|error| error.to_string())?
        .with_supporting_transport(supporting_transport.clone());
    let details = if terse {
        let offering = client
            .list_offerings(odp_core::Representation::Terse, 1)
            .await
            .map_err(|error| error.to_string())?
            .items
            .into_iter()
            .next()
            .ok_or("terse conformance response omitted its Offering")?;
        odp_agent::OfferingDetails {
            actions: Vec::new(),
            attribute_schema: None,
            issues: Vec::new(),
            offering,
        }
    } else {
        client
            .get_offering_details("item")
            .await
            .map_err(|error| error.to_string())?
    };
    Ok((details, supporting_transport.calls.load(Ordering::Relaxed)))
}

async fn evaluate_pagination(case: &BTreeMap<String, Value>) -> Result<Option<bool>, String> {
    let valid = field::<bool>(case, "valid").unwrap_or(false);
    match operation(case).as_str() {
        "validate-page" => parse_result(case, "page", parse_page::<Value>),
        "validate-limit" => {
            let limit: usize = required(case, "limit")?;
            let service =
                odp_service::ServiceBuilder::new("Conformance", "Conformance", "en", "/odp")
                    .build(Arc::new(ConformanceCatalog))
                    .map_err(|error| error.to_string())?;
            let response = service
                .handle(Request {
                    method: "GET".to_owned(),
                    path: "/odp/offerings".to_owned(),
                    query: format!("limit={limit}"),
                    ..Request::default()
                })
                .await;
            Ok(Some((response.status == 200) == valid))
        }
        "validate-next" => {
            let next: String = required(case, "next")?;
            let origin: String = required(case, "service_origin")?;
            Ok(Some(resolve_continuation(&next, &origin).is_ok() == valid))
        }
        _ => Ok(None),
    }
}

async fn evaluate_errors_and_limits(
    case: &BTreeMap<String, Value>,
) -> Result<Option<bool>, String> {
    let valid = field::<bool>(case, "valid").unwrap_or(false);
    if operation(case) == "validate-problem" {
        let status: u16 = required(case, "http_status")?;
        let problem = raw(case, "problem")?;
        return Ok(Some(
            parse_problem_response(&problem, status).is_ok() == valid,
        ));
    }
    if operation(case) != "validate-limit"
        || field::<String>(case, "resource").as_deref() != Some("request")
    {
        return Ok(None);
    }
    let bytes: usize = required(case, "bytes")?;
    let status = service_request_status(bytes).await?;
    Ok(Some((status == 200) == valid))
}

fn evaluate_baseline(case: &BTreeMap<String, Value>, role: &str) -> Result<Option<bool>, String> {
    if required::<String>(case, "role")? != role {
        return Ok(None);
    }
    let valid = field::<bool>(case, "valid").unwrap_or(false);
    if role == "agent" {
        let behaviors: Vec<String> = required(case, "behaviors")?;
        let required = [
            "enforce-compatibility",
            "enforce-redirect-and-security",
            "follow-pagination",
            "get-offering",
            "handle-errors-and-limits",
            "honor-caching",
            "inspect-service",
            "list-offerings",
            "process-localization",
            "process-representations",
        ];
        return Ok(Some(
            required
                .iter()
                .all(|behavior| behaviors.iter().any(|value| value == behavior))
                == valid,
        ));
    }
    let operations: Vec<Value> = required(case, "operations")?;
    let descriptors = operations
        .into_iter()
        .map(|name| json!({"authentication": "not-required", "name": name}))
        .collect::<Vec<_>>();
    let document = serde_json::to_vec(&json!({
        "description": "Conformance Service",
        "http": {"endpoint_base": "/odp"},
        "language": "en",
        "localizations": ["en"],
        "name": "Conformance Service",
        "odp_version": VERSION,
        "operations": descriptors
    }))
    .map_err(|error| error.to_string())?;
    let list_response = raw(case, "list_response")?;
    let get_response = raw(case, "get_response")?;
    let actual = parse_service_document(&document).is_ok()
        && parse_page::<Offering>(&list_response).is_ok()
        && parse_offering(&get_response).is_ok();
    Ok(Some(actual == valid))
}

fn parse_result<T>(
    case: &BTreeMap<String, Value>,
    name: &str,
    parser: fn(&[u8]) -> Result<T, odp_core::ParseError>,
) -> Result<Option<bool>, String> {
    let valid = field::<bool>(case, "valid").unwrap_or(false);
    Ok(Some(parser(&raw(case, name)?).is_ok() == valid))
}

fn operation(case: &BTreeMap<String, Value>) -> String {
    field(case, "operation").unwrap_or_default()
}

fn field<T: DeserializeOwned>(case: &BTreeMap<String, Value>, name: &str) -> Option<T> {
    case.get(name)
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
}

fn required<T: DeserializeOwned>(case: &BTreeMap<String, Value>, name: &str) -> Result<T, String> {
    field(case, name).ok_or_else(|| format!("case omitted {name}"))
}

fn raw(case: &BTreeMap<String, Value>, name: &str) -> Result<Vec<u8>, String> {
    serde_json::to_vec(
        case.get(name)
            .ok_or_else(|| format!("case omitted {name}"))?,
    )
    .map_err(|error| error.to_string())
}

struct ConformanceCatalog;

#[async_trait]
impl Catalog for ConformanceCatalog {
    fn operations(&self) -> Vec<Operation> {
        vec![
            Operation::GetOffering,
            Operation::ListOfferings,
            Operation::SearchOfferings,
        ]
    }

    async fn list_offerings(
        &self,
        _request: CatalogRequest,
    ) -> Result<OfferingPage<Offering>, ServiceError> {
        Ok(empty_offerings())
    }

    async fn get_offering(
        &self,
        _id: &str,
        _request: CatalogRequest,
    ) -> Result<Option<Offering>, ServiceError> {
        Ok(None)
    }

    async fn search_offerings(
        &self,
        _query: OfferingSearchRequest,
        _request: CatalogRequest,
    ) -> Result<OfferingPage<Offering>, ServiceError> {
        Ok(empty_offerings())
    }
}

fn empty_offerings() -> OfferingPage<Offering> {
    OfferingPage {
        additional: BTreeMap::new(),
        auth_expands: false,
        items: Vec::new(),
        next: String::new(),
        odp_version: VERSION.to_owned(),
        refinements: Vec::new(),
    }
}

async fn service_request_status(size: usize) -> Result<u16, String> {
    let document = parse_service_document(
        br#"{"description":"Conformance Service","http":{"endpoint_base":"/odp"},"language":"en","localizations":["en"],"name":"Conformance Service","odp_version":"1.0","operations":[{"authentication":"not-required","name":"get-offering"},{"authentication":"not-required","name":"list-offerings"},{"authentication":"not-required","name":"search-offerings"}]}"#,
    )
    .map_err(|error| error.to_string())?;
    let service =
        Service::new(document, Arc::new(ConformanceCatalog)).map_err(|error| error.to_string())?;
    let payload = br#"{"odp_version":"1.0","query":"gpu"}"#;
    let mut body = payload.to_vec();
    body.resize(size, b' ');
    let response = service
        .handle(Request {
            body,
            headers: BTreeMap::from([
                ("accept".to_owned(), MEDIA_TYPE.to_owned()),
                ("content-type".to_owned(), MEDIA_TYPE.to_owned()),
            ]),
            method: "POST".to_owned(),
            path: "/odp/offerings/search".to_owned(),
            query: String::new(),
        })
        .await;
    Ok(response.status)
}
