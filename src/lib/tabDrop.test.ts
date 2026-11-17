import { describe, expect, it } from "vitest";
import type { Label, SplitRule } from "../ipc/types";
import { canDropOnTab, isViewTab, labelTab, tabRoute, threadAfterMove, type TabRoute } from "./tabDrop";

const split = (id: number, extra: Partial<SplitRule>): SplitRule => ({
  id,
  name: `split ${id}`,
  position: id,
  query: {},
  ...extra,
});

const splits: SplitRule[] = [
  split(1, { condition: { kind: "label", id: 10 } }),
  split(2, { condition: { kind: "any", of: [{ kind: "label", id: 20 }, { kind: "label", id: 21 }] } }),
  split(3, { query: { senders: ["@github.com"] } }),
  split(4, { condition: { kind: "label", id: 40 }, target: "label:101" }),
  split(5, { condition: { kind: "label", id: 50 }, ai: { instruction: "a receipt?" } }),
  split(6, { query: { labels: [60] } }),
  // Learned senders keep a label tab a label tab.
  split(7, {
    condition: {
      kind: "any",
      of: [
        { kind: "label", id: 70 },
        { kind: "sender", op: "equals", value: "ann@shop.example" },
      ],
    },
  }),
  split(8, { condition: { kind: "isStarred", value: true }, exclusive: false }),
  split(9, { condition: { kind: "label", id: 90 }, exclusive: false }),
];

const labels: Label[] = [
  { id: 10, name: "Finance", color: "#000", keyword: "Finance", position: 0 },
  { id: 101, name: "Marketing", color: "#000", keyword: "ComailAutoMarketing", position: 9, isAuto: true },
  { id: 102, name: "News", color: "#000", keyword: "ComailAutoNews", position: 10, isAuto: true },
];

describe("tabRoute", () => {
  it("names tabs by route key", () => {
    expect(tabRoute({ splitId: -1 })).toBe("important");
    expect(tabRoute({ splitId: -2 })).toBe("other");
    expect(tabRoute({ splitId: 7 })).toBe("split:7");
    expect(tabRoute({ labelId: 101 })).toBe("label:101");
    expect(tabRoute({ splitId: null })).toBeNull();
  });
});

describe("drop targets", () => {
  it("label tabs are the splits that take mail", () => {
    expect(labelTab("split:1", splits)?.anchor).toEqual({ kind: "label", labelId: 10 });
    expect(labelTab("split:2", splits)).toBeNull(); // two labels: no one label's tab
    expect(labelTab("split:3", splits)).toBeNull();
    expect(labelTab("split:4", splits)).toBeNull(); // routes into another tab
    expect(labelTab("split:5", splits)).toBeNull(); // AI check decides
    expect(labelTab("split:6", splits)?.anchor).toEqual({ kind: "label", labelId: 60 }); // legacy query
    expect(labelTab("split:7", splits)?.anchor).toEqual({ kind: "label", labelId: 70 });
    expect(labelTab("split:8", splits)?.anchor).toEqual({ kind: "starred" });
    expect(labelTab("important", splits)).toBeNull();
    expect(isViewTab("split:8", splits)).toBe(true);
    expect(isViewTab("split:1", splits)).toBe(false);
  });

  it("accepts Important, Other, categories and label tabs", () => {
    const ok = (r: TabRoute) => canDropOnTab(r, splits, labels);
    expect(ok("important")).toBe(true);
    expect(ok("other")).toBe(true);
    expect(ok("label:101")).toBe(true);
    expect(ok("label:10")).toBe(false); // a user label is no tab
    expect(ok("split:1")).toBe(true);
    expect(ok("split:8")).toBe(true);
    expect(ok("split:2")).toBe(false);
    expect(ok("split:3")).toBe(false);
    expect(ok("split:99")).toBe(false);
  });
});

describe("threadAfterMove", () => {
  const after = (labelIds: number[], to: TabRoute, from: TabRoute | null = null, isStarred = false) =>
    threadAfterMove({ labels: labelIds, isStarred }, to, from, splits, labels);

  it("swaps exclusive tab labels and keeps the rest", () => {
    expect(after([10, 7], "split:7")).toEqual({ labels: [7, 70], isStarred: false });
    expect(after([70, 10], "split:7")).toEqual({ labels: [70], isStarred: false });
  });

  it("follows the category route", () => {
    expect(after([10, 101, 7], "label:102").labels).toEqual([7, 102]);
    expect(after([101, 7], "important").labels).toEqual([7]);
  });

  it("only adds for a view tab, and leaving one takes its label or star", () => {
    expect(after([10, 7], "split:9", "important")).toEqual({ labels: [10, 7, 90], isStarred: false });
    expect(after([10], "split:8", "important")).toEqual({ labels: [10], isStarred: true });
    expect(after([90, 7], "important", "split:9")).toEqual({ labels: [7], isStarred: false });
    expect(after([7], "important", "split:8", true)).toEqual({ labels: [7], isStarred: false });
    // Starred mail dragged from Important stays starred.
    expect(after([], "split:1", "important", true)).toEqual({ labels: [10], isStarred: true });
  });
});
