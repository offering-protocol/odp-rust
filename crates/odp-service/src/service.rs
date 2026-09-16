use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use odp_core::{
    AdditionalMembers, AuthenticationRequirement, Collection, CollectionSearchRequest,
    EnrollmentProtocol, HttpConfiguration, McpEndpoint, Offering, OfferingPage,
    OfferingSearchRequest, Operation, OperationDescriptor, Page, PaymentProtocol, ProblemDetails,
    Representation, SearchCapabilities, ServiceBranding, ServiceDocument, ServiceOpenApi,
    ServiceProtocols, TrustProtocol, VERSION, is_local_resource_identifier, parse_collection,
    parse_collection_search_request, parse_offering, parse_offering_search_request,
    parse_offering_search_response, parse_page, parse_service_document,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use thiserror::Error;
use url::form_urlencoded;

pub const MEDIA_TYPE: &str = "application/odp+json";
pub const PROBLEM_MEDIA_TYPE: &str = "application/problem+json";
const MAXIMUM_REQUEST_BYTES: usize = 65_536;
const MAXIMUM_RESOURCE_BYTES: usize = 524_288;
/// ERR-04: the bounds a Problem Details object's own schema puts on what it says.
const MAXIMUM_TITLE_CHARACTERS: usize = 128;
const MAXIMUM_DETAIL_CHARACTERS: usize = 2_048;
/// ERR-32: a Service that turns a request away has to say when to come back.
const DEFAULT_RETRY_AFTER: u64 = 1;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Request {
    pub body: Vec<u8>,
    pub headers: BTreeMap<String, String>,
    pub method: String,
    pub path: String,
    pub query: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Response {
    pub body: Vec<u8>,
    pub headers: BTreeMap<String, String>,
    pub status: u16,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CatalogRequest {
    /// The `Accept-Language` field as the Agent sent it.
    pub accept_language: Option<String>,
    pub cursor: Option<String>,
    /// SVC-58: the localization the Service selected by RFC 4647 Lookup, which is what the
    /// catalog should answer in. Empty only when the Service advertises no localizations.
    pub language: String,
    pub limit: usize,
    pub path: String,
    pub representation: Representation,
}

impl Default for CatalogRequest {
    fn default() -> Self {
        Self {
            accept_language: None,
            cursor: None,
            language: String::new(),
            limit: 0,
            path: String::new(),
            representation: Representation::Terse,
        }
    }
}

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("invalid Service configuration: {0}")]
    InvalidConfiguration(String),
    #[error("invalid Service response: {0}")]
    InvalidResponse(String),
    #[error("catalog operation failed: {0}")]
    Catalog(String),
    #[error("{message}")]
    Request {
        code: &'static str,
        message: String,
        /// ERR-32 and ERR-33: how many seconds an Agent should wait. A `429` that names none
        /// is given a default, because the field is required there.
        retry_after: Option<u64>,
        status: u16,
    },
}

impl ServiceError {
    /// A failure an Agent caused, described the way ODP describes one.
    #[must_use]
    pub fn request(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self::Request {
            code,
            message: message.into(),
            retry_after: None,
            status,
        }
    }

    /// A failure an Agent should retry after `seconds`.
    #[must_use]
    pub fn retry_after(
        status: u16,
        code: &'static str,
        message: impl Into<String>,
        seconds: u64,
    ) -> Self {
        Self::Request {
            code,
            message: message.into(),
            retry_after: Some(seconds),
            status,
        }
    }
}

#[async_trait]
pub trait Catalog: Send + Sync {
    fn operations(&self) -> Vec<Operation>;

    async fn list_offerings(
        &self,
        request: CatalogRequest,
    ) -> Result<OfferingPage<Offering>, ServiceError>;

    async fn get_offering(
        &self,
        id: &str,
        request: CatalogRequest,
    ) -> Result<Option<Offering>, ServiceError>;

    async fn search_offerings(
        &self,
        _query: OfferingSearchRequest,
        _request: CatalogRequest,
    ) -> Result<OfferingPage<Offering>, ServiceError> {
        Err(ServiceError::Catalog(
            "search-offerings is unsupported".to_owned(),
        ))
    }

    async fn list_collections(
        &self,
        _request: CatalogRequest,
    ) -> Result<Page<Collection>, ServiceError> {
        Err(ServiceError::Catalog(
            "list-collections is unsupported".to_owned(),
        ))
    }

    async fn get_collection(
        &self,
        _id: &str,
        _request: CatalogRequest,
    ) -> Result<Option<Collection>, ServiceError> {
        Err(ServiceError::Catalog(
            "get-collection is unsupported".to_owned(),
        ))
    }

    async fn search_collections(
        &self,
        _query: CollectionSearchRequest,
        _request: CatalogRequest,
    ) -> Result<Page<Collection>, ServiceError> {
        Err(ServiceError::Catalog(
            "search-collections is unsupported".to_owned(),
        ))
    }

    async fn list_collection_offerings(
        &self,
        _collection_id: &str,
        _request: CatalogRequest,
    ) -> Result<OfferingPage<Offering>, ServiceError> {
        Err(ServiceError::Catalog(
            "list-collection-offerings is unsupported".to_owned(),
        ))
    }
}

pub struct ServiceBuilder {
    document: ServiceDocument,
}

impl ServiceBuilder {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        language: impl Into<String>,
        endpoint_base: impl Into<String>,
    ) -> Self {
        let language = language.into();
        Self {
            document: ServiceDocument {
                additional: AdditionalMembers::new(),
                branding: None,
                description: description.into(),
                documentation_url: String::new(),
                http: HttpConfiguration {
                    additional: AdditionalMembers::new(),
                    endpoint_base: endpoint_base.into(),
                    openapi: None,
                },
                keywords: Vec::new(),
                language: language.clone(),
                localizations: vec![language],
                mcp: Vec::new(),
                name: name.into(),
                odp_version: VERSION.to_owned(),
                operations: Vec::new(),
                payment_origins: Vec::new(),
                protocols: None,
                search_capabilities: None,
                status_url: String::new(),
                support_url: String::new(),
                website_url: String::new(),
            },
        }
    }

    pub fn branding(mut self, branding: ServiceBranding) -> Self {
        self.document.branding = Some(branding);
        self
    }

    pub fn documentation_url(mut self, url: impl Into<String>) -> Self {
        self.document.documentation_url = url.into();
        self
    }

    pub fn keywords<I, S>(mut self, keywords: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.document.keywords = keywords.into_iter().map(Into::into).collect();
        self
    }

    pub fn localizations<I, S>(mut self, localizations: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.document.localizations = localizations.into_iter().map(Into::into).collect();
        self
    }

    pub fn mcp(mut self, endpoints: Vec<McpEndpoint>) -> Self {
        self.document.mcp = endpoints;
        self
    }

    pub fn openapi(mut self, openapi: ServiceOpenApi) -> Self {
        self.document.http.openapi = Some(openapi);
        self
    }

    pub fn operation_authentication(
        mut self,
        operation: Operation,
        authentication: AuthenticationRequirement,
    ) -> Self {
        if let Some(descriptor) = self
            .document
            .operations
            .iter_mut()
            .find(|descriptor| descriptor.name == operation)
        {
            descriptor.authentication = authentication;
        } else {
            self.document.operations.push(OperationDescriptor {
                authentication,
                name: operation,
            });
        }
        self
    }

    pub fn payment_origins<I, S>(mut self, origins: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.document.payment_origins = origins.into_iter().map(Into::into).collect();
        self
    }

    pub fn protocols(
        mut self,
        enrollment: Vec<EnrollmentProtocol>,
        payments: Vec<PaymentProtocol>,
    ) -> Self {
        self.document.protocols = Some(ServiceProtocols {
            enrollment,
            payments,
            trust: Vec::new(),
        });
        self
    }

    pub fn trust(mut self, protocols: Vec<TrustProtocol>) -> Self {
        self.document
            .protocols
            .get_or_insert_with(ServiceProtocols::default)
            .trust = protocols;
        self
    }

    pub fn search_capabilities(mut self, capabilities: SearchCapabilities) -> Self {
        self.document.search_capabilities = Some(capabilities);
        self
    }

    pub fn status_url(mut self, url: impl Into<String>) -> Self {
        self.document.status_url = url.into();
        self
    }

    pub fn support_url(mut self, url: impl Into<String>) -> Self {
        self.document.support_url = url.into();
        self
    }

    pub fn website_url(mut self, url: impl Into<String>) -> Self {
        self.document.website_url = url.into();
        self
    }

    pub fn build(self, catalog: Arc<dyn Catalog>) -> Result<Service, ServiceError> {
        Service::new(self.document, catalog)
    }
}

pub struct Service {
    catalog: Arc<dyn Catalog>,
    document: ServiceDocument,
    endpoint_base: String,
}

impl Service {
    pub fn new(
        mut document: ServiceDocument,
        catalog: Arc<dyn Catalog>,
    ) -> Result<Self, ServiceError> {
        let operations = catalog.operations();
        if !operations.contains(&Operation::ListOfferings)
            || !operations.contains(&Operation::GetOffering)
        {
            return Err(ServiceError::InvalidConfiguration(
                "Catalog must support list-offerings and get-offering".to_owned(),
            ));
        }
        document.odp_version = VERSION.to_owned();
        document.operations = operations
            .into_iter()
            .map(|name| OperationDescriptor {
                authentication: document
                    .operations
                    .iter()
                    .find(|descriptor| descriptor.name == name)
                    .map(|descriptor| descriptor.authentication)
                    .unwrap_or(AuthenticationRequirement::NotRequired),
                name,
            })
            .collect();
        let encoded = serde_json::to_vec(&document)
            .map_err(|error| ServiceError::InvalidConfiguration(error.to_string()))?;
        document = parse_service_document(&encoded)
            .map_err(|error| ServiceError::InvalidConfiguration(error.to_string()))?;
        // SVC-83 and SVC-84: a Service that cannot serve its own Service Document within the
        // limit is misconfigured, and says so now rather than failing every request later.
        if serde_json::to_vec(&document)
            .map_err(|error| ServiceError::InvalidConfiguration(error.to_string()))?
            .len()
            > MAXIMUM_REQUEST_BYTES
        {
            return Err(ServiceError::InvalidConfiguration(
                "Service Document exceeds 65536 bytes".to_owned(),
            ));
        }
        let endpoint_base = document.http.endpoint_base.trim_end_matches('/').to_owned();
        Ok(Self {
            catalog,
            document,
            endpoint_base,
        })
    }

    pub fn document(&self) -> ServiceDocument {
        self.document.clone()
    }

    pub async fn handle(&self, request: Request) -> Response {
        // A HEAD is the GET it shadows, answered without the body (RFC 9110). Deciding it here
        // means every resource that answers GET answers HEAD, with the same headers.
        let head = request.method.eq_ignore_ascii_case("HEAD");
        let conditional = request.headers.get("if-none-match").cloned();
        let mut request = request;
        if head {
            request.method = "GET".to_owned();
        }
        let mut response = match self.handle_result(request).await {
            Ok(response) => response,
            Err(ServiceError::Request {
                code,
                message,
                retry_after,
                status,
            }) => problem_with_retry(status, code, &message, retry_after),
            Err(error) => problem(500, "INTERNAL_ERROR", &error.to_string()),
        };
        // PAG-31 and SVC-61: an entity tag distinguishes variants, and a validator that still
        // matches means the Agent already holds this representation.
        if response.status == 200 {
            if let Some(etag) = response.headers.get("etag").cloned() {
                if conditional.is_some_and(|value| matches_etag(&value, &etag)) {
                    response.body.clear();
                    response.status = 304;
                    response.headers.remove("content-type");
                }
            }
        }
        if head {
            response
                .headers
                .insert("content-length".to_owned(), response.body.len().to_string());
            response.body.clear();
        }
        response
    }

    async fn handle_result(&self, request: Request) -> Result<Response, ServiceError> {
        if !accepts_odp(request.headers.get("accept").map(String::as_str)) {
            return Ok(problem(
                406,
                "NOT_ACCEPTABLE",
                "Accept must allow application/odp+json",
            ));
        }
        // SVC-58 and SVC-59: Lookup against what the Service actually publishes, and fall back to
        // the default representation rather than refusing the request.
        let language = self.select_language(request.headers.get("accept-language"));
        if request.path == "/.well-known/odp" {
            if request.method != "GET" {
                return Ok(method_not_allowed("GET, HEAD"));
            }
            return self.represent(200, &self.document, MAXIMUM_REQUEST_BYTES, &language);
        }
        let Some(path) = request.path.strip_prefix(&self.endpoint_base) else {
            return Ok(problem(404, "NOT_FOUND", "ODP resource not found"));
        };
        if let Some(operation) = path_operation(request.method.as_str(), path) {
            if !self
                .document
                .operations
                .iter()
                .any(|descriptor| descriptor.name == operation)
            {
                return Ok(problem(404, "NOT_FOUND", "ODP operation is not supported"));
            }
        }
        let input = catalog_request(&request, language.clone())?;
        let limit = input.limit;
        match (request.method.as_str(), path) {
            ("GET", "/offerings") => {
                let representation = input.representation;
                let page = self.catalog.list_offerings(input).await?;
                validate_offering_page(&page, false, representation, limit, &request)?;
                self.represent(200, &page, MAXIMUM_RESOURCE_BYTES, &language)
            }
            ("POST", "/offerings/search") => {
                let query = decode_offering_search(&request)?;
                let representation = input.representation;
                let page = self.catalog.search_offerings(query, input).await?;
                validate_offering_page(&page, true, representation, limit, &request)?;
                self.represent(200, &page, MAXIMUM_RESOURCE_BYTES, &language)
            }
            ("GET", "/collections") => {
                let representation = input.representation;
                let page = self.catalog.list_collections(input).await?;
                validate_collection_page(&page, representation, limit, &request)?;
                self.represent(200, &page, MAXIMUM_RESOURCE_BYTES, &language)
            }
            ("POST", "/collections/search") => {
                let query = decode_collection_search(&request)?;
                let representation = input.representation;
                let page = self.catalog.search_collections(query, input).await?;
                validate_collection_page(&page, representation, limit, &request)?;
                self.represent(200, &page, MAXIMUM_RESOURCE_BYTES, &language)
            }
            // A reserved path exists, but only for the method its operation uses.
            (_, value) if RESERVED_PATHS.contains(&value) => {
                Ok(method_not_allowed(allowed_methods(value)))
            }
            ("GET", _) => self.get_path(path, input, &request, &language).await,
            (_, value) => Ok(method_not_allowed(allowed_methods(value))),
        }
    }

    async fn get_path(
        &self,
        path: &str,
        input: CatalogRequest,
        request: &Request,
        language: &str,
    ) -> Result<Response, ServiceError> {
        if let Some(id) = path.strip_prefix("/offerings/") {
            if !is_local_resource_identifier(id) {
                return Ok(problem(
                    400,
                    "INVALID_REQUEST",
                    "Offering identifier is invalid",
                ));
            }
            let representation = input.representation;
            return match self.catalog.get_offering(id, input).await? {
                Some(offering) if offering.id == id => {
                    let encoded = serde_json::to_vec(&offering)
                        .map_err(|error| ServiceError::InvalidResponse(error.to_string()))?;
                    parse_offering(&encoded)
                        .map_err(|error| ServiceError::InvalidResponse(error.to_string()))?;
                    validate_offering_representation(&offering, representation)?;
                    self.represent(200, &offering, MAXIMUM_RESOURCE_BYTES, language)
                }
                Some(_) => Err(ServiceError::InvalidResponse(
                    "Offering identifier does not match request path".to_owned(),
                )),
                None => Ok(problem(404, "NOT_FOUND", "Offering not found")),
            };
        }
        if let Some(value) = path.strip_prefix("/collections/") {
            let limit = input.limit;
            if let Some(id) = value.strip_suffix("/offerings") {
                // IDN-08: a Collection identifier is checked the way an Offering identifier is,
                // so a path that could never name a resource never reaches the catalog.
                if !is_local_resource_identifier(id) {
                    return Ok(problem(
                        400,
                        "INVALID_REQUEST",
                        "Collection identifier is invalid",
                    ));
                }
                let representation = input.representation;
                let page = self.catalog.list_collection_offerings(id, input).await?;
                validate_offering_page(&page, false, representation, limit, request)?;
                return self.represent(200, &page, MAXIMUM_RESOURCE_BYTES, language);
            }
            if !is_local_resource_identifier(value) {
                return Ok(problem(
                    400,
                    "INVALID_REQUEST",
                    "Collection identifier is invalid",
                ));
            }
            let representation = input.representation;
            return match self.catalog.get_collection(value, input).await? {
                Some(collection) if collection.id == value => {
                    let encoded = serde_json::to_vec(&collection)
                        .map_err(|error| ServiceError::InvalidResponse(error.to_string()))?;
                    parse_collection(&encoded)
                        .map_err(|error| ServiceError::InvalidResponse(error.to_string()))?;
                    validate_collection_representation(&collection, representation)?;
                    self.represent(200, &collection, MAXIMUM_RESOURCE_BYTES, language)
                }
                Some(_) => Err(ServiceError::InvalidResponse(
                    "Collection identifier does not match request path".to_owned(),
                )),
                None => Ok(problem(404, "NOT_FOUND", "Collection not found")),
            };
        }
        Ok(problem(404, "NOT_FOUND", "ODP resource not found"))
    }

    /// SVC-58: RFC 4647 Lookup against the localizations the Service Document advertises.
    ///
    /// Lookup walks a range down its own subtags and never sideways, so `de-CH-1901` finds
    /// `de-CH` and then `de`, but never `de-DE`.
    fn select_language(&self, accept_language: Option<&String>) -> String {
        let default = self.document.language.clone();
        let Some(header) = accept_language else {
            return default;
        };
        for range in ranges_by_weight(header) {
            if range == "*" {
                return default;
            }
            let mut candidate = range.as_str();
            loop {
                if let Some(found) = self
                    .document
                    .localizations
                    .iter()
                    .find(|value| value.eq_ignore_ascii_case(candidate))
                {
                    return found.clone();
                }
                let Some(shorter) = candidate.rsplit_once('-') else {
                    break;
                };
                candidate = shorter.0;
                // RFC 4647: a single-character subtag introduces an extension rather than naming
                // a language, so truncating to it would leave a range no tag can match.
                if let Some((prefix, subtag)) = candidate.rsplit_once('-') {
                    if subtag.len() == 1 {
                        candidate = prefix;
                    }
                }
            }
        }
        // SVC-59: nothing matched, so the default representation is returned, not a 406.
        default
    }

    /// Serializes an ODP representation with the fields that describe the variant it is.
    fn represent<T: serde::Serialize>(
        &self,
        status: u16,
        value: &T,
        maximum_bytes: usize,
        language: &str,
    ) -> Result<Response, ServiceError> {
        let mut response = json_response(status, value, maximum_bytes)?;
        if !language.is_empty() {
            // SVC-60: say which variant this is, and that the choice depends on the request.
            response
                .headers
                .insert("content-language".to_owned(), language.to_owned());
            response
                .headers
                .insert("vary".to_owned(), "Accept-Language".to_owned());
        }
        // SVC-61: the tag covers the language too, so two variants never share one.
        response
            .headers
            .insert("etag".to_owned(), entity_tag(&response.body, language));
        Ok(response)
    }
}

fn catalog_request(request: &Request, language: String) -> Result<CatalogRequest, ServiceError> {
    let mut parameters: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, value) in form_urlencoded::parse(request.query.as_bytes()).into_owned() {
        parameters.entry(name).or_default().push(value);
    }
    // SVC-73: a repeated `representation` describes two requests, and the Service answers
    // neither. The same reasoning applies to the other single-valued parameters.
    for name in ["representation", "limit", "cursor"] {
        if parameters.get(name).is_some_and(|values| values.len() > 1) {
            return Err(request_error(
                400,
                "INVALID_REQUEST",
                &format!("{name} must not be repeated"),
            ));
        }
    }
    let single = |name: &str| parameters.get(name).and_then(|values| values.first());
    let representation = match single("representation").map(String::as_str) {
        None | Some("terse") => Representation::Terse,
        Some("full") => Representation::Full,
        Some(_) => {
            return Err(request_error(
                400,
                "INVALID_REQUEST",
                "representation is invalid",
            ));
        }
    };
    let limit = single("limit")
        .map(|value| value.parse::<usize>())
        .transpose()
        .map_err(|_| request_error(400, "INVALID_REQUEST", "limit is invalid"))?
        .unwrap_or(0);
    if limit > 100 {
        return Err(request_error(400, "INVALID_REQUEST", "limit exceeds 100"));
    }
    Ok(CatalogRequest {
        accept_language: request.headers.get("accept-language").cloned(),
        cursor: single("cursor").cloned(),
        language,
        limit,
        path: request.path.clone(),
        representation,
    })
}

/// The language ranges of an `Accept-Language` field, most preferred first.
///
/// A range weighted zero is not acceptable at all, so it is dropped rather than ordered last.
fn ranges_by_weight(header: &str) -> Vec<String> {
    let mut ranges = header
        .split(',')
        .filter_map(|entry| {
            let mut parts = entry.split(';');
            let range = parts.next().unwrap_or_default().trim();
            if range.is_empty() {
                return None;
            }
            let weight = quality(parts);
            (weight > 0.0).then(|| (range.to_owned(), weight))
        })
        .collect::<Vec<_>>();
    // A stable sort keeps equally weighted ranges in the order the Agent wrote them.
    ranges.sort_by(|left, right| right.1.total_cmp(&left.1));
    ranges.into_iter().map(|(range, _)| range).collect()
}

/// The `q` parameter of one media range or language range, defaulting to 1.
fn quality<'a>(parameters: impl Iterator<Item = &'a str>) -> f32 {
    for parameter in parameters {
        let Some((name, value)) = parameter.split_once('=') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("q") {
            return value.trim().parse::<f32>().unwrap_or(0.0);
        }
    }
    1.0
}

fn validate_search_request(request: &Request) -> Result<(), ServiceError> {
    if request.body.len() > MAXIMUM_REQUEST_BYTES {
        return Err(request_error(
            413,
            "REQUEST_TOO_LARGE",
            "request body is too large",
        ));
    }
    // MED-06: a media type is compared by its essence, which is case-insensitive and carries no
    // surrounding whitespace (RFC 9110).
    let essence = request
        .headers
        .get("content-type")
        .map(|value| value.split(';').next().unwrap_or_default().trim())
        .unwrap_or_default();
    if !essence.eq_ignore_ascii_case(MEDIA_TYPE) {
        return Err(request_error(
            415,
            "UNSUPPORTED_MEDIA_TYPE",
            &format!("Content-Type must be {MEDIA_TYPE}"),
        ));
    }
    Ok(())
}

fn decode_offering_search(request: &Request) -> Result<OfferingSearchRequest, ServiceError> {
    validate_search_request(request)?;
    parse_offering_search_request(&request.body)
        .map_err(|error| request_error(400, "INVALID_REQUEST", &error.to_string()))
}

fn decode_collection_search(request: &Request) -> Result<CollectionSearchRequest, ServiceError> {
    validate_search_request(request)?;
    parse_collection_search_request(&request.body)
        .map_err(|error| request_error(400, "INVALID_REQUEST", &error.to_string()))
}

fn request_error(status: u16, code: &'static str, message: &str) -> ServiceError {
    ServiceError::request(status, code, message)
}

/// PAG-11 and PAG-14: what a page owes the request that produced it.
fn validate_page_envelope(
    next: &str,
    items: usize,
    limit: usize,
    request: &Request,
) -> Result<(), ServiceError> {
    // PAG-14 lets a Service answer with fewer items than asked for, never with more.
    if limit != 0 && items > limit {
        return Err(ServiceError::InvalidResponse(
            "Catalog returned more items than the request allows".to_owned(),
        ));
    }
    if next.is_empty() {
        return Ok(());
    }
    // PAG-11: a continuation that points at the request it answers never advances.
    let current = if request.query.is_empty() {
        request.path.clone()
    } else {
        format!("{}?{}", request.path, request.query)
    };
    if next == current {
        return Err(ServiceError::InvalidResponse(
            "Catalog returned the request URL as its own continuation".to_owned(),
        ));
    }
    Ok(())
}

fn validate_offering_page(
    page: &OfferingPage<Offering>,
    search_response: bool,
    representation: Representation,
    limit: usize,
    request: &Request,
) -> Result<(), ServiceError> {
    validate_page_envelope(&page.next, page.items.len(), limit, request)?;
    let encoded = serde_json::to_vec(page)
        .map_err(|error| ServiceError::InvalidResponse(error.to_string()))?;
    if search_response {
        parse_offering_search_response(&encoded)
            .map_err(|error| ServiceError::InvalidResponse(error.to_string()))?;
    } else {
        parse_page::<serde_json::Value>(&encoded)
            .map_err(|error| ServiceError::InvalidResponse(error.to_string()))?;
    }
    for offering in &page.items {
        let mut inherited = offering.clone();
        if inherited.odp_version.is_empty() {
            inherited.odp_version.clone_from(&page.odp_version);
        }
        let encoded = serde_json::to_vec(&inherited)
            .map_err(|error| ServiceError::InvalidResponse(error.to_string()))?;
        parse_offering(&encoded)
            .map_err(|error| ServiceError::InvalidResponse(error.to_string()))?;
        validate_offering_representation(offering, representation)?;
    }
    Ok(())
}

fn validate_collection_page(
    page: &Page<Collection>,
    representation: Representation,
    limit: usize,
    request: &Request,
) -> Result<(), ServiceError> {
    validate_page_envelope(&page.next, page.items.len(), limit, request)?;
    let encoded = serde_json::to_vec(page)
        .map_err(|error| ServiceError::InvalidResponse(error.to_string()))?;
    parse_page::<serde_json::Value>(&encoded)
        .map_err(|error| ServiceError::InvalidResponse(error.to_string()))?;
    for collection in &page.items {
        let mut inherited = collection.clone();
        if inherited.odp_version.is_empty() {
            inherited.odp_version.clone_from(&page.odp_version);
        }
        let encoded = serde_json::to_vec(&inherited)
            .map_err(|error| ServiceError::InvalidResponse(error.to_string()))?;
        parse_collection(&encoded)
            .map_err(|error| ServiceError::InvalidResponse(error.to_string()))?;
        validate_collection_representation(collection, representation)?;
    }
    Ok(())
}

fn validate_offering_representation(
    offering: &Offering,
    representation: Representation,
) -> Result<(), ServiceError> {
    if representation == Representation::Terse && !offering.actions.is_empty() {
        return Err(ServiceError::InvalidResponse(
            "Catalog returned Actions in a Terse Offering".to_owned(),
        ));
    }
    if representation == Representation::Full && !offering.detail_fields.is_empty() {
        return Err(ServiceError::InvalidResponse(
            "Catalog returned detail_fields in a Full Offering".to_owned(),
        ));
    }
    Ok(())
}

fn validate_collection_representation(
    collection: &Collection,
    representation: Representation,
) -> Result<(), ServiceError> {
    if representation == Representation::Full && !collection.detail_fields.is_empty() {
        return Err(ServiceError::InvalidResponse(
            "Catalog returned detail_fields in a Full Collection".to_owned(),
        ));
    }
    Ok(())
}

/// The two paths that name an operation rather than a resource.
///
/// `/offerings/search` is the search endpoint, so it is never read as an Offering called
/// `search`, however valid that identifier would otherwise be.
const RESERVED_PATHS: &[&str] = &["/offerings/search", "/collections/search"];

fn path_operation(method: &str, path: &str) -> Option<Operation> {
    match (method, path) {
        ("GET", "/offerings") => Some(Operation::ListOfferings),
        ("POST", "/offerings/search") => Some(Operation::SearchOfferings),
        ("GET", "/collections") => Some(Operation::ListCollections),
        ("POST", "/collections/search") => Some(Operation::SearchCollections),
        (_, value) if RESERVED_PATHS.contains(&value) => None,
        ("GET", value) if value.starts_with("/offerings/") => Some(Operation::GetOffering),
        ("GET", value) if value.starts_with("/collections/") && value.ends_with("/offerings") => {
            Some(Operation::ListCollectionOfferings)
        }
        ("GET", value) if value.starts_with("/collections/") => Some(Operation::GetCollection),
        _ => None,
    }
}

fn json_response<T: serde::Serialize>(
    status: u16,
    value: &T,
    maximum_bytes: usize,
) -> Result<Response, ServiceError> {
    let body = serde_json::to_vec(value)
        .map_err(|error| ServiceError::InvalidResponse(error.to_string()))?;
    if body.len() > maximum_bytes {
        return Err(ServiceError::InvalidResponse(
            "response body is too large".to_owned(),
        ));
    }
    Ok(Response {
        body,
        headers: BTreeMap::from([("content-type".to_owned(), MEDIA_TYPE.to_owned())]),
        status,
    })
}

fn problem(status: u16, code: &str, detail: &str) -> Response {
    problem_with_retry(status, code, detail, None)
}

fn problem_with_retry(status: u16, code: &str, detail: &str, retry_after: Option<u64>) -> Response {
    let value = ProblemDetails {
        additional: BTreeMap::new(),
        code: code.to_owned(),
        // ERR-04: a Problem Details object that breaks its own limits describes nothing.
        detail: bounded(detail, MAXIMUM_DETAIL_CHARACTERS),
        instance: String::new(),
        invalid_params: Vec::new(),
        problem_type: format!(
            "https://offeringprotocol.org/problems/{}",
            code.to_ascii_lowercase().replace('_', "-")
        ),
        status,
        title: bounded(detail, MAXIMUM_TITLE_CHARACTERS),
    };
    let mut headers = BTreeMap::from([("content-type".to_owned(), PROBLEM_MEDIA_TYPE.to_owned())]);
    // ERR-32 requires Retry-After on a 429 and ERR-33 asks for it on a 503, so a Service that
    // named no interval still tells the Agent to come back rather than leaving it to guess.
    if matches!(status, 429 | 503) {
        headers.insert(
            "retry-after".to_owned(),
            retry_after.unwrap_or(DEFAULT_RETRY_AFTER).to_string(),
        );
    } else if let Some(seconds) = retry_after {
        headers.insert("retry-after".to_owned(), seconds.to_string());
    }
    Response {
        body: serde_json::to_vec(&value)
            .unwrap_or_else(|_| json!({"status":500}).to_string().into_bytes()),
        headers,
        status,
    }
}

/// Keeps a message within a code-point bound, marking where it was cut.
fn bounded(value: &str, maximum: usize) -> String {
    if value.chars().count() <= maximum {
        return value.to_owned();
    }
    let mut kept = value.chars().take(maximum - 1).collect::<String>();
    kept.push('…');
    kept
}

/// A 405 states what the resource does allow (RFC 9110).
fn method_not_allowed(allow: &str) -> Response {
    let mut response = problem(
        405,
        "METHOD_NOT_ALLOWED",
        "ODP operation uses a fixed HTTP method",
    );
    response
        .headers
        .insert("allow".to_owned(), allow.to_owned());
    response
}

fn allowed_methods(path: &str) -> &'static str {
    match path {
        "/offerings/search" | "/collections/search" => "POST",
        _ => "GET, HEAD",
    }
}

/// MED-03 and MED-04: whether the request's `Accept` permits an ODP representation.
///
/// The most specific range that matches decides, and a range weighted zero rejects rather than
/// merely deprioritizes — so `application/odp+json;q=0` is a refusal, not a preference.
fn accepts_odp(value: Option<&str>) -> bool {
    let Some(value) = value else {
        return true;
    };
    if value.trim().is_empty() {
        return true;
    }
    let mut best: Option<(u8, f32)> = None;
    for entry in value.split(',') {
        let mut parts = entry.split(';');
        let media_range = parts.next().unwrap_or_default().trim();
        let specificity = if media_range.eq_ignore_ascii_case(MEDIA_TYPE) {
            2
        } else if media_range.eq_ignore_ascii_case("application/*") {
            1
        } else if media_range == "*/*" {
            0
        } else {
            continue;
        };
        let weight = quality(parts);
        if best.is_none_or(|(found, _)| specificity > found) {
            best = Some((specificity, weight));
        }
    }
    best.is_some_and(|(_, weight)| weight > 0.0)
}

/// SVC-61: a strong validator over the bytes served and the variant they represent.
fn entity_tag(body: &[u8], language: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(language.as_bytes());
    digest.update([0]);
    digest.update(body);
    let bytes = digest.finalize();
    let mut encoded = String::with_capacity(bytes.len() * 2 + 2);
    encoded.push('"');
    for byte in bytes {
        encoded.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        encoded.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
    }
    encoded.push('"');
    encoded
}

/// Whether an `If-None-Match` field lists this entity tag, weakly compared (RFC 9110).
fn matches_etag(header: &str, etag: &str) -> bool {
    let bare = |value: &str| {
        value
            .trim()
            .trim_start_matches("W/")
            .trim()
            .trim_matches('"')
            .to_owned()
    };
    let wanted = bare(etag);
    header
        .split(',')
        .any(|entry| entry.trim() == "*" || bare(entry) == wanted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use odp_core::{
        Action, ActionRelation, AdditionalMembers, HttpActionTarget, HttpConfiguration,
        parse_collection, parse_service_document,
    };

    struct TestCatalog;

    #[async_trait]
    impl Catalog for TestCatalog {
        fn operations(&self) -> Vec<Operation> {
            vec![Operation::GetOffering, Operation::ListOfferings]
        }

        async fn list_offerings(
            &self,
            _request: CatalogRequest,
        ) -> Result<OfferingPage<Offering>, ServiceError> {
            Ok(OfferingPage {
                additional: AdditionalMembers::new(),
                auth_expands: false,
                items: vec![offering()],
                next: String::new(),
                odp_version: VERSION.to_owned(),
                refinements: Vec::new(),
            })
        }

        async fn get_offering(
            &self,
            id: &str,
            _request: CatalogRequest,
        ) -> Result<Option<Offering>, ServiceError> {
            Ok((id == "plant-1").then(offering))
        }
    }

    fn offering() -> Offering {
        Offering {
            actions: Vec::new(),
            additional: AdditionalMembers::new(),
            attributes: BTreeMap::new(),
            auth_expands: false,
            collection_ids: Vec::new(),
            description: "A healthy plant".to_owned(),
            detail_fields: Vec::new(),
            id: "plant-1".to_owned(),
            images: Vec::new(),
            language: "en".to_owned(),
            localizations: vec!["en".to_owned()],
            name: "Plant".to_owned(),
            odp_version: VERSION.to_owned(),
            price: None,
            schema: None,
            web_url: String::new(),
        }
    }

    fn document() -> ServiceDocument {
        parse_service_document(br#"{"description":"Plants","http":{"endpoint_base":"/odp"},"language":"en","localizations":["en"],"name":"Indica Flowers","odp_version":"1.0","operations":[{"authentication":"not-required","name":"get-offering"},{"authentication":"not-required","name":"list-offerings"}]}"#).unwrap()
    }

    #[test]
    fn builder_derives_version_and_catalog_operations() {
        let service =
            ServiceBuilder::new("Indica Flowers", "An AI-enabled plant store.", "en", "/odp")
                .keywords(["plants", "flowers"])
                .operation_authentication(
                    Operation::GetOffering,
                    AuthenticationRequirement::Required,
                )
                .protocols(
                    vec![EnrollmentProtocol {
                        name: odp_core::Protocol::Aep,
                    }],
                    Vec::new(),
                )
                .trust(vec![TrustProtocol {
                    name: odp_core::Protocol::Tap,
                }])
                .build(Arc::new(TestCatalog))
                .unwrap();
        let document = service.document();
        assert_eq!(document.odp_version, VERSION);
        assert_eq!(document.localizations, ["en"]);
        assert_eq!(document.keywords, ["plants", "flowers"]);
        assert_eq!(document.operations.len(), 2);
        assert_eq!(
            document.protocols.as_ref().unwrap().trust,
            [TrustProtocol {
                name: odp_core::Protocol::Tap
            }]
        );
        assert_eq!(
            document
                .operations
                .iter()
                .find(|descriptor| descriptor.name == Operation::GetOffering)
                .unwrap()
                .authentication,
            AuthenticationRequirement::Required
        );
        assert_eq!(
            document
                .operations
                .iter()
                .find(|descriptor| descriptor.name == Operation::ListOfferings)
                .unwrap()
                .authentication,
            AuthenticationRequirement::NotRequired
        );
    }

    #[tokio::test]
    async fn serves_a_framework_neutral_offering_request() {
        let service = Service::new(document(), Arc::new(TestCatalog)).unwrap();
        let response = service
            .handle(Request {
                headers: BTreeMap::from([("accept".to_owned(), MEDIA_TYPE.to_owned())]),
                method: "GET".to_owned(),
                path: "/odp/offerings/plant-1".to_owned(),
                query: "representation=full".to_owned(),
                ..Request::default()
            })
            .await;
        assert_eq!(response.status, 200);
        assert_eq!(parse_offering(&response.body).unwrap().id, "plant-1");

        let listed = service
            .handle(Request {
                headers: BTreeMap::from([("accept".to_owned(), MEDIA_TYPE.to_owned())]),
                method: "GET".to_owned(),
                path: "/odp/offerings".to_owned(),
                ..Request::default()
            })
            .await;
        assert_eq!(listed.status, 200);
        assert_eq!(
            parse_page::<serde_json::Value>(&listed.body)
                .unwrap()
                .items
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn returns_problem_types_that_correspond_to_the_code() {
        let service = Service::new(document(), Arc::new(TestCatalog)).unwrap();
        let response = service
            .handle(Request {
                method: "GET".to_owned(),
                path: "/odp/missing".to_owned(),
                ..Request::default()
            })
            .await;
        let problem = odp_core::parse_problem_details(&response.body).unwrap();

        assert_eq!(problem.code, "NOT_FOUND");
        assert_eq!(
            problem.problem_type,
            "https://offeringprotocol.org/problems/not-found"
        );
    }

    #[test]
    fn model_http_configuration_remains_framework_neutral() {
        let configuration = HttpConfiguration {
            additional: AdditionalMembers::new(),
            endpoint_base: "/odp".to_owned(),
            openapi: None,
        };
        assert_eq!(configuration.endpoint_base, "/odp");
    }

    #[test]
    fn rejects_search_bodies_above_the_normative_limit() {
        let error = validate_search_request(&Request {
            body: vec![b' '; MAXIMUM_REQUEST_BYTES + 1],
            headers: BTreeMap::from([("content-type".to_owned(), MEDIA_TYPE.to_owned())]),
            method: "POST".to_owned(),
            path: "/odp/offerings/search".to_owned(),
            query: String::new(),
        })
        .unwrap_err();
        assert!(matches!(error, ServiceError::Request { status: 413, .. }));
    }

    #[test]
    fn rejects_invalid_page_envelopes() {
        let page = OfferingPage {
            additional: AdditionalMembers::new(),
            auth_expands: false,
            items: vec![offering()],
            next: String::new(),
            odp_version: String::new(),
            refinements: Vec::new(),
        };

        assert!(
            validate_offering_page(&page, false, Representation::Terse, 0, &Request::default())
                .is_err()
        );
    }

    #[test]
    fn rejects_representation_contract_violations() {
        let mut terse = offering();
        terse.actions.push(Action {
            authentication: AuthenticationRequirement::NotRequired,
            description: String::new(),
            http: Some(HttpActionTarget {
                href: "/purchase".to_owned(),
                method: "POST".to_owned(),
                request: None,
                response_content_types: Vec::new(),
            }),
            id: "purchase".to_owned(),
            openapi: None,
            rel: ActionRelation::Purchase,
        });
        assert!(validate_offering_representation(&terse, Representation::Terse).is_err());

        let mut full = offering();
        full.detail_fields.push("/description".to_owned());
        assert!(validate_offering_representation(&full, Representation::Full).is_err());

        let mut collection =
            parse_collection(br#"{"id":"plants","name":"Plants","odp_version":"1.0"}"#).unwrap();
        collection.detail_fields.push("/description".to_owned());
        assert!(validate_collection_representation(&collection, Representation::Full).is_err());
    }
}
