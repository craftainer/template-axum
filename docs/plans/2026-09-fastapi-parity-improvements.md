# Adopt remaining template-fastapi improvements

## Status

Draft

## Goal

`template-fastapi` is this repo's sibling template and has continued to
evolve since the last parity pass (`feat: close remaining
template-fastapi parity gaps`). A structural comparison against a fresh
clone (see Approach) turned up four gaps that are genuine process/
organization improvements rather than Python-specific detail, plus two
smaller items worth a decision. This plan tracks porting them.

## Approach

Comparison method: cloned `git@github.com:craftainer/template-fastapi.git`
into a scratch worktree and diffed top-level structure, `docs/`,
`.claude/`, `.github/`, `.devcontainer/`, `CLAUDE.md`, and `src/`
layering against this repo. Items below are ordered by priority; each is
independent and can be picked up on its own.

### 1. Split the moderation workflows so Claude never holds write-scoped tokens

`/workspace/.github/workflows/moderate-bug-fix.yml` and
`moderate-feature-build.yml` currently declare
`contents: write` / `issues: write` / `pull-requests: write` on the same
job that invokes Claude against untrusted issue/PR content. Fastapi
splits this in two: the Claude-invoking job runs with `contents: read`
only and either commits locally or writes a `.moderation-outcome.md`;
a separate `moderate-bug-fix-apply.yml` / `moderate-feature-build-apply.yml`
workflow (no Claude invocation) does the actual push/PR/relabel with
write permissions, driven by small `moderate_apply_result.sh` /
`moderate_package_result.sh` scripts.

- Read fastapi's `moderate-bug-fix.yml` + `moderate-bug-fix-apply.yml` +
  `.github/scripts/moderate_apply_result.sh` /
  `moderate_package_result.sh` as the reference shape.
- Mirror the split for both `moderate-bug-fix.yml` and
  `moderate-feature-build.yml` in this repo, adding the two `-apply.yml`
  workflows and their scripts under `.github/scripts/`.
- Update `.github/workflows/README.md` and `.github/scripts/README.md`
  to describe the new two-stage flow (they already document the
  existing scripts' conventions — extend, don't rewrite).

### 2. Add a release smoke-test gate

Fastapi's `release.yml` boots the just-built image against a real
Postgres/Redis/S3/Keycloak stack (a dedicated root-level `compose.yml`,
separate from `.devcontainer/compose.yml`) and polls `/health/ready`
before the release proceeds to `make sbom`. This repo's `release.yml` /
`Makefile` ships the OCI image without ever running it first.

- Add a root-level `compose.yml` (smoke-test stack) modeled on
  fastapi's, adapted to this repo's services (check
  `.devcontainer/stack/` for which backing services actually apply —
  don't include services fastapi has that this repo doesn't).
- Add a `smoke.yml` step (or inline step in `release.yml`) that starts
  the stack, runs the built `runner` image against it, and polls the
  readiness endpoint before `make sbom` / `make publish`.
- Add an NFR documenting the gate (fastapi's is
  `NFR-0010-release-smoke-test-gate.md`); this repo's `docs/nfrs/`
  currently has no equivalent between `NFR-0009` and `NFR-0011` — check
  numbering doesn't collide before assigning.

### 3. Formalize the generic-CRUD vs. resource-specific split in `src/`

`/workspace/src/README.md`'s own "Layering" section already
acknowledges this: import direction is enforced "by convention plus
each module's own doc comment, not an automated lint — a documented gap
against the stricter enforcement template-fastapi's `import-linter`
provides." Fastapi enforces the split structurally: `src/crud/` holds
only generic, resource-agnostic code (models/views/interfaces/
repositories/controllers, forbidden from containing resource-specific
logic by `NFR-0004-generic-crud-excludes-resource-logic`), and
`src/app/` holds only the Hero-specific code that builds on it — each
with its own `import-linter` contract.

This repo's `controllers/`, `views/`, `models/`, `repositories/` each
mix generic and Hero-specific code in the same directory today (e.g.
`src/controllers/` has `crud_actions.rs`/`crud_query.rs` next to
`heroes.rs`/`heroes_web.rs` with no directory boundary).

- Decide whether to physically split each layer into a `crud/`
  subtree (generic) and an app-equivalent subtree (Hero-specific),
  mirroring fastapi's two-package structure, or to keep flat directories
  but add automated enforcement of the existing convention.
- If splitting: this touches every layer and all their call sites —
  plan it as its own follow-up plan document once scoped, don't fold
  the mechanical rename into this one.
- Either way, add automated import-direction enforcement in place of
  the doc-comment convention — evaluate a Rust equivalent of
  `import-linter` (e.g. a `cargo-modules`-based check, or a small custom
  lint script run via `prek`) rather than assuming a 1:1 tool exists.
- This is the largest item here; treat 1/2/4 as independently shippable
  first.

### 4. Add `docs/glossary.md` and `docs/stakeholders.md`

Cheap, clearly portable, no language dependency:

- `docs/glossary.md` — table of domain terms (Hero, MODE, CrudService,
  AppError/Problem Details, RBAC, Sunset header, JWKS, OIDC,
  Deprecation, etc.) with rules for when a term earns an entry. Base the
  structure on fastapi's version but populate with this repo's actual
  terms (check `src/README.md` and the ADRs/FRs/NFRs for the current
  vocabulary rather than copying fastapi's Hero-specific rows verbatim).
- `docs/stakeholders.md` — table of who cares about the system (API
  consumers, operators/SRE, security/compliance, platform maintainers,
  QA/CI, release engineering), referenced from FR/NFR "Source" sections
  going forward.

Consider `docs/system-design.md` (app-level layering/request-flow
diagrams, kept consistent with `src/README.md`'s own diagrams) and
`docs/architecture.md` (infra/deployment topology, devcontainer service
graph, smoke-test stack topology once #2 lands) as a follow-on — lower
priority than the glossary/stakeholders tables since `src/README.md`
and `.devcontainer/README.md` already cover most of that ground at the
package level.

### Smaller items, worth a decision rather than immediate action

- **`.claude/mcp/playwright_selenium_bridge.py`**: fastapi has an MCP
  bridge script letting Claude drive the devcontainer's Selenium
  service via Playwright MCP for e2e browser work. This repo has
  Selenium in `.devcontainer/stack/` but no such bridge. Worth adding
  only if e2e work here actually wants Claude-driven browser
  interaction — ask before porting rather than assuming.
- **`docs/TEMPLATE.md`**: still reads close to the bare `template-base`
  scaffold; fastapi's has since been filled in with Python-specific
  Getting-Started/Checks/Release detail. Check whether this repo's copy
  needs the same treatment for Rust, independent of the other items
  here.

## Open questions

- For #3, is a full directory split (mirroring fastapi's two-package
  structure) worth the churn versus adding automated enforcement to the
  existing flat layout? This plan doesn't decide that — scope it
  separately once #1/#2/#4 are done.
- For #2, which services belong in the smoke-test stack depends on
  which of this repo's `.devcontainer/stack/` services are actually
  exercised at runtime vs. dev-only — needs a quick check against
  `src/` before writing the compose file.
