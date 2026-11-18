import type React from "react";

/**
 * Moves focus to the first enabled `[role="menuitem"]` inside a popup menu container.
 */
export function focusFirstMenuItem(menuEl: HTMLElement | null): void {
  if (!menuEl) return;
  const firstItem = menuEl.querySelector<HTMLElement>('[role="menuitem"]:not(:disabled)');
  firstItem?.focus();
}

/**
 * Handles WAI-ARIA Menu keyboard navigation (`ArrowDown`, `ArrowUp`, `Home`, `End`, `Escape`, `Tab`).
 */
export function handlePopupMenuKeyDown(
  e: React.KeyboardEvent<HTMLElement>,
  onClose: () => void,
  triggerEl?: HTMLElement | null,
): void {
  const items = Array.from(
    e.currentTarget.querySelectorAll<HTMLElement>('[role="menuitem"]:not(:disabled)'),
  );
  if (items.length === 0) {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onClose();
      triggerEl?.focus();
    }
    return;
  }

  const currentIndex = items.indexOf(document.activeElement as HTMLElement);
  switch (e.key) {
    case "ArrowDown": {
      e.preventDefault();
      const nextIdx = currentIndex < 0 ? 0 : (currentIndex + 1) % items.length;
      items[nextIdx]?.focus();
      break;
    }
    case "ArrowUp": {
      e.preventDefault();
      const prevIdx =
        currentIndex < 0
          ? items.length - 1
          : (currentIndex - 1 + items.length) % items.length;
      items[prevIdx]?.focus();
      break;
    }
    case "Home":
      e.preventDefault();
      items[0]?.focus();
      break;
    case "End":
      e.preventDefault();
      items[items.length - 1]?.focus();
      break;
    case "Escape":
      e.preventDefault();
      e.stopPropagation();
      onClose();
      triggerEl?.focus();
      break;
    case "Tab":
      onClose();
      break;
  }
}
