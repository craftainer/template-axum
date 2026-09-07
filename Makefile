# The release contract release.yml drives -- see docs/TEMPLATE.md's
# "Release: a Makefile contract" section. This instance's release artifact
# is the `runner` stage OCI image, tagged with RELEASE_VERSION (release.yml
# exports it, per the same doc's "Two environment variables" note).

IMAGE_NAME := template-axum
IMAGE_TAG := $(IMAGE_NAME):$(RELEASE_VERSION)
IMAGE_TAR := dist/$(IMAGE_NAME)-$(RELEASE_VERSION).tar
SBOM_FILE := dist/$(IMAGE_NAME)-$(RELEASE_VERSION).spdx.json

.PHONY: build sbom release-assets publish

build:
	docker build --target runner -t $(IMAGE_TAG) .

# Syft introspects the local Docker daemon's image directly -- no separate
# OCI-tarball export needed since, unlike release.yml's own amd64/arm64
# matrix build, this Makefile only ever builds for the host's own arch.
sbom:
	mkdir -p dist
	docker run --rm \
	    -v /var/run/docker.sock:/var/run/docker.sock \
	    anchore/syft:v1.23.1 \
	    "docker:$(IMAGE_TAG)" -o spdx-json > $(SBOM_FILE)

release-assets:
	mkdir -p dist
	docker save $(IMAGE_TAG) -o $(IMAGE_TAR)

# Skips cleanly when OCI_REGISTRY isn't set, matching template-fastapi's own
# publish step (see its release.yml's "Push runner image to OCI registry").
publish:
	@if [ -z "$(OCI_REGISTRY)" ]; then \
	    echo "OCI_REGISTRY not set -- skipping publish."; \
	else \
	    docker tag $(IMAGE_TAG) $(OCI_REGISTRY)/$(IMAGE_NAME):$(RELEASE_VERSION) && \
	    docker push $(OCI_REGISTRY)/$(IMAGE_NAME):$(RELEASE_VERSION); \
	fi
