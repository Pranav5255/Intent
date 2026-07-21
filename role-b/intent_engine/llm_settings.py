"""Local-only persistence for optional production LLM settings.

The development .env remains a source-tree convenience. Installed builds use
a per-user EnvironmentFile so package upgrades never touch secrets. This
module deliberately never returns an API key to a caller.
"""

from __future__ import annotations

import os
import tempfile
from pathlib import Path
from typing import Any

from intent_engine.providers import copilot_enabled


PROVIDERS = ("openai", "groq", "gemini", "bedrock")
_SECRET_KEYS = {
    "openai": "OPENAI_API_KEY",
    "groq": "GROQ_API_KEY",
    "gemini": "GEMINI_API_KEY",
    "bedrock": "AWS_BEARER_TOKEN_BEDROCK",
}
_MANAGED_KEYS = {
    "LLM_PROVIDER",
    "ROLE_B_LLM_ENABLED",
    "ENABLE_COPILOT",
    "INTENT_OS_LLM_MODEL",
    "OPENAI_API_KEY",
    "GROQ_API_KEY",
    "GROQ_BASE_URL",
    "GEMINI_API_KEY",
    "GOOGLE_CLOUD_PROJECT",
    "GOOGLE_CLOUD_LOCATION",
    "AWS_BEARER_TOKEN_BEDROCK",
    "BEDROCK_REGION",
    "BEDROCK_AWS_PROFILE",
}
_DEFAULT_GROQ_BASE_URL = "https://api.groq.com/openai/v1"


def settings_path() -> Path:
    override = os.environ.get("INTENT_OS_LLM_CONFIG", "").strip()
    if override:
        return Path(override)
    config_home = Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config"))
    return config_home / "intent-os" / "llm.env"


def load_saved_settings() -> dict[str, str]:
    """Read the private systemd EnvironmentFile, keeping only managed fields."""

    path = settings_path()
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError:
        return {}

    values: dict[str, str] = {}
    for raw_line in lines:
        line = raw_line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        key = key.strip()
        if key in _MANAGED_KEYS and _is_safe_value(value):
            values[key] = value
    return values


def settings_summary() -> dict[str, Any]:
    """Return provider settings without returning any credential material."""

    saved = load_saved_settings()

    def effective(name: str, default: str = "") -> str:
        return saved.get(name, os.environ.get(name, default)).strip()

    provider = effective("LLM_PROVIDER", "gemini").lower()
    if provider not in PROVIDERS:
        provider = "gemini"
    secret_name = _SECRET_KEYS[provider]
    return {
        "provider": provider,
        "model": effective("INTENT_OS_LLM_MODEL"),
        "copilot_enabled": copilot_enabled(),
        "api_key_configured": bool(effective(secret_name)),
        "groq_base_url": effective("GROQ_BASE_URL", _DEFAULT_GROQ_BASE_URL),
        "google_cloud_project": effective("GOOGLE_CLOUD_PROJECT"),
        "google_cloud_location": effective("GOOGLE_CLOUD_LOCATION", "us-central1"),
        "bedrock_region": effective("BEDROCK_REGION") or effective("AWS_REGION"),
        "bedrock_profile": effective("BEDROCK_AWS_PROFILE") or effective("AWS_PROFILE"),
    }


def save_settings(update: dict[str, Any]) -> dict[str, Any]:
    """Persist validated local settings and apply them to the running process."""

    provider = str(update["provider"]).strip().lower()
    if provider not in PROVIDERS:
        raise ValueError("Unsupported provider")

    values = load_saved_settings()
    values["LLM_PROVIDER"] = provider
    values["ROLE_B_LLM_ENABLED"] = "true" if update.get("enable_copilot", True) else "false"
    values["ENABLE_COPILOT"] = "true" if update.get("enable_copilot", True) else "false"
    _set_optional(values, "INTENT_OS_LLM_MODEL", update.get("model"))
    _set_optional(values, "GROQ_BASE_URL", update.get("groq_base_url"))
    _set_optional(values, "GOOGLE_CLOUD_PROJECT", update.get("google_cloud_project"))
    _set_optional(values, "GOOGLE_CLOUD_LOCATION", update.get("google_cloud_location"))
    _set_optional(values, "BEDROCK_REGION", update.get("bedrock_region"))
    _set_optional(values, "BEDROCK_AWS_PROFILE", update.get("bedrock_profile"))

    secret_name = _SECRET_KEYS[provider]
    if update.get("clear_api_key"):
        values.pop(secret_name, None)
    elif update.get("api_key") is not None:
        _set_optional(values, secret_name, update.get("api_key"))

    _write_settings(values)
    _apply_environment(values)
    return settings_summary()


def _set_optional(values: dict[str, str], name: str, raw_value: object | None) -> None:
    if raw_value is None:
        return
    value = str(raw_value).strip()
    if value:
        _require_safe_value(value)
        values[name] = value
    else:
        values.pop(name, None)


def _apply_environment(values: dict[str, str]) -> None:
    for name in _MANAGED_KEYS:
        if name in values:
            os.environ[name] = values[name]
        else:
            os.environ.pop(name, None)


def _write_settings(values: dict[str, str]) -> None:
    for value in values.values():
        _require_safe_value(value)

    path = settings_path()
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    path.parent.chmod(0o700)
    body = (
        "# Intent optional LLM settings. Managed locally; never commit this file.\n"
        + "".join(f"{key}={values[key]}\n" for key in sorted(values))
    )
    descriptor, temporary_name = tempfile.mkstemp(prefix=".llm-", dir=path.parent, text=True)
    try:
        os.fchmod(descriptor, 0o600)
        with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
            handle.write(body)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary_name, path)
        path.chmod(0o600)
    finally:
        try:
            os.unlink(temporary_name)
        except FileNotFoundError:
            pass


def _is_safe_value(value: str) -> bool:
    return bool(value) and not any(character.isspace() for character in value) and not any(
        character in value for character in ("\x00", "\n", "\r")
    )


def _require_safe_value(value: str) -> None:
    if not _is_safe_value(value):
        raise ValueError("Settings values must be non-empty single-line values without whitespace")
