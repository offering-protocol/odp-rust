use odp_core::{derive_service_origin, is_local_resource_identifier, parse_agent_service_document};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    CollectionResult, DirectoryError, DirectoryIssue, DirectoryResult, Facets, SearchResponse,
    ServiceResult,
};

pub(crate) fn decode(body: &[u8]) -> Result<SearchResponse, DirectoryError> {
    #[derive(Deserialize)]
    struct Envelope {
        items: Vec<Value>,
        facets: Option<Facets>,
        next: Option<String>,
        #[serde(flatten)]
        additional: odp_core::AdditionalMembers,
    }
    let envelope: Envelope = serde_json::from_slice(body).map_err(invalid)?;
    if envelope.items.len() > 100 {
        return Err(invalid("Directory response exceeds 100 results"));
    }
    if envelope
        .next
        .as_ref()
        .is_some_and(|next| next.trim().is_empty())
    {
        return Err(invalid("Directory continuation is empty"));
    }
    if envelope.facets.as_ref().is_some_and(|facets| {
        facets
            .trust
            .iter()
            .any(|facet| facet.value.name != odp_core::Protocol::Tap)
    }) {
        return Err(invalid("Directory trust facets are invalid"));
    }
    let mut items = Vec::new();
    let mut issues = Vec::new();
    for (index, raw) in envelope.items.into_iter().enumerate() {
        match result(raw) {
            Ok(item) => items.push(item),
            Err(error) => issues.push(DirectoryIssue {
                index,
                message: error.to_string(),
            }),
        }
    }
    Ok(SearchResponse {
        items,
        issues,
        facets: envelope.facets,
        next: envelope.next,
        additional: envelope.additional,
    })
}

fn result(mut raw: Value) -> Result<DirectoryResult, DirectoryError> {
    let kind = text(&raw, "type", 128)?.to_owned();
    if kind != "service" && kind != "collection" {
        return Ok(DirectoryResult::Unknown { kind, raw });
    }
    crate::client::require_indexed_at(raw.get("indexed_at"))?;
    let service = raw
        .get_mut("service")
        .ok_or_else(|| invalid("Missing service"))?;
    text(service, "service_id", 128)?;
    origin(service)?;
    crate::client::require_indexed_at(service.get("indexed_at"))?;
    let source = crate::sources::read(service.get("source"))?;
    let object = service
        .as_object_mut()
        .ok_or_else(|| invalid("service must be an object"))?;
    for name in [
        "branding",
        "http",
        "mcp",
        "odp_version",
        "payment_origins",
        "search_capabilities",
    ] {
        object.remove(name);
    }
    if source.source_type == "odp" {
        native_service(object)?;
    } else {
        crate::sources::imported_service(object)?;
    }
    finish_result(raw, &kind)
}

fn native_service(object: &mut serde_json::Map<String, Value>) -> Result<(), DirectoryError> {
    let mut projection = object.clone();
    projection.remove("source");
    let mut document = Value::Object(projection);
    document["odp_version"] = json!("1.0");
    document["http"] = json!({"endpoint_base":"/"});
    let parsed = parse_agent_service_document(&serde_json::to_vec(&document).map_err(invalid)?)
        .map_err(invalid)?;
    object.insert(
        "operations".to_owned(),
        serde_json::to_value(parsed.operations).map_err(invalid)?,
    );
    if let Some(protocols) = parsed.protocols {
        object.insert(
            "protocols".to_owned(),
            serde_json::to_value(protocols).map_err(invalid)?,
        );
    } else {
        object.remove("protocols");
    }
    Ok(())
}

fn finish_result(mut raw: Value, kind: &str) -> Result<DirectoryResult, DirectoryError> {
    if kind == "service" {
        if let Some(publisher) = raw.get("publisher").filter(|value| !value.is_null()) {
            text(publisher, "publisher_id", 128)?;
            text(publisher, "name", 128)?;
            let website = url::Url::parse(text(publisher, "website_url", 512)?).map_err(invalid)?;
            if website.scheme() != "https"
                || website.host_str().is_none()
                || !website.username().is_empty()
                || website.password().is_some()
            {
                return Err(invalid(
                    "Publisher website must be an HTTPS URL without credentials",
                ));
            }
        }
        raw.as_object_mut()
            .ok_or_else(|| invalid("Invalid result"))?
            .remove("type");
        Ok(DirectoryResult::Service(Box::new(
            serde_json::from_value::<ServiceResult>(raw).map_err(invalid)?,
        )))
    } else {
        let collection = raw
            .get("collection")
            .ok_or_else(|| invalid("Missing collection"))?;
        if !is_local_resource_identifier(text(collection, "id", 128)?) {
            return Err(invalid("collection.id must be a local resource identifier"));
        }
        text(collection, "name", 128)?;
        if let Some(description) = collection.get("description") {
            if description
                .as_str()
                .is_none_or(|value| value.chars().count() > 1024)
            {
                return Err(invalid(
                    "collection.description must be a string of at most 1024 characters",
                ));
            }
        }
        raw.as_object_mut()
            .ok_or_else(|| invalid("Invalid result"))?
            .remove("type");
        Ok(DirectoryResult::Collection(Box::new(
            serde_json::from_value::<CollectionResult>(raw).map_err(invalid)?,
        )))
    }
}

fn origin(value: &Value) -> Result<(), DirectoryError> {
    let origin = text(value, "service_origin", 2048)?;
    if !origin.starts_with("https://") || derive_service_origin(origin).map_err(invalid)? != origin
    {
        return Err(invalid("service_origin must be a canonical HTTPS origin"));
    }
    Ok(())
}

pub(crate) fn text<'a>(
    value: &'a Value,
    field: &str,
    maximum: usize,
) -> Result<&'a str, DirectoryError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty() && text.chars().count() <= maximum)
        .ok_or_else(|| {
            invalid(format!(
                "{field} must be a nonempty string of at most {maximum} characters"
            ))
        })
}

pub(crate) fn invalid(error: impl std::fmt::Display) -> DirectoryError {
    DirectoryError::InvalidResponse(error.to_string())
}
