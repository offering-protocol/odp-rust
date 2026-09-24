#!/bin/sh
set -eu

specs_dir=${ODP_SPECS_DIR:-../odp-specs}
output_dir=${ODP_CONFORMANCE_OUTPUT:-.conformance/reports}
implementation_version=${ODP_RUST_VERSION:-0.2.0}
implementation_version=${implementation_version#v}
adapter=target/debug/odp-conformance-adapter

mkdir -p "$output_dir"
cargo build --quiet --locked -p odp-conformance --bin odp-conformance-adapter

for role in agent service; do
  ruby "$specs_dir/ietf/scripts/run_conformance.rb" \
    --role "$role" \
    --implementation-name odp-rust \
    --implementation-version "$implementation_version" \
    --output "$output_dir/$role.json" \
    -- "$adapter"
done
