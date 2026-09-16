use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const VERSION: &str = "1.0";
pub type AdditionalMembers = BTreeMap<String, Value>;

macro_rules! string_enum {
    ($name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
        pub enum $name {
            $(#[serde(rename = $value)] $variant),+
        }
    };
}

string_enum!(Representation { Terse => "terse", Full => "full" });
string_enum!(PriceType {
    Fixed => "fixed",
    Free => "free",
    Metered => "metered",
    Quote => "quote",
    Range => "range",
    StartingAt => "starting_at",
});
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ActionRelation {
    Download,
    Invoke,
    Purchase,
    Quote,
    Reserve,
    Other(String),
}

impl<'de> Deserialize<'de> for ActionRelation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(match String::deserialize(deserializer)?.as_str() {
            "download" => Self::Download,
            "invoke" => Self::Invoke,
            "purchase" => Self::Purchase,
            "quote" => Self::Quote,
            "reserve" => Self::Reserve,
            other => Self::Other(other.to_owned()),
        })
    }
}

impl Serialize for ActionRelation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Download => serializer.serialize_str("download"),
            Self::Invoke => serializer.serialize_str("invoke"),
            Self::Purchase => serializer.serialize_str("purchase"),
            Self::Quote => serializer.serialize_str("quote"),
            Self::Reserve => serializer.serialize_str("reserve"),
            Self::Other(value) => serializer.serialize_str(value),
        }
    }
}
string_enum!(ResourceType { Collection => "collection", Offering => "offering" });
string_enum!(Operation {
    GetCollection => "get-collection",
    GetOffering => "get-offering",
    ListCollectionOfferings => "list-collection-offerings",
    ListCollections => "list-collections",
    ListOfferings => "list-offerings",
    SearchCollections => "search-collections",
    SearchOfferings => "search-offerings",
});
string_enum!(AuthenticationRequirement {
    NotRequired => "not-required",
    Optional => "optional",
    Required => "required",
});
string_enum!(Protocol { Aep => "aep", Mpp => "mpp", Tap => "tap", X402 => "x402" });
string_enum!(PaymentOption {
    Algorand => "algorand",
    Aptos => "aptos",
    Arbitrum => "arbitrum",
    Avalanche => "avalanche",
    Base => "base",
    Card => "card",
    Ethereum => "ethereum",
    Hedera => "hedera",
    Inflow => "inflow",
    Lightning => "lightning",
    Polygon => "polygon",
    Solana => "solana",
    Stellar => "stellar",
    Stripe => "stripe",
    Tempo => "tempo",
    Ton => "ton",
});
string_enum!(FilterType {
    Boolean => "boolean",
    Date => "date",
    DateTime => "date-time",
    Decimal => "decimal",
    Integer => "integer",
    Number => "number",
    String => "string",
});
string_enum!(FilterOperator {
    Equal => "eq",
    Exists => "exists",
    GreaterThan => "gt",
    GreaterThanOrEqual => "gte",
    In => "in",
    LessThan => "lt",
    LessThanOrEqual => "lte",
});
string_enum!(SortDirection { Ascending => "ascending", Descending => "descending" });
string_enum!(MissingPlacement { First => "first", Last => "last" });
string_enum!(ServiceBrandingImageType {
    Png => "image/png",
    Svg => "image/svg+xml",
    WebP => "image/webp",
});
string_enum!(ResourceImageType {
    Avif => "image/avif",
    Jpeg => "image/jpeg",
    Png => "image/png",
    Svg => "image/svg+xml",
    WebP => "image/webp",
});
string_enum!(McpEndpointType { StreamableHttp => "streamable-http" });

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct OperationDescriptor {
    pub authentication: AuthenticationRequirement,
    pub name: Operation,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ServiceProtocols {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enrollment: Vec<EnrollmentProtocol>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub payments: Vec<PaymentProtocol>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trust: Vec<TrustProtocol>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EnrollmentProtocol {
    pub name: Protocol,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PaymentProtocol {
    pub authentication: AuthenticationRequirement,
    pub name: Protocol,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<PaymentOption>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TrustProtocol {
    pub name: Protocol,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct HttpConfiguration {
    pub endpoint_base: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openapi: Option<ServiceOpenApi>,
    #[serde(flatten)]
    pub additional: AdditionalMembers,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CapabilityLink {
    pub href: String,
    #[serde(flatten)]
    pub additional: AdditionalMembers,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FilterUnit {
    pub code: String,
    pub system: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(flatten)]
    pub additional: AdditionalMembers,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FilterDefinition {
    pub description: String,
    pub id: String,
    pub operators: Vec<FilterOperator>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub refinable: bool,
    pub title: String,
    #[serde(rename = "type")]
    pub filter_type: FilterType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<FilterUnit>,
    #[serde(flatten)]
    pub additional: AdditionalMembers,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SortKey {
    pub direction: SortDirection,
    pub filter_id: String,
    pub missing: MissingPlacement,
    #[serde(flatten)]
    pub additional: AdditionalMembers,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SortDefinition {
    pub description: String,
    pub id: String,
    pub keys: Vec<SortKey>,
    pub title: String,
    #[serde(flatten)]
    pub additional: AdditionalMembers,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct FilterCapabilitySource {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inline: Vec<FilterDefinition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linked: Option<CapabilityLink>,
    #[serde(flatten)]
    pub additional: AdditionalMembers,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct SortCapabilitySource {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inline: Vec<SortDefinition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linked: Option<CapabilityLink>,
    #[serde(flatten)]
    pub additional: AdditionalMembers,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct SearchCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filters: Option<FilterCapabilitySource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sorts: Option<SortCapabilitySource>,
    #[serde(flatten)]
    pub additional: AdditionalMembers,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ServiceBrandingImage {
    pub src: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub media_type: Option<ServiceBrandingImageType>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ServiceBranding {
    pub icon: ServiceBrandingImage,
    pub logo: ServiceBrandingImage,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ServiceOpenApi {
    pub url: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct McpEndpoint {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(rename = "type")]
    pub endpoint_type: McpEndpointType,
    pub url: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ServiceDocument {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branding: Option<ServiceBranding>,
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub documentation_url: String,
    pub http: HttpConfiguration,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    pub language: String,
    pub localizations: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp: Vec<McpEndpoint>,
    pub name: String,
    pub odp_version: String,
    pub operations: Vec<OperationDescriptor>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub payment_origins: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocols: Option<ServiceProtocols>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_capabilities: Option<SearchCapabilities>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub status_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub support_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub website_url: String,
    #[serde(flatten)]
    pub additional: AdditionalMembers,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ResourceImage {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub alt: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub height: u32,
    pub src: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub media_type: Option<ResourceImageType>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub width: u32,
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Collection {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auth_expands: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detail_fields: Vec<String>,
    pub id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ResourceImage>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub language: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub localizations: Vec<String>,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub odp_version: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parent_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_capabilities: Option<SearchCapabilities>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub web_url: String,
    #[serde(flatten)]
    pub additional: AdditionalMembers,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SchemaReference {
    pub url: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PricePreview {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub amount: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub currency: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub maximum: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub minimum: String,
    #[serde(rename = "type")]
    pub price_type: PriceType,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub unit: String,
    #[serde(flatten)]
    pub additional: AdditionalMembers,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ActionRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<SchemaReference>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct HttpActionTarget {
    pub href: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<ActionRequest>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub response_content_types: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct OpenApiActionTarget {
    pub operation_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Action {
    pub authentication: AuthenticationRequirement,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http: Option<HttpActionTarget>,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openapi: Option<OpenApiActionTarget>,
    pub rel: ActionRelation,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Offering {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<Action>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auth_expands: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collection_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detail_fields: Vec<String>,
    pub id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ResourceImage>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub language: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub localizations: Vec<String>,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub odp_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<PricePreview>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<SchemaReference>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub web_url: String,
    #[serde(flatten)]
    pub additional: AdditionalMembers,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct InvalidParameter {
    #[serde(rename = "in")]
    pub location: String,
    pub name: String,
    pub reason: String,
    #[serde(flatten)]
    pub additional: AdditionalMembers,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ProblemDetails {
    pub code: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub instance: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invalid_params: Vec<InvalidParameter>,
    pub status: u16,
    pub title: String,
    #[serde(rename = "type")]
    pub problem_type: String,
    #[serde(flatten)]
    pub additional: AdditionalMembers,
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct ResourceIdentity {
    pub id: String,
    pub service: String,
    #[serde(rename = "type")]
    pub resource_type: ResourceType,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct CollectionSearchRequest {
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub limit: usize,
    pub odp_version: String,
    /// COL-06: an omitted `parent_id` applies no hierarchy constraint, while a JSON `null` asks
    /// for root Collections. `None` is the first, `Some(None)` the second.
    #[serde(
        default,
        deserialize_with = "explicit_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub parent_id: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub query: String,
    #[serde(flatten)]
    pub additional: AdditionalMembers,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct OfferingSearchRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub collection_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<FilterExpression>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub include_descendants: bool,
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub limit: usize,
    pub odp_version: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub query: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refinements: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sort: String,
    #[serde(flatten)]
    pub additional: AdditionalMembers,
}

fn is_zero_usize(value: &usize) -> bool {
    *value == 0
}

/// Reads a member that was present as `Some`, even when what it carried was `null`.
///
/// `Option<Option<T>>` on its own cannot tell the two apart: serde reads a JSON `null` straight
/// into the outer `None`, the same answer an absent member gives. Only the inner `Option` is read
/// here, so `#[serde(default)]` supplies `None` when the member is absent and this supplies
/// `Some(None)` when it is present and null.
fn explicit_null<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FilterExpression {
    pub id: String,
    pub operator: FilterOperator,
    pub value: Value,
    #[serde(flatten)]
    pub additional: AdditionalMembers,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RefinementBucket {
    pub count: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub count_relation: String,
    pub value: Value,
    #[serde(flatten)]
    pub additional: AdditionalMembers,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RefinementGroup {
    pub filter_id: String,
    pub values: Vec<RefinementBucket>,
    #[serde(flatten)]
    pub additional: AdditionalMembers,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Page<T> {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auth_expands: bool,
    pub items: Vec<T>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub next: String,
    pub odp_version: String,
    #[serde(flatten)]
    pub additional: AdditionalMembers,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct OfferingPage<T> {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auth_expands: bool,
    pub items: Vec<T>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub next: String,
    pub odp_version: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refinements: Vec<RefinementGroup>,
    #[serde(flatten)]
    pub additional: AdditionalMembers,
}
