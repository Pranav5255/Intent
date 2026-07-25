/// <reference types="vite/client" />

interface IntentBridgeResponse {
  ok: boolean;
  status: number;
  body: string;
}

interface Window {
  intent?: {
    request: (url: string, init?: RequestInit) => Promise<IntentBridgeResponse>;
    setInteractionActive: (active: boolean) => void;
    setOverlayVisible: (visible: boolean) => void;
    onToggleOverlay: (listener: () => void) => () => void;
    pickGeminiCredentials?: () => Promise<{ ok: boolean; error?: string }>;
  };
}
