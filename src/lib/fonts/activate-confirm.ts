/**
 * In-app Activate All & Deactivate All option dialogs.
 * Activate: OK = all remainder / Cancel = keep prefer if wave0 queued, else visible/first-page / Abort = stop.
 * Deactivate: all | google (Google Fonts only) | keep-fast (keep active + enable Fast Resume) | cancel.
 */
export type ActivateConfirmChoice = "ok" | "cancel" | "abort";

export type ActivateConfirmRequest = {
  label: string;
  total: number;
  preferCount: number;
  remainderCount: number;
  visibleCount: number;
  cancelCount: number;
  /** Softened ETA copy — not a hard minute promise. */
  etaHint: string;
  resolve: (choice: ActivateConfirmChoice) => void;
};

export type DeactivateConfirmChoice = "all" | "google" | "keep-fast" | "cancel";

export type DeactivateConfirmRequest = {
  label: string;
  total: number;
  googleCount: number;
  localCount: number;
  resolve: (choice: DeactivateConfirmChoice) => void;
};

function createConfirmChannel<TReq extends { resolve: (choice: TChoice) => void }, TChoice>(
  replaceChoice: TChoice,
) {
  let current: TReq | null = null;
  const listeners = new Set<(req: TReq | null) => void>();
  const emit = () => {
    for (const l of listeners) l(current);
  };
  return {
    subscribe(listener: (req: TReq | null) => void): () => void {
      listeners.add(listener);
      listener(current);
      return () => {
        listeners.delete(listener);
      };
    },
    request(build: (resolve: (choice: TChoice) => void) => TReq): Promise<TChoice> {
      return new Promise((resolve) => {
        if (current) current.resolve(replaceChoice);
        current = build(resolve);
        emit();
      });
    },
    resolve(choice: TChoice) {
      const req = current;
      current = null;
      emit();
      req?.resolve(choice);
    },
  };
}

const activateChannel = createConfirmChannel<ActivateConfirmRequest, ActivateConfirmChoice>("abort");
const deactivateChannel = createConfirmChannel<DeactivateConfirmRequest, DeactivateConfirmChoice>("cancel");

export const subscribeActivateConfirm = activateChannel.subscribe;
export const resolveActivateConfirm = activateChannel.resolve;

export function requestActivateConfirm(input: {
  label: string;
  total: number;
  preferCount: number;
  remainderCount: number;
  visibleCount: number;
  cancelCount: number;
  etaHint: string;
}): Promise<ActivateConfirmChoice> {
  return activateChannel.request((resolve) => ({ ...input, resolve }));
}

export const subscribeDeactivateConfirm = deactivateChannel.subscribe;
export const resolveDeactivateConfirm = deactivateChannel.resolve;

export function requestDeactivateConfirm(input: {
  label: string;
  total: number;
  googleCount: number;
  localCount: number;
}): Promise<DeactivateConfirmChoice> {
  return deactivateChannel.request((resolve) => ({ ...input, resolve }));
}
