# Glossary

Domain terminology, so a requirement can use a term precisely instead
of redefining it inline every time it's used.

| Term | Definition |
| --- | --- |
| Hero | The example CRUD resource this template ships with; see `src/README.md`'s "Example CRUD resource: Hero". |
| MODE | The single startup-time setting (`dev` / `mock` / `production`) selecting repository, health-check, and auth backends together; see [FR-0021](frs/FR-0021-mode-based-backend-selection.md). |
| `CrudService` | The generic, resource-agnostic list/create/get/update/delete service built from a `Repository`; see [FR-0001](frs/FR-0001-generic-crud-interface.md) and `src/crud/`. Not to be confused with `controllers::crud_actions`/`crud_query`/`crud_events` (the resource-agnostic controller-layer helpers Hero's routers share) or `src/controllers`' Hero-specific `crud_*` handlers, which sit above it. |
| Repository | The storage-agnostic persistence trait behind a resource's `CrudService` (SeaORM-backed or in-memory under `Mode::Mock`); see `src/repositories/`. |
| View / DTO | The request/response shape (serde-derived struct) presenting a resource for a given API version/format (JSON, XML, HTML form); see `src/views/`. |
| Problem Details | The RFC 9457 `application/problem+json` error response shape (`problem_details::AppError`) used for every error this app returns; see [FR-0018](frs/FR-0018-uniform-problem-details-errors.md) and [ADR 0004](adrs/0004-uniform-problem-details-error-responses.md). |
| RBAC | Role-based access control: granting access based on Keycloak client-role claims (`resource_access.<client_id>.roles`) present in a validated OIDC bearer token; see [FR-0014](frs/FR-0014-rbac-client-roles.md). |
| Sunset header | The RFC 8594 HTTP response header (with `Deprecation`/`Link`) announcing the date a deprecated API version will stop being served, attached via `http_headers::Sunset`; see [FR-0024](frs/FR-0024-sunset-deprecation-header-mechanism.md) and [ADR 0012](adrs/0012-sunset-headers-via-intoresponseparts-not-middleware.md). |
| JWKS | JSON Web Key Set: the public keys an OIDC provider publishes, used to verify bearer token signatures. |
| OIDC | OpenID Connect: the provider-agnostic identity layer this app validates bearer tokens against (`oidc::OidcVerifier`); see [FR-0013](frs/FR-0013-oidc-bearer-auth.md) and [ADR 0003](adrs/0003-auth-strategy-provider-agnostic-oidc.md). |
| Deprecation | A still-functional but sunset-scheduled API version (e.g. `heroes_v1`), as opposed to one already removed. |
| `HasId` | The trait a model implements to give it a stable integer identity, used generically by `controllers::crud_actions`; see `src/models/`. |
| Soft delete / `archived_at` | Marking a record unavailable via an `archived_at` timestamp column rather than physically deleting the row; see [ADR 0008](adrs/0008-soft-delete-via-archived-at-marker-column.md). |
| Owner scoping | Restricting writes to the caller's own `sub` claim while leaving reads open to every authenticated caller; see [ADR 0007](adrs/0007-owner-scoped-hero-example-resource.md). |
| `EventBus` / event stream | The publish/subscribe mechanism (MQTT-backed, or in-memory under `Mode::Mock`) behind `GET <prefix>/events`, announcing Hero create/update/delete activity; see [FR-0030](frs/FR-0030-crud-event-stream.md) and [ADR 0016](adrs/0016-mqtt-backed-sse-crud-event-stream.md). |
| `Mode::Mock` | The zero-infrastructure test/demo mode: in-memory repository, always-healthy fakes, in-memory event bus, unverified bearer tokens; requires `ALLOW_MOCK_MODE=1`. See `src/README.md`'s "MODE" section. |
| Bulk action | A single request applying an update or delete to a filtered set of records at once (`BulkUpdateResult`/`BulkDeleteResult`), capped in size; see [FR-0026](frs/FR-0026-hero-bulk-update-and-delete.md) and [NFR-0027](nfrs/NFR-0027-bulk-action-size-cap.md). |
| Layering | The strict, one-directional import order between `src/`'s modules (`config` → `oidc` → `models` → `views` → `repositories` → `crud` → `health` → `controllers` → `lib`/`main`); see `src/README.md`'s "Layering" and [NFR-0018](nfrs/NFR-0018-strict-module-layering.md). |

## Do

- Add a term here the first time a requirement needs to use it
  precisely and it isn't already common English.
- Keep definitions short — one or two sentences; link to an ADR or a
  `src/` `README.md` section for anything that needs more.

## Don't

- Define implementation types or functions here that a requirement
  never needs to reference by name — that's a code reference (a doc
  comment in `src/`), not a domain term.
