#!/usr/bin/env bash
set -euo pipefail

repository=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
consumer=$(mktemp -d)
trap 'rm -rf "$consumer"' EXIT
mkdir -p "$consumer/src"

source=${ODP_CONSUMER_SOURCE:-path}
if [[ "$source" == "path" ]]; then
  dependencies=$(cat <<EOF
odp-agent = { path = "$repository/crates/odp-agent" }
odp-core = { path = "$repository/crates/odp-core" }
odp-directory = { path = "$repository/crates/odp-directory" }
odp-service = { path = "$repository/crates/odp-service" }
EOF
)
elif [[ "$source" == "registry" ]]; then
  version=${ODP_RUST_VERSION:?ODP_RUST_VERSION is required for a registry consumer check}
  dependencies=$(cat <<EOF
odp-agent = "=$version"
odp-core = "=$version"
odp-directory = "=$version"
odp-service = "=$version"
EOF
)
else
  echo "ODP_CONSUMER_SOURCE must be path or registry." >&2
  exit 1
fi

cat > "$consumer/Cargo.toml" <<EOF
[package]
name = "odp-rust-consumer"
version = "0.1.0"
edition = "2024"

[dependencies]
$dependencies
EOF

cat > "$consumer/src/main.rs" <<'EOF'
use odp_agent::ServiceClient;
use odp_core::{Representation, ResourceIdentity, ResourceType};
use odp_directory::{DirectoryClient, Environment, ResourceSearchRequest, ResultType};
use odp_service::ServiceBuilder;

fn main() {
    let _ = ServiceClient::new("https://demo.inflowpay.ai");
    let _ = ResourceIdentity::new(
        "https://demo.inflowpay.ai/.well-known/odp",
        ResourceType::Offering,
        "rubber-plant",
    );
    let _ = DirectoryClient::new(Environment::Production);
    let _ = ResourceSearchRequest {
        types: Some(vec![ResultType::Collection]),
        ..Default::default()
    };
    let _ = ServiceBuilder::new("Example", "Example Service", "en", "/odp");
    let _ = Representation::Terse;
}
EOF

cargo generate-lockfile --manifest-path "$consumer/Cargo.toml"
cargo check --locked --manifest-path "$consumer/Cargo.toml"
