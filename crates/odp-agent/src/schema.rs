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
const VOCABULARIES: &[&str] = &[
    "core",
    "applicator",
    "unevaluated",
    "validation",
    "meta-data",
    "format-annotation",
    "content",
];

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
        let resource = client
            .supporting_resource(
                url.as_str(),
                "attribute-schema",
                "application/schema+json",
                &["application/schema+json"],
                MAXIMUM_DOCUMENT_BYTES,
                client.cache_fallbacks().schema,
            )
            .await?;
        let mut document = resource.value;
        require_schema(&document)?;
        graph_bytes = graph_bytes.saturating_add(resource.bytes);
        if graph_bytes > MAXIMUM_GRAPH_BYTES {
            return Err(AgentError::InvalidResponse(
                "ODP Attribute Schema graph exceeds its byte limit".to_owned(),
            ));
        }
        let final_url = Url::parse(&resource.final_url)
            .map_err(|error| AgentError::InvalidResponse(error.to_string()))?;
        if let Some(object) = document.as_object_mut() {
            if object.get("$id").is_some_and(|id| !id.is_string()) {
                return Err(AgentError::InvalidResponse(
                    "Schema $id must be a string".to_owned(),
                ));
            }
            let identifier = object.get("$id").and_then(Value::as_str).unwrap_or("");
            let identifier = final_url
                .join(identifier)
                .map_err(|error| AgentError::InvalidResponse(error.to_string()))?;
            if identifier
                .fragment()
                .is_some_and(|fragment| !fragment.is_empty())
            {
                return Err(AgentError::InvalidResponse(
                    "Schema $id must not contain a fragment".to_owned(),
                ));
            }
            object.insert("$id".to_owned(), Value::String(identifier.to_string()));
        }
        for reference_url in schema_references(&document, &final_url)? {
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
    Ok((bundle(root, &documents)?, valid))
}

fn bundle(root: &Value, documents: &BTreeMap<String, Value>) -> Result<Value, AgentError> {
    let mut bundled = root.clone();
    let root_id = root.get("$id").and_then(Value::as_str).unwrap_or_default();
    let aliases = documents
        .iter()
        .map(|(retrieval, value)| {
            (
                retrieval.clone(),
                value
                    .get("$id")
                    .and_then(Value::as_str)
                    .unwrap_or(retrieval)
                    .to_owned(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    rewrite_references(
        &mut bundled,
        &Url::parse(root_id).map_err(|error| AgentError::InvalidResponse(error.to_string()))?,
        &aliases,
    )?;
    let object = bundled.as_object_mut().ok_or_else(|| {
        AgentError::InvalidResponse("Attribute Schema must be an object".to_owned())
    })?;
    let definitions = object
        .entry("$defs")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| AgentError::InvalidResponse("Schema $defs must be an object".to_owned()))?;
    let mut resources = BTreeSet::from([root_id.to_owned()]);
    for (retrieval_url, document) in documents {
        let identifier = document
            .get("$id")
            .and_then(Value::as_str)
            .unwrap_or(retrieval_url);
        if resources.insert(identifier.to_owned()) {
            let mut document = document.clone();
            rewrite_references(
                &mut document,
                &Url::parse(identifier)
                    .map_err(|error| AgentError::InvalidResponse(error.to_string()))?,
                &aliases,
            )?;
            insert_definition(definitions, document);
        }
    }
    Ok(bundled)
}

fn rewrite_references(
    value: &mut Value,
    base: &Url,
    aliases: &BTreeMap<String, String>,
) -> Result<(), AgentError> {
    let Some(values) = value.as_object_mut() else {
        return Ok(());
    };
    let base = match values.get("$id").and_then(Value::as_str) {
        Some(id) => base
            .join(id)
            .map_err(|error| AgentError::InvalidResponse(error.to_string()))?,
        None => base.clone(),
    };
    if let Some(reference) = values.get_mut("$ref") {
        if let Some(text) = reference.as_str() {
            let mut target = base
                .join(text)
                .map_err(|error| AgentError::InvalidResponse(error.to_string()))?;
            let fragment = target.fragment().map(str::to_owned);
            target.set_fragment(None);
            if let Some(canonical) = aliases.get(target.as_str()) {
                target = Url::parse(canonical)
                    .map_err(|error| AgentError::InvalidResponse(error.to_string()))?;
            }
            target.set_fragment(fragment.as_deref());
            *reference = Value::String(target.to_string());
        }
    }
    for (name, child) in values {
        match name.as_str() {
            "$defs" | "properties" | "patternProperties" | "dependentSchemas" => {
                if let Some(map) = child.as_object_mut() {
                    for child in map.values_mut() {
                        rewrite_references(child, &base, aliases)?;
                    }
                }
            }
            "allOf" | "anyOf" | "oneOf" | "prefixItems" => {
                if let Some(array) = child.as_array_mut() {
                    for child in array {
                        rewrite_references(child, &base, aliases)?;
                    }
                }
            }
            "not"
            | "if"
            | "then"
            | "else"
            | "items"
            | "contains"
            | "additionalProperties"
            | "propertyNames"
            | "unevaluatedItems"
            | "unevaluatedProperties"
            | "contentSchema" => rewrite_references(child, &base, aliases)?,
            _ => {}
        }
    }
    Ok(())
}

fn insert_definition(definitions: &mut Map<String, Value>, value: Value) {
    let mut index = definitions.len();
    loop {
        let name = format!("odp_resource_{index}");
        if !definitions.contains_key(&name) {
            definitions.insert(name, value);
            return;
        }
        index += 1;
    }
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
        if let Value::Object(values) = value {
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
                    if required == &Value::Bool(true)
                        && !url
                            .strip_prefix("https://json-schema.org/draft/2020-12/vocab/")
                            .is_some_and(|name| VOCABULARIES.contains(&name))
                    {
                        return Err(AgentError::InvalidResponse(format!(
                            "ODP Attribute Schema requires unsupported vocabulary {url}"
                        )));
                    }
                }
            }
            pending.extend(subschemas(values));
        }
    }
    Ok(())
}

fn schema_references(document: &Value, retrieval_url: &Url) -> Result<Vec<Url>, AgentError> {
    let mut references = Vec::new();
    let mut local_resources = BTreeSet::from([retrieval_url.to_string()]);
    let mut pending = vec![(document, retrieval_url.clone())];
    while let Some((value, inherited_base)) = pending.pop() {
        if let Value::Object(values) = value {
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
                subschemas(values)
                    .into_iter()
                    .map(|value| (value, base.clone())),
            );
        }
    }
    references.retain(|reference| !local_resources.contains(reference.as_str()));
    Ok(references)
}

fn subschemas(values: &Map<String, Value>) -> Vec<&Value> {
    let mut children = Vec::new();
    for name in [
        "$defs",
        "properties",
        "patternProperties",
        "dependentSchemas",
    ] {
        if let Some(Value::Object(map)) = values.get(name) {
            children.extend(map.values());
        }
    }
    for name in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if let Some(Value::Array(array)) = values.get(name) {
            children.extend(array);
        }
    }
    for name in [
        "not",
        "if",
        "then",
        "else",
        "items",
        "contains",
        "additionalProperties",
        "propertyNames",
        "unevaluatedItems",
        "unevaluatedProperties",
        "contentSchema",
    ] {
        if let Some(value) = values.get(name) {
            children.push(value);
        }
    }
    children
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
