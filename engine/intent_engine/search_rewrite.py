"""Optional, privacy-safe natural-language search query rewriting."""

from __future__ import annotations

import re

from intent_engine.llm_base import LLMClient
from intent_engine.llm import redact_for_prompt


MAX_REWRITE_QUERIES = 5
MAX_QUERY_LENGTH = 100


async def rewrite_search_queries(question: str, llm: LLMClient) -> list[str]:
    original = _sanitize(question)
    if not original:
        return [question.strip() or ""]
    try:
        result = await llm.respond_json(
            system="Return 1 to 5 short FTS keyword queries as JSON.",
            user=redact_for_prompt(question[:2000]),
            schema_name="search_queries",
            schema={
                "type": "object",
                "properties": {"queries": {"type": "array", "items": {"type": "string", "maxLength": MAX_QUERY_LENGTH}, "minItems": 1, "maxItems": MAX_REWRITE_QUERIES}},
                "required": ["queries"],
                "additionalProperties": False,
            },
        )
        queries = result.get("queries") if isinstance(result, dict) else None
        if not isinstance(queries, list):
            return [original]
        cleaned = []
        for query in queries[:MAX_REWRITE_QUERIES]:
            if isinstance(query, str):
                value = _sanitize(query)
                if value and value not in cleaned:
                    cleaned.append(value)
        return cleaned or [original]
    except Exception:
        return [original]


def _sanitize(value: str) -> str:
    value = redact_for_prompt(str(value or ""))
    value = re.sub(r"[\"'`*?:;(){}\[\]]", " ", value)
    return " ".join(value.split())[:MAX_QUERY_LENGTH].strip()
