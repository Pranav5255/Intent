// Captures the design-review screenshots from the mock-mode dev server.
//   VITE_MOCK=1 npx vite --port 1420 &   then   node scripts/screenshots.mjs
// Uses an already-installed Playwright (set PLAYWRIGHT_DIR to its node_modules
// folder if it isn't resolvable from here). It never downloads browsers.

import { createRequire } from 'node:module';
import { mkdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const out = resolve(here, '..', 'docs', 'screenshots');
mkdirSync(out, { recursive: true });

const require = createRequire(import.meta.url);
let playwright;
try {
  playwright = require(process.env.PLAYWRIGHT_DIR ? join(process.env.PLAYWRIGHT_DIR, 'playwright') : 'playwright');
} catch {
  console.error('Playwright is not installed. Set PLAYWRIGHT_DIR to an existing node_modules that contains it.');
  process.exit(1);
}

const BASE = process.env.BASE_URL ?? 'http://127.0.0.1:1420/';
const only = process.argv.slice(2);

const browser = await playwright.chromium.launch();

async function shot(name, { url, width = 1280, height = 800, scheme = 'dark', run }) {
  if (only.length && !only.includes(name)) return;
  const context = await browser.newContext({
    viewport: { width, height },
    deviceScaleFactor: 2,
    colorScheme: scheme,
    reducedMotion: 'reduce',
  });
  const page = await context.newPage();
  page.on('pageerror', (e) => console.error(`[${name}] page error:`, e.message));
  page.on('console', (m) => m.type() === 'error' && console.error(`[${name}] console:`, m.text()));
  await page.goto(new URL(url, BASE).toString());
  await page.waitForLoadState('networkidle');
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(250);
  if (run) await run(page);
  await page.waitForTimeout(200);
  const file = join(out, `${name}.png`);
  await page.screenshot({ path: file });
  console.log(file);
  await context.close();
}

const OV = { width: 1100, height: 720 };

await shot('overlay-results', {
  url: '?#/overlay',
  ...OV,
  run: async (p) => {
    await p.keyboard.press('ArrowDown');
  },
});

await shot('overlay-search', {
  url: '?#/overlay',
  ...OV,
  run: async (p) => {
    await p.keyboard.type('checkout');
    await p.waitForTimeout(400);
  },
});

await shot('overlay-copilot', {
  url: '?#/overlay',
  ...OV,
  run: async (p) => {
    await p.keyboard.type('? where did I leave the retention purge');
    await p.keyboard.press('Enter');
    await p.waitForTimeout(1300);
  },
});

await shot('overlay-actions', {
  url: '?#/overlay',
  ...OV,
  run: async (p) => {
    await p.keyboard.press('ArrowDown');
    await p.keyboard.press('Control+k');
    await p.keyboard.press('Meta+k');
    await p.waitForTimeout(200);
  },
});

await shot('overlay-commands', {
  url: '?#/overlay',
  ...OV,
  run: async (p) => {
    await p.keyboard.type('>pause');
  },
});

await shot('overlay-no-results', {
  url: '?#/overlay',
  ...OV,
  run: async (p) => {
    await p.keyboard.type('kubernetes ingress tls');
    await p.waitForTimeout(400);
  },
});

await shot('overlay-paused', {
  url: '?paused=1#/overlay',
  ...OV,
});

await shot('main-today', { url: '?#/main/today' });

await shot('main-timeline', {
  url: '?#/main/timeline',
  run: async (p) => {
    await p.locator('.tl-bar').nth(3).hover();
  },
});

await shot('main-timeline-14d', {
  url: '?#/main/timeline',
  run: async (p) => {
    await p.getByRole('button', { name: '14 days' }).click();
  },
});

await shot('restore-review', {
  url: '?#/main/today',
  run: async (p) => {
    await p.getByRole('button', { name: 'Review', exact: true }).nth(1).click();
    await p.waitForTimeout(150);
    // Show a partial restore: untick one tab.
    await p.locator('.review .rc-row').nth(4).locator('.check').click();
  },
});

await shot('main-intent-detail', {
  url: '?#/main/intent:int_7f3a04',
  height: 900,
});

await shot('main-search', {
  url: '?#/main/search',
  run: async (p) => {
    await p.keyboard.type('payments');
    await p.waitForTimeout(500);
  },
});

await shot('settings-intelligence', {
  url: '?#/main/settings:intelligence',
  height: 900,
});

await shot('settings-intelligence-cloud', {
  url: '?#/main/settings:intelligence',
  height: 900,
  run: async (p) => {
    await p.getByLabel('Provider').selectOption('openai');
    await p.waitForTimeout(100);
    await p.locator('#settings-intelligence').evaluate((el) => el.scrollIntoView());
    await p.locator('.set-scroll').evaluate((el) => el.scrollBy(0, 120));
  },
});

await shot('settings-capture', { url: '?#/main/settings:capture', height: 900 });

await shot('settings-privacy-confirm', {
  url: '?#/main/settings:privacy',
  height: 900,
  run: async (p) => {
    await p.getByRole('button', { name: 'Review…' }).click();
    await p.waitForTimeout(200);
  },
});

await shot('settings-search', {
  url: '?#/main/settings',
  run: async (p) => {
    await p.getByLabel('Search settings').fill('pause');
  },
});

await shot('shortcuts', {
  url: '?#/main/settings:shortcuts',
  run: async (p) => {
    await p.getByRole('button', { name: 'Show all' }).click();
  },
});

await shot('main-paused-banner', { url: '?paused=1#/main/today' });

await shot('onboarding', { url: '?firstrun=1#/main/today' });

await shot('light-overlay', {
  url: '?#/overlay',
  ...OV,
  scheme: 'light',
  run: async (p) => {
    await p.keyboard.press('ArrowDown');
  },
});

await shot('light-today', { url: '?#/main/today', scheme: 'light' });

await shot('light-settings', { url: '?#/main/settings:intelligence', scheme: 'light', height: 900 });

await browser.close();
