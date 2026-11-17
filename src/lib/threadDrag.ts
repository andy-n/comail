import { create } from "zustand";
import type { Label, SplitRule } from "../ipc/types";
import { queryClient } from "../queries/client";
import { moveThreadsToTab } from "../queries/tabMove";
import {
  elementWithAttr,
  pastThreshold,
  setDragCursor,
  swallowClickAfterRelease,
  swallowNextClick,
} from "./pointerDrag";
import { canDropOnTab, isViewTab, type TabRoute } from "./tabDrop";

const DROP_ATTR = "data-drop-tab";

/** Spread on a tab, bundle or sidebar row that takes dragged mail. */
export function dropTargetProps(route: TabRoute, name: string) {
  return { [DROP_ATTR]: route, "data-drop-name": name };
}

/** The drop target under the pointer; `ok` false = it refuses mail (a sender
 *  or subject rule's tab); `view` = a view tab, which only adds the mail. */
export type DropHover = { route: TabRoute; name: string; ok: boolean; view: boolean };

export type ThreadDrag = {
  ids: number[];
  /** Subject of a single dragged thread, for the drag chip. */
  subject: string;
  x: number;
  y: number;
  over: DropHover | null;
};

/** The mail drag in progress, if any (drives the chip and drop highlights). */
export const useThreadDrag = create<{ drag: ThreadDrag | null }>(() => ({ drag: null }));

/** True while dragged mail hovers this tab and would land there. */
export function useDropHighlight(route: TabRoute | null): boolean {
  return useThreadDrag((s) => route != null && s.drag?.over?.route === route && s.drag.over.ok);
}

/**
 * Pick up threads and follow the pointer until release; dropping on a
 * `dropTargetProps` element moves them to that tab. The thread list starts
 * this after a press-and-hold on a row (or an Option/Alt-drag), so a plain
 * drag over rows stays a range sweep. `from` holds the tab(s) the mail is in:
 * hovering that tab is a no-op. With `moved` false (a hold that hasn't moved
 * yet), releasing in place stays a plain click that opens the thread.
 */
export function pickUpThreads(opts: {
  ids: number[];
  subject: string;
  from: ReadonlySet<TabRoute | null>;
  x: number;
  y: number;
  moved: boolean;
}) {
  const { ids, subject, from } = opts;
  // The one tab every dragged thread came from, if there is one.
  const source = from.size === 1 ? [...from][0] : null;
  let moved = opts.moved;

  const hover = (x: number, y: number): DropHover | null => {
    const el = elementWithAttr(x, y, DROP_ATTR);
    const route = el?.getAttribute(DROP_ATTR) as TabRoute | null | undefined;
    if (!el || !route || (from.size === 1 && from.has(route))) return null;
    const splits = queryClient.getQueryData<SplitRule[]>(["splits"]);
    const ok = canDropOnTab(route, splits, queryClient.getQueryData<Label[]>(["labels"]));
    return { route, name: el.dataset.dropName ?? "", ok, view: isViewTab(route, splits) };
  };
  const show = (x: number, y: number) => {
    const over = moved ? hover(x, y) : null;
    setDragCursor(over && !over.ok ? "noDrop" : "grab");
    useThreadDrag.setState({ drag: { ids, subject, x, y, over } });
  };

  const finish = () => {
    window.removeEventListener("mousemove", onMove);
    window.removeEventListener("mouseup", onUp);
    window.removeEventListener("keydown", onKey, true);
    setDragCursor(null);
    useThreadDrag.setState({ drag: null });
  };
  const onMove = (ev: MouseEvent) => {
    if (!moved && !pastThreshold(opts.x, opts.y, ev.clientX, ev.clientY)) return;
    moved = true;
    show(ev.clientX, ev.clientY);
  };
  const onUp = (ev: MouseEvent) => {
    const over = moved ? hover(ev.clientX, ev.clientY) : null;
    finish();
    // A hold released in place is a slow click: let it open the thread.
    if (!moved) return;
    swallowNextClick();
    if (over?.ok) void moveThreadsToTab(ids, over.route, over.name, source);
  };
  const onKey = (ev: KeyboardEvent) => {
    if (ev.key !== "Escape") return;
    ev.preventDefault();
    ev.stopPropagation();
    finish();
    swallowClickAfterRelease();
  };

  window.addEventListener("mousemove", onMove);
  window.addEventListener("mouseup", onUp);
  window.addEventListener("keydown", onKey, true);
  show(opts.x, opts.y);
}
