export interface FocusTargetLike {
  focus: () => void;
}

export interface ModalTrapAdapter<T extends FocusTargetLike> {
  getFocusables: () => readonly T[];
  getActiveElement: () => T | null;
  containsTarget: (target: T | null) => boolean;
  isPreviouslyFocusedConnected: (prev: T) => boolean;
  focusDialogContainer: () => void;
}

export function computeNextModalFocusTarget<T>(
  focusables: readonly T[],
  active: T | null,
  shiftKey: boolean,
): T | null {
  if (focusables.length === 0) {
    return null;
  }
  const index = active !== null ? focusables.indexOf(active) : -1;
  if (index === -1) {
    return shiftKey ? focusables[focusables.length - 1] : focusables[0];
  }
  if (shiftKey) {
    return focusables[(index - 1 + focusables.length) % focusables.length];
  }
  return focusables[(index + 1) % focusables.length];
}

export interface ModalKeyEventLike {
  key: string;
  shiftKey: boolean;
  preventDefault: () => void;
}

export function createModalFocusSession<T extends FocusTargetLike>(
  adapter: ModalTrapAdapter<T>,
  previouslyFocused: T | null,
  onCloseRequest: () => void,
  allowEscape = true,
): {
  focusInitial: () => void;
  handleKeyDown: (event: ModalKeyEventLike) => void;
  handleFocusIn: (target: T | null) => void;
  restoreFocus: () => void;
} {
  let closed = false;

  function focusInitial(): void {
    if (closed) return;
    const focusables = adapter.getFocusables();
    if (focusables.length > 0) {
      focusables[0].focus();
    } else {
      adapter.focusDialogContainer();
    }
  }

  function handleKeyDown(event: ModalKeyEventLike): void {
    if (closed) return;
    if (event.key === "Escape") {
      if (allowEscape) {
        event.preventDefault();
        onCloseRequest();
      }
      return;
    }

    if (event.key !== "Tab") {
      return;
    }

    event.preventDefault();
    const focusables = adapter.getFocusables();
    const next = computeNextModalFocusTarget(
      focusables,
      adapter.getActiveElement(),
      event.shiftKey,
    );
    if (next) {
      next.focus();
    } else {
      adapter.focusDialogContainer();
    }
  }

  function handleFocusIn(target: T | null): void {
    if (closed) return;
    if (!adapter.containsTarget(target)) {
      focusInitial();
    }
  }

  function restoreFocus(): void {
    closed = true;
    if (previouslyFocused && adapter.isPreviouslyFocusedConnected(previouslyFocused)) {
      previouslyFocused.focus();
    }
  }

  return {
    focusInitial,
    handleKeyDown,
    handleFocusIn,
    restoreFocus,
  };
}
