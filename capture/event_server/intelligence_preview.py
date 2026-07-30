"""Redacted examples of data that may be sent to optional LLM providers."""

from __future__ import annotations

from typing import Any


def build_preview_sample(detailed_config: dict[str, Any]) -> dict[str, Any]:
    browser = detailed_config.get("browser", {}) if isinstance(detailed_config.get("browser"), dict) else {}
    browser_enabled = bool(browser.get("enabled"))
    context_enabled = bool(browser.get("context_enabled"))

    semantic_example = {
        "mode": "compact",
        "packets": [
            {
                "packet_id": "p0",
                "event_count": 3,
                "deterministic_cluster_id": 0,
                "timeline": [
                    {"offset_s": 0, "family": "browser", "domain": "docs.python.org", "action": "click"},
                    {"offset_s": 42, "family": "editor", "file": "README.md"},
                    {"offset_s": 95, "family": "command", "action": "pytest"},
                ],
                "domains": ["docs.python.org"],
                "files": ["README.md"],
            }
        ],
    }
    if context_enabled:
        semantic_example["packets"][0]["snippet"] = "Example bounded page excerpt (max 180 chars per packet)."

    labeling_example = {
        "policy_version": "safe-intent-features-v2",
        "domains": ["docs.python.org"],
        "file_names": ["README.md"],
        "dominant_family": "browser",
        "semantic_topic": "Python asyncio timeouts",
        "event_counts": {"browser": 2, "editor": 1},
        "duration_seconds": 900,
    }

    disclosure = [
        "Domain names and file basenames may be sent; full paths and query strings are not.",
        "Activity counts, durations, and optional semantic topics may be sent.",
        "Page excerpts are sent only when browser detailed capture and context are enabled.",
        "Passwords, blocked domains, API keys, and full page HTML are never sent.",
    ]
    if not browser_enabled:
        disclosure.append("Browser actions and excerpts are off; tab metadata stays local unless semantic full capture is enabled separately in Intelligence settings.")

    return {
        "semantic_clustering_example": semantic_example,
        "labeling_example": labeling_example,
        "copilot_example": {
            "question_chars_max": 2000,
            "tool_results": ["intent label", "summary", "browser domains", "semantic topic"],
            "raw_events_sent": False,
        },
        "disclosure": disclosure,
        "browser_detailed_enabled": browser_enabled,
        "browser_context_enabled": context_enabled,
    }
