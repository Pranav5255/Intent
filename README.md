# Intent

Intent is a local-first Ubuntu desktop companion that helps you recall,
review, and safely resume recent work. It collects consented activity metadata,
derives a timeline of work sessions, and presents the result in a compact
Electron overlay.

The core experience is deliberately review-first: inspect the saved context
before restoring it. Continue opens only the stored Firefox tabs for a
session; Resume uses the reviewed local restore payload. Shell commands are
shown as context and are never run automatically.

## What it does

- Captures consented desktop, browser, editor, shell, and filesystem metadata.
- Builds deterministic intent trees from local activity exports.
- Provides a searchable session timeline and bounded restore payloads.
- Opens a first-run guide explaining the local workflow.
- Optionally enables a grounded Copilot using OpenAI, Groq, Gemini, or Amazon
  Bedrock.

The deterministic timeline, search, review, and restore flow work without a
cloud account or API key.

## Architecture

| Layer | Responsibility |
| --- | --- |
| Role A | Local collection, redaction, event storage, and the reviewed restore API on port 9477. |
| Role B | Local normalization, intent clustering, storage, search, and optional Copilot API on port 9478. |
| Role C | Electron/React desktop UI: onboarding, search, session review, restore controls, and production settings. |

Role B does not read Role A’s database directly, and Role C does not invent
restore payloads. The layers communicate through their loopback APIs.

## Privacy and safety

- Captured data and derived timelines stay on the local machine by default.
- Detailed editor and browser content capture is opt-in.
- Common credential-like data, secret files, and sensitive URLs are redacted or
  excluded before storage.
- Cloud providers are optional. Provider requests use the project’s defined
  safe and consented boundaries; they are not required for the local workflow.
- Production API keys are saved only in the per-user Intent configuration
  file with owner-only permissions. They are never returned to the UI after
  saving. Do not commit environment files or credentials.

See [Role A privacy notes](role-a/docs/PRIVACY.md) and the
[Role B pipeline guide](role-b/docs/PIPELINE.md) for the detailed boundaries.

## Install and run on Ubuntu

Intent targets Ubuntu with a GNOME X11 session. The developer package bundles
the Electron runtime and Role B Python environment.

    cd role-a
    make package-dev
    sudo apt install ./dist/intent-os_0.1.8_amd64.deb
    intent-os

The first launch enables the per-user local services, waits for the loopback
APIs, and opens the onboarding flow. Press Ctrl+Space to open or close the
overlay thereafter.

For release-package and companion-extension steps, see
[role-a/docs/INSTALL.md](role-a/docs/INSTALL.md).

## Using the overlay

1. Complete the first-run welcome sequence.
2. Open the overlay with Ctrl+Space.
3. Search saved work, or type a question mark followed by a question for
   optional Copilot.
4. Select **Review** to inspect a session’s local restore context in the side
   panel.
5. Choose **Resume** to reopen the reviewed local context, or **Continue** to
   open only the session’s saved Firefox tabs.
6. In the production app, use the settings control to select an optional LLM
   provider and configure its local credentials.

## Repository layout

    role-a/  Local collectors, event server, restore API, integrations, packaging
    role-b/  Intent engine, local API, deterministic pipeline, optional Copilot
    role-c/  Electron + React desktop overlay

Useful entry points:

- [Role A README](role-a/README.md)
- [Role B operator guide](role-b/README.md)
- [Role C frontend design](role-c/design.md)
- [Role C implementation handoff](ROLE-C-HANDOFF.md)

## Development and video workflow

Intent was built with Codex workflows using GPT-5.6 to help implement and
review the local services, Electron experience, packaging, tests, and recording
fixtures. Changes remained reviewable as normal source edits and test results;
Codex is not part of the installed runtime or capture pipeline.

The product-demo video was prepared with the [HyperFrames](https://hyperframes.video/)
plugin for Codex. HyperFrames turns HTML/CSS/JavaScript compositions into video,
which made it useful for editing the screen recording and adding deterministic
title cards, motion effects, highlights, captions, transitions, previews, and
final MP4 renders. It is a development and media-production tool only and does
not access Intent activity data.

## Development checks

    # Role C renderer and Electron checks
    cd role-c/app && npm test && npm run build

    # Role A checks
    cd ../../role-a && make test

    # Focused Role B settings checks
    cd ../role-b && ./.venv/bin/python -m pytest tests/test_llm_settings.py -q

Use the role-specific documentation for setup details, replay fixtures, API
contracts, optional integrations, and release packaging.
