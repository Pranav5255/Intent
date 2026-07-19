"""Run the deterministic Role B pipeline against the bundled demo fixture."""

from __future__ import annotations

import asyncio
import sys
from pathlib import Path

if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8")

sys.path.insert(0, str(Path(__file__).parents[1]))

from intent_engine.cluster import cluster_session
from intent_engine.enrich import compute_stats, derive_project_tag
from intent_engine.normalize import normalize_events
from intent_engine.resume import build_resume_payload
from intent_engine.sessionize import sessionize
from intent_engine.source import load_replay_fixture


FIXTURE_PATH = Path(__file__).parent / "fixtures" / "demo-day.json"


async def _create_clusters(normalized_events):
    sessions = await sessionize(normalized_events)
    clusters = []
    for session in sessions:
        clusters.extend(await cluster_session(session))
    return sessions, clusters


def main() -> int:
    try:
        export = load_replay_fixture(str(FIXTURE_PATH))
        print(f"✅ Loaded {len(export.events)} events")
    except Exception as exc:
        print(f"❌ Load fixture failed: {type(exc).__name__}: {exc}")
        return 1

    try:
        normalized_events, warnings = normalize_events(export.events)
        print(f"✅ Normalized {len(normalized_events)} events ({len(warnings)} warnings)")
    except Exception as exc:
        print(f"❌ Normalize events failed: {type(exc).__name__}: {exc}")
        return 1

    try:
        sessions, clusters = asyncio.run(_create_clusters(normalized_events))
        print(f"✅ Created {len(sessions)} sessions")
        print(f"✅ Created {len(clusters)} clusters total")
    except Exception as exc:
        print(f"❌ Sessionize/cluster failed: {type(exc).__name__}: {exc}")
        return 1

    project_tags: list[str] = []
    all_files: list[str] = []
    all_urls: list[str] = []
    try:
        for index, cluster in enumerate(clusters, start=1):
            stats = compute_stats(cluster)
            project_tag = derive_project_tag(cluster)
            payload = build_resume_payload(cluster)
            if project_tag:
                project_tags.append(project_tag)
            all_files.extend(payload.files)
            all_urls.extend(payload.urls)
            print(
                f"Cluster {index}: events={stats.event_count}, duration={stats.duration_seconds}s, "
                f"project_tag={project_tag}, files={payload.files}, urls={payload.urls}"
            )
    except Exception as exc:
        print(f"❌ Enrichment/resume failed: {type(exc).__name__}: {exc}")
        return 1

    checks = [
        ("At least 2 clusters", len(clusters) >= 2),
        ("Total events == 28", len(export.events) == 28),
        ("Project tag present", "project:taskflow-app" in project_tags or bool(project_tags)),
        ("Resume contains auth.tsx", any(path.endswith("auth.tsx") for path in all_files)),
        ("Resume contains URLs", bool(all_urls)),
    ]
    for label, passed in checks:
        icon = "✅" if passed else "❌"
        print(f"{icon} {label}")

    passed_count = sum(passed for _, passed in checks)
    if passed_count == len(checks):
        print(f"✅ PASS: {passed_count}/{len(checks)} checks")
        return 0

    print(f"❌ FAIL: {passed_count}/{len(checks)} checks")
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
