use std::collections::BTreeSet;

use odp_core::validate_value;
use serde_json::{Map, Value};
use url::Url;

use crate::{
    DirectoryError, DirectorySource,
    results::{invalid, text},
};

pub(crate) fn read(value: Option<&Value>) -> Result<DirectorySource, DirectoryError> {
    let value = value.ok_or_else(|| invalid("Missing source"))?;
    text(value, "type", 128)?;
    let exact = text(value, "url", 2048)?;
    let url = Url::parse(exact).map_err(invalid)?;
    if !exact
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
        || exact.chars().any(char::is_whitespace)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid(
            "source.url must be an HTTPS document URL without credentials or fragment",
        ));
    }
    crate::client::require_public_host(&url)?;
    serde_json::from_value(value.clone()).map_err(invalid)
}

pub(crate) fn imported_service(object: &mut Map<String, Value>) -> Result<(), DirectoryError> {
    let name = object.get("name").and_then(Value::as_str);
    if name.is_none_or(|name| name.trim().is_empty() || name.chars().count() > 128) {
        return Err(invalid("Imported Service name is invalid"));
    }
    for field in [
        "description",
        "language",
        "documentation_url",
        "status_url",
        "support_url",
        "website_url",
    ] {
        if object.get(field).is_some_and(|value| !value.is_string()) {
            return Err(invalid(format!("{field} must be a string")));
        }
    }
    object.remove("operations");
    if let Some(protocols) = object.get("protocols") {
        let protocols = protocols
            .as_object()
            .ok_or_else(|| invalid("protocols must be an object"))?;
        let mut retained = Map::new();
        for (category, known, schema) in [
            (
                "enrollment",
                &["aep"][..],
                "enrollment-protocol.schema.json",
            ),
            (
                "payments",
                &["mpp", "x402"][..],
                "payment-protocol.schema.json",
            ),
            ("trust", &["tap"][..], "trust-protocol.schema.json"),
        ] {
            let Some(values) = protocols.get(category) else {
                continue;
            };
            let values = values
                .as_array()
                .filter(|values| !values.is_empty())
                .ok_or_else(|| invalid(format!("protocols.{category} must be a nonempty array")))?;
            let mut selected = Vec::new();
            let mut names = BTreeSet::new();
            for descriptor in values {
                let name = text(descriptor, "name", 128)?;
                if !known.contains(&name) {
                    continue;
                }
                if !names.insert(name) {
                    return Err(invalid(format!("Duplicate {category} descriptor")));
                }
                validate_value(descriptor, schema, category).map_err(invalid)?;
                selected.push(descriptor.clone());
            }
            if !selected.is_empty() {
                retained.insert(category.to_owned(), Value::Array(selected));
            }
        }
        object.insert("protocols".to_owned(), Value::Object(retained));
    }
    Ok(())
}
