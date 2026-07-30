"""Local-only persistence for optional production LLM settings.

Installed builds store settings in ~/.config/intent/llm.env. The desktop UI is
the source of truth for provider credentials. This module never returns secrets.
"""

from __future__ import annotations

import json
import os
import tempfile
from pathlib import Path
from typing import Any

from intent_engine.providers import (
    copilot_enabled,
    semantic_clustering_enabled,
    semantic_content_consent_granted,
    semantic_full_capture_consent_granted,
    semantic_timeout_ms,
)


PROVIDERS = ("openai", "groq", "gemini", "bedrock")
_SECRET_KEYS = {
    "openai": "OPENAI_API_KEY",
    "groq": "GROQ_API_KEY",
    "bedrock": "AWS_BEARER_TOKEN_BEDROCK",
}
_MANAGED_KEYS = {
    "LLM_PROVIDER",
    "ENGINE_LLM_ENABLED",
    "ENABLE_COPILOT",
    "ENGINE_SEMANTIC_CLUSTER",
    "ENGINE_SEMANTIC_CONTENT_CONSENT",
    "ENGINE_SEMANTIC_FULL_CAPTURE_CONSENT",
    "ENGINE_SEMANTIC_TIMEOUT_MS",
    "INTENT_LLM_MODEL",
    "OPENAI_API_KEY",
    "GROQ_API_KEY",
    "GROQ_BASE_URL",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "GEMINI_CREDENTIALS_PATH",
    "GOOGLE_CLOUD_PROJECT",
    "GOOGLE_CLOUD_LOCATION",
    "AWS_BEARER_TOKEN_BEDROCK",
    "BEDROCK_REGION",
    "BEDROCK_AWS_PROFILE",
}
_DEFAULT_GROQ_BASE_URL = "https://api.groq.com/openai/v1"
_GEMINI_CREDENTIALS_NAME = "gemini-credentials.json"


def config_dir() -> Path:
    override = os.environ.get("INTENT_CONFIG_DIR", "").strip()
    if override:
        return Path(override).expanduser()
    config_home = Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config"))
    return config_home / "intent"


def settings_path() -> Path:
    override = os.environ.get("INTENT_LLM_CONFIG", "").strip()
    if override:
        return Path(override)
    return config_dir() / "llm.env"


def gemini_credentials_path() -> Path:
    return config_dir() / _GEMINI_CREDENTIALS_NAME


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


def _credentials_configured(saved: dict[str, str]) -> bool:
    raw_path = saved.get("GOOGLE_APPLICATION_CREDENTIALS") or saved.get("GEMINI_CREDENTIALS_PATH") or ""
    if not raw_path:
        return False
    path = Path(raw_path).expanduser()
    if not path.is_file():
        return False
    try:
        validate_gemini_credentials(path.read_bytes())
    except ValueError:
        return False
    return True


def settings_summary() -> dict[str, Any]:
    """Return provider settings without returning any credential material."""

    saved = load_saved_settings()

    def effective(name: str, default: str = "") -> str:
        return saved.get(name, default).strip()

    provider = effective("LLM_PROVIDER", "gemini").lower()
    if provider not in PROVIDERS:
        provider = "gemini"
    secret_name = _SECRET_KEYS.get(provider)
    credentials_configured = _credentials_configured(saved)
    api_key_configured = bool(effective(secret_name)) if secret_name else False
    if provider == "gemini":
        api_key_configured = credentials_configured
    return {
        "provider": provider,
        "model": effective("INTENT_LLM_MODEL"),
        "copilot_enabled": copilot_enabled(),
        "semantic_cluster_enabled": semantic_clustering_enabled(),
        "semantic_content_consent": semantic_content_consent_granted(),
        "semantic_full_capture_consent": semantic_full_capture_consent_granted(),
        "semantic_timeout_ms": semantic_timeout_ms(),
        "api_key_configured": api_key_configured,
        "credentials_configured": credentials_configured,
        "groq_base_url": effective("GROQ_BASE_URL", _DEFAULT_GROQ_BASE_URL),
        "google_cloud_project": effective("GOOGLE_CLOUD_PROJECT"),
        "google_cloud_location": effective("GOOGLE_CLOUD_LOCATION", "us-central1"),
        "bedrock_region": effective("BEDROCK_REGION"),
        "bedrock_profile": effective("BEDROCK_AWS_PROFILE"),
    }


def validate_gemini_credentials(raw: bytes) -> dict[str, Any]:
    try:
        payload = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ValueError("Gemini credentials must be valid JSON") from exc
    if not isinstance(payload, dict):
        raise ValueError("Gemini credentials must be a JSON object")
    for key in ("type", "project_id", "private_key", "client_email"):
        value = payload.get(key)
        if not isinstance(value, str) or not value.strip():
            raise ValueError(f"Gemini credentials missing required field: {key}")
    if payload["type"] != "service_account":
        raise ValueError("Gemini credentials must be a service account JSON file")
    return payload


def save_gemini_credentials(raw: bytes) -> dict[str, Any]:
    payload = validate_gemini_credentials(raw)
    directory = config_dir()
    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    directory.chmod(0o700)
    destination = gemini_credentials_path()
    descriptor, temporary_name = tempfile.mkstemp(prefix=".gemini-", dir=directory, text=False)
    try:
        os.fchmod(descriptor, 0o600)
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(raw)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary_name, destination)
        destination.chmod(0o600)
    finally:
        try:
            os.unlink(temporary_name)
        except FileNotFoundError:
            pass

    values = load_saved_settings()
    credential_path = str(destination)
    values["GOOGLE_APPLICATION_CREDENTIALS"] = credential_path
    values["GEMINI_CREDENTIALS_PATH"] = credential_path
    project_id = str(payload["project_id"]).strip()
    if project_id and not values.get("GOOGLE_CLOUD_PROJECT"):
        values["GOOGLE_CLOUD_PROJECT"] = project_id
    _write_settings(values)
    apply_saved_settings(values)
    return settings_summary()


def clear_gemini_credentials() -> dict[str, Any]:
    gemini_credentials_path().unlink(missing_ok=True)
    values = load_saved_settings()
    values.pop("GOOGLE_APPLICATION_CREDENTIALS", None)
    values.pop("GEMINI_CREDENTIALS_PATH", None)
    _write_settings(values)
    apply_saved_settings(values)
    return settings_summary()


def save_settings(update: dict[str, Any]) -> dict[str, Any]:
    """Persist validated local settings and apply them to the running process."""

    provider = str(update["provider"]).strip().lower()
    if provider not in PROVIDERS:
        raise ValueError("Unsupported provider")
    if provider == "gemini" and update.get("api_key") is not None:
        raise ValueError("Gemini uses service-account JSON attached from the Intent app")

    values = load_saved_settings()
    values["LLM_PROVIDER"] = provider
    copilot_on = bool(update.get("enable_copilot", True))
    semantic_cluster = update.get("enable_semantic_cluster")
    semantic_content = update.get("enable_semantic_content_consent")
    llm_on = copilot_on or bool(semantic_cluster) or bool(semantic_content)
    values["ENGINE_LLM_ENABLED"] = "true" if llm_on else "false"
    values["ENABLE_COPILOT"] = "true" if copilot_on else "false"
    if update.get("enable_semantic_cluster") is not None:
        values["ENGINE_SEMANTIC_CLUSTER"] = "true" if update["enable_semantic_cluster"] else "false"
    if update.get("enable_semantic_content_consent") is not None:
        values["ENGINE_SEMANTIC_CONTENT_CONSENT"] = "true" if update["enable_semantic_content_consent"] else "false"
    if update.get("enable_semantic_full_capture_consent") is not None:
        values["ENGINE_SEMANTIC_FULL_CAPTURE_CONSENT"] = "true" if update["enable_semantic_full_capture_consent"] else "false"
    if update.get("semantic_timeout_ms") is not None:
        timeout = int(update["semantic_timeout_ms"])
        if timeout < 1_000 or timeout > 120_000:
            raise ValueError("semantic_timeout_ms must be between 1000 and 120000")
        values["ENGINE_SEMANTIC_TIMEOUT_MS"] = str(timeout)
    _set_optional(values, "INTENT_LLM_MODEL", update.get("model"))
    _set_optional(values, "GROQ_BASE_URL", update.get("groq_base_url"))
    _set_optional(values, "GOOGLE_CLOUD_PROJECT", update.get("google_cloud_project"))
    _set_optional(values, "GOOGLE_CLOUD_LOCATION", update.get("google_cloud_location"))
    _set_optional(values, "BEDROCK_REGION", update.get("bedrock_region"))
    _set_optional(values, "BEDROCK_AWS_PROFILE", update.get("bedrock_profile"))

    secret_name = _SECRET_KEYS.get(provider)
    if secret_name:
        if update.get("clear_api_key"):
            values.pop(secret_name, None)
        elif update.get("api_key") is not None:
            _set_optional(values, secret_name, update.get("api_key"))

    _write_settings(values)
    apply_saved_settings(values)
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


def apply_saved_settings(values: dict[str, str] | None = None) -> None:
    _apply_environment(values if values is not None else load_saved_settings())


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
