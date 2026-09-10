#!/usr/bin/env python3
"""Enforce src/'s module layering (docs/adrs/0009, docs/nfrs/NFR-0018) --
the automated gate that ADR 0009 documented as a follow-up rather than
building at the time, and that
docs/plans/2026-09-fastapi-parity-improvements.md's item 3 asks for in
place of convention/review alone. Backs the `check-layering` prek hook.

Two rules, checked against every real `crate::`-qualified path reference
in src/ (comments excluded -- a module doc mentioning a name in prose
isn't a dependency):

1. Independence: no file under `src/generic/` may reference
   `crate::hero::` (or any future sibling resource package) at all --
   this is the actual promise `src/README.md`'s "Generic vs.
   Hero-specific split" and `docs/nfrs/NFR-0004-generic-crud-excludes-
   resource-logic.md` make, mirroring template-fastapi's `import-linter`
   independence contract between its own `crud/` and `app/`.
2. Layering: every module category may only import from the categories
   its own entry in ALLOWED_IMPORTS lists -- a lower/earlier layer never
   imports from a higher/later one. The category graph is a DAG, not a
   strict total order, since a few cross-cutting modules
   (`problem_details`, `oidc`) sit outside the main
   config -> ... -> controllers chain (see each module's own doc
   comment in src/ for why); ALLOWED_IMPORTS reflects the real edges
   src/README.md's "Layering" section and each module's own header
   document, not an idealized chain.

Exit status is non-zero (with every violation printed) if either rule is
broken anywhere in src/.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

DEFAULT_SRC = Path(__file__).resolve().parent.parent.parent / "src"

# Every module category a file under src/ can belong to, keyed by the
# `crate::`-path prefix that names it, mapped to the set of categories it
# may import from (itself is always implicitly allowed -- a file may
# always reference a sibling in its own category).
ALLOWED_IMPORTS: dict[str, set[str]] = {
    "config": set(),
    # Cross-cutting utility modules (src/README.md: "stay flat, outside
    # any submodule ... no resource-specific code and no state of its
    # own"). problem_details reaches into generic::views/repositories
    # for AppError's `From` impls and FieldError; oidc depends on
    # problem_details for AppError; the rest are leaves.
    "problem_details": {"config", "generic::views", "generic::repositories"},
    "oidc": {"config", "problem_details"},
    "events": set(),
    "rate_limit": {"problem_details"},
    "http_headers": set(),
    "telemetry": set(),
    "generic::models": set(),
    "hero::models": {"generic::models"},
    "generic::views": {"generic::models"},
    "hero::views": {"generic::models", "generic::views", "hero::models"},
    "generic::repositories": {"generic::views"},
    "hero::repositories": {
        "generic::repositories",
        "generic::views",
        "hero::models",
        "hero::views",
    },
    "crud": {"generic::repositories"},
    "health": set(),
    # SeaORM migrations, applied at startup (src/README.md's
    # "Migrations") -- self-contained, no crate:: dependency of its own.
    "migration": set(),
    "generic::controllers": {
        "config",
        "oidc",
        "problem_details",
        "events",
        "rate_limit",
        "http_headers",
        "generic::models",
        "generic::views",
        "generic::repositories",
        "crud",
        "health",
    },
    "hero::controllers": {
        "config",
        "oidc",
        "problem_details",
        "events",
        "rate_limit",
        "http_headers",
        "generic::models",
        "generic::views",
        "generic::repositories",
        "crud",
        "health",
        "generic::controllers",
        "hero::models",
        "hero::views",
        "hero::repositories",
    },
}

# lib.rs/main.rs: the wiring root, above every layer -- may import
# anything. Filled in after the literal above so "root" doesn't have to
# enumerate (and keep in sync with) every other key by hand.
ALLOWED_IMPORTS["root"] = set(ALLOWED_IMPORTS)

# A file's category is its path relative to src/, with everything after
# the first two directory levels (if any) dropped -- e.g.
# "generic/controllers/heroes.rs" -> "generic::controllers",
# "config.rs" -> "config", "lib.rs"/"main.rs" -> "root".
_LINE_COMMENT = re.compile(r"^\s*//")
_CRATE_PATH = re.compile(r"\bcrate::([a-zA-Z_][a-zA-Z0-9_]*(?:::[a-zA-Z_][a-zA-Z0-9_]*)*)")


def category_for(path: Path, src: Path) -> str:
    """The module category a src/ file belongs to, per ALLOWED_IMPORTS's keys."""
    rel = path.relative_to(src)
    parts = rel.parts
    if parts[0] in ("lib.rs", "main.rs"):
        return "root"
    if parts[0].endswith(".rs"):
        return parts[0][: -len(".rs")]
    if len(parts) == 2 and parts[0] in ("generic", "hero") and parts[1] == "mod.rs":
        # The package's own top-level file (e.g. src/generic/mod.rs) --
        # pure `pub mod ...` re-exports, nothing to check imports
        # against; give it the most permissive category so it never
        # false-positives.
        return "root"
    if len(parts) >= 2 and parts[0] in ("generic", "hero"):
        return f"{parts[0]}::{parts[1]}"
    # migration/, crud/, health/, oidc/ are single-package directories.
    return parts[0]


def referenced_categories(text: str) -> set[str]:
    """Every module category actually referenced via a real `crate::`
    path in this file's code (not inside a `//`/`//!`/`///` comment)."""
    categories = set()
    for line in text.splitlines():
        if _LINE_COMMENT.match(line):
            continue
        # Strip a trailing line comment on an otherwise-code line, so a
        # trailing `// crate::hero::...` explanatory aside isn't treated
        # as a real reference either.
        code = line.split("//", 1)[0]
        for match in _CRATE_PATH.finditer(code):
            # A macro invocation (`crate::dyn_repository!(...)`), not a
            # module path -- `#[macro_export]` macros live at the crate
            # root regardless of which module defines them, so they
            # aren't part of the module layering this script checks.
            if code[match.end() :].lstrip().startswith("!"):
                continue
            path = match.group(1)
            segments = path.split("::")
            if segments[0] in ("generic", "hero") and len(segments) >= 2:
                categories.add(f"{segments[0]}::{segments[1]}")
            else:
                categories.add(segments[0])
    return categories


def check(src: Path) -> list[str]:
    """Every layering violation found under `src` -- empty if none."""
    violations: list[str] = []
    for rs_file in sorted(src.rglob("*.rs")):
        own_category = category_for(rs_file, src)
        allowed = ALLOWED_IMPORTS.get(own_category)
        if allowed is None:
            # An unrecognized top-level module -- fail loudly rather than
            # silently skipping it (a new src/ module should be added to
            # ALLOWED_IMPORTS deliberately, not slip past this check).
            violations.append(
                f"{rs_file.relative_to(src.parent)}: module category "
                f"'{own_category}' has no ALLOWED_IMPORTS entry -- add one."
            )
            continue

        text = rs_file.read_text()
        for referenced in referenced_categories(text):
            if referenced == own_category:
                continue
            if referenced not in ALLOWED_IMPORTS:
                # Reference to a category this script doesn't know about
                # (e.g. a typo, or a new top-level module) -- surface it
                # rather than silently ignoring it.
                violations.append(
                    f"{rs_file.relative_to(src.parent)}: references unknown "
                    f"module category '{referenced}' via crate::{referenced}::..."
                )
                continue
            if referenced not in allowed:
                violations.append(
                    f"{rs_file.relative_to(src.parent)} ('{own_category}') "
                    f"imports '{referenced}', which isn't in its allowed set "
                    f"{sorted(allowed) or '{}'} -- see docs/adrs/0009 and "
                    "src/README.md's \"Generic vs. Hero-specific split\"."
                )
    return violations


def main() -> int:
    # Overridable src/ root, argv[1], only so tests/check_layering.rs can
    # point this at a small synthetic tree instead of the real src/ --
    # every real invocation (the prek hook, a manual run) takes the
    # default.
    src = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_SRC
    violations = check(src)

    if violations:
        print("Layering violations found:", file=sys.stderr)
        for violation in violations:
            print(f"  - {violation}", file=sys.stderr)
        return 1

    print(f"check_layering: {len(list(src.rglob('*.rs')))} files OK.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
