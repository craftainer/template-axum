# Stakeholders

Who has a stake in this app, what they care about, and how much say
they have over its direction. Referenced from `frs/` and `nfrs/`
requirements' `Source` sections.

| Name / role | Interest | Influence | Notes |
| --- | --- | --- | --- |
| API consumers | Stable, predictable CRUD/versioning/error behavior across JSON, XML, and HTML | Medium | Includes both current (v2) and legacy (v1) integrators, plus the progressively-enhanced HTML form. |
| Legacy API consumers | v1 endpoints keep working, with clear deprecation notice before removal | Medium | Depend on [FR-0031](frs/FR-0031-hero-v1-deprecated-compat-crud.md), [FR-0024](frs/FR-0024-sunset-deprecation-header-mechanism.md). |
| Operators / SRE | Health checks, structured logs, automatic migrations, config-from-env, rate-limit fail-fast behavior | High | Run and monitor the deployed service; see [NFR-0009](nfrs/NFR-0009-extensible-health-registry.md), [NFR-0025](nfrs/NFR-0025-rate-limit-backend-outage-fails-fast.md). |
| Security / compliance | Auth enforcement, RBAC, error redaction, server-side-only authorization | High | Drives [ADR 0003](adrs/0003-auth-strategy-provider-agnostic-oidc.md), [NFR-0011](nfrs/NFR-0011-server-side-authorization-only.md), [NFR-0015](nfrs/NFR-0015-exception-detail-redaction.md). |
| Platform / infrastructure maintainers | Stateless, regionally-scalable token validation; provider-agnostic OIDC; devcontainer/CI stack topology | High | See [NFR-0013](nfrs/NFR-0013-stateless-token-validation-scalability.md), [NFR-0014](nfrs/NFR-0014-oidc-claim-shape-portability.md). |
| Developers maintaining the template | Layering, extensibility, low-cost pattern for adding a new resource/representation | High | The template's primary audience; see `src/README.md`'s "Example CRUD resource: Hero". |
| QA / CI | Coverage gate, independent test tiers, load-test tier | Medium | See [NFR-0023](nfrs/NFR-0023-test-coverage-gate.md), [NFR-0024](nfrs/NFR-0024-independent-test-tiers.md). |
| Release engineering | Release smoke-test gate against real dependencies, SBOM/publish contract | Medium | See [NFR-0010](nfrs/NFR-0010-release-smoke-test-gate.md). |

## Do

- Add a row as soon as a new requirement's `Source` would otherwise
  need to describe a stakeholder inline.
- Keep "Interest" specific enough that a reader can guess which
  requirements trace back to this stakeholder without opening each one.

## Don't

- Name an individual by their personal name if their role is what
  matters to the requirement — prefer the role (e.g. "on-call SRE"),
  unless a specific person's involvement is itself relevant context.
