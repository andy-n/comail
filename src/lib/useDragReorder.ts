import { useCallback, useEffect, useRef, useState, type MouseEvent as ReactMouseEvent } from "react";
import { pastThreshold, setDragCursor, swallowNextClick } from "./pointerDrag";

export type ReorderDrag = { from: string; over: string | null };

/** Everything one reorderable item needs: hit-test attributes, the press
 *  handler, and its drag-time look. */
export type ReorderWiring = {
  attrs: Record<string, string>;
  onMouseDown: (e: ReactMouseEvent) => void;
  /** this item is the one being dragged */
  dragging: boolean;
  /** draw the drop line on this side of the item */
  dropSide: "before" | "after" | null;
};

/**
 * Press-and-drag reordering of keyed items in one list (Settings → Split
 * inbox, the tab bar, the sidebar's tab rows). Spread `handle(key)` on what
 * the user grabs and `target(key)` on what they can drop onto (often the same
 * element). A press that never travels past the threshold stays a plain click,
 * so click-to-select and keyboard handling are untouched; a real drag
 * swallows the trailing click. Escape cancels.
 *
 * `group` scopes hit-testing so two lists on screen can't drop into each other.
 */
export function useDragReorder(group: string, onDrop: (fromKey: string, toKey: string) => void) {
  const [drag, setDrag] = useState<ReorderDrag | null>(null);
  const onDropRef = useRef(onDrop);
  onDropRef.current = onDrop;
  const stopRef = useRef<(() => void) | null>(null);
  useEffect(() => () => stopRef.current?.(), []);

  const begin = useCallback(
    (key: string, e: ReactMouseEvent) => {
      if (e.button !== 0 || e.shiftKey || e.metaKey || e.ctrlKey || e.altKey) return;
      stopRef.current?.();
      const x0 = e.clientX;
      const y0 = e.clientY;
      let dragging = false;
      let over: string | null = null;

      const hit = (x: number, y: number): string | null => {
        const el = document
          .elementFromPoint(x, y)
          ?.closest<HTMLElement>(`[data-reorder-group="${group}"]`);
        return el?.dataset.reorderKey ?? null;
      };
      const stop = () => {
        window.removeEventListener("mousemove", onMove);
        window.removeEventListener("mouseup", onUp);
        window.removeEventListener("keydown", onKey, true);
        stopRef.current = null;
        if (dragging) setDragCursor(null);
        setDrag(null);
      };
      const onMove = (ev: MouseEvent) => {
        if (!dragging) {
          if (!pastThreshold(x0, y0, ev.clientX, ev.clientY)) return;
          dragging = true;
          setDragCursor("grab");
        }
        over = hit(ev.clientX, ev.clientY);
        setDrag({ from: key, over });
      };
      const onUp = () => {
        const moved = dragging;
        stop();
        if (!moved) return;
        swallowNextClick();
        if (over != null && over !== key) onDropRef.current(key, over);
      };
      const onKey = (ev: KeyboardEvent) => {
        if (ev.key !== "Escape" || !dragging) return;
        ev.preventDefault();
        ev.stopPropagation();
        stop();
        // The release still fires a click on the pressed element; eat it.
        const eat = () => {
          swallowNextClick();
          window.removeEventListener("mouseup", eat, true);
        };
        window.addEventListener("mouseup", eat, true);
      };
      window.addEventListener("mousemove", onMove);
      window.addEventListener("mouseup", onUp);
      window.addEventListener("keydown", onKey, true);
      stopRef.current = stop;
    },
    [group],
  );

  const handle = useCallback(
    (key: string) => ({ onMouseDown: (e: ReactMouseEvent) => begin(key, e) }),
    [begin],
  );
  const target = useCallback(
    (key: string) => ({ "data-reorder-group": group, "data-reorder-key": key }),
    [group],
  );

  /** Where the dragged item would land relative to `key`, for the drop line:
   *  the dragged item takes the target's slot, so it lands after a target
   *  further down the list and before one further up. */
  const dropSide = (key: string, order: readonly string[]): "before" | "after" | null => {
    if (!drag || drag.over !== key || drag.from === key) return null;
    const from = order.indexOf(drag.from);
    const to = order.indexOf(key);
    if (from < 0 || to < 0) return null;
    return from < to ? "after" : "before";
  };

  /** `target` + `handle` on one element, plus its drag-time state. */
  const wire = (key: string, order: readonly string[]): ReorderWiring => ({
    attrs: target(key),
    onMouseDown: handle(key).onMouseDown,
    dragging: drag?.from === key,
    dropSide: dropSide(key, order),
  });

  return { drag, handle, target, wire };
}
