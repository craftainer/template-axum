# Per-instance settings for the image release contract in ./Makefile
# (template-owned). See docs/TEMPLATE.md's "Image layer (template-docker)"
# and docs/adrs/0019-re-parent-onto-template-docker.md.

# The `runner` stage of ./Dockerfile (both are the defaults, stated for
# clarity).
DOCKERFILE := Dockerfile
TARGET := runner

# The app does not read these; they are exposed to operators as env vars.
BUILD_ARGS := --build-arg APP_VERSION=$(RELEASE_VERSION) --build-arg APP_COMMIT=$(shell git rev-parse HEAD)

OCI_DESCRIPTION := Rust/axum REST service with generic CRUD, OIDC auth and health checks
OCI_LICENSE := GPL-3.0-or-later

# Written to dist/ by `coverage` below (a release-check step) and attached
# to the release; the image label points at it.
COVERAGE_REPORT := coverage.xml

# Per-arch smoke (`make release-assets`, on a bare CI runner): mock mode
# boots with zero infrastructure and answers /health/live. The real-services
# gate (NFR-0010) is `stack-smoke` below.
IMAGE_SMOKE_PORT := 8000
IMAGE_SMOKE_PATH := /health/live
IMAGE_SMOKE_RUN_ARGS := -e MODE=mock -e ALLOW_MOCK_MODE=1
IMAGE_SMOKE_TIMEOUT := 60

# release-check runs inside the devcontainer, where the stack is up.
release-check: coverage stack-smoke

.PHONY: coverage stack-smoke

coverage:
	mkdir -p dist
	cargo llvm-cov --cobertura --output-path dist/coverage.xml

# NFR-0010: the release image against real Postgres, Redis, S3 and Keycloak,
# polling /health/ready. The image runs on the devcontainer's own Docker
# daemon, which cannot join the stack's compose network, so each stack host
# name is pinned to the IP the devcontainer resolves it to.
stack-smoke:
	@args="-e MODE=dev -e POSTGRES_PORT=5432 -e POSTGRES_HOST=postgres -e POSTGRES_DB=$${POSTGRES_DB:-app} -e S3_ENDPOINT_URL=http://s3:9000 -e REDIS_URL=redis://redis:6379/0"; \
	for h in postgres redis s3 keycloak; do \
		ip="$$(getent hosts $$h | awk '{print $$1; exit}')"; \
		test -n "$$ip" || { echo "stack-smoke: cannot resolve $$h" >&2; exit 1; }; \
		args="$$args --add-host=$$h:$$ip"; \
	done; \
	for e in postgres/postgres s3/s3 keycloak/keycloak; do args="$$args --env-file=.devcontainer/stack/$$e.env"; done; \
	IMAGE_SMOKE_RUN_ARGS="$$args" IMAGE_SMOKE_PATH=/health/ready IMAGE_SMOKE_TIMEOUT=150 \
		bash scripts/image-smoke.sh
