use odp_agent::ServiceClient;
use odp_core::{AuthenticationRequirement, Operation};
use odp_directory::{DirectoryClient, DirectoryResult, Environment, ResourceSearchRequest};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let environment = match args.next().as_deref().unwrap_or("production") {
        "production" => Environment::Production,
        "sandbox" => Environment::Sandbox,
        _ => return Err("Environment must be production or sandbox".into()),
    };
    let directory = DirectoryClient::new(environment)?;
    let response = directory
        .search(&ResourceSearchRequest {
            query: args.collect::<Vec<_>>().join(" "),
            limit: 5,
            ..Default::default()
        })
        .await?;
    for issue in response.issues {
        eprintln!("Skipped result {}: {}", issue.index, issue.message);
    }
    for result in response.items {
        match result {
            DirectoryResult::Service(item) => println!(
                "Service: {} ({})\nDiscovery document: {}",
                item.service.name, item.service.service_origin, item.service.source.url
            ),
            DirectoryResult::Collection(item) => {
                println!(
                    "Collection: {} ({}, through {})",
                    item.collection.name, item.collection.id, item.service.service_origin
                );
                if item.service.source.source_type != "odp" {
                    println!("Discovery document: {}", item.service.source.url);
                    continue;
                }
                let client = ServiceClient::new(&item.service.service_origin)?;
                let inspection = client.inspect().await?;
                if inspection.document.operations.iter().any(|operation| {
                    operation.name == Operation::GetCollection
                        && operation.authentication != AuthenticationRequirement::Required
                }) {
                    let collection = client.get_collection(&item.collection.id).await?;
                    println!("{}", serde_json::to_string_pretty(&collection)?);
                } else {
                    println!("The Service does not advertise anonymous Collection retrieval.");
                }
            }
            DirectoryResult::Unknown { kind, .. } => println!("Unsupported result type: {kind}"),
        }
    }
    Ok(())
}
