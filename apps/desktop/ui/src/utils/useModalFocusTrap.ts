import { useEffect, useRef, type RefObject } from "react";

const FOCUSABLE_SELECTOR = [
  'a[href]:not([tabindex="-1"])',
  'button:not(:disabled):not([tabindex="-1"])',
  'input:not(:disabled):not([type="hidden"]):not([tabindex="-1"])',
  'textarea:not(:disabled):not([tabindex="-1"])',
  'select:not(:disabled):not([tabindex="-1"])',
  '[tabindex]:not([tabindex="-1"])',
].join(", ");

/**
 * Traps Tab / Shift+Tab focus within an active modal dialog, focuses an initial element,
 * and restores focus to the previously focused trigger when the modal closes (`H-9`).
 */
export function useModalFocusTrap(
  isOpen: boolean,
  containerRef: RefObject<HTMLElement | null>,
  initialFocusRef?: RefObject<HTMLElement | null>,
): void {
  const restoreRafRef = useRef<number | null>(null);

  useEffect(() => {
    if (!isOpen || typeof document === "undefined") {
      return undefined;
    }

    if (restoreRafRef.current !== null) {
      window.cancelAnimationFrame(restoreRafRef.current);
      restoreRafRef.current = null;
    }

    const previouslyFocused =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;

    const focusInitial = () => {
      const container = containerRef.current;
      if (!container) return;
      if (initialFocusRef?.current && container.contains(initialFocusRef.current)) {
        initialFocusRef.current.focus();
        return;
      }
      if (container.contains(document.activeElement) && document.activeElement !== document.body) {
        return;
      }
      const focusable = Array.from(
        container.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR),
      );
      if (focusable[0]) {
        focusable[0].focus();
      } else {
        container.tabIndex = -1;
        container.focus();
      }
    };

    const raf = window.requestAnimationFrame(focusInitial);

    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== "Tab") return;
      const container = containerRef.current;
      if (!container) return;
      const focusable = Array.from(
        container.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR),
      );
      if (focusable.length === 0) {
        e.preventDefault();
        return;
      }
      const first = focusable[0]!;
      const last = focusable[focusable.length - 1]!;
      const active = document.activeElement as HTMLElement | null;

      if (e.shiftKey) {
        if (!active || active === first || !container.contains(active)) {
          e.preventDefault();
          last.focus();
        }
      } else if (!active || active === last || !container.contains(active)) {
        e.preventDefault();
        first.focus();
      }
    };

    document.addEventListener("keydown", onKeyDown, true);
    return () => {
      window.cancelAnimationFrame(raf);
      document.removeEventListener("keydown", onKeyDown, true);
      if (previouslyFocused && document.contains(previouslyFocused)) {
        restoreRafRef.current = window.requestAnimationFrame(() => {
          restoreRafRef.current = null;
          if (document.contains(previouslyFocused)) {
            try {
              previouslyFocused.focus();
            } catch {
              // Ignore focus restore errors on unmounted triggers
            }
          }
        });
      }
    };
  }, [isOpen, containerRef, initialFocusRef]);
}
