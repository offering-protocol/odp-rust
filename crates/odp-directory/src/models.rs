use std::collections::BTreeMap;

use odp_core::{
    AdditionalMembers, AuthenticationRequirement, EnrollmentProtocol, Operation,
    OperationDescriptor, PaymentOption, PaymentProtocol, Protocol, ServiceProtocols, TrustProtocol,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Environment {
    #[default]
    Production,
    Sandbox,
}

impl Environment {
    pub const fn origin(self) -> &'static str {
        match self {
            Self::Production => "https://api.inflowpay.ai",
            Self::Sandbox => "https://sandbox.inflowpay.ai",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ServiceFilters {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enrollment: Vec<EnrollmentProtocol>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operations: Vec<OperationFilter>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub payments: Vec<PaymentFilter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sources: Option<Vec<SourceType>>,
    /// A trust filter is either empty or the single-item array `[{"name":"tap"}]`: `tap` is the
    /// only trust protocol this ODP version names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trust: Vec<TrustProtocol>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct OperationFilter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authentication: Option<AuthenticationRequirement>,
    pub name: Operation,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PaymentFilter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authentication: Option<AuthenticationRequirement>,
    pub name: Protocol,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<PaymentOption>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct SearchRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filters: Option<ServiceFilters>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub limit: usize,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub query: String,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ResultType {
    Service,
    Collection,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceType {
    Odp,
    Openapi,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DirectorySource {
    /// Unknown future formats remain readable but are not ODP capabilities.
    #[serde(rename = "type")]
    pub source_type: String,
    pub url: String,
    pub x402_discovery: bool,
    #[serde(flatten)]
    pub additional: AdditionalMembers,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct DirectoryIndexedService {
    pub description: Option<String>,
    pub documentation_url: Option<String>,
    pub indexed_at: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    pub language: Option<String>,
    #[serde(default)]
    pub localizations: Vec<String>,
    pub name: String,
    #[serde(default)]
    pub operations: Vec<OperationDescriptor>,
    pub protocols: Option<ServiceProtocols>,
    pub service_id: String,
    pub service_origin: String,
    pub source: DirectorySource,
    pub status_url: Option<String>,
    pub support_url: Option<String>,
    pub website_url: Option<String>,
    #[serde(flatten)]
    pub additional: AdditionalMembers,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ResourceSearchRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filters: Option<ServiceFilters>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub limit: usize,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub types: Option<Vec<ResultType>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DirectoryResult {
    Service(Box<ServiceResult>),
    Collection(Box<CollectionResult>),
    Unknown { kind: String, raw: Value },
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ServiceResult {
    pub service: DirectoryIndexedService,
    pub indexed_at: String,
    pub publisher: Option<Publisher>,
    #[serde(flatten)]
    pub additional: AdditionalMembers,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct CollectionResult {
    pub service: DirectoryIndexedService,
    pub indexed_at: String,
    pub collection: CollectionSummary,
    #[serde(flatten)]
    pub additional: AdditionalMembers,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Publisher {
    pub publisher_id: String,
    pub name: String,
    pub website_url: String,
    #[serde(flatten)]
    pub additional: AdditionalMembers,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct CollectionSummary {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    #[serde(flatten)]
    pub additional: AdditionalMembers,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DirectoryIssue {
    pub index: usize,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchResponse {
    pub facets: Option<Facets>,
    pub items: Vec<DirectoryResult>,
    pub next: Option<String>,
    pub issues: Vec<DirectoryIssue>,
    pub additional: AdditionalMembers,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct DirectoryService {
    pub description: String,
    #[serde(default)]
    pub documentation_url: String,
    pub indexed_at: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    pub language: String,
    pub localizations: Vec<String>,
    pub name: String,
    pub operations: Vec<OperationDescriptor>,
    #[serde(default)]
    pub protocols: Option<ServiceProtocols>,
    pub service_origin: String,
    #[serde(default)]
    pub status_url: String,
    #[serde(default)]
    pub support_url: String,
    #[serde(default)]
    pub website_url: String,
    #[serde(flatten)]
    pub additional: AdditionalMembers,
}

impl DirectoryService {
    pub fn service_id(&self) -> Option<&str> {
        self.additional.get("service_id").and_then(Value::as_str)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Facet<T> {
    pub count: u64,
    pub value: T,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Facets {
    #[serde(default)]
    pub enrollment: Vec<Facet<EnrollmentProtocol>>,
    #[serde(default)]
    pub keywords: Vec<Facet<String>>,
    #[serde(default)]
    pub operations: Vec<Facet<OperationDescriptor>>,
    #[serde(default)]
    pub payment_options: Vec<Facet<PaymentOptionFacetValue>>,
    #[serde(default)]
    pub payments: Vec<Facet<PaymentProtocol>>,
    /// A trust facet counts Services by trust protocol. `tap` is the only one this ODP version
    /// names, so every descriptor carries that name and nothing else.
    #[serde(default)]
    pub trust: Vec<Facet<TrustProtocol>>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct PaymentOptionFacetValue {
    pub name: Protocol,
    pub option: PaymentOption,
}

/// A record the Directory published that this client would not hand back.
///
/// ROLE-03: a Directory result is discovery metadata, so one unusable record is a note about that
/// record rather than a reason to withhold the page.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceIssue {
    /// The record's position in the page the Directory sent.
    pub index: usize,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct SearchPage {
    #[serde(default)]
    pub facets: Option<Facets>,
    /// The records this client was able to read. Withheld records appear in `issues`.
    pub items: Vec<DirectoryService>,
    #[serde(default, skip)]
    pub issues: Vec<ServiceIssue>,
    #[serde(default, deserialize_with = "absent_as_empty")]
    pub next: String,
    #[serde(flatten)]
    pub additional: BTreeMap<String, Value>,
}

/// A Directory that offers no continuation may omit `next` or send it as null; both mean the same.
fn absent_as_empty<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SuggestionRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filters: Option<ServiceFilters>,
    #[serde(skip_serializing_if = "is_zero")]
    pub limit: usize,
    pub prefix: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct IterationOptions {
    pub max_items: usize,
    pub max_pages: usize,
}
