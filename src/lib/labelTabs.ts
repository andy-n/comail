import type { RuleCondition, SplitRule } from "../ipc/types";
import { RULE_MAX_NODES } from "./ruleCondition";

/**
 * Label tabs: a split that shows one user label (or starred mail) as an inbox
 * tab. Settings → Split inbox → Label tabs creates them, and senders can be
 * "learned" into or out of a tab (e.g. by dragging a thread onto it) without
 * applying the label to the mail.
 *
 * Canonical condition shapes, all recognised by {@link parseTabCondition}:
 *
 *   any[anchor, sender = a, sender = b, …]                      (include form)
 *   all[any[anchor, sender = a, …], not(any[sender = x, …])]    (with excludes)
 *
 * where `anchor` is `{kind:"label",id}` or `{kind:"isStarred",value:true}`.
 * A bare anchor leaf (how older label tabs were saved) and single-child
 * `all`/`any` wrappers around it are recognised too.
 */
export type TabAnchor = { kind: "label"; labelId: number } | { kind: "starred" };

/** A label tab condition, split into its parts. Senders are oldest first. */
export interface TabCondition {
  anchor: TabAnchor;
  /** Learned senders whose mail joins the tab even without the label. */
  include: string[];
  /** Learned senders whose mail never shows in the tab, label or not. */
  exclude: string[];
}

/** Lowercased, trimmed address: how learned senders are compared and stored. */
export const normalizeSender = (email: string) => email.trim().toLowerCase();

function anchorOf(c: RuleCondition): TabAnchor | null {
  if (c.kind === "label") return { kind: "label", labelId: c.id };
  if (c.kind === "isStarred" && c.value) return { kind: "starred" };
  return null;
}

function anchorLeaf(a: TabAnchor): RuleCondition {
  return a.kind === "label"
    ? { kind: "label", id: a.labelId }
    : { kind: "isStarred", value: true };
}

function senderEquals(c: RuleCondition): string | null {
  return c.kind === "sender" && c.op === "equals" && c.value.trim() !== ""
    ? c.value
    : null;
}

const senderLeaf = (email: string): RuleCondition => ({
  kind: "sender",
  op: "equals",
  value: email,
});

/** `any[anchor, sender = …]`, a bare anchor, or a one-child wrapper of either. */
function parseInclude(c: RuleCondition): Omit<TabCondition, "exclude"> | null {
  const bare = anchorOf(c);
  if (bare) return { anchor: bare, include: [] };
  if (c.kind === "all" && c.of.length === 1) return parseInclude(c.of[0]);
  if (c.kind !== "any") return null;
  if (c.of.length === 1) return parseInclude(c.of[0]);
  let anchor: TabAnchor | null = null;
  const include: string[] = [];
  for (const child of c.of) {
    const a = anchorOf(child);
    if (a) {
      if (anchor) return null; // two anchors: not a single label's tab
      anchor = a;
      continue;
    }
    const s = senderEquals(child);
    if (s == null) return null;
    include.push(s);
  }
  return anchor ? { anchor, include } : null;
}

/** `not(any[sender = …])` or `not(sender = …)`. */
function parseExclude(c: RuleCondition): string[] | null {
  if (c.kind !== "not") return null;
  const inner = c.cond;
  const one = senderEquals(inner);
  if (one != null) return [one];
  if (inner.kind !== "any" || inner.of.length === 0) return null;
  const out: string[] = [];
  for (const child of inner.of) {
    const s = senderEquals(child);
    if (s == null) return null;
    out.push(s);
  }
  return out;
}

/** The parts of a label tab condition, or null for any other condition. */
export function parseTabCondition(
  c: RuleCondition | null | undefined,
): TabCondition | null {
  if (!c) return null;
  const inc = parseInclude(c);
  if (inc) return { ...inc, exclude: [] };
  if (c.kind !== "all" || c.of.length !== 2) return null;
  for (const [a, b] of [
    [c.of[0], c.of[1]],
    [c.of[1], c.of[0]],
  ]) {
    const include = parseInclude(a);
    const exclude = parseExclude(b);
    if (include && exclude) return { ...include, exclude };
  }
  return null;
}

/** Canonical condition for a label tab (see the module comment). */
export function tabCondition(t: TabCondition): RuleCondition {
  const include: RuleCondition = {
    kind: "any",
    of: [anchorLeaf(t.anchor), ...t.include.map(senderLeaf)],
  };
  if (t.exclude.length === 0) return include;
  return {
    kind: "all",
    of: [include, { kind: "not", cond: { kind: "any", of: t.exclude.map(senderLeaf) } }],
  };
}

/** Node count of {@link tabCondition}'s output, as Rust `RuleCondition::nodes` counts. */
function nodes(t: TabCondition): number {
  const include = 2 + t.include.length;
  return t.exclude.length === 0 ? include : 1 + include + 2 + t.exclude.length;
}

/**
 * The label or starred mail a split shows as its own tab, or null when it is
 * any other kind of split. Only a split with its own tab (no `target`) and no
 * AI check counts. Legacy splits saved as the flat query `{labels:[id]}` count
 * too.
 */
export function tabMatch(rule: SplitRule): TabAnchor | null {
  if (rule.target != null || rule.ai) return null;
  if (rule.condition) return parseTabCondition(rule.condition)?.anchor ?? null;
  const q = rule.query;
  const fields = Object.values(q).filter((v) => v != null).length;
  return fields === 1 && q.labels?.length === 1
    ? { kind: "label", labelId: q.labels[0] }
    : null;
}

export function sameAnchor(a: TabAnchor | null, b: TabAnchor): boolean {
  if (a == null) return false;
  if (a.kind === "starred" || b.kind === "starred") return a.kind === b.kind;
  return a.labelId === b.labelId;
}

/** The first split showing `anchor` as a tab, if any. */
export function findTab(
  splits: SplitRule[] | undefined,
  anchor: TabAnchor,
): SplitRule | undefined {
  return (splits ?? []).find((s) => sameAnchor(tabMatch(s), anchor));
}

/**
 * Move `email` into `into` (removing it from `from`), then trim to fit
 * RULE_MAX_NODES. Over budget, the oldest learned senders go first: from the
 * set just grown while it holds anything besides `email`, else from the other
 * set. The anchor is never dropped.
 */
function learn(t: TabCondition, email: string, into: "include" | "exclude"): TabCondition {
  const key = normalizeSender(email);
  const from = into === "include" ? "exclude" : "include";
  const next: TabCondition = {
    anchor: t.anchor,
    include: t.include.filter((s) => normalizeSender(s) !== key),
    exclude: t.exclude.filter((s) => normalizeSender(s) !== key),
  };
  next[into].push(key);
  while (nodes(next) > RULE_MAX_NODES) {
    if (next[into].length > 1) next[into].shift();
    else next[from].shift();
  }
  return next;
}

/**
 * `cond` with `email` learned into the tab: mail from that exact address shows
 * in it even without the label (and no longer stays out). Returns null when
 * `cond` is not a label tab condition, or `email` is blank.
 */
export function learnSender(cond: RuleCondition | null | undefined, email: string): RuleCondition | null {
  const t = parseTabCondition(cond);
  if (!t || normalizeSender(email) === "") return null;
  return tabCondition(learn(t, email, "include"));
}

/**
 * `cond` with `email` learned out of the tab: mail from that exact address
 * never shows in it, even when labelled (and is no longer pulled in). Returns
 * null when `cond` is not a label tab condition, or `email` is blank.
 */
export function excludeSender(cond: RuleCondition | null | undefined, email: string): RuleCondition | null {
  const t = parseTabCondition(cond);
  if (!t || normalizeSender(email) === "") return null;
  return tabCondition(learn(t, email, "exclude"));
}
