#!/usr/bin/env python3
"""Start the workspace fallback collector for the current user configuration."""

from __future__ import annotations

import os
import sys
from pathlib import Path

INSTALL_ROOT = Path(os.environ.get("INTENT_OS_INSTALL_ROOT", "/opt/intent-os"))
if str(INSTALL_ROOT) not in sys.path:
    sys.path.insert(0, str(INSTALL_ROOT))

from tools.workspaces import load


def main() -> int:
    try:
        paths = [Path(item) for item in load()["workspaces"]]
        if not paths:
            return 0
        from collectors.workspace.watcher import run
        run("http://127.0.0.1:9477/v1/event", paths)
    except (OSError, ValueError) as exc:
        print(f"Intent OS workspace watcher: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
