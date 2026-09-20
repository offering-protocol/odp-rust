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
            .collect_services(
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
                return Ok(odp_response(br#"{"description":"Plants","http":{"endpoint_base":"/odp"},"language":"en","localizations":["en"],"name":"Plants","odp_version":"1.0","operations":[{"authentication":"not-required","name":"get-offering"},{"authentication":"not-required","name":"list-offerings"}]}"#));
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
}
