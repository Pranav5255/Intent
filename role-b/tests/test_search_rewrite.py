import asyncio

from intent_engine.search_rewrite import rewrite_search_queries


class RewriteLLM:
    def __init__(self, result):
        self.result = result
        self.calls = []

    async def respond_json(self, **kwargs):
        self.calls.append(kwargs)
        return self.result


def run(awaitable):
    return asyncio.run(awaitable)


def test_rewrite_sanitizes_and_caps_queries():
    llm = RewriteLLM({"queries": ["AccessDenied", "iam", "terraform", "git", "docs", "extra", "sk-secret"]})
    result = run(rewrite_search_queries("Why did AWS permissions fail during Terraform apply?", llm))
    assert result == ["AccessDenied", "iam", "terraform", "git", "docs"]
    assert "sk-secret" not in str(llm.calls)


def test_rewrite_failure_falls_back_to_original():
    class Broken:
        async def respond_json(self, **kwargs):
            raise RuntimeError("offline")

    result = run(rewrite_search_queries("Why did AWS permissions fail?", Broken()))
    assert result == ["Why did AWS permissions fail"]
