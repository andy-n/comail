import { describe, expect, it } from "vitest";
import type { Label, SplitRule } from "../ipc/types";
import {
  activeInboxTabKey,
  inboxTabOrder,
  mergeTabOrder,
  moveTab,
  moveTabByKey,
  nextTabPosition,
  tabOrderPayload,
  type OrderedTab,
} from "./splitOrder";

function split(id: number, name: string, position: number): SplitRule {
  return { id, name, position, query: {} };
}

function autoLabel(id: number, name: string, position: number): Label {
  return {
    id,
    name,
    position,
    color: "#000000",
    keyword: `ComailAuto${name}`,
    isAuto: true,
  };
}

describe("inboxTabOrder", () => {
  it("uses the arranged interleaved order between fixed endpoints", () => {
    const tabs = inboxTabOrder(
      [split(1, "Ads", 0), split(2, "Xcode", 2)],
      [autoLabel(101, "News", 1), autoLabel(102, "Social", 3)],
      true,
    );

    expect(tabs.map((tab) => tab.kind)).toEqual([
      "important",
      "split",
      "label",
      "split",
      "label",
      "other",
    ]);
    expect(tabs.map((tab) => "name" in tab ? tab.name : tab.kind)).toEqual([
      "important",
      "Ads",
      "News",
      "Xcode",
      "Social",
      "other",
    ]);
  });

  it("keeps the legacy grouped order while positions still collide", () => {
    const tabs = inboxTabOrder(
      [split(1, "Ads", 0)],
      [autoLabel(101, "News", 0)],
      true,
    );
    expect(tabs.map((tab) => "name" in tab ? tab.name : tab.kind)).toEqual([
      "important",
      "Ads",
      "News",
      "other",
    ]);
  });

  it("excludes rules that route their matches into an existing tab", () => {
    const routed = { ...split(2, "Receipts", 1), target: "label:101" };
    const tabs = inboxTabOrder(
      [split(1, "Ads", 0), routed],
      [autoLabel(101, "News", 2)],
      true,
    );
    expect(tabs.map((tab) => "name" in tab ? tab.name : tab.kind)).toEqual([
      "important",
      "Ads",
      "News",
      "other",
    ]);
  });

  it("omits auto labels when the feature is disabled", () => {
    const tabs = inboxTabOrder(
      [split(1, "Ads", 0)],
      [autoLabel(101, "News", 1)],
      false,
    );
    expect(tabs.map((tab) => "name" in tab ? tab.name : tab.kind)).toEqual([
      "important",
      "Ads",
      "other",
    ]);
  });
});

describe("nextTabPosition", () => {
  it("starts at 0 with no tabs", () => {
    expect(nextTabPosition([], [])).toBe(0);
    expect(nextTabPosition(undefined, undefined)).toBe(0);
  });

  it("never collides with an auto-label position when positions have gaps", () => {
    // After a delete, positions are 0, 1, 3: a count-based position (3)
    // would collide with News and flip the order back to grouped.
    const splits = [split(1, "Ads", 0), split(2, "Xcode", 3)];
    const labels = [autoLabel(101, "News", 1)];
    expect(nextTabPosition(splits, labels)).toBe(4);
  });

  it("ignores non-auto labels", () => {
    const manual = { ...autoLabel(200, "Work", 9), isAuto: false };
    expect(nextTabPosition([split(1, "Ads", 0)], [manual])).toBe(1);
  });

  it("keeps the arranged interleaving when the popover adds a split", () => {
    const splits = [split(1, "Ads", 0), split(2, "Xcode", 2)];
    const labels = [autoLabel(101, "News", 1), autoLabel(102, "Social", 3)];
    const created = split(3, "GitHub", nextTabPosition(splits, labels));
    const tabs = inboxTabOrder([...splits, created], labels, true);
    expect(tabs.map((tab) => "name" in tab ? tab.name : tab.kind)).toEqual([
      "important",
      "Ads",
      "News",
      "Xcode",
      "Social",
      "GitHub",
      "other",
    ]);
  });
});

describe("activeInboxTabKey", () => {
  it("maps the fixed buckets, custom splits and labels", () => {
    expect(activeInboxTabKey(-1, null)).toBe("important");
    expect(activeInboxTabKey(-2, null)).toBe("other");
    expect(activeInboxTabKey(7, null)).toBe("s7");
    expect(activeInboxTabKey(null, 101)).toBe("l101");
  });

  it("highlights no tab for the unsplit whole inbox", () => {
    expect(activeInboxTabKey(null, null)).toBeNull();
  });
});

describe("tab reorder payloads", () => {
  // Ads(split) · News(label) · Receipts(split routed into News, no own tab) · Xcode(split) · Social(label)
  const routed = { ...split(3, "Receipts", 2), target: "label:101" };
  const all = () =>
    mergeTabOrder(
      [split(1, "Ads", 0), routed, split(2, "Xcode", 3)],
      [autoLabel(101, "News", 1), autoLabel(102, "Social", 4)],
    );
  const payload = (items: OrderedTab[] | null) =>
    items && tabOrderPayload(items).map((r) => `${r.kind}:${r.id}`);

  it("sends every tab, routed rules included, as kind + id", () => {
    expect(payload(all())).toEqual(["split:1", "label:101", "split:3", "split:2", "label:102"]);
  });

  it("moves by index the way Settings does", () => {
    expect(payload(moveTab(all(), 0, 3))).toEqual([
      "label:101",
      "split:3",
      "split:2",
      "split:1",
      "label:102",
    ]);
    expect(payload(moveTab(all(), 4, 0))).toEqual([
      "label:102",
      "split:1",
      "label:101",
      "split:3",
      "split:2",
    ]);
    expect(moveTab(all(), 2, 2)).toBeNull();
    expect(moveTab(all(), 0, 9)).toBeNull();
  });

  it("matches moveTab when every tab is visible", () => {
    const items = all();
    const keys = payload(items) ?? [];
    for (let from = 0; from < items.length; from++) {
      for (let to = 0; to < items.length; to++) {
        expect(payload(moveTabByKey(items, keys[from], keys[to]))).toEqual(
          payload(moveTab(items, from, to)),
        );
      }
    }
  });

  it("drops past a visible tab without moving hidden routed rules", () => {
    // The bar shows Ads · News · Xcode · Social. Ads dropped on Xcode lands right
    // after Xcode; Receipts keeps its slot between News and Xcode.
    expect(payload(moveTabByKey(all(), "split:1", "split:2"))).toEqual([
      "label:101",
      "split:3",
      "split:2",
      "split:1",
      "label:102",
    ]);
    // Social dragged up onto News lands just before News.
    expect(payload(moveTabByKey(all(), "label:102", "label:101"))).toEqual([
      "split:1",
      "label:102",
      "label:101",
      "split:3",
      "split:2",
    ]);
  });

  it("ignores unknown keys and drops onto itself", () => {
    expect(moveTabByKey(all(), "split:1", "split:1")).toBeNull();
    expect(moveTabByKey(all(), "split:9", "split:1")).toBeNull();
  });
});
