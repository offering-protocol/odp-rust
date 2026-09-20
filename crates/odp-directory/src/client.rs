use std::{collections::BTreeMap, sync::Arc};

use odp_core::{derive_service_origin, parse_agent_service_document};
use serde_json::{Value, json};
use thiserror::Error;
use url::Url;

use crate::{
    DirectoryService, Environment, HttpRequest, HttpResponse, IterationOptions,
    ResourceSearchRequest, SearchPage, SearchRequest, SearchResponse, SuggestionRequest, Transport,
    TransportError, default_transport,
};

const MAXIMUM_REDIRECTS: usize = 5;
const MAXIMUM_RESPONSE_BYTES: usize = 524_288;

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

    pub async fn collect_services(
        &self,
        request: &SearchRequest,
        options: IterationOptions,
    ) -> Result<Vec<DirectoryService>, DirectoryError> {
        let maximum_items = bounded(options.max_items, 10_000, 10_000, "max_items")?;
        let maximum_responses = bounded(options.max_pages, 16, 16, "max_pages")?;
        let mut services = Vec::new();
        let mut page = self.search_services(request).await?;
        for index in 0..maximum_responses {
            services.extend(page.items.into_iter().take(maximum_items - services.len()));
            if page.next.is_empty()
                || services.len() == maximum_items
                || index + 1 == maximum_responses
            {
                break;
            }
            page = self.continue_search_services(&page.next).await?;
        }
        Ok(services)
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
        #[derive(serde::Deserialize)]
        struct Suggestions {
            items: Vec<String>,
        }
        let suggestions = serde_json::from_slice::<Suggestions>(&response.body)
            .map_err(|error| DirectoryError::InvalidResponse(error.to_string()))?
            .items;
        if suggestions.len() > 25
            || suggestions.iter().any(|value| {
                value.trim() != value || value.is_empty() || value.chars().count() > 128
            })
        {
            return Err(DirectoryError::InvalidResponse(
                "Directory suggestions are invalid".to_owned(),
            ));
        }
        Ok(suggestions)
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
        if let Some(items) = value
            .as_object_mut()
            .and_then(|object| object.get_mut("items"))
            .and_then(Value::as_array_mut)
        {
            for item in items {
                normalize_service_protocols(item)?;
            }
        }
        let page = serde_json::from_value::<SearchPage>(value)
            .map_err(|error| DirectoryError::InvalidResponse(error.to_string()))?;
        if page.items.len() > 100 {
            return Err(DirectoryError::InvalidResponse(
                "Directory search page exceeds 100 Services".to_owned(),
            ));
        }
        if page.facets.as_ref().is_some_and(|facets| {
            facets
                .trust
                .iter()
                .any(|facet| facet.value.name != odp_core::Protocol::Tap)
        }) {
            return Err(DirectoryError::InvalidResponse(
                "Directory trust facets are invalid".to_owned(),
            ));
        }
        for service in &page.items {
            let canonical = derive_service_origin(&service.service_origin)
                .map_err(|error| DirectoryError::InvalidResponse(error.to_string()))?;
            if canonical != service.service_origin {
                return Err(DirectoryError::InvalidResponse(
                    "Directory Service origin is not canonical".to_owned(),
                ));
            }
        }
        Ok(page)
    }

    async fn request(
        &self,
        mut method: &str,
        mut target: Url,
        mut body: Vec<u8>,
    ) -> Result<HttpResponse, DirectoryError> {
        for redirects in 0..=MAXIMUM_REDIRECTS {
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
        }
        Err(DirectoryError::InvalidResponse(
            "Directory response exceeded its redirect limit".to_owned(),
        ))
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

fn normalize_service_protocols(item: &mut Value) -> Result<(), DirectoryError> {
    let Some(object) = item.as_object_mut() else {
        return Ok(());
    };
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

fn bounded(
    value: usize,
    fallback: usize,
    maximum: usize,
    name: &str,
) -> Result<usize, DirectoryError> {
    let value = if value == 0 { fallback } else { value };
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
    if limit > 100 {
        return Err(DirectoryError::InvalidRequest(
            "limit must be from 1 through 100".to_owned(),
        ));
    }
    if query.trim() != query || query.chars().count() > 512 {
        return Err(DirectoryError::InvalidRequest(
            "query must contain at most 512 characters without surrounding whitespace".to_owned(),
        ));
    }
    if let Some(filters) = filters {
        if filters.keywords.len() > 32
            || filters
                .keywords
                .iter()
                .any(|value| value.is_empty() || value.chars().count() > 64)
        {
            return Err(DirectoryError::InvalidRequest(
                "keywords must contain at most 32 values of at most 64 characters".to_owned(),
            ));
        }
        if !filters.trust.is_empty()
            && (filters.trust.len() != 1 || filters.trust[0].name != odp_core::Protocol::Tap)
        {
            return Err(DirectoryError::InvalidRequest(
                "trust must contain exactly one tap descriptor".to_owned(),
            ));
        }
    }
    Ok(())
}

fn consume_response(response: HttpResponse) -> Result<HttpResponse, DirectoryError> {
    if response.body.len() > MAXIMUM_RESPONSE_BYTES {
        return Err(DirectoryError::InvalidResponse(
            "Directory response exceeds 524288 bytes".to_owned(),
        ));
    }
    if !(200..300).contains(&response.status) {
        let message = String::from_utf8_lossy(&response.body).into_owned();
        return Err(DirectoryError::Request {
            headers: response.headers,
            message,
            status: response.status,
        });
    }
    let content_type = response
        .headers
        .get("content-type")
        .map(|value| value.split(';').next().unwrap_or_default().trim())
        .unwrap_or_default();
    if !content_type.eq_ignore_ascii_case("application/json") {
        return Err(DirectoryError::InvalidResponse(
            "Directory response must use application/json".to_owned(),
        ));
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::*;
    use crate::{Facet, ServiceFilters};

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
        assert!(
            client
                .search_services(&SearchRequest::default())
                .await
                .is_err()
        );
    }
}
