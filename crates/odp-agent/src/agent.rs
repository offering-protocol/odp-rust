use std::sync::Arc;

use futures::{StreamExt, stream};
use odp_core::{Offering, OfferingSearchRequest, Representation};
use odp_directory::{
    DirectoryClient, DirectoryService, Environment, IterationOptions, SearchRequest,
};

use crate::{AgentError, ServiceClient, TraversalOptions};

pub trait ServiceClientFactory: Send + Sync {
    fn create(&self, service: &DirectoryService) -> Result<ServiceClient, AgentError>;
}

struct DefaultServiceClientFactory;

impl ServiceClientFactory for DefaultServiceClientFactory {
    fn create(&self, service: &DirectoryService) -> Result<ServiceClient, AgentError> {
        ServiceClient::new(&service.service_origin)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FederatedSearchRequest {
    pub concurrency: usize,
    pub max_offerings_per_service: usize,
    pub max_services: usize,
    pub offerings: OfferingSearchRequest,
    pub services: SearchRequest,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DiscoveryEvent {
    pub issue: Option<String>,
    pub offering: Option<Offering>,
    pub service: DirectoryService,
}

pub struct Agent {
    directory: DirectoryClient,
    factory: Arc<dyn ServiceClientFactory>,
}

impl Agent {
    pub fn new(environment: Environment) -> Result<Self, AgentError> {
        let directory = DirectoryClient::new(environment)
            .map_err(|error| AgentError::Directory(error.to_string()))?;
        Ok(Self {
            directory,
            factory: Arc::new(DefaultServiceClientFactory),
        })
    }

    pub fn with_clients(
        directory: DirectoryClient,
        factory: Arc<dyn ServiceClientFactory>,
    ) -> Self {
        Self { directory, factory }
    }

    pub const fn environment(&self) -> Environment {
        self.directory.environment()
    }

    pub async fn search_offerings_across_services(
        &self,
        request: &FederatedSearchRequest,
    ) -> Result<Vec<DiscoveryEvent>, AgentError> {
        let maximum_services = bounded(request.max_services, 10, 100, "max_services")?;
        let maximum_offerings = bounded(
            request.max_offerings_per_service,
            10,
            100,
            "max_offerings_per_service",
        )?;
        let concurrency = bounded(request.concurrency, 4, 16, "concurrency")?;
        let services = self
            .directory
            .search_services(
                &request.services,
                IterationOptions {
                    max_items: maximum_services,
                    max_pages: 0,
                },
            )
            .await
            .map_err(|error| AgentError::Directory(error.to_string()))?;
        let offerings = request.offerings.clone();
        let factory = &self.factory;
        let results = stream::iter(services.into_iter().map(|service| {
            let offerings = offerings.clone();
            async move {
                let result =
                    search_service(factory.as_ref(), &service, &offerings, maximum_offerings).await;
                (service, result)
            }
        }))
        .buffered(concurrency)
        .collect::<Vec<_>>()
        .await;
        Ok(results
            .into_iter()
            .flat_map(|(service, result)| match result {
                Ok(offerings) => offerings
                    .into_iter()
                    .map(|offering| DiscoveryEvent {
                        issue: None,
                        offering: Some(offering),
                        service: service.clone(),
                    })
                    .collect(),
                Err(error) => vec![DiscoveryEvent {
                    issue: Some(error.to_string()),
                    offering: None,
                    service,
                }],
            })
            .collect())
    }
}

async fn search_service(
    factory: &dyn ServiceClientFactory,
    service: &DirectoryService,
    request: &OfferingSearchRequest,
    maximum: usize,
) -> Result<Vec<Offering>, AgentError> {
    let client = factory.create(service)?;
    let traversal = TraversalOptions {
        max_items: maximum,
        max_pages: 0,
    };
    if has_search(request) {
        client
            .search_all_offerings(request, Representation::Terse, traversal)
            .await
    } else {
        client
            .list_all_offerings(Representation::Terse, 0, traversal)
            .await
    }
}

fn has_search(request: &OfferingSearchRequest) -> bool {
    !request.query.is_empty()
        || !request.filters.is_empty()
        || request.include_descendants
        || !request.sort.is_empty()
        || !request.refinements.is_empty()
        || !request.collection_id.is_empty()
}

fn bounded(value: usize, fallback: usize, maximum: usize, name: &str) -> Result<usize, AgentError> {
    let value = if value == 0 { fallback } else { value };
    if value > maximum {
        return Err(AgentError::InvalidRequest(format!(
            "{name} must be from 1 through {maximum}"
        )));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc};

    use async_trait::async_trait;
    use odp_directory::{HttpRequest, HttpResponse, Transport, TransportError};

    use super::*;

    struct DirectoryTransport;

    #[async_trait]
    impl Transport for DirectoryTransport {
        async fn send(&self, _request: HttpRequest) -> Result<HttpResponse, TransportError> {
            Ok(json_response(br#"{"items":[{"description":"One","indexed_at":"2026-08-25T00:00:00Z","language":"en","localizations":["en"],"name":"One","operations":[],"service_origin":"https://one.example"},{"description":"Two","indexed_at":"2026-08-25T00:00:00Z","language":"en","localizations":["en"],"name":"Two","operations":[],"service_origin":"https://two.example"}]}"#))
        }
    }

    struct ServiceTransport;

    #[async_trait]
    impl Transport for ServiceTransport {
        async fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
            if request.url.ends_with("/.well-known/odp") {
                return Ok(odp_response(br#"{"description":"Plants","http":{"endpoint_base":"/odp"},"language":"en","localizations":["en"],"name":"Plants","odp_version":"1.0","operations":[{"authentication":"not-required","name":"get-offering"},{"authentication":"not-required","name":"list-offerings"},{"authentication":"not-required","name":"search-offerings"}]}"#));
            }
            let id = if request.url.starts_with("https://one.example") {
                "one"
            } else {
                "two"
            };
            Ok(odp_response(
                format!(
                    r#"{{"items":[{{"id":"{id}","name":"Plant {id}","odp_version":"1.0"}}],"odp_version":"1.0"}}"#
                )
                .as_bytes(),
            ))
        }
    }

    struct Factory;

    impl ServiceClientFactory for Factory {
        fn create(&self, service: &DirectoryService) -> Result<ServiceClient, AgentError> {
            ServiceClient::with_transport(&service.service_origin, Arc::new(ServiceTransport))
        }
    }

    fn json_response(body: &[u8]) -> HttpResponse {
        HttpResponse {
            body: body.to_vec(),
            headers: BTreeMap::from([("content-type".to_owned(), "application/json".to_owned())]),
            status: 200,
        }
    }

    fn odp_response(body: &[u8]) -> HttpResponse {
        HttpResponse {
            body: body.to_vec(),
            headers: BTreeMap::from([(
                "content-type".to_owned(),
                "application/odp+json".to_owned(),
            )]),
            status: 200,
        }
    }

    #[tokio::test]
    async fn preserves_directory_order_across_concurrent_service_searches() {
        let directory =
            DirectoryClient::with_transport(Environment::Production, Arc::new(DirectoryTransport));
        let agent = Agent::with_clients(directory, Arc::new(Factory));
        let events = agent
            .search_offerings_across_services(&FederatedSearchRequest {
                concurrency: 2,
                ..FederatedSearchRequest::default()
            })
            .await
            .unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].service.name, "One");
        assert_eq!(events[1].service.name, "Two");
    }

    /// A search request drives the search operation; a bare request just lists.
    #[tokio::test]
    async fn searches_a_service_only_when_the_request_asks_a_question() {
        let recorder = Arc::new(RecordingFactory::default());
        let directory =
            DirectoryClient::with_transport(Environment::Production, Arc::new(DirectoryTransport));
        let agent = Agent::with_clients(directory, recorder.clone());

        agent
            .search_offerings_across_services(&FederatedSearchRequest {
                offerings: OfferingSearchRequest {
                    query: "rubber".to_owned(),
                    ..OfferingSearchRequest::default()
                },
                ..FederatedSearchRequest::default()
            })
            .await
            .unwrap();

        let urls = recorder.urls();
        assert!(
            urls.iter().any(|url| url.contains("/offerings/search")),
            "{urls:?}"
        );
    }

    /// FED-04: one Service that cannot answer is reported, and the others still report offerings.
    #[tokio::test]
    async fn reports_a_failing_service_without_losing_the_others() {
        struct HalfBroken;

        impl ServiceClientFactory for HalfBroken {
            fn create(&self, service: &DirectoryService) -> Result<ServiceClient, AgentError> {
                if service.service_origin.starts_with("https://one.example") {
                    return Err(AgentError::InvalidRequest("no client for One".to_owned()));
                }
                ServiceClient::with_transport(&service.service_origin, Arc::new(ServiceTransport))
            }
        }

        let directory =
            DirectoryClient::with_transport(Environment::Production, Arc::new(DirectoryTransport));
        let agent = Agent::with_clients(directory, Arc::new(HalfBroken));
        let events = agent
            .search_offerings_across_services(&FederatedSearchRequest::default())
            .await
            .unwrap();

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].service.name, "One");
        assert!(events[0].offering.is_none());
        assert!(events[0].issue.is_some());
        assert!(events[1].offering.is_some());
        assert!(events[1].issue.is_none());
    }

    /// Each bound has a ceiling, so a request asking for more is refused before anything is sent.
    #[tokio::test]
    async fn refuses_a_request_that_asks_for_more_than_the_bounds_allow() {
        let directory =
            DirectoryClient::with_transport(Environment::Production, Arc::new(DirectoryTransport));
        let agent = Agent::with_clients(directory, Arc::new(Factory));

        for (request, name) in [
            (
                FederatedSearchRequest {
                    max_services: 101,
                    ..FederatedSearchRequest::default()
                },
                "max_services",
            ),
            (
                FederatedSearchRequest {
                    max_offerings_per_service: 101,
                    ..FederatedSearchRequest::default()
                },
                "max_offerings_per_service",
            ),
            (
                FederatedSearchRequest {
                    concurrency: 17,
                    ..FederatedSearchRequest::default()
                },
                "concurrency",
            ),
        ] {
            let error = agent
                .search_offerings_across_services(&request)
                .await
                .unwrap_err();
            assert!(error.to_string().contains(name), "{name}: {error}");
        }
    }

    #[test]
    fn builds_an_agent_for_an_environment_it_keeps() {
        let agent = Agent::new(Environment::Sandbox).unwrap();
        assert_eq!(agent.environment(), Environment::Sandbox);
    }

    /// The default factory reaches the Service Origin the Directory listed.
    #[test]
    fn builds_a_default_client_for_a_listed_service() {
        let service: DirectoryService = serde_json::from_str(
            r#"{"description":"One","indexed_at":"2026-08-25T00:00:00Z","language":"en","localizations":["en"],"name":"One","operations":[],"service_origin":"https://plants.example"}"#,
        )
        .unwrap();
        let client = DefaultServiceClientFactory.create(&service).unwrap();
        assert_eq!(client.service_origin(), "https://plants.example");
    }

    /// A factory that records the URLs its clients are asked for.
    #[derive(Default)]
    struct RecordingFactory {
        urls: Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl RecordingFactory {
        fn urls(&self) -> Vec<String> {
            self.urls.lock().unwrap().clone()
        }
    }

    impl ServiceClientFactory for RecordingFactory {
        fn create(&self, service: &DirectoryService) -> Result<ServiceClient, AgentError> {
            ServiceClient::with_transport(
                &service.service_origin,
                Arc::new(RecordingTransport {
                    urls: self.urls.clone(),
                }),
            )
        }
    }

    struct RecordingTransport {
        urls: Arc<std::sync::Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl Transport for RecordingTransport {
        async fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
            self.urls.lock().unwrap().push(request.url.clone());
            ServiceTransport.send(request).await
        }
    }
}
