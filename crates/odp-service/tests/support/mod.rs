//! The fixtures the Service conformance tests are written against.
//!
//! Each test binary compiles this module separately, so not every binary uses every fixture.
#![allow(dead_code)]

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use odp_core::{
    AdditionalMembers, Collection, CollectionSearchRequest, Offering, OfferingPage,
    OfferingSearchRequest, Operation, Page, Representation, ServiceDocument, VERSION,
    parse_collection, parse_offering, parse_service_document,
};
use odp_service::{Catalog, CatalogRequest, MEDIA_TYPE, Request, Response, Service, ServiceError};

pub const ENDPOINT_BASE: &str = "/odp";

/// Every operation this crate can route, so a test can choose what to advertise.
pub const ALL_OPERATIONS: &[Operation] = &[
    Operation::GetCollection,
    Operation::GetOffering,
    Operation::ListCollectionOfferings,
    Operation::ListCollections,
    Operation::ListOfferings,
    Operation::SearchCollections,
    Operation::SearchOfferings,
];

pub fn offering(id: &str) -> Offering {
    parse_offering(format!(r#"{{"id":"{id}","name":"Plant {id}","odp_version":"1.0"}}"#).as_bytes())
        .unwrap()
}

pub fn collection(id: &str) -> Collection {
    parse_collection(
        format!(r#"{{"id":"{id}","name":"Collection {id}","odp_version":"1.0"}}"#).as_bytes(),
    )
    .unwrap()
}

pub fn offering_page(items: Vec<Offering>, next: &str) -> OfferingPage<Offering> {
    OfferingPage {
        additional: AdditionalMembers::new(),
        auth_expands: false,
        items,
        next: next.to_owned(),
        odp_version: VERSION.to_owned(),
        refinements: Vec::new(),
    }
}

pub fn collection_page(items: Vec<Collection>, next: &str) -> Page<Collection> {
    Page {
        additional: AdditionalMembers::new(),
        auth_expands: false,
        items,
        next: next.to_owned(),
        odp_version: VERSION.to_owned(),
    }
}

/// What a [`Stub`] should do when the catalog is asked for something.
pub enum Answer {
    /// Serve this many Offerings or Collections, with the given continuation.
    Items(usize, String),
    /// Fail the way a Service under load would.
    Fail(u16, &'static str, Option<u64>),
    /// Fail with a message of this many characters.
    LongFailure(usize),
}

/// A catalog whose answers a test chooses, and which records what it was asked.
pub struct Stub {
    answer: Answer,
    operations: Vec<Operation>,
    requests: Mutex<Vec<CatalogRequest>>,
}

impl Stub {
    pub fn new(answer: Answer) -> Arc<Self> {
        Self::with_operations(answer, ALL_OPERATIONS.to_vec())
    }

    pub fn with_operations(answer: Answer, operations: Vec<Operation>) -> Arc<Self> {
        Arc::new(Self {
            answer,
            operations,
            requests: Mutex::new(Vec::new()),
        })
    }

    /// A catalog that simply serves `items` Offerings and Collections.
    pub fn serving(items: usize) -> Arc<Self> {
        Self::new(Answer::Items(items, String::new()))
    }

    pub fn requests(&self) -> Vec<CatalogRequest> {
        self.requests.lock().unwrap().clone()
    }

    pub fn last(&self) -> CatalogRequest {
        self.requests().last().cloned().expect("a catalog request")
    }

    fn record(&self, request: CatalogRequest) {
        self.requests.lock().unwrap().push(request);
    }

    fn offerings(&self, request: CatalogRequest) -> Result<OfferingPage<Offering>, ServiceError> {
        self.record(request);
        match &self.answer {
            Answer::Items(count, next) => Ok(offering_page(
                (0..*count)
                    .map(|index| offering(&format!("p{index}")))
                    .collect(),
                next,
            )),
            answer => Err(failure(answer)),
        }
    }

    fn collections(&self, request: CatalogRequest) -> Result<Page<Collection>, ServiceError> {
        self.record(request);
        match &self.answer {
            Answer::Items(count, next) => Ok(collection_page(
                (0..*count)
                    .map(|index| collection(&format!("c{index}")))
                    .collect(),
                next,
            )),
            answer => Err(failure(answer)),
        }
    }
}

fn failure(answer: &Answer) -> ServiceError {
    match answer {
        Answer::Fail(status, code, Some(seconds)) => {
            ServiceError::retry_after(*status, code, "Come back later", *seconds)
        }
        Answer::Fail(status, code, None) => ServiceError::request(*status, code, "Not right now"),
        Answer::LongFailure(length) => ServiceError::Catalog("x".repeat(*length)),
        Answer::Items(..) => unreachable!("an items answer is not a failure"),
    }
}

#[async_trait]
impl Catalog for Stub {
    async fn continue_offering_search(
        &self,
        request: CatalogRequest,
    ) -> Result<OfferingPage<Offering>, ServiceError> {
        self.offerings(request)
    }

    async fn continue_collection_search(
        &self,
        request: CatalogRequest,
    ) -> Result<Page<Collection>, ServiceError> {
        self.collections(request)
    }
    fn operations(&self) -> Vec<Operation> {
        self.operations.clone()
    }

    async fn list_offerings(
        &self,
        request: CatalogRequest,
    ) -> Result<OfferingPage<Offering>, ServiceError> {
        self.offerings(request)
    }

    async fn get_offering(
        &self,
        id: &str,
        request: CatalogRequest,
    ) -> Result<Option<Offering>, ServiceError> {
        let representation = request.representation;
        self.record(request);
        if let Answer::Items(..) = self.answer {
            let mut value = offering(id);
            if representation == Representation::Full {
                value.description = "A healthy plant".to_owned();
            }
            return Ok((id != "absent").then_some(value));
        }
        Err(failure(&self.answer))
    }

    async fn search_offerings(
        &self,
        _query: OfferingSearchRequest,
        request: CatalogRequest,
    ) -> Result<OfferingPage<Offering>, ServiceError> {
        self.offerings(request)
    }

    async fn list_collections(
        &self,
        request: CatalogRequest,
    ) -> Result<Page<Collection>, ServiceError> {
        self.collections(request)
    }

    async fn get_collection(
        &self,
        id: &str,
        request: CatalogRequest,
    ) -> Result<Option<Collection>, ServiceError> {
        self.record(request);
        if let Answer::Items(..) = self.answer {
            return Ok((id != "absent").then(|| collection(id)));
        }
        Err(failure(&self.answer))
    }

    async fn search_collections(
        &self,
        _query: CollectionSearchRequest,
        request: CatalogRequest,
    ) -> Result<Page<Collection>, ServiceError> {
        self.collections(request)
    }

    async fn list_collection_offerings(
        &self,
        _collection_id: &str,
        request: CatalogRequest,
    ) -> Result<OfferingPage<Offering>, ServiceError> {
        self.offerings(request)
    }
}

/// A Service Document advertising every operation, in the given localizations.
pub fn document(localizations: &[&str]) -> ServiceDocument {
    let operations = ALL_OPERATIONS
        .iter()
        .map(|operation| {
            format!(
                r#"{{"authentication":"not-required","name":"{}"}}"#,
                operation_name(*operation)
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let tags = localizations
        .iter()
        .map(|value| format!(r#""{value}""#))
        .collect::<Vec<_>>()
        .join(",");
    parse_service_document(
        format!(
            r#"{{"description":"Plants for agents.","http":{{"endpoint_base":"{ENDPOINT_BASE}"}},"language":"{}","localizations":[{tags}],"name":"Plants","odp_version":"1.0","operations":[{operations}]}}"#,
            localizations[0]
        )
        .as_bytes(),
    )
    .unwrap()
}

pub const fn operation_name(operation: Operation) -> &'static str {
    match operation {
        Operation::GetCollection => "get-collection",
        Operation::GetOffering => "get-offering",
        Operation::ListCollectionOfferings => "list-collection-offerings",
        Operation::ListCollections => "list-collections",
        Operation::ListOfferings => "list-offerings",
        Operation::SearchCollections => "search-collections",
        Operation::SearchOfferings => "search-offerings",
    }
}

/// A Service over the given catalog, advertising English only.
pub fn service(catalog: Arc<Stub>) -> Service {
    Service::new(document(&["en"]), catalog).unwrap()
}

pub fn localized(catalog: Arc<Stub>, localizations: &[&str]) -> Service {
    Service::new(document(localizations), catalog).unwrap()
}

pub fn get(path: &str) -> Request {
    Request {
        headers: BTreeMap::from([("accept".to_owned(), MEDIA_TYPE.to_owned())]),
        method: "GET".to_owned(),
        path: path.to_owned(),
        ..Request::default()
    }
}

pub fn query(path: &str, query: &str) -> Request {
    Request {
        query: query.to_owned(),
        ..get(path)
    }
}

pub fn post(path: &str, body: &[u8]) -> Request {
    Request {
        body: body.to_vec(),
        headers: BTreeMap::from([
            ("accept".to_owned(), MEDIA_TYPE.to_owned()),
            ("content-type".to_owned(), MEDIA_TYPE.to_owned()),
        ]),
        method: "POST".to_owned(),
        path: path.to_owned(),
        ..Request::default()
    }
}

pub fn with_header(request: Request, name: &str, value: &str) -> Request {
    let mut request = request;
    request.headers.insert(name.to_owned(), value.to_owned());
    request
}

pub fn header<'a>(response: &'a Response, name: &str) -> Option<&'a str> {
    response.headers.get(name).map(String::as_str)
}
