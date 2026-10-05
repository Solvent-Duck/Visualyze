# Pinned public reference fixtures

These partial fixtures describe facts inspected in public repositories. They do
not describe a live deployment or any company's private architecture. Each
supported entity and relationship carries evidence pointing to an exact commit
and, where applicable, the relevant source lines. Source code was inspected in a
temporary directory; no upstream implementation or repository was copied into
Visualyze.

## Online Boutique

- Upstream: [GoogleCloudPlatform/microservices-demo](https://github.com/GoogleCloudPlatform/microservices-demo).
- Commit: `38e7348eb289eb5b87c0c6e8cb19ced0449dc389`, resolved from `refs/heads/main` with `git ls-remote` during fixture creation.
- Fixture: `online-boutique.json`; 12 logical components plus the application scope, with 16 configured endpoint dependencies.
- Primary evidence: [release configuration](https://github.com/GoogleCloudPlatform/microservices-demo/blob/38e7348eb289eb5b87c0c6e8cb19ced0449dc389/release/kubernetes-manifests.yaml) and [component descriptions](https://github.com/GoogleCloudPlatform/microservices-demo/blob/38e7348eb289eb5b87c0c6e8cb19ced0449dc389/README.md#L15-L37).
- The repository's pinned [LICENSE](https://github.com/GoogleCloudPlatform/microservices-demo/blob/38e7348eb289eb5b87c0c6e8cb19ced0449dc389/LICENSE) states Apache License 2.0.

Each named Deployment supplies evidence for one logical component; Kubernetes
resource and replica details are not represented as architecture entities.
Redis is represented as storage, and the documented workload generator as a
client. The web frontend is a service, consistent with its server role.

Relationships record configured endpoint addresses with `depends_on`, runtime
phase, and unknown necessity. An environment setting alone does not establish a
request, a read/write operation, or whether the dependency is required. The
shopping assistant address is omitted because it has no matching Deployment in
this release configuration and its optional feature enablement is not inferred.

## DeathStarBench Hotel Reservation

- Upstream: [delimitrou/DeathStarBench](https://github.com/delimitrou/DeathStarBench).
- Commit: `6ecb09706140f8730b5385c08f1386c654c3c526`, resolved from `refs/heads/master` with `git ls-remote` during fixture creation.
- Fixture: `deathstar-hotel.json`; 23 logical components plus the application scope, with 22 startup dependencies and 9 verified request relationships.
- Primary configuration: [Compose file](https://github.com/delimitrou/DeathStarBench/blob/6ecb09706140f8730b5385c08f1386c654c3c526/hotelReservation/docker-compose.yml) and [service configuration](https://github.com/delimitrou/DeathStarBench/blob/6ecb09706140f8730b5385c08f1386c654c3c526/hotelReservation/config.json).
- Verified request code: [frontend clients and handlers](https://github.com/delimitrou/DeathStarBench/blob/6ecb09706140f8730b5385c08f1386c654c3c526/hotelReservation/services/frontend/server.go) and [search implementation](https://github.com/delimitrou/DeathStarBench/blob/6ecb09706140f8730b5385c08f1386c654c3c526/hotelReservation/services/search/server.go).
- The repository's pinned [LICENSE](https://github.com/delimitrou/DeathStarBench/blob/6ecb09706140f8730b5385c08f1386c654c3c526/LICENSE) states Apache License 2.0.

Compose `depends_on` entries become startup dependencies with unknown runtime
necessity. They are not interpreted as RPCs or storage reads/writes. MongoDB and
Memcached components are classified from their declared images and configuration.

The frontend's typed client bindings and call sites verify requests to search,
profile, recommendation, user, reservation, review, and attractions. Search's
typed clients and `Nearby` implementation verify requests to geo and rate. Each
relationship cites both client binding and invocation evidence. Repeated methods
to the same service are summarized as one caller-to-service relationship.

Jaeger and tracing links, workloads, replica placement, volume topology, storage
operations, and other internal call paths are deliberately outside this fixture.
No authentication concern or policy coverage is inferred from the user service.

## Reproduction and interpretation

Both fixtures use the unchanged public snapshot contract and its full validator.
The snapshot `source_revision` and every evidence URL retain the pinned commit.
Updating a reference requires re-reading the new source, revisiting omissions,
and incrementing the local model revision.

```sh
cargo run -p visualyze --locked -- --model private/layout/fixtures/online-boutique.json --validate
cargo run -p visualyze --locked -- --model private/layout/fixtures/deathstar-hotel.json --validate
cargo run -p visualyze-layout --example compare --locked -- private/layout/fixtures/online-boutique.json private/layout/fixtures/deathstar-hotel.json
```

The comparison uses 32 deterministic candidate evaluations and the same default
projection, router, quality weights, and validator for all algorithms. These are
additional reference cases, without fixture-specific weight tuning. Their
measurements do not establish production-scale performance or comprehension.
