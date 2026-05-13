API
===

Move Platform v1
----------------
The public scoped V1 HTTP surface is a narrow positive list. It covers Move
package publishing, Move entry-function calls, Move object/module queries, node
health, event lookup/finality polling, app subnet registration/list, signed user
registration, explicit account resource queries, and signed user transfer for
pre-funded accounts.

Advanced contract behavior, operator diagnostics, and mounted-but-unpromoted
routes are not public stable V1 promises unless they are explicitly promoted in
the capability boundary docs.

Scope
-----
- Submit promoted public writes to the validator.
- Poll `/api/v1/event/:id` or committed/finalized state for durable receipts.
- Query promoted Move objects/modules and explicit account resource fields.
- Use health/metrics as readiness and topology information only.

Not public stable scope for V1:

- `/api/v1/transfer/status` as a durable receipt; it is a process-local
    operator tracker.
- Raw transfer, raw batch transfer, and raw event as public wallet/client APIs.
- Raw object bytes through `/api/v1/state/object/:key`; the current contract is
    HTTP 410 unsupported.
- Governance, credentials, tokenomics/fees, TEE-security claims, cross-subnet
    transfers, Sui compatibility, and PTB shared-object semantics.

Interfaces
----------
- HTTP/JSON for external clients; gRPC for node-to-node if needed.
- Authn/Authz pluggable (API key/JWT); rate limits at ingress.
- Idempotent submission keyed by transfer id.

Open questions
--------------
- Pagination/filtering for DAG/Fold queries.
- Exposure of shard routing hints to clients.

Durable receipt contract
------------------------
A successful submit response means the validator accepted the request for
processing. It is not a durable receipt. For durable confirmation, poll the
returned event id with `/api/v1/event/:id` until the event is finalized/on-chain,
or verify the expected committed state with a finalized/committed query.

Cross-CF first-visibility contract (Move object visibility)
-----------------------------------------------------------
Shared objects created via `transfer::share` only become referenceable by
other transactions' `shared_object_ids` **after their creating CF has
finalized on the queried node**. A transaction that references such an
object within the same CF — i.e. before `CF_create` has finalized — will be
rejected at the prepare phase with `ObjectNotFound`.

This note explains committed object visibility timing. It does not promote
Sui-style PTB shared-object semantics, which remain outside the V1 stable scope.

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
