use odp_agent::ServiceClient;
use odp_core::{PriceType, Representation};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let service_url = std::env::args()
        .nth(1)
        .ok_or("usage: odp-node-interop SERVICE_URL")?;
    let client = ServiceClient::for_local_development(&service_url)?;
    let inspection = client.inspect().await?;
    if inspection.document.name != "Small Example Store" {
        return Err(format!("unexpected Service {:?}", inspection.document.name).into());
    }
    let offerings = client.list_offerings(Representation::Terse, 50).await?;
    for expected in ["architecture-review", "incident-plan"] {
        if !offerings
            .items
            .iter()
            .any(|offering| offering.id == expected)
        {
            return Err(format!("Offering list omitted {expected}").into());
        }
    }
    let details = client.get_offering_details("incident-plan").await?;
    if details.offering.name != "Incident Response Plan"
        || details
            .offering
            .price
            .as_ref()
            .map(|price| price.price_type)
            != Some(PriceType::Free)
    {
        return Err("full Offering did not match the Node.js example".into());
    }
    let resolved = client.resolve_action("incident-plan", "download").await?;
    let action = resolved.action.http.ok_or("download Action omitted HTTP")?;
    if action.url != format!("{service_url}/downloads/incident-plan.txt") {
        return Err("download Action did not resolve to the Node.js Service".into());
    }
    println!("Rust Agent interoperates with the Node.js example Service");
    Ok(())
}
