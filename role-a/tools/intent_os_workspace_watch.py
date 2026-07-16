#!/usr/bin/env python3
"""Start the workspace fallback collector for the current user configuration."""

from __future__ import annotations

import os
import sys
from pathlib import Path

INSTALL_ROOT = Path(os.environ.get("INTENT_OS_INSTALL_ROOT", "/opt/intent-os"))
SOURCE_ROOT = Path(__file__).resolve().parents[1]
IMPORT_ROOT = SOURCE_ROOT if (SOURCE_ROOT / "event_server").is_dir() else INSTALL_ROOT
if str(IMPORT_ROOT) not in sys.path:
    sys.path.insert(0, str(IMPORT_ROOT))

from tools.workspaces import load
from tools.filesystem_capture import load as load_filesystem_capture
from event_server.detailed_capture import load as load_detailed_capture


def main() -> int:
    try:
        paths = [Path(item) for item in load()["workspaces"]]
        filesystem = load_filesystem_capture()
        detailed = load_detailed_capture()
        if filesystem["all_accessible"]:
            paths.append(Path("/"))
        if not paths:
            return 0
        from collectors.workspace.watcher import run
        run("http://127.0.0.1:9477/v1/event", paths, capture_content=detailed["filesystem"]["enabled"])
    except (OSError, ValueError) as exc:
        print(f"Intent OS workspace watcher: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
