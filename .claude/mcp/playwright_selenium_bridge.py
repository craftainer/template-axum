# /// script
# requires-python = ">=3.14"
# dependencies = ["selenium==4.49.0"]
# ///
"""Launch Playwright's MCP server against the selenium stack service's
browser -- ported from template-fastapi's `.claude/mcp/
playwright_selenium_bridge.py`, adapted for this repo's Rust/no-uv-project
setup: the PEP 723 inline metadata above lets `uv run` resolve `selenium`
as a throwaway dependency (see `../.mcp.json`'s `playwright` entry) rather
than requiring a Python project with a `dev` extra, which this repo has
no reason to carry.

@playwright/mcp only takes a static --cdp-endpoint, but selenium (Selenium
Grid) only hands out a CDP URL per WebDriver session -- there is no static
endpoint to point at. This opens that session, passes its `se:cdp`
capability through, and keeps the session alive for the MCP server's
lifetime (closing it would tear down the browser CDP is talking to). See
.devcontainer/stack/selenium/README.md and .claude/README.md.
"""

from __future__ import annotations

import os
import shutil
import signal
import subprocess
import sys
from types import FrameType

from selenium.webdriver import ChromeOptions, Remote

# renovate: datasource=npm depName=@playwright/mcp
_PLAYWRIGHT_MCP_VERSION = "0.0.80"


def main() -> None:
    """Open a selenium session, then run @playwright/mcp against its CDP URL."""
    selenium_url = os.environ.get("E2E_SELENIUM_URL", "http://selenium:4444")
    npx = shutil.which("npx")
    if npx is None:
        raise RuntimeError("npx not found on PATH")

    driver = Remote(command_executor=f"{selenium_url}/wd/hub", options=ChromeOptions())
    try:
        cdp_url = str(driver.capabilities["se:cdp"])
        process = subprocess.Popen(  # noqa: S603
            [npx, "-y", f"@playwright/mcp@{_PLAYWRIGHT_MCP_VERSION}", "--cdp-endpoint", cdp_url]
        )

        def _forward(signum: int, _frame: FrameType | None) -> None:
            process.send_signal(signum)

        signal.signal(signal.SIGTERM, _forward)
        signal.signal(signal.SIGINT, _forward)
        sys.exit(process.wait())
    finally:
        driver.quit()


if __name__ == "__main__":
    main()
