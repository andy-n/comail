import { describe, expect, it } from "vitest";
import type { Label, SplitRule } from "../ipc/types";
import { canDropOnTab, labelsAfterMove, splitDropLabels, tabRoute, type TabRoute } from "./tabDrop";

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
  it("label-only own-tab splits take mail; rule tabs don't", () => {
    expect(splitDropLabels(splits[0])).toEqual([10]);
    expect(splitDropLabels(splits[1])).toEqual([20, 21]);
    expect(splitDropLabels(splits[2])).toBeNull();
    expect(splitDropLabels(splits[3])).toBeNull(); // routes into another tab
    expect(splitDropLabels(splits[4])).toBeNull(); // AI check decides
    expect(splitDropLabels(splits[5])).toEqual([60]); // legacy label query
  });

  it("accepts Important, Other, categories and label tabs", () => {
    const ok = (r: TabRoute) => canDropOnTab(r, splits, labels);
    expect(ok("important")).toBe(true);
    expect(ok("other")).toBe(true);
    expect(ok("label:101")).toBe(true);
    expect(ok("label:10")).toBe(false); // a user label is no tab
    expect(ok("split:1")).toBe(true);
    expect(ok("split:3")).toBe(false);
    expect(ok("split:99")).toBe(false);
  });
});

describe("labelsAfterMove", () => {
  it("swaps tab labels and keeps the rest", () => {
    expect(labelsAfterMove([10, 7], "split:2", splits, labels)).toEqual([7, 20]);
    expect(labelsAfterMove([21, 10], "split:2", splits, labels)).toEqual([21]);
  });

  it("follows the category route", () => {
    expect(labelsAfterMove([10, 101, 7], "label:102", splits, labels)).toEqual([7, 102]);
    expect(labelsAfterMove([101, 7], "important", splits, labels)).toEqual([7]);
  });
});
