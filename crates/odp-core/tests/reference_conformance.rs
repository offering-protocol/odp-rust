//! SEC-05 and the URL rules: where a reference written by somebody else is allowed to point.

use odp_core::{
    Operation, ReferenceError, ResourceIdentity, ResourceType, build_operation_url,
    derive_service_origin, is_local_resource_identifier, is_public, operation_method,
    resolve_continuation, resolve_resource_reference,
};

// -- Service Origin ---------------------------------------------------------------------------------

/// SVC-07: the Service Origin is the scheme, host and port of the Service Document URL, and the
/// canonical spelling of those, so two spellings of one Service are one Service.
#[test]
fn canonicalizes_a_service_origin() {
    for (url, origin) in [
        (
            "https://plants.example/.well-known/odp",
            "https://plants.example",
        ),
        (
            "https://PLANTS.example/.well-known/odp",
            "https://plants.example",
        ),
        (
            "https://plants.example:443/.well-known/odp",
            "https://plants.example",
        ),
        (
            "https://plants.example:8443/.well-known/odp",
            "https://plants.example:8443",
        ),
        (
            "http://localhost:3000/.well-known/odp",
            "http://localhost:3000",
        ),
        (
            "http://127.0.0.1:3000/.well-known/odp",
            "http://127.0.0.1:3000",
        ),
    ] {
        assert_eq!(derive_service_origin(url).unwrap(), origin, "{url}");
    }
}

/// SEC-05: ODP travels over HTTPS, and the one exception is a loopback host a developer is running
/// against.
#[test]
fn refuses_a_service_document_url_that_is_not_secure() {
    assert_eq!(
        derive_service_origin("http://plants.example/.well-known/odp"),
        Err(ReferenceError::InsecureUrl)
    );
    // A host merely beginning with a loopback name is another host entirely.
    assert_eq!(
        derive_service_origin("http://localhost.plants.example/.well-known/odp"),
        Err(ReferenceError::InsecureUrl)
    );
    assert!(derive_service_origin("http://[::1]:3000/.well-known/odp").is_ok());
}

/// Credentials in a URL are a way to make one host look like another, so they are refused outright.
#[test]
fn refuses_credentials_in_a_service_document_url() {
    for url in [
        "https://user@plants.example/.well-known/odp",
        "https://user:secret@plants.example/.well-known/odp",
    ] {
        assert_eq!(
            derive_service_origin(url),
            Err(ReferenceError::UserInformation),
            "{url}"
        );
    }
}

#[test]
fn refuses_a_service_document_url_that_is_not_a_url() {
    assert!(matches!(
        derive_service_origin("not a url"),
        Err(ReferenceError::InvalidUrl(_))
    ));
    assert_eq!(
        derive_service_origin("mailto:plants@example.com"),
        Err(ReferenceError::MissingHost)
    );
}

// -- resource references ------------------------------------------------------------------------------

/// REF-01: a reference is an origin-relative absolute path or a secure absolute URL, so a relative
/// path that would resolve against whatever came before it is not one.
#[test]
fn resolves_a_reference_the_spec_allows() {
    for (reference, resolved) in [
        (
            "/odp/offerings/plant-1",
            "https://plants.example/odp/offerings/plant-1",
        ),
        (
            "/odp/offerings?limit=10",
            "https://plants.example/odp/offerings?limit=10",
        ),
        ("https://cdn.example/a.png", "https://cdn.example/a.png"),
    ] {
        assert_eq!(
            resolve_resource_reference(reference, "https://plants.example")
                .unwrap()
                .as_str(),
            resolved
        );
    }
}

#[test]
fn refuses_a_reference_that_resolves_against_nothing_fixed() {
    for reference in ["offerings/plant-1", "./a.png", "../a.png", "a.png"] {
        assert_eq!(
            resolve_resource_reference(reference, "https://plants.example"),
            Err(ReferenceError::InvalidReference),
            "{reference}"
        );
    }
}

/// A scheme-relative reference takes its scheme from the page that carried it, which is exactly the
/// ambiguity ODP removes.
#[test]
fn refuses_a_scheme_relative_reference() {
    assert_eq!(
        resolve_resource_reference("//cdn.example/a.png", "https://plants.example"),
        Err(ReferenceError::SchemeRelativeReference)
    );
}

/// A fragment addresses part of a document, and ODP references address documents.
#[test]
fn refuses_a_reference_carrying_a_fragment() {
    assert_eq!(
        resolve_resource_reference("/odp/offerings#first", "https://plants.example"),
        Err(ReferenceError::Fragment)
    );
}

#[test]
fn refuses_a_reference_carrying_credentials() {
    assert_eq!(
        resolve_resource_reference("https://user@cdn.example/a.png", "https://plants.example"),
        Err(ReferenceError::UserInformation)
    );
}

/// SEC-05 again, from the other end: an absolute reference to a plaintext host is refused, and a
/// loopback one is the exception.
#[test]
fn refuses_an_absolute_reference_that_is_not_secure() {
    assert_eq!(
        resolve_resource_reference("http://cdn.example/a.png", "https://plants.example"),
        Err(ReferenceError::InvalidReference)
    );
    for reference in [
        "http://localhost:3000/a.png",
        "http://127.0.0.1:3000/a.png",
        "http://[::1]:3000/a.png",
    ] {
        assert!(
            resolve_resource_reference(reference, "http://localhost:3000").is_ok(),
            "{reference}"
        );
    }
}

/// A host that merely starts with a loopback name is refused when the resolution is done, whatever
/// the prefix looked like.
#[test]
fn refuses_a_host_that_only_looks_like_loopback() {
    for reference in [
        "http://localhost.plants.example/a.png",
        "http://127.0.0.1.plants.example/a.png",
    ] {
        assert_eq!(
            resolve_resource_reference(reference, "https://plants.example"),
            Err(ReferenceError::InsecureUrl),
            "{reference}"
        );
    }
}

#[test]
fn refuses_a_service_origin_that_is_not_a_url() {
    assert!(matches!(
        resolve_resource_reference("/odp/offerings", "not a url"),
        Err(ReferenceError::InvalidUrl(_))
    ));
}

// -- continuations -------------------------------------------------------------------------------------

/// PAG-08: a continuation stays on the Service that issued it, so following one cannot be redirected
/// into somebody else's catalog.
#[test]
fn keeps_a_continuation_on_the_service_that_issued_it() {
    assert_eq!(
        resolve_continuation("/odp/offerings?cursor=2", "https://plants.example")
            .unwrap()
            .as_str(),
        "https://plants.example/odp/offerings?cursor=2"
    );
    assert_eq!(
        resolve_continuation("https://plants.example/next", "https://plants.example")
            .unwrap()
            .as_str(),
        "https://plants.example/next"
    );
    assert_eq!(
        resolve_continuation("https://elsewhere.example/next", "https://plants.example"),
        Err(ReferenceError::CrossOriginContinuation)
    );
}

// -- operation URLs --------------------------------------------------------------------------------------

/// SVC-65: every operation hangs off the advertised endpoint base at the path ODP fixes for it.
#[test]
fn builds_the_path_odp_fixes_for_each_operation() {
    for (operation, id, path) in [
        (Operation::ListCollections, None, "/odp/collections"),
        (
            Operation::SearchCollections,
            None,
            "/odp/collections/search",
        ),
        (
            Operation::GetCollection,
            Some("plants"),
            "/odp/collections/plants",
        ),
        (
            Operation::ListCollectionOfferings,
            Some("plants"),
            "/odp/collections/plants/offerings",
        ),
        (Operation::ListOfferings, None, "/odp/offerings"),
        (Operation::SearchOfferings, None, "/odp/offerings/search"),
        (
            Operation::GetOffering,
            Some("plant-1"),
            "/odp/offerings/plant-1",
        ),
    ] {
        assert_eq!(
            build_operation_url("/odp", operation, "https://plants.example", id)
                .unwrap()
                .path(),
            path,
            "{operation:?}"
        );
    }
}

/// A base of `/` and a base with a trailing slash name the same place.
#[test]
fn reads_every_spelling_of_one_endpoint_base() {
    for base in ["/odp", "/odp/"] {
        assert_eq!(
            build_operation_url(
                base,
                Operation::ListOfferings,
                "https://plants.example",
                None
            )
            .unwrap()
            .path(),
            "/odp/offerings",
            "{base}"
        );
    }
    assert_eq!(
        build_operation_url(
            "/",
            Operation::ListOfferings,
            "https://plants.example",
            None
        )
        .unwrap()
        .path(),
        "/offerings"
    );
}

/// SVC-64: the endpoint base is origin-relative, so it cannot send the Agent to another host.
#[test]
fn refuses_an_endpoint_base_that_leaves_the_origin() {
    for base in [
        "odp",
        "//elsewhere.example/odp",
        "https://elsewhere.example/odp",
    ] {
        assert_eq!(
            build_operation_url(
                base,
                Operation::ListOfferings,
                "https://plants.example",
                None
            ),
            Err(ReferenceError::InvalidEndpointBase),
            "{base}"
        );
    }
}

/// An operation that addresses one resource needs a valid identifier, and one that addresses a
/// collection of them takes none.
#[test]
fn refuses_an_identifier_the_operation_cannot_use() {
    for operation in [
        Operation::GetCollection,
        Operation::GetOffering,
        Operation::ListCollectionOfferings,
    ] {
        for id in [
            None,
            Some(""),
            Some("."),
            Some(".."),
            Some("a/b"),
            Some("a?b"),
        ] {
            assert_eq!(
                build_operation_url("/odp", operation, "https://plants.example", id),
                Err(ReferenceError::InvalidResourceIdentifier(operation)),
                "{operation:?} {id:?}"
            );
        }
    }

    for operation in [
        Operation::ListCollections,
        Operation::ListOfferings,
        Operation::SearchCollections,
        Operation::SearchOfferings,
    ] {
        assert_eq!(
            build_operation_url("/odp", operation, "https://plants.example", Some("plant-1")),
            Err(ReferenceError::UnexpectedResourceIdentifier(operation)),
            "{operation:?}"
        );
    }
}

/// REF-04: an identifier is a bounded run of unreserved characters, so it cannot carry a path
/// segment, a query, or a traversal.
#[test]
fn reads_a_local_resource_identifier() {
    for id in [
        "a",
        "plant-1",
        "plant_1",
        "plant.1",
        "plant~1",
        &"a".repeat(128),
    ] {
        assert!(is_local_resource_identifier(id), "{id}");
    }
    for id in [
        "",
        ".",
        "..",
        "a/b",
        "a b",
        "a%2Fb",
        "a#b",
        "\u{1f33f}",
        &"a".repeat(129),
    ] {
        assert!(!is_local_resource_identifier(id), "{id}");
    }
}

/// SVC-66: a search is a POST because its request is a document; everything else is a GET.
#[test]
fn names_the_method_each_operation_uses() {
    for operation in [Operation::SearchCollections, Operation::SearchOfferings] {
        assert_eq!(operation_method(operation), "POST", "{operation:?}");
    }
    for operation in [
        Operation::GetCollection,
        Operation::GetOffering,
        Operation::ListCollectionOfferings,
        Operation::ListCollections,
        Operation::ListOfferings,
    ] {
        assert_eq!(operation_method(operation), "GET", "{operation:?}");
    }
}

// -- resource identity -----------------------------------------------------------------------------------

/// REF-10: one resource is identified by its Service, its kind and its identifier together, so the
/// same identifier at two Services is two resources.
#[test]
fn composes_an_identity_that_two_services_cannot_collide_on() {
    let here = ResourceIdentity::new(
        "https://plants.example/.well-known/odp",
        ResourceType::Offering,
        "plant-1",
    )
    .unwrap();
    let there = ResourceIdentity::new(
        "https://other.example/.well-known/odp",
        ResourceType::Offering,
        "plant-1",
    )
    .unwrap();
    let collection = ResourceIdentity::new(
        "https://plants.example/.well-known/odp",
        ResourceType::Collection,
        "plant-1",
    )
    .unwrap();

    assert_eq!(here.key(), "https://plants.example\0offering\0plant-1");
    assert_eq!(
        collection.key(),
        "https://plants.example\0collection\0plant-1"
    );
    assert_ne!(here.key(), there.key());
    assert_ne!(here.key(), collection.key());
}

/// An identity is built from the same identifier rule a URL is, and names the operation that would
/// have addressed the resource.
#[test]
fn refuses_an_identity_whose_identifier_addresses_nothing() {
    assert_eq!(
        ResourceIdentity::new(
            "https://plants.example/.well-known/odp",
            ResourceType::Offering,
            "a/b"
        ),
        Err(ReferenceError::InvalidResourceIdentifier(
            Operation::GetOffering
        ))
    );
    assert_eq!(
        ResourceIdentity::new(
            "https://plants.example/.well-known/odp",
            ResourceType::Collection,
            ".."
        ),
        Err(ReferenceError::InvalidResourceIdentifier(
            Operation::GetCollection
        ))
    );
}

#[test]
fn refuses_an_identity_at_a_service_that_is_not_secure() {
    assert_eq!(
        ResourceIdentity::new(
            "http://plants.example/.well-known/odp",
            ResourceType::Offering,
            "plant-1"
        ),
        Err(ReferenceError::InsecureUrl)
    );
}

// -- addresses ---------------------------------------------------------------------------------------------

/// SEC-08: an address the public internet does not route is one a name a third party controls
/// should not be able to reach.
#[test]
fn judges_an_address_by_the_special_purpose_registries() {
    for value in ["8.8.8.8", "203.1.113.1", "2606:4700::1111"] {
        assert!(is_public(value.parse().unwrap()), "{value}");
    }
    for value in [
        "169.254.169.254",
        "10.0.0.1",
        "::ffff:169.254.169.254",
        "64:ff9b::a9fe:a9fe",
    ] {
        assert!(!is_public(value.parse().unwrap()), "{value}");
    }
}

/// Every error this module reports says what was wrong in words a caller can pass on.
#[test]
fn describes_every_failure_it_reports() {
    for error in [
        ReferenceError::InvalidUrl("relative URL without a base".to_owned()),
        ReferenceError::MissingHost,
        ReferenceError::UserInformation,
        ReferenceError::InsecureUrl,
        ReferenceError::InvalidReference,
        ReferenceError::SchemeRelativeReference,
        ReferenceError::Fragment,
        ReferenceError::CrossOriginContinuation,
        ReferenceError::InvalidEndpointBase,
        ReferenceError::InvalidResourceIdentifier(Operation::GetOffering),
        ReferenceError::UnexpectedResourceIdentifier(Operation::ListOfferings),
    ] {
        assert!(!error.to_string().is_empty(), "{error:?}");
    }
}
