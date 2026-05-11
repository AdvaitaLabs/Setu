API
===

Move Platform v1
----------------
The public HTTP surface covers transfer submission, Move package publishing,
Move entry-function calls, object queries, node health, and version-aware
object visibility. Advanced contract behavior should be treated as testnet
scope unless it is documented in this README or another public README.

Scope
-----
- Submit transfers to the DAG builder.
- Query DAG/FoldGraph state (transfer status, fold status, votes).
- Health/metrics for nodes.

Interfaces
----------
- HTTP/JSON for external clients; gRPC for node-to-node if needed.
- Authn/Authz pluggable (API key/JWT); rate limits at ingress.
- Idempotent submission keyed by transfer id.

Open questions
--------------
- Pagination/filtering for DAG/Fold queries.
- Exposure of shard routing hints to clients.

Cross-CF first-visibility contract (shared objects)
---------------------------------------------------
Shared objects created via `transfer::share` only become referenceable by
other transactions' `shared_object_ids` **after their creating CF has
finalized on the queried node**. A transaction that references such an
object within the same CF — i.e. before `CF_create` has finalized — will be
rejected at the prepare phase with `ObjectNotFound`.

This is a physical consequence of when `GlobalStateManager` publishes its
post-commit snapshot, not a client-side bug. Clients must wait for
ownership to transition to `Shared` (observable via
`GET /api/v1/move/objects/{id}`) before submitting dependent transactions.

Implemented long-poll helper:

        GET /api/v1/move/objects/{id}?wait_min_version={v}&timeout_ms={ms}

This endpoint will block until the object reaches `version >= v` or a
timeout elapses. If `timeout_ms` is omitted, the API default is 30 seconds.

Response taxonomy:

- `200 OK`: object is already at or eventually reaches `version >= v`.
- `408 Request Timeout`: timeout elapsed first; body contains the latest
    observed object state.
- `429 Too Many Requests`: per-object or global waiter cap is exceeded.
- `503 Service Unavailable`: validator was started without a watcher attached.
