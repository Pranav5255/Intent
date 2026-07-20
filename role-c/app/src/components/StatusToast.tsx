export interface StatusMessage {
  text: string;
  variant: 'info' | 'error' | 'success';
}

export function StatusToast({ status }: { status: StatusMessage | null }) {
  if (!status) return null;
  return <div className={`status-toast ${status.variant}`} role="status" aria-live="polite">{status.text}</div>;
}
