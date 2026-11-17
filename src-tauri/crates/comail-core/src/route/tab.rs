//! Label tabs: a split that shows one user label (or starred mail) as its own
//! tab, with senders "learned" into or out of it (a drag onto the tab learns
//! the thread's sender) without labelling the mail. A line-for-line mirror of
//! `src/lib/labelTabs.ts`; keep the two in step.
//!
//! Canonical condition shapes, all recognised by [`parse_tab_condition`]:
//!
//! ```text
//! any[anchor, sender = a, sender = b, …]                      (include form)
//! all[any[anchor, sender = a, …], not(any[sender = x, …])]    (with excludes)
//! ```
//!
//! where `anchor` is `label{id}` or `isStarred{true}`. A bare anchor leaf and
//! single-child `all`/`any` wrappers around it are recognised too.

use crate::models::{RULE_MAX_NODES, RuleCondition, SplitRule, TextOp};

/// What a label tab shows: one user label, or starred mail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabAnchor {
    Label(i64),
    Starred,
}

/// A label tab condition, split into its parts. Senders are oldest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabCondition {
    pub anchor: TabAnchor,
    /// Learned senders whose mail joins the tab even without the label.
    pub include: Vec<String>,
    /// Learned senders whose mail never shows in the tab, label or not.
    pub exclude: Vec<String>,
}

/// Lowercased, trimmed address: how learned senders are compared and stored.
pub fn normalize_sender(email: &str) -> String {
    email.trim().to_lowercase()
}

fn anchor_of(c: &RuleCondition) -> Option<TabAnchor> {
    match c {
        RuleCondition::Label { id } => Some(TabAnchor::Label(*id)),
        RuleCondition::IsStarred { value: true } => Some(TabAnchor::Starred),
        _ => None,
    }
}

fn anchor_leaf(a: TabAnchor) -> RuleCondition {
    match a {
        TabAnchor::Label(id) => RuleCondition::Label { id },
        TabAnchor::Starred => RuleCondition::IsStarred { value: true },
    }
}

fn sender_equals(c: &RuleCondition) -> Option<&str> {
    match c {
        RuleCondition::Sender {
            op: TextOp::Equals,
            value,
        } if !value.trim().is_empty() => Some(value),
        _ => None,
    }
}

fn sender_leaf(email: &str) -> RuleCondition {
    RuleCondition::Sender {
        op: TextOp::Equals,
        value: email.to_string(),
    }
}

/// `any[anchor, sender = …]`, a bare anchor, or a one-child wrapper of either.
fn parse_include(c: &RuleCondition) -> Option<(TabAnchor, Vec<String>)> {
    if let Some(a) = anchor_of(c) {
        return Some((a, Vec::new()));
    }
    let of = match c {
        RuleCondition::All { of } if of.len() == 1 => return parse_include(&of[0]),
        RuleCondition::Any { of } if of.len() == 1 => return parse_include(&of[0]),
        RuleCondition::Any { of } => of,
        _ => return None,
    };
    let mut anchor = None;
    let mut include = Vec::new();
    for child in of {
        if let Some(a) = anchor_of(child) {
            if anchor.is_some() {
                return None; // two anchors: not a single label's tab
            }
            anchor = Some(a);
            continue;
        }
        include.push(sender_equals(child)?.to_string());
    }
    anchor.map(|a| (a, include))
}

/// `not(any[sender = …])` or `not(sender = …)`.
fn parse_exclude(c: &RuleCondition) -> Option<Vec<String>> {
    let RuleCondition::Not { cond } = c else {
        return None;
    };
    if let Some(one) = sender_equals(cond) {
        return Some(vec![one.to_string()]);
    }
    match cond.as_ref() {
        RuleCondition::Any { of } if !of.is_empty() => of
            .iter()
            .map(|child| sender_equals(child).map(str::to_string))
            .collect(),
        _ => None,
    }
}

/// The parts of a label tab condition, or `None` for any other condition.
pub fn parse_tab_condition(c: &RuleCondition) -> Option<TabCondition> {
    if let Some((anchor, include)) = parse_include(c) {
        return Some(TabCondition {
            anchor,
            include,
            exclude: Vec::new(),
        });
    }
    let RuleCondition::All { of } = c else {
        return None;
    };
    if of.len() != 2 {
        return None;
    }
    for (a, b) in [(&of[0], &of[1]), (&of[1], &of[0])] {
        if let (Some((anchor, include)), Some(exclude)) = (parse_include(a), parse_exclude(b)) {
            return Some(TabCondition {
                anchor,
                include,
                exclude,
            });
        }
    }
    None
}

/// Canonical condition for a label tab (see the module comment).
pub fn tab_condition(t: &TabCondition) -> RuleCondition {
    let mut of = vec![anchor_leaf(t.anchor)];
    of.extend(t.include.iter().map(|s| sender_leaf(s)));
    let include = RuleCondition::Any { of };
    if t.exclude.is_empty() {
        return include;
    }
    RuleCondition::All {
        of: vec![
            include,
            RuleCondition::Not {
                cond: Box::new(RuleCondition::Any {
                    of: t.exclude.iter().map(|s| sender_leaf(s)).collect(),
                }),
            },
        ],
    }
}

/// Node count of [`tab_condition`]'s output, as `RuleCondition::nodes` counts.
fn nodes(t: &TabCondition) -> usize {
    let include = 2 + t.include.len();
    if t.exclude.is_empty() {
        include
    } else {
        1 + include + 2 + t.exclude.len()
    }
}

/// The label or starred mail a split shows as its own tab, or `None` for any
/// other kind of split. Only a split with its own tab (no `target`) and no AI
/// check counts. Legacy splits saved as the flat query `{labels:[id]}` count
/// too.
pub fn tab_match(rule: &SplitRule) -> Option<TabAnchor> {
    if rule.target.is_some() || rule.ai.is_some() {
        return None;
    }
    if let Some(c) = &rule.condition {
        return parse_tab_condition(c).map(|t| t.anchor);
    }
    let q = &rule.query;
    let others = q.senders.is_some()
        || q.exclude_senders.is_some()
        || q.recipients.is_some()
        || q.subject_contains.is_some()
        || q.is_automated.is_some()
        || q.has_attachment.is_some();
    match q.labels.as_deref() {
        Some([id]) if !others => Some(TabAnchor::Label(*id)),
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Include,
    Exclude,
}

/// Move `email` into `into` (removing it from the other set), then trim to
/// fit `RULE_MAX_NODES`. Over budget, the oldest learned senders go first:
/// from the set just grown while it holds anything besides `email`, else from
/// the other set. The anchor is never dropped.
fn learn(t: &TabCondition, email: &str, into: Side) -> TabCondition {
    let key = normalize_sender(email);
    let keep = |v: &[String]| -> Vec<String> {
        v.iter()
            .filter(|s| normalize_sender(s) != key)
            .cloned()
            .collect()
    };
    let mut next = TabCondition {
        anchor: t.anchor,
        include: keep(&t.include),
        exclude: keep(&t.exclude),
    };
    match into {
        Side::Include => next.include.push(key),
        Side::Exclude => next.exclude.push(key),
    }
    while nodes(&next) > RULE_MAX_NODES {
        let (grown, other) = match into {
            Side::Include => (&mut next.include, &mut next.exclude),
            Side::Exclude => (&mut next.exclude, &mut next.include),
        };
        if grown.len() > 1 {
            grown.remove(0);
        } else {
            other.remove(0);
        }
    }
    next
}

/// `cond` with `email` learned into the tab: mail from that exact address
/// shows in it even without the label (and no longer stays out). `None` when
/// `cond` is not a label tab condition, or `email` is blank.
pub fn learn_sender(cond: &RuleCondition, email: &str) -> Option<RuleCondition> {
    let t = parse_tab_condition(cond)?;
    (!normalize_sender(email).is_empty()).then(|| tab_condition(&learn(&t, email, Side::Include)))
}

/// `cond` with `email` learned out of the tab: mail from that exact address
/// never shows in it, even when labelled (and is no longer pulled in). `None`
/// when `cond` is not a label tab condition, or `email` is blank.
pub fn exclude_sender(cond: &RuleCondition, email: &str) -> Option<RuleCondition> {
    let t = parse_tab_condition(cond)?;
    (!normalize_sender(email).is_empty()).then(|| tab_condition(&learn(&t, email, Side::Exclude)))
}

#[cfg(test)]
mod tests {
    //! The cases of `src/lib/labelTabs.test.ts` that pin the shared format.
    use super::*;
    use crate::models::{RouteKey, SplitRuleQuery};
    use serde_json::json;

    fn cond(v: serde_json::Value) -> RuleCondition {
        serde_json::from_value(v).unwrap()
    }

    fn from(email: &str) -> serde_json::Value {
        json!({ "kind": "sender", "op": "equals", "value": email })
    }

    fn label() -> serde_json::Value {
        json!({ "kind": "label", "id": 4 })
    }

    #[test]
    fn parses_the_canonical_forms_and_rejects_the_rest() {
        assert_eq!(
            parse_tab_condition(&cond(json!({ "kind": "all", "of": [
                { "kind": "any", "of": [{ "kind": "isStarred", "value": true }, from("a@x.io")] },
                { "kind": "not", "cond": { "kind": "any", "of": [from("spam@x.io")] } },
            ] }))),
            Some(TabCondition {
                anchor: TabAnchor::Starred,
                include: vec!["a@x.io".into()],
                exclude: vec!["spam@x.io".into()],
            })
        );
        assert_eq!(
            parse_tab_condition(&cond(
                json!({ "kind": "all", "of": [{ "kind": "not", "cond": from("z@x.io") }, label()] })
            ))
            .map(|t| t.exclude),
            Some(vec!["z@x.io".to_string()])
        );
        for c in [
            from("a@x.io"),
            json!({ "kind": "isStarred", "value": false }),
            json!({ "kind": "any", "of": [] }),
            json!({ "kind": "any", "of": [label(), { "kind": "label", "id": 5 }] }),
            json!({ "kind": "any", "of": [label(), { "kind": "sender", "op": "contains", "value": "x.io" }] }),
            json!({ "kind": "all", "of": [label(), from("a@x.io")] }),
            json!({ "kind": "not", "cond": label() }),
        ] {
            assert_eq!(parse_tab_condition(&cond(c.clone())), None, "{c}");
        }
    }

    #[test]
    fn tab_match_counts_own_tab_splits_and_legacy_label_queries() {
        let rule = |condition: Option<RuleCondition>, query: SplitRuleQuery| SplitRule {
            id: 1,
            name: "t".into(),
            position: 0,
            query,
            condition,
            target: None,
            ai: None,
            count_all: false,
            exclusive: false,
        };
        let tab = rule(Some(cond(label())), SplitRuleQuery::default());
        assert_eq!(tab_match(&tab), Some(TabAnchor::Label(4)));
        let routed = SplitRule {
            target: Some(RouteKey::Other),
            ..tab.clone()
        };
        assert_eq!(tab_match(&routed), None);
        let legacy = |labels: Vec<i64>, senders: Option<Vec<String>>| {
            tab_match(&rule(
                None,
                SplitRuleQuery {
                    labels: Some(labels),
                    senders,
                    ..SplitRuleQuery::default()
                },
            ))
        };
        assert_eq!(legacy(vec![4], None), Some(TabAnchor::Label(4)));
        assert_eq!(legacy(vec![4, 5], None), None);
        assert_eq!(legacy(vec![4], Some(vec!["a".into()])), None);
    }

    #[test]
    fn learning_moves_a_sender_between_the_sets() {
        let learned = learn_sender(&cond(label()), "  Alice@X.io ").unwrap();
        assert_eq!(
            serde_json::to_value(&learned).unwrap(),
            json!({ "kind": "any", "of": [label(), from("alice@x.io")] })
        );
        assert_eq!(
            learn_sender(&learned, "alice@x.io").as_ref(),
            Some(&learned)
        );
        let excluded = exclude_sender(&learned, "ALICE@x.io").unwrap();
        assert_eq!(
            serde_json::to_value(&excluded).unwrap(),
            json!({ "kind": "all", "of": [
                { "kind": "any", "of": [label()] },
                { "kind": "not", "cond": { "kind": "any", "of": [from("alice@x.io")] } },
            ] })
        );
        assert_eq!(learn_sender(&excluded, "alice@x.io"), Some(learned));
        assert_eq!(learn_sender(&cond(from("a@x.io")), "b@x.io"), None);
        assert_eq!(learn_sender(&cond(label()), "   "), None);
    }

    #[test]
    fn learning_stays_within_the_node_limit() {
        let mut c = cond(label());
        for i in 0..100 {
            c = learn_sender(&c, &format!("s{i}@x.io")).unwrap();
        }
        let t = parse_tab_condition(&c).unwrap();
        assert_eq!(t.include.len(), RULE_MAX_NODES - 2);
        assert_eq!(t.include.last().unwrap(), "s99@x.io");
        c = exclude_sender(&c, "spam@x.io").unwrap();
        for i in 0..100 {
            c = exclude_sender(&c, &format!("x{i}@x.io")).unwrap();
        }
        let t = parse_tab_condition(&c).unwrap();
        assert_eq!(t.exclude.last().unwrap(), "x99@x.io");
        assert!(!t.exclude.contains(&"spam@x.io".to_string()));
        assert!(c.validate(&[4]).is_empty());
    }
}
