.PHONY: test test-python test-firefox test-vscode package-dev package-check

PACKAGE_VERSION := 0.1.8
PACKAGE_ARCH := $(shell dpkg --print-architecture)

test: test-python test-firefox test-vscode

test-python:
	PYTHONPATH=shell .venv/bin/python -m unittest discover -s event_server/tests -v
	.venv/bin/python -m unittest discover -s collectors/tests -v
	PYTHONPATH=shell .venv/bin/python -m unittest discover -s shell/tests -v
	.venv/bin/python -m unittest discover -s tools/tests -v

test-firefox:
	cd integrations/firefox-extension && npm test

test-vscode:
	cd integrations/vscode-extension && node --test tests/*.test.js

package-dev:
	bash packaging/build-deb.sh --dev

package-check:
	bash packaging/tests/check_deb.sh dist/intent-os_$(PACKAGE_VERSION)_$(PACKAGE_ARCH).deb
