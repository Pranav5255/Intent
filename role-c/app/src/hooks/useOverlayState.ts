import { useCallback, useEffect, useState } from 'react';

export function useOverlayState(surfaceVisible: boolean) {
  const [open, setOpen] = useState(false);

  const close = useCallback(() => setOpen(false), []);
  const toggle = useCallback(() => setOpen((visible) => !visible), []);

  useEffect(() => {
    const removeListener = window.intentOS?.onToggleOverlay(toggle);
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.ctrlKey && event.code === 'Space') {
        event.preventDefault();
        toggle();
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => {
      removeListener?.();
      window.removeEventListener('keydown', onKeyDown);
    };
  }, [toggle]);

  useEffect(() => {
    const visible = open || surfaceVisible;
    window.intentOS?.setInteractionActive(visible);
    window.intentOS?.setOverlayVisible(visible);
  }, [open, surfaceVisible]);

  return { open, setOpen, close, toggle };
}
