# selenium

A browser (Chromium, via Selenium Grid's standalone image) for the e2e
suite, added once phase 3 brings tests -- see template-fastapi's own
`tests/e2e/conftest.py` for the pattern this instance intends to follow:
a `browser` fixture opens a WebDriver session against this container
purely to read its `se:cdp` Chrome DevTools Protocol URL, then hands that
URL to Playwright's `connect_over_cdp`, so Playwright (run from the `api`
container) drives this remote browser instead of launching a local one.
That's what lets the e2e suite run from inside the devcontainer itself,
unlike the Postgres/RustFS/Keycloak-style pattern of exec-ing into a
sibling container from the host.

- Compose file: `compose.yml`
- Image: `selenium/standalone-chromium`
- Reached from: the e2e suite (phase 3), at `http://selenium:4444`
- Target under test: `http://api:8000`

## Do

- Run the e2e suite from the devcontainer's own terminal, same as any
  other check, once phase 3 adds one.

## Don't

- Point the e2e suite's own selenium/base-URL config at anything other
  than this stack's own in-network service addresses.

## Removing this service

Delete this directory, its compose file entry in
`.devcontainer/compose.yml`'s `include:` list, and the `api` service's
matching `depends_on:` entry there.
