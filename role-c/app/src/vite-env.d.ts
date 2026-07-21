/// <reference types="vite/client" />

interface IntentOSBridgeResponse {
  ok: boolean;
  status: number;
  body: string;
}

interface Window {
  intentOS?: {
    request: (url: string, init?: RequestInit) => Promise<IntentOSBridgeResponse>;
    setInteractionActive: (active: boolean) => void;
    setOverlayVisible: (visible: boolean) => void;
    onToggleOverlay: (listener: () => void) => () => void;
  };
}
