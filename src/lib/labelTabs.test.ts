import { describe, expect, it } from "vitest";
import type { RuleCondition, SplitRule } from "../ipc/types";
import {
  excludeSender,
  findTab,
  learnSender,
  parseTabCondition,
  sameAnchor,
  tabCondition,
  tabMatch,
} from "./labelTabs";
import { conditionProblems, RULE_MAX_NODES } from "./ruleCondition";

const label: RuleCondition = { kind: "label", id: 4 };
const starred: RuleCondition = { kind: "isStarred", value: true };
const from = (value: string, op: "equals" | "contains" = "equals"): RuleCondition => ({
  kind: "sender",
  op,
  value,
});

function split(p: Partial<SplitRule>): SplitRule {
  return { id: 1, name: "t", position: 0, query: {}, ...p };
}

/** Node count, as Rust `RuleCondition::nodes` counts it. */
function nodes(c: RuleCondition): number {
  if (c.kind === "all" || c.kind === "any") return 1 + c.of.reduce((n, x) => n + nodes(x), 0);
  if (c.kind === "not") return 1 + nodes(c.cond);
  return 1;
}

const last = (xs: string[]) => xs[xs.length - 1];

describe("parseTabCondition / tabMatch", () => {
  it("recognises bare leaves, the canonical forms and one-child wrappers", () => {
    expect(parseTabCondition(label)).toEqual({
      anchor: { kind: "label", labelId: 4 },
      include: [],
      exclude: [],
    });
    expect(parseTabCondition({ kind: "any", of: [starred] })?.anchor).toEqual({ kind: "starred" });
    expect(parseTabCondition({ kind: "all", of: [label] })?.anchor).toEqual({
      kind: "label",
      labelId: 4,
    });
    expect(parseTabCondition({ kind: "any", of: [label, from("a@x.io"), from("b@x.io")] })).toEqual({
      anchor: { kind: "label", labelId: 4 },
      include: ["a@x.io", "b@x.io"],
      exclude: [],
    });
    const withExcludes: RuleCondition = {
      kind: "all",
      of: [
        { kind: "any", of: [starred, from("a@x.io")] },
        { kind: "not", cond: { kind: "any", of: [from("spam@x.io")] } },
      ],
    };
    expect(parseTabCondition(withExcludes)).toEqual({
      anchor: { kind: "starred" },
      include: ["a@x.io"],
      exclude: ["spam@x.io"],
    });
    // Order inside `all` and a bare negated sender are tolerated too.
    expect(
      parseTabCondition({ kind: "all", of: [{ kind: "not", cond: from("z@x.io") }, label] }),
    ).toEqual({ anchor: { kind: "label", labelId: 4 }, include: [], exclude: ["z@x.io"] });
  });

  it("rejects anything else", () => {
    const rejects: (RuleCondition | null)[] = [
      null,
      from("a@x.io"),
      { kind: "isStarred", value: false },
      { kind: "any", of: [] },
      { kind: "any", of: [label, { kind: "label", id: 5 }] },
      { kind: "any", of: [label, from("x.io", "contains")] },
      { kind: "all", of: [label, from("a@x.io")] },
      { kind: "all", of: [label, { kind: "not", cond: { kind: "any", of: [] } }] },
      { kind: "all", of: [label, { kind: "not", cond: label }] },
      { kind: "not", cond: label },
      { kind: "any", of: [from("a@x.io"), from("b@x.io")] },
    ];
    for (const c of rejects) expect(parseTabCondition(c)).toBeNull();
  });

  it("only counts own-tab splits without an AI check, and legacy label queries", () => {
    expect(tabMatch(split({ condition: label }))).toEqual({ kind: "label", labelId: 4 });
    expect(tabMatch(split({ condition: label, exclusive: false }))).toEqual({
      kind: "label",
      labelId: 4,
    });
    expect(tabMatch(split({ condition: label, target: "other" }))).toBeNull();
    expect(tabMatch(split({ condition: label, ai: { instruction: "x" } }))).toBeNull();
    expect(tabMatch(split({ query: { labels: [4] } }))).toEqual({ kind: "label", labelId: 4 });
    expect(tabMatch(split({ query: { labels: [4], senders: ["a"] } }))).toBeNull();
    expect(tabMatch(split({ query: { labels: [4, 5] } }))).toBeNull();
    expect(tabMatch(split({ query: { senders: ["a"] } }))).toBeNull();
  });

  it("finds the tab for an anchor", () => {
    const splits = [
      split({ id: 1, condition: from("a@x.io") }),
      split({ id: 2, condition: { kind: "any", of: [starred] } }),
      split({ id: 3, condition: label }),
    ];
    expect(findTab(splits, { kind: "label", labelId: 4 })?.id).toBe(3);
    expect(findTab(splits, { kind: "starred" })?.id).toBe(2);
    expect(findTab(splits, { kind: "label", labelId: 9 })).toBeUndefined();
    expect(sameAnchor(null, { kind: "starred" })).toBe(false);
    expect(sameAnchor({ kind: "label", labelId: 1 }, { kind: "starred" })).toBe(false);
  });

  it("builds the canonical forms, which round-trip and validate", () => {
    const t = { anchor: { kind: "label" as const, labelId: 4 }, include: ["a@x.io"], exclude: [] };
    expect(tabCondition(t)).toEqual({ kind: "any", of: [label, from("a@x.io")] });
    const both = { ...t, exclude: ["b@x.io"] };
    expect(parseTabCondition(tabCondition(both))).toEqual(both);
    expect(conditionProblems(tabCondition(both))).toEqual([]);
    expect(tabCondition({ anchor: { kind: "starred" }, include: [], exclude: [] })).toEqual({
      kind: "any",
      of: [starred],
    });
  });
});

describe("learnSender / excludeSender", () => {
  it("adds to one set and removes from the other, normalizing addresses", () => {
    const learned = learnSender(label, "  Alice@X.io ");
    expect(learned).toEqual({ kind: "any", of: [label, from("alice@x.io")] });
    // Learning again is a no-op beyond moving it to the newest slot.
    expect(learnSender(learned, "alice@x.io")).toEqual(learned);

    const excluded = excludeSender(learned, "ALICE@x.io");
    expect(excluded).toEqual({
      kind: "all",
      of: [
        { kind: "any", of: [label] },
        { kind: "not", cond: { kind: "any", of: [from("alice@x.io")] } },
      ],
    });
    // Learning back in drops the exclusion and the `all` wrapper with it.
    expect(learnSender(excluded, "alice@x.io")).toEqual(learned);
  });

  it("keeps other learned senders and their order", () => {
    let c: RuleCondition | null = { kind: "any", of: [starred] };
    for (const s of ["a@x.io", "b@x.io", "c@x.io"]) c = learnSender(c, s);
    c = excludeSender(c, "b@x.io");
    c = excludeSender(c, "z@x.io");
    expect(parseTabCondition(c)).toEqual({
      anchor: { kind: "starred" },
      include: ["a@x.io", "c@x.io"],
      exclude: ["b@x.io", "z@x.io"],
    });
  });

  it("returns null for conditions that are not label tabs, or a blank address", () => {
    expect(learnSender(from("a@x.io", "contains"), "b@x.io")).toBeNull();
    expect(excludeSender(null, "b@x.io")).toBeNull();
    expect(learnSender(label, "   ")).toBeNull();
  });

  it("stays within RULE_MAX_NODES by dropping the oldest learned senders", () => {
    let c: RuleCondition | null = label;
    for (let i = 0; i < 100; i++) c = learnSender(c, `s${i}@x.io`);
    let t = parseTabCondition(c)!;
    expect(nodes(c!)).toBeLessThanOrEqual(RULE_MAX_NODES);
    expect(t.include).toHaveLength(RULE_MAX_NODES - 2);
    expect(last(t.include)).toBe("s99@x.io");
    expect(t.include[0]).toBe(`s${100 - (RULE_MAX_NODES - 2)}@x.io`);

    // The first exclusion costs the `all`/`not`/`any` wrappers: the oldest
    // includes make room, never the new exclusion.
    c = excludeSender(c, "spam@x.io");
    t = parseTabCondition(c)!;
    expect(nodes(c!)).toBeLessThanOrEqual(RULE_MAX_NODES);
    expect(t.exclude).toEqual(["spam@x.io"]);
    expect(last(t.include)).toBe("s99@x.io");

    // Growing the excludes drops the oldest exclusions once only they are left.
    for (let i = 0; i < 100; i++) c = excludeSender(c, `x${i}@x.io`);
    t = parseTabCondition(c)!;
    expect(nodes(c!)).toBeLessThanOrEqual(RULE_MAX_NODES);
    expect(last(t.exclude)).toBe("x99@x.io");
    expect(t.exclude).not.toContain("spam@x.io");
    expect(conditionProblems(c!)).toEqual([]);
  });
});
