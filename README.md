# Intent

Local-first Ubuntu desktop companion for recalling and resuming recent work.

## Structure

| Directory | Purpose |
|-----------|---------|
| `capture/` | Event collection, companions, packaging, `intentctl` |
| `engine/` | Intent timeline, search, optional Copilot API |
| `desktop/app/` | Electron overlay UI |

## Build and install

```bash
# Build companions
cd capture/integrations/firefox-extension && npm install && npm run build
cd ../vscode-extension && npm install
npx --yes @vscode/vsce package --out dist/intent-vscode.vsix

# Sign Firefox XPI (AMO credentials in capture/.env), then package
cd ../../..
bash scripts/sign_firefox.sh
INTENT_FIREFOX_XPI="$(pwd)/integrations/firefox-extension/dist/intent-firefox-signed.xpi" make -C capture package

# Install and run
sudo apt install ./capture/dist/intent_1.0.1_amd64.deb
intent
```

## Uninstall

```bash
intentctl uninstall
```
