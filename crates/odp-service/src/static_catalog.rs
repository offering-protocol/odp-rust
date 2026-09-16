use std::{
    collections::BTreeMap,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, KeyInit, Mac};
use odp_core::{Collection, Offering, OfferingPage, Operation, Page, VERSION};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use url::form_urlencoded;

use crate::{Catalog, CatalogRequest, ServiceError};

const DEFAULT_PAGE_LIMIT: usize = 50;
const CONTINUATION_LIFETIME: Duration = Duration::from_secs(60 * 60);

#[derive(Clone, Debug, Default)]
pub struct StaticCatalogOptions {
    pub collections: Vec<Collection>,
    pub offerings: Vec<Offering>,
}

pub struct StaticCatalog {
    collections: Vec<Collection>,
    collection_by_id: BTreeMap<String, Collection>,
    offerings: Vec<Offering>,
    offering_by_id: BTreeMap<String, Offering>,
    continuation_key: [u8; 32],
}

impl StaticCatalog {
    pub fn new(options: StaticCatalogOptions) -> Result<Self, ServiceError> {
        let mut continuation_key = [0_u8; 32];
        getrandom::fill(&mut continuation_key).map_err(|error| {
            ServiceError::InvalidConfiguration(format!(
                "Unable to create the continuation signing key: {error}"
            ))
        })?;
        let mut offering_by_id = BTreeMap::new();
        for offering in &options.offerings {
            validate_offering(offering)?;
            if offering_by_id
                .insert(offering.id.clone(), offering.clone())
                .is_some()
            {
                return Err(ServiceError::InvalidConfiguration(
                    "Offering identifiers must be unique".to_owned(),
                ));
            }
        }
        let mut collection_by_id = BTreeMap::new();
        for collection in &options.collections {
            validate_collection(collection)?;
            if collection_by_id
                .insert(collection.id.clone(), collection.clone())
                .is_some()
            {
                return Err(ServiceError::InvalidConfiguration(
                    "Collection identifiers must be unique".to_owned(),
                ));
            }
        }
        for offering in &options.offerings {
            if offering
                .collection_ids
                .iter()
                .any(|id| !collection_by_id.contains_key(id))
            {
                return Err(ServiceError::InvalidConfiguration(format!(
                    "Offering {} refers to an unknown Collection",
                    offering.id
                )));
            }
        }
        Ok(Self {
            collections: options.collections,
            collection_by_id,
            offerings: options.offerings,
            offering_by_id,
            continuation_key,
        })
    }
}

#[async_trait]
impl Catalog for StaticCatalog {
    fn operations(&self) -> Vec<Operation> {
        let mut operations = vec![Operation::GetOffering, Operation::ListOfferings];
        if !self.collections.is_empty() {
            operations.extend([
                Operation::GetCollection,
                Operation::ListCollectionOfferings,
                Operation::ListCollections,
            ]);
        }
        operations
    }

    async fn list_offerings(
        &self,
        request: CatalogRequest,
    ) -> Result<OfferingPage<Offering>, ServiceError> {
        let (items, next) = page(&self.offerings, &request, &self.continuation_key, |value| {
            offering_representation(value, request.representation, true)
        })?;
        Ok(OfferingPage {
            additional: BTreeMap::new(),
            auth_expands: false,
            items,
            next,
            odp_version: VERSION.to_owned(),
            refinements: Vec::new(),
        })
    }

    async fn get_offering(
        &self,
        id: &str,
        request: CatalogRequest,
    ) -> Result<Option<Offering>, ServiceError> {
        Ok(self
            .offering_by_id
            .get(id)
            .cloned()
            .map(|value| offering_representation(value, request.representation, false)))
    }

    async fn list_collections(
        &self,
        request: CatalogRequest,
    ) -> Result<Page<Collection>, ServiceError> {
        let (items, next) = page(
            &self.collections,
            &request,
            &self.continuation_key,
            |value| collection_representation(value, request.representation, true),
        )?;
        Ok(Page {
            additional: BTreeMap::new(),
            auth_expands: false,
            items,
            next,
            odp_version: VERSION.to_owned(),
        })
    }

    async fn get_collection(
        &self,
        id: &str,
        request: CatalogRequest,
    ) -> Result<Option<Collection>, ServiceError> {
        Ok(self
            .collection_by_id
            .get(id)
            .cloned()
            .map(|value| collection_representation(value, request.representation, false)))
    }

    async fn list_collection_offerings(
        &self,
        collection_id: &str,
        request: CatalogRequest,
    ) -> Result<OfferingPage<Offering>, ServiceError> {
        if !self.collection_by_id.contains_key(collection_id) {
            return Err(ServiceError::request(
                404,
                "NOT_FOUND",
                "Collection not found",
            ));
        }
        let offerings = self
            .offerings
            .iter()
            .filter(|offering| offering.collection_ids.iter().any(|id| id == collection_id))
            .cloned()
            .collect::<Vec<_>>();
        let (items, next) = page(&offerings, &request, &self.continuation_key, |value| {
            offering_representation(value, request.representation, true)
        })?;
        Ok(OfferingPage {
            additional: BTreeMap::new(),
            auth_expands: false,
            items,
            next,
            odp_version: VERSION.to_owned(),
            refinements: Vec::new(),
        })
    }
}

#[derive(Deserialize, Serialize)]
struct Cursor {
    expires: u64,
    limit: usize,
    offset: usize,
    path: String,
    representation: String,
}

fn page<T: Clone>(
    values: &[T],
    request: &CatalogRequest,
    continuation_key: &[u8; 32],
    represent: impl Fn(T) -> T,
) -> Result<(Vec<T>, String), ServiceError> {
    let limit = if request.limit == 0 {
        DEFAULT_PAGE_LIMIT
    } else {
        request.limit
    };
    let offset = decode_cursor(request, limit, continuation_key)?;
    if offset > values.len() {
        return Err(invalid_cursor());
    }
    let end = (offset + limit).min(values.len());
    let next = if end < values.len() {
        encode_cursor(request, limit, end, continuation_key)?
    } else {
        String::new()
    };
    Ok((
        values[offset..end].iter().cloned().map(represent).collect(),
        next,
    ))
}

fn offering_representation(
    mut value: Offering,
    representation: odp_core::Representation,
    embedded: bool,
) -> Offering {
    if representation == odp_core::Representation::Terse {
        // OFR-55: a Terse Offering carries no Actions, but it may still point at them.
        value.actions.clear();
    } else {
        // REP-12: a Full Representation omits nothing, so it has nothing to point at.
        value.detail_fields.clear();
    }
    if embedded && representation == odp_core::Representation::Terse {
        value.odp_version.clear();
    }
    value
}

fn collection_representation(
    mut value: Collection,
    representation: odp_core::Representation,
    embedded: bool,
) -> Collection {
    if representation != odp_core::Representation::Terse {
        // REP-12: a Full Representation omits nothing, so it has nothing to point at.
        value.detail_fields.clear();
    }
    if embedded && representation == odp_core::Representation::Terse {
        value.odp_version.clear();
    }
    value
}

fn encode_cursor(
    request: &CatalogRequest,
    limit: usize,
    offset: usize,
    continuation_key: &[u8; 32],
) -> Result<String, ServiceError> {
    let expires = SystemTime::now()
        .checked_add(CONTINUATION_LIFETIME)
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_secs())
        .ok_or_else(|| ServiceError::Catalog("system clock is unavailable".to_owned()))?;
    let value = Cursor {
        expires,
        limit,
        offset,
        path: request.path.clone(),
        representation: representation_name(request).to_owned(),
    };
    let data =
        serde_json::to_vec(&value).map_err(|error| ServiceError::Catalog(error.to_string()))?;
    let payload = URL_SAFE_NO_PAD.encode(data);
    let mut mac = Hmac::<Sha256>::new_from_slice(continuation_key)
        .map_err(|error| ServiceError::Catalog(error.to_string()))?;
    mac.update(payload.as_bytes());
    let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    let cursor = format!("{payload}.{signature}");
    let query = form_urlencoded::Serializer::new(String::new())
        .append_pair("cursor", &cursor)
        .append_pair("limit", &limit.to_string())
        .append_pair("representation", representation_name(request))
        .finish();
    Ok(format!("{}?{query}", request.path))
}

fn decode_cursor(
    request: &CatalogRequest,
    limit: usize,
    continuation_key: &[u8; 32],
) -> Result<usize, ServiceError> {
    let Some(cursor) = &request.cursor else {
        return Ok(0);
    };
    let (payload, encoded_signature) = cursor.split_once('.').ok_or_else(invalid_cursor)?;
    let signature = URL_SAFE_NO_PAD
        .decode(encoded_signature)
        .map_err(|_| invalid_cursor())?;
    let mut mac = Hmac::<Sha256>::new_from_slice(continuation_key).map_err(|_| invalid_cursor())?;
    mac.update(payload.as_bytes());
    mac.verify_slice(&signature).map_err(|_| invalid_cursor())?;
    let data = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| invalid_cursor())?;
    let value = serde_json::from_slice::<Cursor>(&data).map_err(|_| invalid_cursor())?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| invalid_cursor())?
        .as_secs();
    if value.expires < now {
        return Err(expired_cursor());
    }
    if value.limit != limit
        || value.path != request.path
        || value.representation != representation_name(request)
    {
        return Err(invalid_cursor());
    }
    Ok(value.offset)
}

fn invalid_cursor() -> ServiceError {
    ServiceError::request(
        410,
        "CONTINUATION_UNAVAILABLE",
        "Continuation is unavailable",
    )
}

/// PAG-20: a continuation that simply aged out is named as such, so an Agent can tell a stale
/// cursor from one that was never valid and restart the traversal deliberately.
fn expired_cursor() -> ServiceError {
    ServiceError::request(410, "CONTINUATION_EXPIRED", "Continuation has expired")
}

fn representation_name(request: &CatalogRequest) -> &'static str {
    match request.representation {
        odp_core::Representation::Terse => "terse",
        odp_core::Representation::Full => "full",
    }
}

fn validate_offering(offering: &Offering) -> Result<(), ServiceError> {
    let data = serde_json::to_vec(offering)
        .map_err(|error| ServiceError::InvalidConfiguration(error.to_string()))?;
    odp_core::parse_offering(&data)
        .map_err(|error| ServiceError::InvalidConfiguration(error.to_string()))?;
    Ok(())
}

fn validate_collection(collection: &Collection) -> Result<(), ServiceError> {
    let data = serde_json::to_vec(collection)
        .map_err(|error| ServiceError::InvalidConfiguration(error.to_string()))?;
    odp_core::parse_collection(&data)
        .map_err(|error| ServiceError::InvalidConfiguration(error.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use odp_core::parse_offering;

    use super::*;

    fn offering(id: &str) -> Offering {
        parse_offering(
            format!(
                r#"{{"actions":[{{"authentication":"not-required","http":{{"href":"/actions/{id}","method":"GET"}},"id":"view","rel":"invoke"}}],"id":"{id}","name":"Plant {id}","odp_version":"1.0"}}"#
            )
            .as_bytes(),
        )
        .unwrap()
    }

    /// PAG-19 and PAG-20: a continuation stays usable for an hour, and a stale one is named
    /// as stale so an Agent can restart the traversal rather than guess.
    #[tokio::test]
    async fn distinguishes_a_stale_continuation_from_an_invalid_one() {
        let catalog = StaticCatalog::new(StaticCatalogOptions {
            collections: Vec::new(),
            offerings: vec![offering("one"), offering("two")],
        })
        .unwrap();
        let request = CatalogRequest {
            limit: 1,
            path: "/odp/offerings".to_owned(),
            ..CatalogRequest::default()
        };

        // A cursor this catalog signed, but issued an hour and a second ago.
        let stale = Cursor {
            expires: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs()
                - 1,
            limit: 1,
            offset: 1,
            path: request.path.clone(),
            representation: "terse".to_owned(),
        };
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&stale).unwrap());
        let mut mac = Hmac::<Sha256>::new_from_slice(&catalog.continuation_key).unwrap();
        mac.update(payload.as_bytes());
        let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());

        let error = catalog
            .list_offerings(CatalogRequest {
                cursor: Some(format!("{payload}.{signature}")),
                ..request.clone()
            })
            .await
            .unwrap_err();
        let ServiceError::Request { code, status, .. } = error else {
            panic!("a stale continuation is a request failure");
        };
        assert_eq!(status, 410);
        assert_eq!(code, "CONTINUATION_EXPIRED");

        // One that is simply unusable reads differently, so the two are never confused.
        let error = catalog
            .list_offerings(CatalogRequest {
                cursor: Some("nonsense".to_owned()),
                ..request
            })
            .await
            .unwrap_err();
        let ServiceError::Request { code, .. } = error else {
            panic!("an unusable continuation is a request failure");
        };
        assert_eq!(code, "CONTINUATION_UNAVAILABLE");
    }

    /// REP-12: a Full Collection omits nothing, so it points at nothing.
    #[tokio::test]
    async fn keeps_collection_detail_fields_on_the_terse_representation() {
        let collection = odp_core::parse_collection(
            br#"{"detail_fields":["/description"],"id":"plants","name":"Plants","odp_version":"1.0"}"#,
        )
        .unwrap();
        let catalog = StaticCatalog::new(StaticCatalogOptions {
            collections: vec![collection],
            offerings: Vec::new(),
        })
        .unwrap();

        let terse = catalog
            .get_collection("plants", CatalogRequest::default())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(terse.detail_fields, ["/description"]);

        let full = catalog
            .get_collection(
                "plants",
                CatalogRequest {
                    representation: odp_core::Representation::Full,
                    ..CatalogRequest::default()
                },
            )
            .await
            .unwrap()
            .unwrap();
        assert!(full.detail_fields.is_empty());
    }

    #[tokio::test]
    async fn provides_bounded_stateless_pages() {
        let catalog = StaticCatalog::new(StaticCatalogOptions {
            collections: Vec::new(),
            offerings: vec![offering("one"), offering("two")],
        })
        .unwrap();
        let first = catalog
            .list_offerings(CatalogRequest {
                limit: 1,
                path: "/odp/offerings".to_owned(),
                ..CatalogRequest::default()
            })
            .await
            .unwrap();
        assert_eq!(first.items[0].id, "one");
        assert!(first.items[0].actions.is_empty());
        assert!(first.items[0].odp_version.is_empty());
        let cursor = form_urlencoded::parse(first.next.split_once('?').unwrap().1.as_bytes())
            .find_map(|(name, value)| (name == "cursor").then(|| value.into_owned()))
            .unwrap();
        let second = catalog
            .list_offerings(CatalogRequest {
                cursor: Some(cursor.clone()),
                limit: 1,
                path: "/odp/offerings".to_owned(),
                ..CatalogRequest::default()
            })
            .await
            .unwrap();
        assert_eq!(second.items[0].id, "two");
        assert!(second.next.is_empty());
        assert!(
            catalog
                .list_offerings(CatalogRequest {
                    cursor: Some(format!("{cursor}A")),
                    limit: 1,
                    path: "/odp/offerings".to_owned(),
                    ..CatalogRequest::default()
                })
                .await
                .is_err()
        );

        let detail = catalog
            .get_offering("one", CatalogRequest::default())
            .await
            .unwrap()
            .unwrap();
        assert!(detail.actions.is_empty());
        assert_eq!(detail.odp_version, VERSION);

        let full = catalog
            .list_offerings(CatalogRequest {
                representation: odp_core::Representation::Full,
                ..CatalogRequest::default()
            })
            .await
            .unwrap();
        assert_eq!(full.items[0].actions.len(), 1);
        assert_eq!(full.items[0].odp_version, VERSION);
    }
}
