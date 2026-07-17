import asyncio
import tempfile
from pathlib import Path

import pytest

import mcp_server
from intent_engine.store import IntentStore


class FakeMCP:
    def __init__(self, name):
        self.name = name
        self.functions = {}

    def tool(self):
        def decorate(function):
            self.functions[function.__name__] = function
            return function
        return decorate

    def run(self):
        return None


def test_adapter_constructs_registry_and_exposes_allowlist(monkeypatch):
    monkeypatch.setattr(mcp_server, "_load_fastmcp", lambda: FakeMCP)
    with tempfile.TemporaryDirectory() as directory:
        server = mcp_server.create_mcp_server(IntentStore(str(Path(directory) / "intents.db")))
        assert set(server.role_b_tool_names) == {
            "search_intents", "get_intent", "get_resume_payload",
            "get_current_intent", "get_intent_stats",
        }
        assert set(server.functions) == set(server.role_b_tool_names)
        result = asyncio.run(server.functions["get_intent"]("missing"))
        assert result == {"error": "not_found"}


def test_missing_sdk_has_install_guidance(monkeypatch):
    def missing():
        raise RuntimeError("MCP support is optional. Install it with pip install -r requirements-mcp.txt")
    monkeypatch.setattr(mcp_server, "_load_fastmcp", missing)
    with pytest.raises(RuntimeError, match="requirements-mcp.txt"):
        mcp_server.create_mcp_server()


def test_adapter_has_no_direct_database_imports():
    source = Path(mcp_server.__file__).read_text(encoding="utf-8")
    assert "import sqlite3" not in source
    assert "import aiosqlite" not in source
