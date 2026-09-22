use std::collections::{BTreeMap, BTreeSet};

use odp_core::{
    Action, ActionRelation, ActionRequest, AuthenticationRequirement, Offering, OpenApiActionTarget,
};
use serde_json::Value;
use url::Url;

use crate::{AgentError, ServiceClient, schema::resolve_schema};

const MAXIMUM_OPENAPI_BYTES: usize = 1_048_576;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OfferingIssueScope {
    Action,
    AttributeSchema,
    Attributes,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OfferingIssue {
    pub action_id: Option<String>,
    pub message: String,
    pub scope: OfferingIssueScope,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DiscoveredHttpAction {
    pub method: String,
    pub request: Option<ActionRequest>,
    pub response_content_types: Vec<String>,
    pub url: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredOpenApiAction {
    pub operation_id: String,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DiscoveredAction {
    pub authentication: AuthenticationRequirement,
    pub description: String,
    pub http: Option<DiscoveredHttpAction>,
    pub id: String,
    pub openapi: Option<DiscoveredOpenApiAction>,
    pub rel: ActionRelation,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OfferingDetails {
    pub actions: Vec<DiscoveredAction>,
    pub attribute_schema: Option<Value>,
    pub issues: Vec<OfferingIssue>,
    pub offering: Offering,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedAction {
    pub action: DiscoveredAction,
    pub openapi_document: Option<Value>,
    pub operation: Option<Value>,
    pub request_schema: Option<Value>,
}

impl ServiceClient {
    pub async fn get_offering_details(&self, id: &str) -> Result<OfferingDetails, AgentError> {
        let inspection = self.inspect().await?;
        let mut offering = self.get_offering(id).await?;
        let service_openapi = inspection
            .document
            .http
            .openapi
            .as_ref()
            .map(|value| value.url.as_str())
            .unwrap_or_default();
        let (actions, mut issues) =
            normalize_actions(&offering.actions, self.service_origin(), service_openapi);
        let mut attribute_schema = None;
        if let Some(reference) = &offering.schema {
            match resolve_https_reference(&reference.url, self.service_origin()) {
                Ok(target) => match resolve_schema(self, &target, Some(&offering.attributes)).await
                {
                    Ok((schema, valid)) => {
                        if valid == Some(false) {
                            offering.attributes.clear();
                            issues.push(OfferingIssue {
                                action_id: None,
                                message: "Offering attributes do not match their Attribute Schema"
                                    .to_owned(),
                                scope: OfferingIssueScope::Attributes,
                            });
                        }
                        attribute_schema = Some(schema);
                    }
                    Err(error) => {
                        offering.attributes.clear();
                        issues.push(OfferingIssue {
                            action_id: None,
                            message: error.to_string(),
                            scope: OfferingIssueScope::AttributeSchema,
                        });
                    }
                },
                Err(error) => {
                    offering.attributes.clear();
                    issues.push(OfferingIssue {
                        action_id: None,
                        message: error.to_string(),
                        scope: OfferingIssueScope::AttributeSchema,
                    });
                }
            }
        }
        Ok(OfferingDetails {
            actions,
            attribute_schema,
            issues,
            offering,
        })
    }

    pub async fn resolve_action(
        &self,
        offering_id: &str,
        action_id: &str,
    ) -> Result<ResolvedAction, AgentError> {
        let details = self.get_offering_details(offering_id).await?;
        let action = details
            .actions
            .into_iter()
            .find(|action| action.id == action_id)
            .ok_or_else(|| {
                AgentError::InvalidRequest(format!(
                    "ODP Offering does not expose usable Action {action_id}"
                ))
            })?;
        let mut result = ResolvedAction {
            action,
            openapi_document: None,
            operation: None,
            request_schema: None,
        };
        if let Some(http) = &result.action.http {
            if let Some(reference) = http
                .request
                .as_ref()
                .and_then(|value| value.schema.as_ref())
            {
                let target = resolve_https_reference(&reference.url, self.service_origin())?;
                result.request_schema = Some(resolve_schema(self, &target, None).await?.0);
            }
            return Ok(result);
        }
        let openapi = result.action.openapi.as_ref().ok_or_else(|| {
            AgentError::InvalidResponse("ODP Action has no usable target".to_owned())
        })?;
        let (document, operation) = self
            .resolve_openapi(&openapi.url, &openapi.operation_id)
            .await?;
        result.openapi_document = Some(document);
        result.operation = Some(operation);
        Ok(result)
    }

    async fn resolve_openapi(
        &self,
        target: &str,
        operation_id: &str,
    ) -> Result<(Value, Value), AgentError> {
        let document = self
            .supporting_json(
                target,
                "openapi",
                "application/vnd.oai.openapi+json;version=3.1, application/json;q=0.9",
                &["application/vnd.oai.openapi+json", "application/json"],
                MAXIMUM_OPENAPI_BYTES,
                // CCH-02 names no fallback for an OpenAPI document, so one is only cached when the
                // Service says how long it stays fresh.
                std::time::Duration::ZERO,
            )
            .await?;
        let version = document
            .get("openapi")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !version.starts_with("3.1.") {
            return Err(AgentError::InvalidResponse(
                "ODP Action requires an OpenAPI 3.1 document".to_owned(),
            ));
        }
        let paths = document
            .get("paths")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                AgentError::InvalidResponse("ODP OpenAPI document must contain paths".to_owned())
            })?;
        let mut matches = Vec::new();
        for path in paths.values().filter_map(Value::as_object) {
            for method in [
                "delete", "get", "head", "options", "patch", "post", "put", "trace",
            ] {
                if let Some(operation) = path.get(method).filter(|value| {
                    value.get("operationId").and_then(Value::as_str) == Some(operation_id)
                }) {
                    matches.push(operation.clone());
                }
            }
        }
        if matches.len() != 1 {
            return Err(AgentError::InvalidResponse(format!(
                "ODP Action operation_id {operation_id} must resolve exactly once"
            )));
        }
        Ok((document, matches.remove(0)))
    }
}

fn normalize_actions(
    actions: &[Action],
    service_origin: &str,
    service_openapi: &str,
) -> (Vec<DiscoveredAction>, Vec<OfferingIssue>) {
    let mut counts = BTreeMap::new();
    for action in actions {
        *counts.entry(action.id.as_str()).or_insert(0_usize) += 1;
    }
    let mut reported = BTreeSet::new();
    let mut discovered = Vec::new();
    let mut issues = Vec::new();
    for action in actions {
        if counts.get(action.id.as_str()).copied().unwrap_or_default() > 1 {
            if reported.insert(action.id.clone()) {
                issues.push(action_issue(
                    action,
                    format!("Duplicate Action identifier {}", action.id),
                ));
            }
            continue;
        }
        match normalize_action(action, service_origin, service_openapi) {
            Ok(Some(value)) => discovered.push(value),
            Ok(None) => {}
            Err(error) => issues.push(action_issue(action, error.to_string())),
        }
    }
    (discovered, issues)
}

fn normalize_action(
    action: &Action,
    service_origin: &str,
    service_openapi: &str,
) -> Result<Option<DiscoveredAction>, AgentError> {
    let mut discovered = DiscoveredAction {
        authentication: action.authentication,
        description: action.description.clone(),
        http: None,
        id: action.id.clone(),
        openapi: None,
        rel: action.rel.clone(),
    };
    if let Some(http) = &action.http {
        discovered.http = Some(DiscoveredHttpAction {
            method: http.method.clone(),
            request: http.request.clone(),
            response_content_types: http.response_content_types.clone(),
            url: resolve_http_reference(&http.href, service_origin)?,
        });
        return Ok(Some(discovered));
    }
    if let Some(openapi) = &action.openapi {
        let target = openapi_target(openapi, service_openapi)?;
        discovered.openapi = Some(DiscoveredOpenApiAction {
            operation_id: openapi.operation_id.clone(),
            url: resolve_https_reference(target, service_origin)?,
        });
        return Ok(Some(discovered));
    }
    Ok(None)
}

fn openapi_target<'a>(
    action: &'a OpenApiActionTarget,
    service_openapi: &'a str,
) -> Result<&'a str, AgentError> {
    if !action.url.is_empty() {
        Ok(&action.url)
    } else if !service_openapi.is_empty() {
        Ok(service_openapi)
    } else {
        Err(AgentError::InvalidResponse(
            "OpenAPI Action has no OpenAPI document URL".to_owned(),
        ))
    }
}

fn resolve_http_reference(reference: &str, base: &str) -> Result<String, AgentError> {
    let base = Url::parse(base).map_err(|error| AgentError::InvalidRequest(error.to_string()))?;
    let target = base
        .join(reference)
        .map_err(|error| AgentError::InvalidResponse(error.to_string()))?;
    if !matches!(target.scheme(), "http" | "https") || target.host_str().is_none() {
        return Err(AgentError::InvalidResponse(
            "ODP Action target must use HTTP or HTTPS".to_owned(),
        ));
    }
    Ok(target.to_string())
}

fn resolve_https_reference(reference: &str, base: &str) -> Result<String, AgentError> {
    let target = resolve_http_reference(reference, base)?;
    if Url::parse(&target)
        .map(|value| value.scheme() != "https")
        .unwrap_or(true)
    {
        return Err(AgentError::InvalidResponse(
            "ODP supporting document URL must use HTTPS".to_owned(),
        ));
    }
    Ok(target)
}

fn action_issue(action: &Action, message: String) -> OfferingIssue {
    OfferingIssue {
        action_id: Some(action.id.clone()),
        message,
        scope: OfferingIssueScope::Action,
    }
}

#[cfg(test)]
mod tests {
    use odp_core::parse_offering;

    use super::*;

    #[test]
    fn normalizes_relative_action_targets_without_invoking_them() {
        let offering = parse_offering(br#"{"actions":[{"authentication":"not-required","description":"Download","http":{"href":"/downloads/plant.pdf","method":"GET","response_content_types":["application/pdf"]},"id":"download","rel":"download"}],"id":"plant","name":"Plant","odp_version":"1.0"}"#).unwrap();
        let (actions, issues) = normalize_actions(&offering.actions, "https://plants.example", "");
        assert!(issues.is_empty());
        assert_eq!(
            actions[0].http.as_ref().map(|value| value.url.as_str()),
            Some("https://plants.example/downloads/plant.pdf")
        );
    }

    /// The Offering schema keeps these shapes out of a parsed document, so they are checked
    /// here: the helpers also run against a Service Origin, which no schema validates.
    #[test]
    fn refuses_an_action_target_that_is_not_an_http_url() {
        for reference in ["mailto:sales@plants.example", "file:///etc/passwd"] {
            let error = resolve_http_reference(reference, "https://plants.example").unwrap_err();
            assert!(error.to_string().contains("HTTP"), "{reference}: {error}");
        }
    }

    #[test]
    fn refuses_a_supporting_document_that_is_not_served_over_https() {
        let error =
            resolve_https_reference("http://plants.example/s.json", "https://plants.example")
                .unwrap_err();
        assert!(error.to_string().contains("HTTPS"), "{error}");
    }

    #[test]
    fn refuses_a_reference_it_has_no_base_for() {
        let error = resolve_http_reference("/plants", "not a base").unwrap_err();
        assert!(matches!(error, AgentError::InvalidRequest(_)), "{error}");
    }

    /// An Action describing no target at all is passed over quietly: there is nothing to call.
    #[test]
    fn passes_over_an_action_that_names_no_target() {
        let action = Action {
            authentication: AuthenticationRequirement::NotRequired,
            description: "Ask us".to_owned(),
            http: None,
            id: "ask".to_owned(),
            openapi: None,
            rel: ActionRelation::Other("contact".to_owned()),
        };
        let (actions, issues) = normalize_actions(
            std::slice::from_ref(&action),
            "https://plants.example",
            "https://plants.example/openapi.json",
        );
        assert!(actions.is_empty());
        assert!(issues.is_empty());
    }
}
