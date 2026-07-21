#!/usr/bin/env python3
"""Replay a Role A day fixture into a local Intent event server."""

from __future__ import annotations

import argparse
import json
import time
from pathlib import Path
from urllib import request


def post(url: str, payload: dict) -> None:
    body = json.dumps(payload).encode("utf-8")
    response = request.urlopen(
        request.Request(url, data=body, headers={"Content-Type": "application/json"}, method="POST"),
        timeout=3,
    )
    # Detailed-capture events may be intentionally disabled on a fresh install.
    # A 204 still proves the fixture was accepted by the local ingest boundary.
    if response.status not in {200, 201, 204}:
        raise RuntimeError(f"server returned HTTP {response.status}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("fixture", type=Path, nargs="?", default=Path("fixtures/demo-day.json"))
    parser.add_argument("--url", default="http://127.0.0.1:9477/v1/event")
    parser.add_argument("--interval", type=float, default=0, help="seconds between events")
    args = parser.parse_args()

    fixture = json.loads(args.fixture.read_text(encoding="utf-8"))
    for event in fixture["events"]:
        post(args.url, event)
        print(f"emitted {event['source']}/{event['type']} {event['id']}")
        if args.interval:
            time.sleep(args.interval)


if __name__ == "__main__":
    main()
