// Shared bits of the app's pointer-driven drags (tab reorder, dragging mail
// onto tabs, range sweep). They use plain mouse events instead of HTML5
// drag-and-drop: no OS drag image, no webview drop handler to fight, and the
// press can still turn into a click when it never moves.

/** Pixels the pointer must travel before a press becomes a drag. */
export const DRAG_THRESHOLD = 5;

export function pastThreshold(x0: number, y0: number, x: number, y: number): boolean {
  return Math.hypot(x - x0, y - y0) >= DRAG_THRESHOLD;
}

/** After a real drag, swallow the click the release would fire so it doesn't
 *  also open or select whatever the drag started on. Self-removes if no click
 *  follows. */
export function swallowNextClick() {
  const swallow = (ev: MouseEvent) => {
    ev.stopPropagation();
    ev.preventDefault();
    window.removeEventListener("click", swallow, true);
  };
  window.addEventListener("click", swallow, true);
  setTimeout(() => window.removeEventListener("click", swallow, true), 0);
}

/** Escape ended a drag while the button is still down: eat the click its
 *  eventual release fires on the pressed element. */
export function swallowClickAfterRelease() {
  const eat = () => {
    swallowNextClick();
    window.removeEventListener("mouseup", eat, true);
  };
  window.addEventListener("mouseup", eat, true);
}

/** Window-wide drag cursor (grabbing, or no-drop over a target that refuses
 *  the drop) and no text selection; `null` ends the drag. */
export function setDragCursor(mode: "grab" | "noDrop" | null) {
  const root = document.documentElement.classList;
  root.toggle("co-dragging", mode != null);
  root.toggle("co-no-drop", mode === "noDrop");
}

/** Innermost element under the pointer carrying `attr`, or null. */
export function elementWithAttr(x: number, y: number, attr: string): HTMLElement | null {
  return document.elementFromPoint(x, y)?.closest<HTMLElement>(`[${attr}]`) ?? null;
}
