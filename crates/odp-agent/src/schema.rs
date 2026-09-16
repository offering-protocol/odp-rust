use std::collections::{BTreeMap, BTreeSet, VecDeque};

use jsonschema::Registry;
use serde_json::{Map, Value};
use url::Url;

use crate::{AgentError, ServiceClient};

const DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";
const MAXIMUM_DEPTH: usize = 8;
const MAXIMUM_DOCUMENT_BYTES: usize = 262_144;
const MAXIMUM_DOCUMENTS: usize = 16;
const MAXIMUM_GRAPH_BYTES: usize = 1_048_576;
const STANDARD_VOCABULARY: &str = "https://json-schema.org/draft/2020-12/vocab/";

pub(crate) async fn resolve_schema(
    client: &ServiceClient,
    target: &str,
    attributes: Option<&BTreeMap<String, Value>>,
) -> Result<(Value, Option<bool>), AgentError> {
    let root_url = document_url(target)?;
    let mut documents = BTreeMap::new();
    let mut pending = VecDeque::from([(root_url.clone(), 0_usize)]);
    let mut graph_bytes = 0_usize;

    while let Some((url, depth)) = pending.pop_front() {
        if documents.contains_key(url.as_str()) {
            continue;
        }
        if documents.len() >= MAXIMUM_DOCUMENTS {
            return Err(AgentError::InvalidResponse(
                "ODP Attribute Schema graph exceeds 16 documents".to_owned(),
            ));
        }
        if depth > MAXIMUM_DEPTH {
            return Err(AgentError::InvalidResponse(
                "ODP Attribute Schema graph exceeds eight reference levels".to_owned(),
            ));
        }
        let document = client
            .supporting_json(
                url.as_str(),
                "attribute-schema",
                "application/schema+json",
                &["application/schema+json"],
                MAXIMUM_DOCUMENT_BYTES,
                client.cache_fallbacks().schema,
            )
            .await?;
        require_schema(&document)?;
        graph_bytes = graph_bytes.saturating_add(
            serde_json::to_vec(&document)
                .map_err(|error| AgentError::InvalidResponse(error.to_string()))?
                .len(),
        );
        if graph_bytes > MAXIMUM_GRAPH_BYTES {
            return Err(AgentError::InvalidResponse(
                "ODP Attribute Schema graph exceeds its byte limit".to_owned(),
            ));
        }
        for reference_url in schema_references(&document, &url)? {
            pending.push_back((reference_url, depth + 1));
        }
        documents.insert(url.to_string(), document);
    }

    let root = documents
        .get(root_url.as_str())
        .ok_or_else(|| AgentError::InvalidResponse("ODP Attribute Schema is missing".to_owned()))?;
    let mut registry = Registry::new();
    for (url, document) in &documents {
        registry = registry
            .add(url, document)
            .map_err(|error| AgentError::InvalidResponse(error.to_string()))?;
    }
    let registry = registry
        .prepare()
        .map_err(|error| AgentError::InvalidResponse(error.to_string()))?;
    let mut validation_root = root.clone();
    if let Some(object) = validation_root.as_object_mut() {
        object
            .entry("$id")
            .or_insert_with(|| Value::String(root_url.to_string()));
    }
    let validator = jsonschema::options()
        .with_registry(&registry)
        .build(&validation_root)
        .map_err(|error| AgentError::InvalidResponse(error.to_string()))?;
    let valid = attributes.map(|attributes| {
        serde_json::to_value(attributes)
            .map(|value| validator.is_valid(&value))
            .unwrap_or(false)
    });
    Ok((root.clone(), valid))
}

fn document_url(value: &str) -> Result<Url, AgentError> {
    let mut url =
        Url::parse(value).map_err(|error| AgentError::InvalidResponse(error.to_string()))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(AgentError::InvalidResponse(
            "ODP Attribute Schema references must use HTTPS".to_owned(),
        ));
    }
    url.set_fragment(None);
    Ok(url)
}

fn require_schema(document: &Value) -> Result<(), AgentError> {
    if document.get("$schema").and_then(Value::as_str) != Some(DIALECT) {
        return Err(AgentError::InvalidResponse(
            "ODP Attribute Schema must declare JSON Schema Draft 2020-12".to_owned(),
        ));
    }
    let mut pending = vec![document];
    while let Some(value) = pending.pop() {
        match value {
            Value::Array(values) => pending.extend(values),
            Value::Object(values) => {
                if let Some(reference) = values.get("$dynamicRef") {
                    if reference
                        .as_str()
                        .is_none_or(|reference| !reference.starts_with('#'))
                    {
                        return Err(AgentError::InvalidResponse(
                            "ODP Attribute Schema $dynamicRef must be a fragment-only reference"
                                .to_owned(),
                        ));
                    }
                }
                if let Some(vocabulary) = values.get("$vocabulary").and_then(Value::as_object) {
                    for (url, required) in vocabulary {
                        if required == &Value::Bool(true) && !url.starts_with(STANDARD_VOCABULARY) {
                            return Err(AgentError::InvalidResponse(format!(
                                "ODP Attribute Schema requires unsupported vocabulary {url}"
                            )));
                        }
                    }
                }
                pending.extend(values.values());
            }
            _ => {}
        }
    }
    Ok(())
}

fn schema_references(document: &Value, retrieval_url: &Url) -> Result<Vec<Url>, AgentError> {
    let mut references = Vec::new();
    let mut local_resources = BTreeSet::from([retrieval_url.to_string()]);
    let mut pending = vec![(document, retrieval_url.clone())];
    while let Some((value, inherited_base)) = pending.pop() {
        match value {
            Value::Array(values) => {
                pending.extend(values.iter().map(|value| (value, inherited_base.clone())));
            }
            Value::Object(values) => {
                let mut base = inherited_base;
                if let Some(identifier) = values.get("$id").and_then(Value::as_str) {
                    base = base
                        .join(identifier)
                        .map_err(|error| AgentError::InvalidResponse(error.to_string()))?;
                    base.set_fragment(None);
                    local_resources.insert(base.to_string());
                }
                add_reference(values, "$ref", &base, &mut references)?;
                pending.extend(
                    values
                        .iter()
                        .filter(|(name, _)| *name != "$ref")
                        .map(|(_, value)| (value, base.clone())),
                );
            }
            _ => {}
        }
    }
    references.retain(|reference| !local_resources.contains(reference.as_str()));
    Ok(references)
}

fn add_reference(
    values: &Map<String, Value>,
    name: &str,
    base: &Url,
    references: &mut Vec<Url>,
) -> Result<(), AgentError> {
    if let Some(reference) = values.get(name).and_then(Value::as_str) {
        let target = base
            .join(reference)
            .map_err(|error| AgentError::InvalidResponse(error.to_string()))?;
        references.push(document_url(target.as_str())?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, VecDeque},
        sync::{Arc, Mutex},
    };

    use async_trait::async_trait;
    use odp_directory::{HttpRequest, HttpResponse, Transport, TransportError};

    use super::*;

    struct MockTransport {
        responses: Mutex<VecDeque<HttpResponse>>,
    }

    #[async_trait]
    impl Transport for MockTransport {
        async fn send(&self, _request: HttpRequest) -> Result<HttpResponse, TransportError> {
            Ok(self.responses.lock().unwrap().pop_front().unwrap())
        }
    }

    fn response(body: &'static [u8]) -> HttpResponse {
        HttpResponse {
            body: body.to_vec(),
            headers: BTreeMap::from([(
                "content-type".to_owned(),
                "application/schema+json".to_owned(),
            )]),
            status: 200,
        }
    }

    fn recursive_schema_responses() -> VecDeque<HttpResponse> {
        VecDeque::from([
            response(
                br#"{"$id":"https://schemas.example/offering.json","$ref":"https://schemas.example/common.json","$schema":"https://json-schema.org/draft/2020-12/schema"}"#,
            ),
            response(
                br##"{"$dynamicAnchor":"node","$id":"https://schemas.example/common.json","$schema":"https://json-schema.org/draft/2020-12/schema","properties":{"children":{"items":{"$dynamicRef":"#node"},"type":"array"},"name":{"type":"string"}},"required":["name"],"type":"object"}"##,
            ),
        ])
    }

    #[tokio::test]
    async fn resolves_external_schema_graphs() {
        let transport = Arc::new(MockTransport {
            responses: Mutex::new(VecDeque::from([
                response(br#"{"$schema":"https://json-schema.org/draft/2020-12/schema","properties":{"plant":{"$ref":"plant.json"}},"type":"object"}"#),
                response(br#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"string"}"#),
            ])),
        });
        let client = ServiceClient::with_transport("https://service.example", transport.clone())
            .unwrap()
            .with_supporting_transport(transport);
        let valid = BTreeMap::from([("plant".to_owned(), Value::String("rubber".to_owned()))]);
        let invalid = BTreeMap::from([("plant".to_owned(), Value::from(4))]);

        assert_eq!(
            resolve_schema(&client, "https://schemas.example/root.json", Some(&valid))
                .await
                .unwrap()
                .1,
            Some(true)
        );

        let transport = Arc::new(MockTransport {
            responses: Mutex::new(VecDeque::from([
                response(br#"{"$schema":"https://json-schema.org/draft/2020-12/schema","properties":{"plant":{"$ref":"plant.json"}},"type":"object"}"#),
                response(br#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"string"}"#),
            ])),
        });
        let client = ServiceClient::with_transport("https://service.example", transport.clone())
            .unwrap()
            .with_supporting_transport(transport);
        assert_eq!(
            resolve_schema(&client, "https://schemas.example/root.json", Some(&invalid))
                .await
                .unwrap()
                .1,
            Some(false)
        );
    }

    #[tokio::test]
    async fn composes_external_schema_with_fragment_dynamic_reference() {
        let transport = Arc::new(MockTransport {
            responses: Mutex::new(recursive_schema_responses()),
        });
        let client = ServiceClient::with_transport("https://service.example", transport.clone())
            .unwrap()
            .with_supporting_transport(transport);
        let valid = BTreeMap::from([
            (
                "children".to_owned(),
                Value::Array(vec![Value::Object(Map::from_iter([(
                    "name".to_owned(),
                    Value::String("child".to_owned()),
                )]))]),
            ),
            ("name".to_owned(), Value::String("root".to_owned())),
        ]);

        assert_eq!(
            resolve_schema(
                &client,
                "https://schemas.example/offering.json",
                Some(&valid)
            )
            .await
            .unwrap()
            .1,
            Some(true)
        );

        let transport = Arc::new(MockTransport {
            responses: Mutex::new(recursive_schema_responses()),
        });
        let client = ServiceClient::with_transport("https://service.example", transport.clone())
            .unwrap()
            .with_supporting_transport(transport);
        let invalid = BTreeMap::from([
            (
                "children".to_owned(),
                Value::Array(vec![Value::Object(Map::from_iter([(
                    "name".to_owned(),
                    Value::from(1),
                )]))]),
            ),
            ("name".to_owned(), Value::String("root".to_owned())),
        ]);
        assert_eq!(
            resolve_schema(
                &client,
                "https://schemas.example/offering.json",
                Some(&invalid)
            )
            .await
            .unwrap()
            .1,
            Some(false)
        );
    }

    #[tokio::test]
    async fn rejects_external_dynamic_reference() {
        for document in [
            br#"{"$dynamicRef":"https://schemas.example/common.json#node","$schema":"https://json-schema.org/draft/2020-12/schema"}"#
                .as_slice(),
            br#"{"$dynamicRef":"common.json#node","$schema":"https://json-schema.org/draft/2020-12/schema"}"#
                .as_slice(),
            br#"{"$dynamicRef":null,"$schema":"https://json-schema.org/draft/2020-12/schema"}"#
                .as_slice(),
        ] {
            let transport = Arc::new(MockTransport {
                responses: Mutex::new(VecDeque::from([response(document)])),
            });
            let client = ServiceClient::with_transport("https://service.example", transport.clone())
                .unwrap()
                .with_supporting_transport(transport);

            let error = resolve_schema(&client, "https://schemas.example/root.json", None)
                .await
                .unwrap_err();
            assert!(matches!(
                error,
                AgentError::InvalidResponse(message)
                    if message == "ODP Attribute Schema $dynamicRef must be a fragment-only reference"
            ));
        }
    }

    #[tokio::test]
    async fn resolves_embedded_schema_resources() {
        let transport = Arc::new(MockTransport {
            responses: Mutex::new(VecDeque::from([response(
                br#"{"$defs":{"plant":{"$id":"plant.json","type":"string"}},"$schema":"https://json-schema.org/draft/2020-12/schema","properties":{"plant":{"$ref":"plant.json"}},"type":"object"}"#,
            )])),
        });
        let client = ServiceClient::with_transport("https://service.example", transport.clone())
            .unwrap()
            .with_supporting_transport(transport);
        let valid = BTreeMap::from([("plant".to_owned(), Value::String("rubber".to_owned()))]);
        let invalid = BTreeMap::from([("plant".to_owned(), Value::from(4))]);

        assert_eq!(
            resolve_schema(&client, "https://schemas.example/root.json", Some(&valid))
                .await
                .unwrap()
                .1,
            Some(true)
        );

        let transport = Arc::new(MockTransport {
            responses: Mutex::new(VecDeque::from([response(
                br#"{"$defs":{"plant":{"$id":"plant.json","type":"string"}},"$schema":"https://json-schema.org/draft/2020-12/schema","properties":{"plant":{"$ref":"plant.json"}},"type":"object"}"#,
            )])),
        });
        let client = ServiceClient::with_transport("https://service.example", transport.clone())
            .unwrap()
            .with_supporting_transport(transport);
        assert_eq!(
            resolve_schema(&client, "https://schemas.example/root.json", Some(&invalid))
                .await
                .unwrap()
                .1,
            Some(false)
        );
    }
}
