import '@fontsource-variable/inter/opsz.css';
import '@fontsource/jetbrains-mono/400.css';
import '@fontsource/jetbrains-mono/500.css';
import './styles/tokens.css';
import './styles/base.css';
import './styles/components.css';
import './styles/overlay.css';
import './styles/main.css';
import './styles/settings.css';

import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { getAccent } from './lib/bridge';
import { IN_TAURI, windowKind } from './lib/env';
import { applyTheme, storedTheme } from './lib/theme';
import { MainWindow } from './main/MainWindow';
import { Overlay } from './overlay/Overlay';

const kind = windowKind();
document.body.classList.add(kind === 'overlay' ? 'is-overlay' : 'is-main');
if (!IN_TAURI) document.body.classList.add('is-browser');

applyTheme(storedTheme());

// GNOME accent colour from the XDG portal, when the Rust side can read it.
void getAccent().then((hex) => {
  if (hex) document.documentElement.style.setProperty('--accent-system', hex);
});

createRoot(document.getElementById('root')!).render(
  <StrictMode>{kind === 'overlay' ? <Overlay /> : <MainWindow />}</StrictMode>,
);
