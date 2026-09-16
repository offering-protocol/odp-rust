use thiserror::Error;
use url::{Host, Url};

use crate::Operation;

#[derive(Clone, Debug, Error, PartialEq)]
pub enum ReferenceError {
    #[error("invalid ODP URL: {0}")]
    InvalidUrl(String),
    #[error("ODP URL must include a host")]
    MissingHost,
    #[error("ODP URL cannot contain user information")]
    UserInformation,
    #[error("ODP URL must use HTTPS except on loopback hosts")]
    InsecureUrl,
    #[error(
        "ODP resource reference must be an origin-relative absolute path or secure absolute URL"
    )]
    InvalidReference,
    #[error("ODP resource reference cannot be scheme-relative")]
    SchemeRelativeReference,
    #[error("ODP resource reference cannot contain a fragment")]
    Fragment,
    #[error("ODP continuation reference must remain on the Service origin")]
    CrossOriginContinuation,
    #[error("ODP endpoint base must be an origin-relative absolute path")]
    InvalidEndpointBase,
    #[error("{0:?} requires a valid local resource identifier")]
    InvalidResourceIdentifier(Operation),
    #[error("{0:?} does not accept a resource identifier")]
    UnexpectedResourceIdentifier(Operation),
}

pub fn is_local_resource_identifier(value: &str) -> bool {
    !matches!(value, "." | "..")
        && !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._~-".contains(&byte))
}

pub const fn operation_method(operation: Operation) -> &'static str {
    match operation {
        Operation::SearchCollections | Operation::SearchOfferings => "POST",
        _ => "GET",
    }
}

pub fn derive_service_origin(service_document_url: &str) -> Result<String, ReferenceError> {
    let url = parse_url(service_document_url)?;
    require_secure_url(&url)?;
    Ok(url.origin().ascii_serialization())
}

pub fn resolve_resource_reference(
    reference: &str,
    service_origin: &str,
) -> Result<Url, ReferenceError> {
    if reference.starts_with("//") {
        return Err(ReferenceError::SchemeRelativeReference);
    }
    if !reference.starts_with('/')
        && !reference.starts_with("https://")
        && !reference.starts_with("http://localhost")
        && !reference.starts_with("http://127.0.0.1")
        && !reference.starts_with("http://[::1]")
    {
        return Err(ReferenceError::InvalidReference);
    }
    let origin = parse_url(service_origin)?;
    let resolved = origin
        .join(reference)
        .map_err(|error| ReferenceError::InvalidUrl(error.to_string()))?;
    if !resolved.username().is_empty() || resolved.password().is_some() {
        return Err(ReferenceError::UserInformation);
    }
    if resolved.fragment().is_some() {
        return Err(ReferenceError::Fragment);
    }
    require_secure_url(&resolved)?;
    Ok(resolved)
}

pub fn resolve_continuation(reference: &str, service_origin: &str) -> Result<Url, ReferenceError> {
    let resolved = resolve_resource_reference(reference, service_origin)?;
    if resolved.origin().ascii_serialization() != derive_service_origin(service_origin)? {
        return Err(ReferenceError::CrossOriginContinuation);
    }
    Ok(resolved)
}

pub fn build_operation_url(
    endpoint_base: &str,
    operation: Operation,
    service_origin: &str,
    id: Option<&str>,
) -> Result<Url, ReferenceError> {
    if !endpoint_base.starts_with('/') || endpoint_base.starts_with("//") {
        return Err(ReferenceError::InvalidEndpointBase);
    }
    let path = operation_path(operation, id)?;
    resolve_resource_reference(
        &format!("{}{}", endpoint_base.trim_end_matches('/'), path),
        service_origin,
    )
}

fn operation_path(operation: Operation, id: Option<&str>) -> Result<String, ReferenceError> {
    let resource = matches!(
        operation,
        Operation::GetCollection | Operation::GetOffering | Operation::ListCollectionOfferings
    );
    if resource && !id.is_some_and(is_local_resource_identifier) {
        return Err(ReferenceError::InvalidResourceIdentifier(operation));
    }
    if !resource && id.is_some() {
        return Err(ReferenceError::UnexpectedResourceIdentifier(operation));
    }
    let id = id.unwrap_or_default();
    Ok(match operation {
        Operation::ListCollections => "/collections".to_owned(),
        Operation::SearchCollections => "/collections/search".to_owned(),
        Operation::GetCollection => format!("/collections/{id}"),
        Operation::ListCollectionOfferings => format!("/collections/{id}/offerings"),
        Operation::ListOfferings => "/offerings".to_owned(),
        Operation::SearchOfferings => "/offerings/search".to_owned(),
        Operation::GetOffering => format!("/offerings/{id}"),
    })
}

fn parse_url(value: &str) -> Result<Url, ReferenceError> {
    let url = Url::parse(value).map_err(|error| ReferenceError::InvalidUrl(error.to_string()))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ReferenceError::UserInformation);
    }
    Ok(url)
}

fn require_secure_url(url: &Url) -> Result<(), ReferenceError> {
    // `host_str` spells an IPv6 literal with its brackets, which is not an address any parser
    // reads, so the host is taken apart rather than parsed back out of its URL spelling.
    let loopback = match url.host().ok_or(ReferenceError::MissingHost)? {
        Host::Domain(name) => name.eq_ignore_ascii_case("localhost"),
        Host::Ipv4(address) => address.is_loopback(),
        Host::Ipv6(address) => address.is_loopback(),
    };
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err(ReferenceError::InsecureUrl);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizes_service_origin() {
        assert_eq!(
            derive_service_origin("https://EXAMPLE.com:443/.well-known/odp").unwrap(),
            "https://example.com"
        );
    }

    #[test]
    fn rejects_cross_origin_continuation() {
        assert_eq!(
            resolve_continuation("https://other.example/next", "https://example.com"),
            Err(ReferenceError::CrossOriginContinuation)
        );
    }

    #[test]
    fn rejects_user_information_in_resource_reference() {
        assert_eq!(
            resolve_resource_reference(
                "https://user@catalog.example/offerings/123",
                "https://example.com"
            ),
            Err(ReferenceError::UserInformation)
        );
    }

    #[test]
    fn builds_fixed_operation_path() {
        assert_eq!(
            build_operation_url(
                "/odp",
                Operation::GetOffering,
                "https://example.com",
                Some("plant-1")
            )
            .unwrap()
            .as_str(),
            "https://example.com/odp/offerings/plant-1"
        );
    }
}
