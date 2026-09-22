use std::{
    collections::{BTreeMap, BTreeSet},
    net::IpAddr,
    sync::Arc,
};

use odp_core::{Protocol, derive_service_origin, is_public, parse_agent_service_document};
use serde_json::{Value, json};
use thiserror::Error;
use url::{Host, Url};

use crate::{
    DirectoryService, Environment, HttpRequest, HttpResponse, IterationOptions,
    ResourceSearchRequest, SearchPage, SearchRequest, SearchResponse, ServiceIssue,
    SuggestionRequest, Transport, TransportError, default_transport,
};

const MAXIMUM_REDIRECTS: usize = 5;
const MAXIMUM_RESPONSE_BYTES: usize = 524_288;
/// ERR-21: a failure is described far more tightly than a result is returned.
const MAXIMUM_ERROR_BYTES: usize = 16_384;
/// How much of a failure body is worth repeating to a caller.
const MAXIMUM_ERROR_MESSAGE: usize = 2_048;
const MAXIMUM_ITEMS_PER_PAGE: usize = 100;
const MAXIMUM_FACET_ENTRIES: usize = 100;
const MAXIMUM_SUGGESTIONS: usize = 25;
const MAXIMUM_TRAVERSAL: usize = 10_000;
const MAXIMUM_REFERENCE_CHARACTERS: usize = 2_048;
const MAXIMUM_PAGES: usize = 16;

#[derive(Debug, Error)]
pub enum DirectoryError {
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error("invalid Directory request: {0}")]
    InvalidRequest(String),
    #[error("invalid Directory response: {0}")]
    InvalidResponse(String),
    #[error("Directory request failed with HTTP {status}: {message}")]
    Request {
        headers: BTreeMap<String, String>,
        message: String,
        status: u16,
    },
}

#[derive(Clone)]
pub struct DirectoryClient {
    environment: Environment,
    transport: Arc<dyn Transport>,
}

impl DirectoryClient {
    pub fn new(environment: Environment) -> Result<Self, DirectoryError> {
        Ok(Self {
            environment,
            transport: default_transport()?,
        })
    }

    pub fn with_transport(environment: Environment, transport: Arc<dyn Transport>) -> Self {
        Self {
            environment,
            transport,
        }
    }

    pub const fn environment(&self) -> Environment {
        self.environment
    }

    pub async fn search(
        &self,
        request: &ResourceSearchRequest,
    ) -> Result<SearchResponse, DirectoryError> {
        validate_search(&request.query, request.limit, request.filters.as_ref())?;
        if let Some(types) = &request.types {
            if types.is_empty() || types.len() > 2 || (types.len() == 2 && types[0] == types[1]) {
                return Err(DirectoryError::InvalidRequest(
                    "types must contain distinct service or collection values".to_owned(),
                ));
            }
        }
        let body = serde_json::to_vec(request)
            .map_err(|error| DirectoryError::InvalidRequest(error.to_string()))?;
        let target = self.continuation_url("/v1/directory/search")?;
        let response = self.request("POST", target, body).await?;
        crate::results::decode(&response.body)
    }

    pub async fn continue_search(&self, next: &str) -> Result<SearchResponse, DirectoryError> {
        let target = self.continuation_url(next)?;
        let response = self.request("GET", target, Vec::new()).await?;
        crate::results::decode(&response.body)
    }

    pub async fn search_services(
        &self,
        request: &SearchRequest,
    ) -> Result<SearchPage, DirectoryError> {
        validate_search_request(request)?;
        let body = serde_json::to_vec(request)
            .map_err(|error| DirectoryError::InvalidRequest(error.to_string()))?;
        self.request_page(
            "POST",
            &format!("{}/v1/services/search", self.environment.origin()),
            body,
        )
        .await
    }

    pub async fn continue_search_services(&self, next: &str) -> Result<SearchPage, DirectoryError> {
        let target = self.continuation_url(next)?;
        self.request_page("GET", target.as_str(), Vec::new()).await
    }

    /// Every page of a search, up to `max_pages`.
    ///
    /// The last page returned still carries its `next`, so a caller that wants to go further can
    /// resume from it: reaching the bound is not the same as reaching the end.
    pub async fn search_pages(
        &self,
        request: &SearchRequest,
        options: IterationOptions,
    ) -> Result<Vec<SearchPage>, DirectoryError> {
        self.traverse(request, options, usize::MAX)
            .await
            .map(|(pages, _)| pages)
    }

    pub async fn collect_services(
        &self,
        request: &SearchRequest,
        options: IterationOptions,
    ) -> Result<Vec<DirectoryService>, DirectoryError> {
        let maximum_items = bounded(options.max_items, MAXIMUM_TRAVERSAL, "max_items")?;
        let (pages, _) = self.traverse(request, options, maximum_items).await?;
        Ok(pages
            .into_iter()
            .flat_map(|page| page.items)
            .take(maximum_items)
            .collect())
    }

    /// Walks a search from its first page, stopping at whichever bound is reached first.
    ///
    /// A page is only fetched when something still wants it, so a caller asking for ten Services
    /// does not pay for a hundred, and a cursor that repeats is refused rather than followed.
    async fn traverse(
        &self,
        request: &SearchRequest,
        options: IterationOptions,
        maximum_items: usize,
    ) -> Result<(Vec<SearchPage>, usize), DirectoryError> {
        let maximum_pages = bounded(options.max_pages, MAXIMUM_PAGES, "max_pages")?;
        // Validated here as well, so `search_pages` refuses the same requests `search_services` does.
        bounded(options.max_items, MAXIMUM_TRAVERSAL, "max_items")?;
        let mut pages = Vec::new();
        let mut visited = BTreeSet::new();
        let mut items = 0_usize;
        let mut page = self.search_services(request).await?;
        loop {
            items += page.items.len();
            let next = page.next.clone();
            pages.push(page);
            if next.is_empty() || pages.len() >= maximum_pages || items >= maximum_items {
                return Ok((pages, items));
            }
            let target = self.continuation_url(&next)?;
            if !visited.insert(target.to_string()) {
                return Err(DirectoryError::InvalidResponse(
                    "Directory pagination loop detected".to_owned(),
                ));
            }
            page = self
                .request_page("GET", target.as_str(), Vec::new())
                .await?;
        }
    }

    pub async fn suggest(
        &self,
        request: &SuggestionRequest,
    ) -> Result<Vec<String>, DirectoryError> {
        self.suggestions("/v1/directory/suggestions", request, true)
            .await
    }

    pub async fn suggest_services(
        &self,
        request: &SuggestionRequest,
    ) -> Result<Vec<String>, DirectoryError> {
        self.suggestions("/v1/services/suggestions", request, false)
            .await
    }

    async fn suggestions(
        &self,
        path: &str,
        request: &SuggestionRequest,
        mixed: bool,
    ) -> Result<Vec<String>, DirectoryError> {
        let prefix = request.prefix.trim();
        if prefix.is_empty() || prefix.chars().count() > 128 {
            return Err(DirectoryError::InvalidRequest(
                "prefix must contain from 1 through 128 characters".to_owned(),
            ));
        }
        if request.limit > 25 {
            return Err(DirectoryError::InvalidRequest(
                "limit must be from 1 through 25".to_owned(),
            ));
        }
        let mut target = Url::parse(&format!("{}{}", self.environment.origin(), path))
            .map_err(|error| DirectoryError::InvalidRequest(error.to_string()))?;
        let response = if mixed {
            validate_search("", 0, request.filters.as_ref())?;
            let payload = SuggestionRequest {
                prefix: prefix.to_owned(),
                ..request.clone()
            };
            let body = serde_json::to_vec(&payload)
                .map_err(|error| DirectoryError::InvalidRequest(error.to_string()))?;
            self.request("POST", target, body).await?
        } else {
            if request.filters.is_some() {
                return Err(DirectoryError::InvalidRequest(
                    "Service-only suggestions do not support filters".to_owned(),
                ));
            }
            target.query_pairs_mut().append_pair("prefix", prefix);
            if request.limit != 0 {
                target
                    .query_pairs_mut()
                    .append_pair("limit", &request.limit.to_string());
            }
            self.request("GET", target, Vec::new()).await?
        };
        parse_suggestions(&response.body)
    }

    async fn request_page(
        &self,
        method: &str,
        target: &str,
        body: Vec<u8>,
    ) -> Result<SearchPage, DirectoryError> {
        let target = Url::parse(target)
            .map_err(|error| DirectoryError::InvalidRequest(error.to_string()))?;
        let response = self.request(method, target, body).await?;
        let mut value = serde_json::from_slice::<Value>(&response.body)
            .map_err(|error| DirectoryError::InvalidResponse(error.to_string()))?;
        let object = value.as_object_mut().ok_or_else(|| {
            DirectoryError::InvalidResponse("Directory search page must be an object".to_owned())
        })?;
        require_continuation(object.get("next"))?;
        require_facets(object.get("facets"))?;
        let raw = object
            .get_mut("items")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| {
                DirectoryError::InvalidResponse(
                    "Directory search page items are invalid".to_owned(),
                )
            })?;
        if raw.len() > MAXIMUM_ITEMS_PER_PAGE {
            return Err(DirectoryError::InvalidResponse(
                "Directory search page exceeds 100 Services".to_owned(),
            ));
        }
        // ROLE-03: one record this client cannot read is a note about that record. Withholding the
        // whole page would let a single bad row in an index nobody controls deny every other Service.
        let mut items = Vec::with_capacity(raw.len());
        let mut issues = Vec::new();
        for (index, item) in raw.iter_mut().enumerate() {
            match read_service(item) {
                Ok(service) => items.push(service),
                Err(error) => issues.push(ServiceIssue {
                    index,
                    message: error.to_string(),
                }),
            }
        }
        object.insert("items".to_owned(), Value::Array(Vec::new()));
        let mut page = serde_json::from_value::<SearchPage>(value)
            .map_err(|error| DirectoryError::InvalidResponse(error.to_string()))?;
        page.items = items;
        page.issues = issues;
        Ok(page)
    }

    async fn request(
        &self,
        mut method: &str,
        mut target: Url,
        mut body: Vec<u8>,
    ) -> Result<HttpResponse, DirectoryError> {
        let mut redirects = 0_usize;
        loop {
            let mut headers =
                BTreeMap::from([("accept".to_owned(), "application/json".to_owned())]);
            if !body.is_empty() {
                headers.insert("content-type".to_owned(), "application/json".to_owned());
            }
            let response = self
                .transport
                .send(HttpRequest {
                    body: body.clone(),
                    headers,
                    method: method.to_owned(),
                    url: target.to_string(),
                })
                .await?;
            if !matches!(response.status, 301 | 302 | 303 | 307 | 308) {
                return consume_response(response);
            }
            if redirects == MAXIMUM_REDIRECTS {
                return Err(DirectoryError::InvalidResponse(
                    "Directory response exceeded five redirects".to_owned(),
                ));
            }
            let location = response.headers.get("location").ok_or_else(|| {
                DirectoryError::InvalidResponse("Directory redirect omitted Location".to_owned())
            })?;
            let next = target
                .join(location)
                .map_err(|error| DirectoryError::InvalidResponse(error.to_string()))?;
            if next.origin() != target.origin() {
                return Err(DirectoryError::InvalidResponse(
                    "Directory redirect changed origin".to_owned(),
                ));
            }
            if response.status == 303 || (matches!(response.status, 301 | 302) && method == "POST")
            {
                method = "GET";
                body.clear();
            }
            target = next;
            redirects += 1;
        }
    }

    fn continuation_url(&self, next: &str) -> Result<Url, DirectoryError> {
        if next.trim().is_empty() {
            return Err(DirectoryError::InvalidRequest(
                "Directory continuation is empty".to_owned(),
            ));
        }
        let origin = Url::parse(self.environment.origin())
            .map_err(|error| DirectoryError::InvalidResponse(error.to_string()))?;
        let target = origin
            .join(next)
            .map_err(|error| DirectoryError::InvalidResponse(error.to_string()))?;
        if target.origin() != origin.origin()
            || !target.username().is_empty()
            || target.password().is_some()
        {
            return Err(DirectoryError::InvalidResponse(
                "Directory continuation changed canonical origin".to_owned(),
            ));
        }
        Ok(target)
    }
}

/// Reads one Directory record, or says why it cannot be used.
fn read_service(item: &mut Value) -> Result<DirectoryService, DirectoryError> {
    let object = item.as_object_mut().ok_or_else(|| {
        DirectoryError::InvalidResponse("Directory Service must be an object".to_owned())
    })?;
    require_service_origin(object.get("service_origin"))?;
    require_indexed_at(object.get("indexed_at"))?;
    normalize_service_protocols(object)?;
    serde_json::from_value::<DirectoryService>(item.take())
        .map_err(|error| DirectoryError::InvalidResponse(error.to_string()))
}

/// SVC-17 and SEC-08: an origin an index published is checked before a caller is handed it.
fn require_service_origin(value: Option<&Value>) -> Result<(), DirectoryError> {
    let origin = value.and_then(Value::as_str).ok_or_else(|| {
        DirectoryError::InvalidResponse("Directory Service origin is missing".to_owned())
    })?;
    let canonical = derive_service_origin(origin)
        .map_err(|error| DirectoryError::InvalidResponse(error.to_string()))?;
    if canonical != origin {
        return Err(DirectoryError::InvalidResponse(
            "Directory Service origin is not canonical".to_owned(),
        ));
    }
    let url =
        Url::parse(origin).map_err(|error| DirectoryError::InvalidResponse(error.to_string()))?;
    // An address literal is judged outright. A name is not resolved here: nothing is being
    // reached, and an Agent that later connects resolves and judges it again for itself.
    let reachable = match url.host() {
        Some(Host::Ipv4(value)) => is_public(IpAddr::V4(value)),
        Some(Host::Ipv6(value)) => is_public(IpAddr::V6(value)),
        Some(Host::Domain(value)) => !is_local_name(value),
        None => false,
    };
    if !reachable {
        return Err(DirectoryError::InvalidResponse(
            "Directory Service origin names a non-public address".to_owned(),
        ));
    }
    Ok(())
}

fn is_local_name(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == "localhost" || host.ends_with(".localhost")
}

/// An indexing time is an RFC 3339 timestamp, not whatever a date parser happens to accept.
fn require_indexed_at(value: Option<&Value>) -> Result<(), DirectoryError> {
    let indexed_at = value.and_then(Value::as_str).ok_or_else(|| {
        DirectoryError::InvalidResponse("Directory indexing time is missing".to_owned())
    })?;
    if !is_rfc3339(indexed_at) {
        return Err(DirectoryError::InvalidResponse(
            "Directory indexing time is not an RFC 3339 timestamp".to_owned(),
        ));
    }
    Ok(())
}

fn is_rfc3339(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() < 20 || value.chars().count() > 64 {
        return false;
    }
    let digits = |range: std::ops::Range<usize>| bytes[range].iter().all(u8::is_ascii_digit);
    if !digits(0..4) || bytes[4] != b'-' || !digits(5..7) || bytes[7] != b'-' || !digits(8..10) {
        return false;
    }
    if !matches!(bytes[10], b'T' | b't') {
        return false;
    }
    if !digits(11..13) || bytes[13] != b':' || !digits(14..16) || bytes[16] != b':' {
        return false;
    }
    if !digits(17..19) {
        return false;
    }
    let mut rest = &value[19..];
    if let Some(fraction) = rest.strip_prefix('.') {
        let taken = fraction.chars().take_while(char::is_ascii_digit).count();
        if taken == 0 {
            return false;
        }
        rest = &fraction[taken..];
    }
    if matches!(rest, "Z" | "z") {
        return true;
    }
    let offset = rest.as_bytes();
    offset.len() == 6
        && matches!(offset[0], b'+' | b'-')
        && offset[1..3].iter().all(u8::is_ascii_digit)
        && offset[3] == b':'
        && offset[4..6].iter().all(u8::is_ascii_digit)
}

/// A continuation the Directory offers is a reference this client could actually use.
fn require_continuation(value: Option<&Value>) -> Result<(), DirectoryError> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.is_null() {
        return Ok(());
    }
    let next = value.as_str().ok_or_else(|| {
        DirectoryError::InvalidResponse("Directory continuation is invalid".to_owned())
    })?;
    if next.trim() != next || next.chars().count() > MAXIMUM_REFERENCE_CHARACTERS {
        return Err(DirectoryError::InvalidResponse(
            "Directory continuation is invalid".to_owned(),
        ));
    }
    Ok(())
}

/// A facet describes how many Services share a value, so the counts have to be countable.
fn require_facets(value: Option<&Value>) -> Result<(), DirectoryError> {
    let Some(Value::Object(groups)) = value else {
        return Ok(());
    };
    // A trust facet counts Services by trust protocol. `tap` is the only one this ODP version
    // names, and a descriptor carries nothing but that name, so anything else describes a
    // vocabulary this client cannot read and the page as a whole is not usable.
    if let Some(entries) = groups.get("trust").and_then(Value::as_array) {
        let readable = entries.iter().all(|entry| {
            entry
                .get("value")
                .and_then(Value::as_object)
                .is_some_and(|descriptor| {
                    descriptor.len() == 1
                        && descriptor.get("name").and_then(Value::as_str) == Some("tap")
                })
        });
        if !readable {
            return Err(DirectoryError::InvalidResponse(
                "Directory trust facets are invalid".to_owned(),
            ));
        }
    }
    for entries in groups.values() {
        let Some(entries) = entries.as_array() else {
            return Err(DirectoryError::InvalidResponse(
                "Directory facets are invalid".to_owned(),
            ));
        };
        if entries.len() > MAXIMUM_FACET_ENTRIES {
            return Err(DirectoryError::InvalidResponse(
                "Directory facet exceeds 100 entries".to_owned(),
            ));
        }
        for entry in entries {
            let countable = entry
                .get("count")
                .is_some_and(|count| count.as_u64().is_some());
            if !countable {
                return Err(DirectoryError::InvalidResponse(
                    "Directory facet count is invalid".to_owned(),
                ));
            }
        }
    }
    Ok(())
}

/// The Directory answers suggestions as an envelope, and repeats nothing this client would not.
fn parse_suggestions(body: &[u8]) -> Result<Vec<String>, DirectoryError> {
    let value = serde_json::from_slice::<Value>(body)
        .map_err(|error| DirectoryError::InvalidResponse(error.to_string()))?;
    let items = value
        .as_object()
        .and_then(|object| object.get("items"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            DirectoryError::InvalidResponse("Directory suggestions are invalid".to_owned())
        })?;
    let mut seen = BTreeSet::new();
    let mut suggestions = Vec::new();
    for item in items {
        let suggestion = item.as_str().filter(|value| {
            !value.trim().is_empty() && value.trim() == *value && value.chars().count() <= 128
        });
        let Some(suggestion) = suggestion else {
            return Err(DirectoryError::InvalidResponse(
                "Directory suggestions are invalid".to_owned(),
            ));
        };
        if seen.insert(suggestion.to_owned()) {
            suggestions.push(suggestion.to_owned());
        }
        if suggestions.len() == MAXIMUM_SUGGESTIONS {
            break;
        }
    }
    Ok(suggestions)
}

/// `read_service` has already established that `object` is a Service-shaped object.
fn normalize_service_protocols(
    object: &mut serde_json::Map<String, Value>,
) -> Result<(), DirectoryError> {
    let Some(protocols) = object.get("protocols").cloned() else {
        return Ok(());
    };
    let candidate = json!({
        "description": "Directory protocol validation",
        "http": {"endpoint_base": "/"},
        "language": "en",
        "localizations": ["en"],
        "name": "Directory Service",
        "odp_version": "1.0",
        "operations": [
            {"authentication": "not-required", "name": "get-offering"},
            {"authentication": "not-required", "name": "list-offerings"}
        ],
        "protocols": protocols
    });
    let encoded = serde_json::to_vec(&candidate)
        .map_err(|error| DirectoryError::InvalidResponse(error.to_string()))?;
    let document = parse_agent_service_document(&encoded)
        .map_err(|error| DirectoryError::InvalidResponse(error.to_string()))?;
    if let Some(protocols) = document.protocols {
        object.insert(
            "protocols".to_owned(),
            serde_json::to_value(protocols)
                .map_err(|error| DirectoryError::InvalidResponse(error.to_string()))?,
        );
    } else {
        object.remove("protocols");
    }
    Ok(())
}

fn bounded(value: usize, maximum: usize, name: &str) -> Result<usize, DirectoryError> {
    let value = if value == 0 { maximum } else { value };
    if value > maximum {
        return Err(DirectoryError::InvalidRequest(format!(
            "{name} must be from 1 through {maximum}"
        )));
    }
    Ok(value)
}

fn validate_search_request(request: &SearchRequest) -> Result<(), DirectoryError> {
    validate_search(&request.query, request.limit, request.filters.as_ref())
}

fn validate_search(
    query: &str,
    limit: usize,
    filters: Option<&crate::ServiceFilters>,
) -> Result<(), DirectoryError> {
    if limit > MAXIMUM_ITEMS_PER_PAGE {
        return Err(DirectoryError::InvalidRequest(
            "limit must be from 1 through 100".to_owned(),
        ));
    }
    if query.trim() != query || query.chars().count() > 512 {
        return Err(DirectoryError::InvalidRequest(
            "query must contain at most 512 characters without surrounding whitespace".to_owned(),
        ));
    }
    let Some(filters) = filters else {
        return Ok(());
    };
    if filters.keywords.len() > 32
        || filters
            .keywords
            .iter()
            .any(|value| value.trim().is_empty() || value.chars().count() > 64)
    {
        return Err(DirectoryError::InvalidRequest(
            "keywords must contain at most 32 values of at most 64 characters".to_owned(),
        ));
    }
    require_unique(&filters.keywords, "keywords")?;
    // A Directory rejects the whole request over a repeated filter, so a caller hears about it
    // here rather than as an opaque 400.
    if filters.enrollment.len() > 1 {
        return Err(DirectoryError::InvalidRequest(
            "enrollment must contain at most 1 value".to_owned(),
        ));
    }
    if filters.operations.len() > 21 {
        return Err(DirectoryError::InvalidRequest(
            "operations must contain at most 21 values".to_owned(),
        ));
    }
    require_unique(&filters.operations, "operations")?;
    if filters.payments.len() > 32 {
        return Err(DirectoryError::InvalidRequest(
            "payments must contain at most 32 values".to_owned(),
        ));
    }
    require_unique(&filters.payments, "payments")?;
    for payment in &filters.payments {
        require_unique(&payment.options, "payment options")?;
    }
    // A trust filter, when present, is the single-item array `[{"name":"tap"}]`: `tap` is the only
    // trust protocol this ODP version names, and the Directory refuses anything else.
    if !filters.trust.is_empty()
        && (filters.trust.len() != 1 || filters.trust[0].name != Protocol::Tap)
    {
        return Err(DirectoryError::InvalidRequest(
            "trust must be the single-item array [{\"name\":\"tap\"}]".to_owned(),
        ));
    }
    Ok(())
}

fn require_unique<T: PartialEq>(values: &[T], name: &str) -> Result<(), DirectoryError> {
    for (index, value) in values.iter().enumerate() {
        if values[..index].contains(value) {
            return Err(DirectoryError::InvalidRequest(format!(
                "{name} must not repeat a value"
            )));
        }
    }
    Ok(())
}

fn consume_response(response: HttpResponse) -> Result<HttpResponse, DirectoryError> {
    let declared = response
        .headers
        .get("content-length")
        .and_then(|value| value.trim().parse::<usize>().ok());
    let failed = !(200..300).contains(&response.status);
    // ERR-21: a failure is read under a far tighter limit than a result.
    let limit = if failed {
        MAXIMUM_ERROR_BYTES
    } else {
        MAXIMUM_RESPONSE_BYTES
    };
    if failed {
        return Err(DirectoryError::Request {
            message: failure_message(&response, limit, declared),
            headers: response.headers,
            status: response.status,
        });
    }
    if declared.is_some_and(|value| value > limit) || response.body.len() > limit {
        return Err(DirectoryError::InvalidResponse(
            "Directory response exceeds 524288 bytes".to_owned(),
        ));
    }
    if !media_type(&response.headers).eq_ignore_ascii_case("application/json") {
        return Err(DirectoryError::InvalidResponse(
            "Directory response must use application/json".to_owned(),
        ));
    }
    Ok(response)
}

/// What a failure is worth repeating to a caller.
///
/// The status already says what happened; this is the Directory's own account of why. The body
/// belongs to whoever answered, so it is read only when it claims to be one of the two shapes
/// that carry a reason, only up to the limit, and only as printable text of bounded length.
/// Anything else leaves the account empty rather than repeating an unread body.
fn failure_message(response: &HttpResponse, limit: usize, declared: Option<usize>) -> String {
    let media_type = media_type(&response.headers);
    let describable = media_type.eq_ignore_ascii_case("application/json")
        || media_type.eq_ignore_ascii_case("application/problem+json");
    if !describable || declared.is_some_and(|value| value > limit) || response.body.len() > limit {
        return String::new();
    }
    let text = String::from_utf8_lossy(&response.body);
    let detail = serde_json::from_str::<Value>(&text)
        .ok()
        .filter(Value::is_object)
        .and_then(|value| {
            ["detail", "title", "message"]
                .iter()
                .filter_map(|member| value.get(*member).and_then(Value::as_str))
                .find(|found| !found.trim().is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| text.into_owned());
    printable(&detail)
}

/// Flattens control characters and keeps the excerpt short enough to read.
fn printable(value: &str) -> String {
    let flattened = value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let collapsed = flattened.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= MAXIMUM_ERROR_MESSAGE {
        return collapsed;
    }
    let mut truncated = collapsed
        .chars()
        .take(MAXIMUM_ERROR_MESSAGE)
        .collect::<String>();
    truncated.push('…');
    truncated
}

fn media_type(headers: &BTreeMap<String, String>) -> &str {
    headers
        .get("content-type")
        .map(|value| value.split(';').next().unwrap_or_default().trim())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use crate::{Facet, ServiceFilters};
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::*;

    struct MockTransport {
        requests: Mutex<Vec<HttpRequest>>,
    }

    struct ResponseTransport(Vec<u8>);

    #[async_trait]
    impl Transport for ResponseTransport {
        async fn send(&self, _request: HttpRequest) -> Result<HttpResponse, TransportError> {
            Ok(HttpResponse {
                body: self.0.clone(),
                headers: BTreeMap::from([(
                    "content-type".to_owned(),
                    "application/json".to_owned(),
                )]),
                status: 200,
            })
        }
    }

    #[async_trait]
    impl Transport for MockTransport {
        async fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
            self.requests.lock().unwrap().push(request);
            Ok(HttpResponse {
                body: br#"{"items":[{"description":"Plants","indexed_at":"2026-08-25T00:00:00Z","language":"en","localizations":["en"],"name":"Indica Flowers","operations":[{"authentication":"not-required","name":"get-offering"},{"authentication":"not-required","name":"list-offerings"}],"protocols":{"trust":[{"name":"tap"}]},"service_origin":"https://demo.inflowpay.ai"}]}"#.to_vec(),
                headers: BTreeMap::from([("content-type".to_owned(), "application/json".to_owned())]),
                status: 200,
            })
        }
    }

    #[tokio::test]
    async fn searches_only_the_selected_canonical_directory() {
        let transport = Arc::new(MockTransport {
            requests: Mutex::new(Vec::new()),
        });
        let client = DirectoryClient::with_transport(Environment::Sandbox, transport.clone());
        let page = client
            .search_services(&SearchRequest {
                filters: Some(ServiceFilters {
                    trust: vec![odp_core::TrustProtocol {
                        name: odp_core::Protocol::Tap,
                    }],
                    ..ServiceFilters::default()
                }),
                query: "plants".to_owned(),
                ..SearchRequest::default()
            })
            .await
            .unwrap();
        assert_eq!(page.items[0].name, "Indica Flowers");
        assert_eq!(
            page.items[0].protocols.as_ref().unwrap().trust,
            [odp_core::TrustProtocol {
                name: odp_core::Protocol::Tap
            }]
        );
        let requests = transport.requests.lock().unwrap();
        assert_eq!(
            requests[0].url,
            "https://sandbox.inflowpay.ai/v1/services/search"
        );
        assert_eq!(requests[0].method, "POST");
        assert_eq!(
            serde_json::from_slice::<Value>(&requests[0].body).unwrap(),
            serde_json::json!({"filters":{"trust":[{"name":"tap"}]},"query":"plants"})
        );
    }

    #[tokio::test]
    async fn decodes_typed_trust_facets() {
        let body = br#"{"items":[],"facets":{"trust":[{"value":{"name":"tap"},"count":2}]}}"#;
        let client = DirectoryClient::with_transport(
            Environment::Production,
            Arc::new(ResponseTransport(body.to_vec())),
        );

        let page = client
            .search_services(&SearchRequest::default())
            .await
            .unwrap();

        assert_eq!(
            page.facets.unwrap().trust,
            [Facet {
                count: 2,
                value: odp_core::TrustProtocol {
                    name: odp_core::Protocol::Tap,
                },
            }]
        );

        let invalid = br#"{"items":[],"facets":{"trust":[{"value":{"name":"mpp"},"count":2}]}}"#;
        let client = DirectoryClient::with_transport(
            Environment::Production,
            Arc::new(ResponseTransport(invalid.to_vec())),
        );
        assert!(
            client
                .search_services(&SearchRequest::default())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn rejects_unbounded_directory_traversal() {
        let client = DirectoryClient::with_transport(
            Environment::Production,
            Arc::new(MockTransport {
                requests: Mutex::new(Vec::new()),
            }),
        );
        let error = client
            .collect_services(
                &SearchRequest::default(),
                IterationOptions {
                    max_items: 10_001,
                    max_pages: 0,
                },
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("max_items"));
    }

    #[tokio::test]
    async fn rejects_unsupported_trust_filters() {
        let client = DirectoryClient::with_transport(
            Environment::Production,
            Arc::new(MockTransport {
                requests: Mutex::new(Vec::new()),
            }),
        );
        let error = client
            .search_services(&SearchRequest {
                filters: Some(ServiceFilters {
                    trust: vec![odp_core::TrustProtocol {
                        name: odp_core::Protocol::Mpp,
                    }],
                    ..ServiceFilters::default()
                }),
                ..SearchRequest::default()
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("trust"));
    }

    #[tokio::test]
    async fn filters_unknown_protocols_and_rejects_malformed_known_protocols() {
        let body = br#"{"items":[{"description":"Plants","indexed_at":"2026-08-25T00:00:00Z","language":"en","localizations":["en"],"name":"Plants","operations":[],"protocols":{"payments":[{"authentication":"not-required","name":"future-payment"},{"authentication":"not-required","name":"mpp"}],"trust":[{"name":"future-trust"},{"name":"tap"}]},"service_origin":"https://demo.inflowpay.ai"}]}"#;
        let client = DirectoryClient::with_transport(
            Environment::Production,
            Arc::new(ResponseTransport(body.to_vec())),
        );
        let page = client
            .search_services(&SearchRequest::default())
            .await
            .unwrap();
        let protocols = page.items[0].protocols.as_ref().unwrap();
        assert_eq!(protocols.payments.len(), 1);
        assert_eq!(protocols.trust.len(), 1);

        let malformed = String::from_utf8_lossy(body)
            .replace("\"name\":\"mpp\"", "\"name\":\"mpp\",\"unexpected\":true");
        let client = DirectoryClient::with_transport(
            Environment::Production,
            Arc::new(ResponseTransport(malformed.into_bytes())),
        );
        // A malformed known protocol makes that one record unusable, and says so, rather than
        // withholding every other Service on the page.
        let page = client
            .search_services(&SearchRequest::default())
            .await
            .unwrap();
        assert!(page.items.is_empty());
        assert_eq!(page.issues.len(), 1);
        assert_eq!(page.issues[0].index, 0);
    }
}
