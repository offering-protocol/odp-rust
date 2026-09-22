use std::collections::{BTreeMap, BTreeSet};

use odp_core::{
    Collection, FilterDefinition, Operation, SearchCapabilities, SortDefinition,
    normalize_agent_response, parse_filter_definition_page, parse_sort_definition_page,
};
use url::Url;

use crate::{AgentError, ServiceClient};

const MAXIMUM_CAPABILITY_PAGES: usize = 16;
const MAXIMUM_FILTERS: usize = 1_024;
const MAXIMUM_SORTS: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityScope {
    Collection,
    Service,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityKind {
    Filters,
    Sorts,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityIssue {
    pub kind: CapabilityKind,
    pub message: String,
    pub scope: CapabilityScope,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedSortDefinition {
    pub definition: SortDefinition,
    pub filters: Vec<FilterDefinition>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchCapabilityCatalog {
    pub filters: BTreeMap<String, FilterDefinition>,
    pub issues: Vec<CapabilityIssue>,
    pub sorts: BTreeMap<String, ResolvedSortDefinition>,
}

impl ServiceClient {
    pub async fn get_collection_search_capabilities(
        &self,
        id: &str,
    ) -> Result<SearchCapabilityCatalog, AgentError> {
        let collection = self.get_collection(id).await?;
        self.resolve_search_capabilities(Some(&collection)).await
    }

    pub async fn get_offering_search_capabilities(
        &self,
        collection_id: Option<&str>,
    ) -> Result<SearchCapabilityCatalog, AgentError> {
        let collection = match collection_id {
            Some(id) => Some(self.get_collection(id).await?),
            None => None,
        };
        self.resolve_search_capabilities(collection.as_ref()).await
    }

    async fn resolve_search_capabilities(
        &self,
        collection: Option<&Collection>,
    ) -> Result<SearchCapabilityCatalog, AgentError> {
        let inspection = self.inspect().await?;
        let mut result = SearchCapabilityCatalog::default();
        let supports_search = inspection
            .document
            .operations
            .iter()
            .any(|value| value.name == Operation::SearchOfferings);
        if !supports_search {
            if collection
                .and_then(|value| value.search_capabilities.as_ref())
                .is_some()
            {
                result.issues.push(CapabilityIssue {
                    kind: CapabilityKind::Filters,
                    message: "Collection search capabilities require search-offerings".to_owned(),
                    scope: CapabilityScope::Collection,
                });
            }
            return Ok(result);
        }
        let mut sorts = BTreeMap::new();
        let mut sort_scopes = BTreeMap::new();
        let sources = [
            (
                inspection.document.search_capabilities.as_ref(),
                CapabilityScope::Service,
            ),
            (
                collection.and_then(|value| value.search_capabilities.as_ref()),
                CapabilityScope::Collection,
            ),
        ];
        for (capabilities, scope) in sources {
            let Some(capabilities) = capabilities else {
                continue;
            };
            self.add_filters(&mut result, scope, capabilities).await;
            self.add_sorts(
                &mut result,
                &mut sorts,
                &mut sort_scopes,
                scope,
                capabilities,
            )
            .await;
        }
        for (id, definition) in sorts {
            let filters = definition
                .keys
                .iter()
                .filter_map(|key| result.filters.get(&key.filter_id).cloned())
                .collect::<Vec<_>>();
            if filters.len() != definition.keys.len() {
                result.issues.push(CapabilityIssue {
                    kind: CapabilityKind::Sorts,
                    message: format!("Sort {id} references an unavailable filter"),
                    scope: sort_scopes[&id],
                });
                continue;
            }
            result.sorts.insert(
                id,
                ResolvedSortDefinition {
                    definition,
                    filters,
                },
            );
        }
        Ok(result)
    }

    async fn add_filters(
        &self,
        result: &mut SearchCapabilityCatalog,
        scope: CapabilityScope,
        capabilities: &SearchCapabilities,
    ) {
        let Some(source) = &capabilities.filters else {
            return;
        };
        let values = if let Some(link) = &source.linked {
            match self.load_filter_pages(&link.href).await {
                Ok(values) => values,
                Err(error) => {
                    result.issues.push(CapabilityIssue {
                        kind: CapabilityKind::Filters,
                        message: error.to_string(),
                        scope,
                    });
                    return;
                }
            }
        } else {
            source.inline.clone()
        };
        let duplicates = duplicate_ids(
            values.iter().map(|value| value.id.as_str()),
            &result.filters,
        );
        for duplicate in &duplicates {
            result.filters.remove(duplicate);
        }
        let accepted = values
            .iter()
            .filter(|value| !duplicates.contains(&value.id))
            .count();
        if result.filters.len() + accepted > MAXIMUM_FILTERS {
            result.issues.push(CapabilityIssue {
                kind: CapabilityKind::Filters,
                message: "Effective filters exceed 1024 entries".to_owned(),
                scope,
            });
            return;
        }
        for value in values {
            if !duplicates.contains(&value.id) {
                result.filters.insert(value.id.clone(), value);
            }
        }
        report_duplicates(
            &duplicates,
            CapabilityKind::Filters,
            scope,
            &mut result.issues,
        );
    }

    async fn add_sorts(
        &self,
        result: &mut SearchCapabilityCatalog,
        target: &mut BTreeMap<String, SortDefinition>,
        scopes: &mut BTreeMap<String, CapabilityScope>,
        scope: CapabilityScope,
        capabilities: &SearchCapabilities,
    ) {
        let Some(source) = &capabilities.sorts else {
            return;
        };
        let values = if let Some(link) = &source.linked {
            match self.load_sort_pages(&link.href).await {
                Ok(values) => values,
                Err(error) => {
                    result.issues.push(CapabilityIssue {
                        kind: CapabilityKind::Sorts,
                        message: error.to_string(),
                        scope,
                    });
                    return;
                }
            }
        } else {
            source.inline.clone()
        };
        let duplicates = duplicate_ids(values.iter().map(|value| value.id.as_str()), target);
        for duplicate in &duplicates {
            target.remove(duplicate);
            scopes.remove(duplicate);
        }
        let accepted = values
            .iter()
            .filter(|value| !duplicates.contains(&value.id))
            .count();
        if target.len() + accepted > MAXIMUM_SORTS {
            result.issues.push(CapabilityIssue {
                kind: CapabilityKind::Sorts,
                message: "Effective sorts exceed 128 entries".to_owned(),
                scope,
            });
            return;
        }
        for value in values {
            if !duplicates.contains(&value.id) {
                scopes.insert(value.id.clone(), scope);
                target.insert(value.id.clone(), value);
            }
        }
        report_duplicates(
            &duplicates,
            CapabilityKind::Sorts,
            scope,
            &mut result.issues,
        );
    }

    async fn load_filter_pages(
        &self,
        reference: &str,
    ) -> Result<Vec<FilterDefinition>, AgentError> {
        let mut values = Vec::new();
        let mut next = reference.to_owned();
        let mut visited = BTreeSet::new();
        for _ in 0..MAXIMUM_CAPABILITY_PAGES {
            if next.is_empty() {
                return Ok(values);
            }
            let target = resolve_reference(&next, self.service_origin())?;
            if !visited.insert(target.to_string()) {
                return Err(AgentError::InvalidResponse(
                    "ODP capability pagination loop detected".to_owned(),
                ));
            }
            let data = self
                .linked_odp(
                    target,
                    self.cache_fallbacks().capabilities,
                    validate_filter_page,
                )
                .await?;
            let page =
                parse_filter_definition_page(&normalize_agent_response(&data, "filter-page")?)?;
            values.extend(page.items);
            next = page.next;
        }
        if next.is_empty() {
            Ok(values)
        } else {
            Err(AgentError::InvalidResponse(
                "ODP capability source exceeded 16 pages".to_owned(),
            ))
        }
    }

    async fn load_sort_pages(&self, reference: &str) -> Result<Vec<SortDefinition>, AgentError> {
        let mut values = Vec::new();
        let mut next = reference.to_owned();
        let mut visited = BTreeSet::new();
        for _ in 0..MAXIMUM_CAPABILITY_PAGES {
            if next.is_empty() {
                return Ok(values);
            }
            let target = resolve_reference(&next, self.service_origin())?;
            if !visited.insert(target.to_string()) {
                return Err(AgentError::InvalidResponse(
                    "ODP capability pagination loop detected".to_owned(),
                ));
            }
            let data = self
                .linked_odp(
                    target,
                    self.cache_fallbacks().capabilities,
                    validate_sort_page,
                )
                .await?;
            let page = parse_sort_definition_page(&normalize_agent_response(&data, "sort-page")?)?;
            values.extend(page.items);
            next = page.next;
        }
        if next.is_empty() {
            Ok(values)
        } else {
            Err(AgentError::InvalidResponse(
                "ODP capability source exceeded 16 pages".to_owned(),
            ))
        }
    }
}

fn duplicate_ids<'a, T>(
    values: impl Iterator<Item = &'a str>,
    existing: &BTreeMap<String, T>,
) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut duplicates = BTreeSet::new();
    for value in values {
        if !seen.insert(value) || existing.contains_key(value) {
            duplicates.insert(value.to_owned());
        }
    }
    duplicates
}

fn report_duplicates(
    duplicates: &BTreeSet<String>,
    kind: CapabilityKind,
    scope: CapabilityScope,
    issues: &mut Vec<CapabilityIssue>,
) {
    if !duplicates.is_empty() {
        issues.push(CapabilityIssue {
            kind,
            message: format!(
                "Duplicate {}: {}",
                match kind {
                    CapabilityKind::Filters => "filters",
                    CapabilityKind::Sorts => "sorts",
                },
                duplicates.iter().cloned().collect::<Vec<_>>().join(", ")
            ),
            scope,
        });
    }
}

fn resolve_reference(reference: &str, origin: &str) -> Result<Url, AgentError> {
    let base = Url::parse(origin).map_err(|error| AgentError::InvalidRequest(error.to_string()))?;
    let target = base
        .join(reference)
        .map_err(|error| AgentError::InvalidResponse(error.to_string()))?;
    if !matches!(target.scheme(), "http" | "https") || target.host_str().is_none() {
        return Err(AgentError::InvalidResponse(
            "ODP capability reference must use HTTP or HTTPS".to_owned(),
        ));
    }
    Ok(target)
}

fn validate_filter_page(data: &[u8]) -> Result<(), AgentError> {
    parse_filter_definition_page(&normalize_agent_response(data, "filter-page")?)?;
    Ok(())
}

fn validate_sort_page(data: &[u8]) -> Result<(), AgentError> {
    parse_sort_definition_page(&normalize_agent_response(data, "sort-page")?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc};

    use async_trait::async_trait;
    use odp_directory::{HttpRequest, HttpResponse, Transport, TransportError};

    use super::*;

    struct DocumentTransport;

    #[async_trait]
    impl Transport for DocumentTransport {
        async fn send(&self, _request: HttpRequest) -> Result<HttpResponse, TransportError> {
            Ok(HttpResponse {
                body: br#"{"description":"Plants","http":{"endpoint_base":"/odp"},"language":"en","localizations":["en"],"name":"Plants","odp_version":"1.0","operations":[{"authentication":"not-required","name":"get-offering"},{"authentication":"not-required","name":"list-offerings"},{"authentication":"not-required","name":"search-offerings"}],"search_capabilities":{"filters":{"inline":[{"description":"Exact price","id":"price","operators":["eq"],"title":"Price","type":"number"}]},"sorts":{"inline":[{"description":"Lowest price first","id":"price-lowest","keys":[{"direction":"ascending","filter_id":"price","missing":"last"}],"title":"Lowest price"}]}}}"#.to_vec(),
                headers: BTreeMap::from([(
                    "content-type".to_owned(),
                    "application/odp+json".to_owned(),
                )]),
                status: 200,
            })
        }
    }

    #[tokio::test]
    async fn resolves_inline_sorts_to_their_filters() {
        let client =
            ServiceClient::with_transport("https://plants.example", Arc::new(DocumentTransport))
                .unwrap();
        let catalog = client.get_offering_search_capabilities(None).await.unwrap();
        assert!(catalog.issues.is_empty());
        assert_eq!(catalog.filters["price"].title, "Price");
        assert_eq!(catalog.sorts["price-lowest"].filters[0].id, "price");
    }

    /// A validated document never carries one, so the helper's own guard is checked here.
    #[test]
    fn refuses_a_capability_reference_that_is_not_an_http_url() {
        for reference in ["mailto:filters@plants.example", "file:///filters.json"] {
            let error = resolve_reference(reference, "https://plants.example").unwrap_err();
            assert!(error.to_string().contains("HTTP"), "{reference}: {error}");
        }
    }

    #[test]
    fn refuses_a_capability_reference_it_has_no_base_for() {
        let error = resolve_reference("/filters", "not a base").unwrap_err();
        assert!(matches!(error, AgentError::InvalidRequest(_)), "{error}");
    }
}
