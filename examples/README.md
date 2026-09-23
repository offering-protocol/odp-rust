# Runnable examples

The examples demonstrate both sides of a minimal ODP integration:

| Example | Purpose |
| --- | --- |
| [`odp-service-small`](odp-service-small/) | Publishes a validated in-memory catalog through the framework-neutral Service runtime. |
| [`odp-agent-discovery`](odp-agent-discovery/) | Injects a mock Directory into the top-level Agent, performs federated discovery, and navigates Collections, Offerings, and Actions. |
| [`odp-directory-discovery`](odp-directory-discovery/main.rs) | Searches the canonical Directory for Services and Collections, then retrieves anonymous Collection details. |

Run the small Service in one terminal, then the Agent in another:

```sh
cargo run -p odp-examples --bin odp-service-small
cargo run -p odp-examples --bin odp-agent-discovery
```

The Agent also accepts Service origins as positional arguments. It converts the compatible origins
into an in-memory Directory response, while all Service requests continue to use the real HTTP
endpoints.

## Canonical Directory discovery

```sh
cargo run -p odp-examples --bin odp-directory-discovery -- sandbox weather
```

Use `production` instead of `sandbox` for production. Omit `weather` to browse rather than search.
This example requires a deployment with `/v1/directory/search`. It requests at most five results,
prints Service and Collection names, reports malformed or unknown results, and retrieves full
Collection details only for ODP sources after inspecting the owning Service's advertised
anonymous operation. Imported Collections print their exact document URL without ODP calls.
It does not enroll, pay, or invoke Actions. Unlike `odp-agent-discovery`, it uses the real Directory,
not a mock. The server's bounded result list is not an exhaustive catalog listing.
