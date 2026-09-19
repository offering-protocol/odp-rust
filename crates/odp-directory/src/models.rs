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
    pub service: DirectoryService,
    pub indexed_at: String,
    pub available_through: Option<ServiceReference>,
    #[serde(flatten)]
    pub additional: AdditionalMembers,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct CollectionResult {
    pub service: DirectoryService,
    pub indexed_at: String,
    pub collection: CollectionSummary,
    #[serde(flatten)]
    pub additional: AdditionalMembers,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ServiceReference {
    pub service_id: String,
    pub service_origin: String,
    pub name: Option<String>,
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
    #[serde(default)]
    pub trust: Vec<Facet<TrustProtocol>>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct PaymentOptionFacetValue {
    pub name: Protocol,
    pub option: PaymentOption,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct SearchPage {
    #[serde(default)]
    pub facets: Option<Facets>,
    pub items: Vec<DirectoryService>,
    #[serde(default)]
    pub next: String,
    #[serde(flatten)]
    pub additional: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SuggestionRequest {
    pub limit: usize,
    pub prefix: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct IterationOptions {
    pub max_items: usize,
    pub max_pages: usize,
}
