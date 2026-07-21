import os
import stat

from intent_engine.llm_settings import save_settings, settings_summary


def test_llm_settings_save_keys_locally_without_returning_them(monkeypatch, tmp_path):
    config_path = tmp_path / "intent-os" / "llm.env"
    monkeypatch.setenv("INTENT_OS_LLM_CONFIG", str(config_path))
    monkeypatch.delenv("OPENAI_API_KEY", raising=False)
    monkeypatch.delenv("ROLE_B_LLM_ENABLED", raising=False)
    monkeypatch.delenv("ENABLE_COPILOT", raising=False)

    secret = "sk-local-test-secret"
    response = save_settings({
        "provider": "openai",
        "api_key": secret,
        "model": "gpt-4o-mini",
        "enable_copilot": True,
    })

    assert secret not in str(response)
    assert response["provider"] == "openai"
    assert response["api_key_configured"] is True
    assert config_path.read_text(encoding="utf-8").count(secret) == 1
    assert stat.S_IMODE(config_path.stat().st_mode) == 0o600
    assert os.environ["OPENAI_API_KEY"] == secret

    summary = settings_summary()
    assert secret not in str(summary)
    assert summary["api_key_configured"] is True


def test_llm_settings_keep_existing_key_when_a_blank_form_is_saved(monkeypatch, tmp_path):
    config_path = tmp_path / "llm.env"
    monkeypatch.setenv("INTENT_OS_LLM_CONFIG", str(config_path))
    save_settings({"provider": "groq", "api_key": "gsk-local", "enable_copilot": True})
    response = save_settings({
        "provider": "groq",
        "model": "openai/gpt-oss-20b",
        "groq_base_url": "https://api.groq.com/openai/v1",
    })

    assert response["api_key_configured"] is True
    assert "gsk-local" in config_path.read_text(encoding="utf-8")


def test_llm_settings_reject_multiline_secret(monkeypatch, tmp_path):
    monkeypatch.setenv("INTENT_OS_LLM_CONFIG", str(tmp_path / "llm.env"))

    try:
        save_settings({"provider": "gemini", "api_key": "line-one\nline-two"})
    except ValueError:
        return
    raise AssertionError("multiline credentials must be rejected")
