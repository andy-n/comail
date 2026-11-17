//! Manual moves between inbox tabs: mail dragged onto a tab, bundle or sidebar
//! row. [`plan_move`] is pure: from one thread's rule facts and the tab
//! definitions it lists the steps that make the thread land in the target tab,
//! replaying the same first-match-wins evaluation routing uses
//! ([`super::decide`]) on a simulated copy of the thread. `Core` executes the
//! steps through the normal label and star actions (IMAP keyword / `\Flagged`
//! sync, reroute) and saves condition edits the way `save_split` does.
//!
//! Only label tabs ([`tab_match`]: one user label or starred mail, plus
//! learned senders) take dropped mail. On a drop:
//! - the target gets its label (or the star) and learns the thread's sender
//!   ([`learn_sender`]); a source label tab learns it out ([`exclude_sender`]);
//! - leaving the Starred tab unstars, leaving a view tab (`exclusive == false`)
//!   takes that tab's label; an exclusive source tab is left by the replay;
//! - a view tab target only adds: the thread stays where routing put it, with
//!   its other labels;
//! - otherwise the replay makes every routing split ahead of the target let
//!   go: label tabs lose their label, or learn the sender out when that isn't
//!   enough. A rule a drop can't undo (a sender or subject rule, an AI split
//!   that said yes) yields [`MovePlan::HeldBy`].

use super::decide::{RuleSubject, gate};
use super::tab::{TabAnchor, exclude_sender, learn_sender, normalize_sender, tab_match};
use crate::models::{RouteKey, RuleCondition, SplitRule};
use serde::{Deserialize, Serialize};

/// Whether mail can be dropped onto the tab `to` at all. Mirrors the UI's
/// drop-target check (`src/lib/tabDrop.ts`).
pub fn droppable(to: RouteKey, splits: &[SplitRule], categories: &[Category]) -> bool {
    match to {
        RouteKey::Important | RouteKey::Other => true,
        RouteKey::Label(id) => categories.iter().any(|c| c.id == id),
        RouteKey::Split(id) => splits
            .iter()
            .find(|s| s.id == id)
            .is_some_and(|s| tab_match(s).is_some()),
        RouteKey::Pending => false,
    }
}

/// An AI auto-category (auto label) a thread can be moved into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Category {
    pub id: i64,
    /// Stable keyword, the form the AI route cache stores verdicts in.
    pub keyword: String,
}

/// One thread as the planner sees it.
pub struct MoveFacts<'a> {
    /// Rule facts, labels and star included (the planner simulates changes on
    /// a copy).
    pub subject: &'a RuleSubject,
    /// `threads.routed_tab` as stored.
    pub routed_tab: Option<&'a str>,
    /// Lowercased From of the newest inbound message; `None` without one.
    pub sender: Option<&'a str>,
    /// The tab the thread was dragged out of, when the UI knows it (a view
    /// tab's bundle lists mail routed elsewhere); `None` = `routed_tab`.
    pub from: Option<RouteKey>,
}

/// One change that moves a thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveStep {
    /// Save a split's condition with a sender learned into or out of it.
    /// Planned ahead of the thread's own changes.
    SetCondition {
        split_id: i64,
        condition: RuleCondition,
    },
    AddLabel(i64),
    RemoveLabel(i64),
    /// Star (`true`) or unstar the thread.
    Star(bool),
    /// Write the route directly: the replay shows the rules agree (or none
    /// decides, for categories, Important and Other). Without one the thread
    /// is re-checked against the rules after the other steps.
    Route(RouteKey),
    /// Remember the sender's category like an AI verdict, so its future mail
    /// follows (`keyword` "" = no category).
    CacheSender {
        key: String,
        keyword: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MovePlan {
    /// The thread is already in the target tab.
    AlreadyThere,
    Steps(Vec<MoveStep>),
    /// This split would keep claiming the thread and a drop can't undo it
    /// (a sender or subject rule, or an AI split that already said yes).
    HeldBy(i64),
}

/// The target tab doesn't take dropped mail (e.g. a sender rule's tab).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotDroppable;

/// Split conditions a plan edits, over the splits it was given.
#[derive(Default)]
struct Edits {
    rules: Vec<SplitRule>,
}

impl Edits {
    /// `sp` with this plan's edits.
    fn rule<'a>(&'a self, sp: &'a SplitRule) -> &'a SplitRule {
        self.rules.iter().find(|r| r.id == sp.id).unwrap_or(sp)
    }

    fn edit(&mut self, sp: &SplitRule, f: impl FnOnce(&RuleCondition) -> Option<RuleCondition>) {
        let current = self.rule(sp).effective_condition();
        let Some(next) = f(&current).filter(|c| *c != current) else {
            return;
        };
        match self.rules.iter_mut().find(|r| r.id == sp.id) {
            Some(r) => r.condition = Some(next),
            None => self.rules.push(SplitRule {
                condition: Some(next),
                ..sp.clone()
            }),
        }
    }

    /// The condition saves, in first-edit order, ahead of `steps`.
    fn into_steps(self, steps: Vec<MoveStep>) -> Vec<MoveStep> {
        self.rules
            .into_iter()
            .filter_map(|r| {
                r.condition.map(|condition| MoveStep::SetCondition {
                    split_id: r.id,
                    condition,
                })
            })
            .chain(steps)
            .collect()
    }
}

/// Take a tab's anchor off the simulated thread.
fn release(anchor: TabAnchor, sim: &mut RuleSubject, steps: &mut Vec<MoveStep>) {
    match anchor {
        TabAnchor::Label(l) => {
            if sim.label_ids.remove(&l) {
                steps.push(MoveStep::RemoveLabel(l));
            }
        }
        TabAnchor::Starred => {
            if sim.is_starred {
                sim.is_starred = false;
                steps.push(MoveStep::Star(false));
            }
        }
    }
}

/// Plan moving one thread into tab `to`. `splits` must be position-ordered
/// (as routing evaluates them).
pub fn plan_move(
    to: RouteKey,
    facts: &MoveFacts,
    splits: &[SplitRule],
    categories: &[Category],
) -> Result<MovePlan, NotDroppable> {
    if !droppable(to, splits, categories) {
        return Err(NotDroppable);
    }
    fn label_tab(rule: &SplitRule) -> Option<(&SplitRule, TabAnchor)> {
        tab_match(rule).map(|a| (rule, a))
    }
    let find = |id: i64| splits.iter().find(|s| s.id == id);
    // The label tab dropped onto, if any (droppable: a split is one).
    let target = match to {
        RouteKey::Split(id) => Some(find(id).and_then(label_tab).ok_or(NotDroppable)?),
        _ => None,
    };
    let sender = facts.sender.map(normalize_sender).filter(|s| !s.is_empty());
    // Drops onto a label tab teach it (and the source tab) the sender.
    let learn = target.and(sender.as_deref());
    // The other label tab the thread is dragged out of.
    let source = facts
        .from
        .or_else(|| facts.routed_tab.and_then(|k| k.parse().ok()));
    let leaving = match source {
        Some(RouteKey::Split(id)) if source != Some(to) => find(id)
            .and_then(label_tab)
            .filter(|&(_, a)| target.map(|(_, t)| t) != Some(a)),
        _ => None,
    };

    let target_key = to.to_string();
    let in_target = match target {
        Some((rule, _)) if !rule.exclusive => gate(rule, facts.subject),
        _ => facts.routed_tab == Some(target_key.as_str()),
    };
    if in_target && leaving.is_none() {
        return Ok(MovePlan::AlreadyThere);
    }

    let mut sim = facts.subject.clone();
    let mut steps = Vec::new();
    let mut edits = Edits::default();

    // Leaving the Starred tab unstars; leaving a view tab drops its label (an
    // exclusive tab's label is the replay's to take).
    if let Some((rule, anchor)) = leaving {
        if anchor == TabAnchor::Starred || !rule.exclusive {
            release(anchor, &mut sim, &mut steps);
        }
    }

    // Join the target: its label or star, and the sender learned in.
    let mut keep = None;
    if let Some((rule, anchor)) = target {
        match anchor {
            TabAnchor::Label(l) => {
                keep = Some(l);
                if sim.label_ids.insert(l) {
                    steps.push(MoveStep::AddLabel(l));
                }
            }
            TabAnchor::Starred => {
                if !sim.is_starred {
                    sim.is_starred = true;
                    steps.push(MoveStep::Star(true));
                }
            }
        }
        if let Some(s) = learn {
            edits.edit(rule, |c| learn_sender(c, s));
        }
    }

    // A view tab lists the thread wherever routing has it: nothing to replay.
    let view = target.is_some_and(|(rule, _)| !rule.exclusive);
    let mut by_rule = false;
    if !view {
        // Replay first-match-wins: every routing split ahead of the target
        // that would still claim the thread must let go of it.
        for sp in splits.iter().filter(|s| s.exclusive) {
            if sp.route_key() == to {
                // The target itself, or a rule already routing into the target.
                by_rule = gate(edits.rule(sp), &sim) && sp.ai.is_none();
                if matches!(to, RouteKey::Split(_)) || by_rule {
                    break;
                }
                continue;
            }
            if !gate(edits.rule(sp), &sim) {
                continue;
            }
            // An AI split only holds the thread if it already said yes.
            if sp.ai.is_some() && facts.routed_tab != Some(sp.route_key().to_string().as_str()) {
                continue;
            }
            let Some(anchor) = tab_match(sp) else {
                return Ok(MovePlan::HeldBy(sp.id));
            };
            // A label tab ahead loses its label (unless the target needs it;
            // a star only goes when the drag leaves the Starred tab).
            if let TabAnchor::Label(l) = anchor {
                if Some(l) != keep && sim.label_ids.remove(&l) {
                    steps.push(MoveStep::RemoveLabel(l));
                }
            }
            if gate(edits.rule(sp), &sim) {
                // Still claimed (a learned sender, the target's own label, a
                // star): learn the sender out of it.
                let Some(s) = sender.as_deref() else {
                    return Ok(MovePlan::HeldBy(sp.id));
                };
                edits.edit(sp, |c| exclude_sender(c, s));
                if gate(edits.rule(sp), &sim) {
                    return Ok(MovePlan::HeldBy(sp.id));
                }
            }
        }
    }
    if let (Some((rule, _)), Some(s)) = (leaving, learn) {
        edits.edit(rule, |c| exclude_sender(c, s));
    }
    if view {
        return Ok(MovePlan::Steps(edits.into_steps(steps)));
    }

    // Write the route as well: the replay above shows the rules now agree (or
    // no rule decides, for categories / Important / Other), and a thread whose
    // labels already fit would otherwise keep a stale route.
    if facts.routed_tab != Some(target_key.as_str()) {
        steps.push(MoveStep::Route(to));
    }
    if matches!(to, RouteKey::Split(_)) || by_rule {
        return Ok(MovePlan::Steps(edits.into_steps(steps)));
    }
    let keyword = match to {
        RouteKey::Label(id) => categories
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.keyword.clone()),
        // Out of a category: the sender's later mail stays uncategorized too.
        _ if facts
            .routed_tab
            .and_then(|k| k.strip_prefix("label:"))
            .and_then(|id| id.parse::<i64>().ok())
            .is_some_and(|id| categories.iter().any(|c| c.id == id)) =>
        {
            Some(String::new())
        }
        _ => None,
    };
    if let (Some(keyword), Some(sender)) = (keyword, facts.sender) {
        steps.push(MoveStep::CacheSender {
            key: super::sender_domain(sender),
            keyword,
        });
    }
    Ok(MovePlan::Steps(edits.into_steps(steps)))
}

/// Result of a manual move, with what it takes to put everything back.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveOutcome {
    /// Threads that changed tab.
    pub moved: Vec<i64>,
    /// Threads a rule keeps where they are, with that rule's split id.
    pub held: Vec<HeldThread>,
    pub undo: MoveUndo,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeldThread {
    pub thread_id: i64,
    pub split_id: i64,
}

/// Inverse of a move: labels and stars to take back, routes, sender verdicts
/// and split conditions to restore.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveUndo {
    pub threads: Vec<ThreadUndo>,
    pub senders: Vec<SenderUndo>,
    /// Split conditions as they were before senders were learned into them.
    #[serde(default)]
    pub conditions: Vec<ConditionUndo>,
    /// The senders learned: their other inbox mail is re-checked against the
    /// rules after the move and after its undo.
    #[serde(default)]
    pub learned: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadUndo {
    pub thread_id: i64,
    /// `routed_tab` before the move.
    pub routed_tab: Option<String>,
    pub added_labels: Vec<i64>,
    pub removed_labels: Vec<i64>,
    /// The star before the move, when the move changed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starred: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SenderUndo {
    pub key: String,
    /// The cached verdict before the move; `None` = there was none.
    pub keyword: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConditionUndo {
    pub split_id: i64,
    /// `None` = the split used its legacy query.
    pub condition: Option<RuleCondition>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AiCheck, SplitRuleQuery, TextOp};
    use serde_json::json;

    const ANN: &str = "ann@shop.example";

    fn rule(id: i64, condition: RuleCondition) -> SplitRule {
        SplitRule {
            id,
            name: format!("split {id}"),
            position: id,
            query: SplitRuleQuery::default(),
            condition: Some(condition),
            target: None,
            ai: None,
            count_all: false,
            exclusive: true,
        }
    }

    /// A view tab: lists matching mail without taking it out of the inbox.
    fn view(id: i64, condition: RuleCondition) -> SplitRule {
        SplitRule {
            exclusive: false,
            ..rule(id, condition)
        }
    }

    fn label(id: i64) -> RuleCondition {
        RuleCondition::Label { id }
    }

    fn starred() -> RuleCondition {
        RuleCondition::IsStarred { value: true }
    }

    fn sender_rule(id: i64, value: &str) -> SplitRule {
        rule(
            id,
            RuleCondition::Sender {
                op: TextOp::EndsWith,
                value: value.into(),
            },
        )
    }

    fn subject(labels: &[i64], sender: &str) -> RuleSubject {
        RuleSubject {
            senders: vec![sender.into()],
            label_ids: labels.iter().copied().collect(),
            latest_is_automated: Some(false),
            ..RuleSubject::default()
        }
    }

    fn cats() -> Vec<Category> {
        vec![
            Category {
                id: 101,
                keyword: "ComailAutoMarketing".into(),
            },
            Category {
                id: 102,
                keyword: "ComailAutoNews".into(),
            },
        ]
    }

    /// A thread from `ANN` being dragged.
    #[derive(Default)]
    struct Drag<'a> {
        labels: &'a [i64],
        starred: bool,
        routed: Option<&'a str>,
        from: Option<RouteKey>,
        /// No inbound sender known (nothing to learn).
        anon: bool,
    }

    fn drag(to: RouteKey, d: Drag, splits: &[SplitRule]) -> Result<MovePlan, NotDroppable> {
        let mut s = subject(d.labels, ANN);
        s.is_starred = d.starred;
        plan_move(
            to,
            &MoveFacts {
                subject: &s,
                routed_tab: d.routed,
                sender: (!d.anon).then_some(ANN),
                from: d.from,
            },
            splits,
            &cats(),
        )
    }

    fn plan(
        to: RouteKey,
        labels: &[i64],
        routed: Option<&str>,
        splits: &[SplitRule],
    ) -> Result<MovePlan, NotDroppable> {
        drag(
            to,
            Drag {
                labels,
                routed,
                ..Drag::default()
            },
            splits,
        )
    }

    fn steps(p: Result<MovePlan, NotDroppable>) -> Vec<MoveStep> {
        match p {
            Ok(MovePlan::Steps(s)) => s,
            other => panic!("expected steps, got {other:?}"),
        }
    }

    /// The thread's own changes, without the learned-sender condition saves.
    fn moves(p: Result<MovePlan, NotDroppable>) -> Vec<MoveStep> {
        steps(p)
            .into_iter()
            .filter(|s| !matches!(s, MoveStep::SetCondition { .. }))
            .collect()
    }

    /// The condition saves, as `(split id, condition JSON)`.
    fn saves(p: &Result<MovePlan, NotDroppable>) -> Vec<(i64, serde_json::Value)> {
        let Ok(MovePlan::Steps(s)) = p else {
            panic!("expected steps, got {p:?}");
        };
        s.iter()
            .filter_map(|s| match s {
                MoveStep::SetCondition {
                    split_id,
                    condition,
                } => Some((*split_id, serde_json::to_value(condition).unwrap())),
                _ => None,
            })
            .collect()
    }

    fn ann() -> serde_json::Value {
        json!({ "kind": "sender", "op": "equals", "value": ANN })
    }

    /// `anchor` with ANN learned in.
    fn learned_in(anchor: serde_json::Value) -> serde_json::Value {
        json!({ "kind": "any", "of": [anchor, ann()] })
    }

    /// `anchor` with ANN learned out.
    fn learned_out(anchor: serde_json::Value) -> serde_json::Value {
        json!({ "kind": "all", "of": [
            { "kind": "any", "of": [anchor] },
            { "kind": "not", "cond": { "kind": "any", "of": [ann()] } },
        ] })
    }

    fn label_json(id: i64) -> serde_json::Value {
        json!({ "kind": "label", "id": id })
    }

    fn starred_json() -> serde_json::Value {
        json!({ "kind": "isStarred", "value": true })
    }

    #[test]
    fn label_tabs_are_the_drop_targets() {
        let two_labels = rule(
            2,
            RuleCondition::Any {
                of: vec![label(20), label(21)],
            },
        );
        let mixed = rule(
            3,
            RuleCondition::Any {
                of: vec![
                    label(30),
                    RuleCondition::Sender {
                        op: TextOp::Contains,
                        value: "x".into(),
                    },
                ],
            },
        );
        // A legacy "has label" query reads as a one-child `all`.
        let legacy = SplitRule {
            condition: None,
            query: SplitRuleQuery {
                labels: Some(vec![40]),
                ..SplitRuleQuery::default()
            },
            ..rule(4, label(40))
        };
        let mut ai = rule(5, label(50));
        ai.ai = Some(AiCheck {
            instruction: "is it a receipt".into(),
            min_confidence: None,
        });
        let mut routed = rule(6, label(60));
        routed.target = Some(RouteKey::Label(101));
        let learned = rule(
            8,
            RuleCondition::Any {
                of: vec![
                    label(80),
                    RuleCondition::Sender {
                        op: TextOp::Equals,
                        value: "bob@x.test".into(),
                    },
                ],
            },
        );
        let splits = [
            rule(1, label(10)),
            two_labels,
            mixed,
            legacy,
            ai,
            routed,
            sender_rule(7, "@x.com"),
            learned,
            view(9, starred()),
        ];
        let ok = |id| droppable(RouteKey::Split(id), &splits, &cats());
        assert!(ok(1) && ok(4) && ok(8) && ok(9));
        assert!(!ok(2) && !ok(3) && !ok(5) && !ok(6) && !ok(7) && !ok(99));
        assert!(droppable(RouteKey::Label(101), &splits, &cats()));
        assert!(!droppable(RouteKey::Label(999), &splits, &cats()));
        assert!(droppable(RouteKey::Important, &splits, &cats()));
        assert!(!droppable(RouteKey::Pending, &splits, &cats()));
        assert_eq!(
            plan(RouteKey::Split(7), &[], None, &splits),
            Err(NotDroppable)
        );
    }

    #[test]
    fn joining_a_label_tab_adds_its_label_and_drops_earlier_tab_labels() {
        let splits = [rule(1, label(10)), rule(2, label(20)), rule(3, label(30))];
        // In tab 1 (label 10), also labelled 20: both would beat tab 3.
        assert_eq!(
            moves(plan(
                RouteKey::Split(3),
                &[10, 20],
                Some("split:1"),
                &splits
            )),
            vec![
                MoveStep::AddLabel(30),
                MoveStep::RemoveLabel(10),
                MoveStep::RemoveLabel(20),
                MoveStep::Route(RouteKey::Split(3)),
            ]
        );
        // Moving up: later tabs' labels don't compete and stay.
        assert_eq!(
            moves(plan(RouteKey::Split(1), &[30], Some("split:3"), &splits)),
            vec![MoveStep::AddLabel(10), MoveStep::Route(RouteKey::Split(1))]
        );
    }

    #[test]
    fn a_drop_learns_the_sender_into_the_target_and_out_of_the_source() {
        let splits = [rule(1, label(10)), rule(2, label(20))];
        let p = plan(RouteKey::Split(2), &[10], Some("split:1"), &splits);
        assert_eq!(
            saves(&p),
            vec![
                (2, learned_in(label_json(20))),
                (1, learned_out(label_json(10))),
            ]
        );
        assert_eq!(
            moves(p),
            vec![
                MoveStep::AddLabel(20),
                MoveStep::RemoveLabel(10),
                MoveStep::Route(RouteKey::Split(2)),
            ]
        );
        // Learned already: nothing to save again.
        let splits = [rule(
            2,
            serde_json::from_value(learned_in(label_json(20))).unwrap(),
        )];
        assert_eq!(
            steps(plan(RouteKey::Split(2), &[], None, &splits)),
            vec![MoveStep::AddLabel(20), MoveStep::Route(RouteKey::Split(2))]
        );
        // Categories and Important learn nothing.
        let splits = [rule(1, label(10))];
        assert!(saves(&plan(RouteKey::Label(101), &[10], Some("split:1"), &splits)).is_empty());
        assert!(saves(&plan(RouteKey::Important, &[10], Some("split:1"), &splits)).is_empty());
    }

    #[test]
    fn a_view_tab_drop_adds_the_label_and_keeps_the_route() {
        let splits = [rule(1, label(10)), view(5, label(50))];
        let from_important = Drag {
            labels: &[77],
            routed: Some("important"),
            from: Some(RouteKey::Important),
            ..Drag::default()
        };
        let p = drag(RouteKey::Split(5), from_important, &splits);
        assert_eq!(saves(&p), vec![(5, learned_in(label_json(50)))]);
        // No route, and label 77 stays.
        assert_eq!(moves(p), vec![MoveStep::AddLabel(50)]);

        // From an exclusive tab: its label stays, the learned exclude moves it.
        let p = plan(RouteKey::Split(5), &[10], Some("split:1"), &splits);
        assert_eq!(
            saves(&p),
            vec![
                (5, learned_in(label_json(50))),
                (1, learned_out(label_json(10))),
            ]
        );
        assert_eq!(moves(p), vec![MoveStep::AddLabel(50)]);

        // Already listed there.
        assert_eq!(
            plan(RouteKey::Split(5), &[50], Some("important"), &splits),
            Ok(MovePlan::AlreadyThere)
        );
    }

    #[test]
    fn dragging_out_of_a_view_tab_takes_only_its_label() {
        let splits = [rule(1, label(10)), view(5, label(50))];
        let out_of_view = || Drag {
            labels: &[50, 77],
            routed: Some("important"),
            from: Some(RouteKey::Split(5)),
            ..Drag::default()
        };
        assert_eq!(
            steps(drag(RouteKey::Important, out_of_view(), &splits)),
            vec![MoveStep::RemoveLabel(50)]
        );
        let p = drag(RouteKey::Split(1), out_of_view(), &splits);
        assert_eq!(
            saves(&p),
            vec![
                (1, learned_in(label_json(10))),
                (5, learned_out(label_json(50))),
            ]
        );
        assert_eq!(
            moves(p),
            vec![
                MoveStep::RemoveLabel(50),
                MoveStep::AddLabel(10),
                MoveStep::Route(RouteKey::Split(1)),
            ]
        );
    }

    #[test]
    fn the_starred_tab_stars_and_dragging_out_of_it_unstars() {
        let splits = [view(6, starred()), rule(2, label(20))];
        let p = drag(
            RouteKey::Split(6),
            Drag {
                routed: Some("important"),
                ..Drag::default()
            },
            &splits,
        );
        assert_eq!(saves(&p), vec![(6, learned_in(starred_json()))]);
        assert_eq!(moves(p), vec![MoveStep::Star(true)]);

        let starred_in = |from| Drag {
            starred: true,
            routed: Some("important"),
            from: Some(from),
            ..Drag::default()
        };
        let p = drag(RouteKey::Split(2), starred_in(RouteKey::Split(6)), &splits);
        assert_eq!(
            saves(&p),
            vec![
                (2, learned_in(label_json(20))),
                (6, learned_out(starred_json())),
            ]
        );
        assert_eq!(
            moves(p),
            vec![
                MoveStep::Star(false),
                MoveStep::AddLabel(20),
                MoveStep::Route(RouteKey::Split(2)),
            ]
        );
        assert_eq!(
            steps(drag(
                RouteKey::Important,
                starred_in(RouteKey::Split(6)),
                &splits
            )),
            vec![MoveStep::Star(false)]
        );
        // Only leaving the Starred tab unstars.
        assert_eq!(
            moves(drag(
                RouteKey::Split(2),
                starred_in(RouteKey::Important),
                &splits
            )),
            vec![MoveStep::AddLabel(20), MoveStep::Route(RouteKey::Split(2))]
        );

        // An exclusive Starred tab the thread is routed to.
        let splits = [rule(6, starred()), rule(2, label(20))];
        let routed_starred = Drag {
            starred: true,
            routed: Some("split:6"),
            ..Drag::default()
        };
        assert_eq!(
            moves(drag(RouteKey::Split(2), routed_starred, &splits)),
            vec![
                MoveStep::Star(false),
                MoveStep::AddLabel(20),
                MoveStep::Route(RouteKey::Split(2)),
            ]
        );
    }

    #[test]
    fn a_learned_sender_holding_the_thread_is_learned_out() {
        let holder = rule(
            1,
            serde_json::from_value(learned_in(label_json(10))).unwrap(),
        );
        let splits = [holder, rule(2, label(20))];
        let p = plan(RouteKey::Split(2), &[], Some("split:1"), &splits);
        assert_eq!(
            saves(&p),
            vec![
                (2, learned_in(label_json(20))),
                (1, learned_out(label_json(10))),
            ]
        );
        assert_eq!(
            moves(p),
            vec![MoveStep::AddLabel(20), MoveStep::Route(RouteKey::Split(2))]
        );
        // Important learns nothing, but the holder still has to let go.
        let p = plan(RouteKey::Important, &[], Some("split:1"), &splits);
        assert_eq!(saves(&p), vec![(1, learned_out(label_json(10)))]);
        assert_eq!(moves(p), vec![MoveStep::Route(RouteKey::Important)]);
        // Without a sender to learn, it holds.
        let anon = Drag {
            routed: Some("split:1"),
            anon: true,
            ..Drag::default()
        };
        assert_eq!(
            drag(RouteKey::Split(2), anon, &splits),
            Ok(MovePlan::HeldBy(1))
        );
    }

    #[test]
    fn already_there_is_a_no_op() {
        let splits = [rule(1, label(10))];
        assert_eq!(
            plan(RouteKey::Split(1), &[10], Some("split:1"), &splits),
            Ok(MovePlan::AlreadyThere)
        );
        assert_eq!(
            plan(RouteKey::Label(101), &[], Some("label:101"), &splits),
            Ok(MovePlan::AlreadyThere)
        );
    }

    #[test]
    fn a_sender_rule_ahead_holds_the_thread() {
        let splits = [sender_rule(1, "@shop.example"), rule(2, label(20))];
        assert_eq!(
            plan(RouteKey::Split(2), &[], Some("split:1"), &splits),
            Ok(MovePlan::HeldBy(1))
        );
        assert_eq!(
            plan(RouteKey::Important, &[], Some("split:1"), &splits),
            Ok(MovePlan::HeldBy(1))
        );
        // Behind the target it doesn't matter.
        let splits = [rule(2, label(20)), sender_rule(1, "@shop.example")];
        assert_eq!(
            moves(plan(RouteKey::Split(2), &[], Some("split:1"), &splits)),
            vec![MoveStep::AddLabel(20), MoveStep::Route(RouteKey::Split(2))]
        );
    }

    #[test]
    fn an_ai_split_holds_only_after_saying_yes() {
        let mut ai = sender_rule(1, "@shop.example");
        ai.ai = Some(AiCheck {
            instruction: "is it a deal".into(),
            min_confidence: None,
        });
        let splits = [ai, rule(2, label(20))];
        assert_eq!(
            moves(plan(RouteKey::Split(2), &[], None, &splits)),
            vec![MoveStep::AddLabel(20), MoveStep::Route(RouteKey::Split(2))]
        );
        assert_eq!(
            plan(RouteKey::Split(2), &[], Some("split:1"), &splits),
            Ok(MovePlan::HeldBy(1))
        );
    }

    #[test]
    fn target_label_feeding_an_earlier_split_is_held() {
        let splits = [
            rule(
                1,
                RuleCondition::Any {
                    of: vec![label(10), label(20)],
                },
            ),
            rule(2, label(20)),
        ];
        assert_eq!(
            plan(RouteKey::Split(2), &[], None, &splits),
            Ok(MovePlan::HeldBy(1))
        );
        // An earlier tab of the same label learns the sender out instead.
        let splits = [rule(1, label(20)), rule(2, label(20))];
        let p = plan(RouteKey::Split(2), &[], None, &splits);
        assert_eq!(
            saves(&p),
            vec![
                (2, learned_in(label_json(20))),
                (1, learned_out(label_json(20))),
            ]
        );
    }

    #[test]
    fn category_move_routes_and_caches_the_sender() {
        let splits = [rule(1, label(10))];
        assert_eq!(
            steps(plan(RouteKey::Label(102), &[10], Some("split:1"), &splits)),
            vec![
                MoveStep::RemoveLabel(10),
                MoveStep::Route(RouteKey::Label(102)),
                MoveStep::CacheSender {
                    key: "shop.example".into(),
                    keyword: "ComailAutoNews".into()
                },
            ]
        );
    }

    #[test]
    fn a_rule_routing_into_the_target_leaves_the_sender_verdict_alone() {
        let mut into_news = sender_rule(1, "@shop.example");
        into_news.target = Some(RouteKey::Label(102));
        assert_eq!(
            steps(plan(
                RouteKey::Label(102),
                &[],
                Some("pending"),
                &[into_news]
            )),
            vec![MoveStep::Route(RouteKey::Label(102))]
        );
    }

    #[test]
    fn important_drops_the_tab_label_or_the_category() {
        let splits = [rule(1, label(10))];
        assert_eq!(
            steps(plan(
                RouteKey::Important,
                &[10, 77],
                Some("split:1"),
                &splits
            )),
            vec![
                MoveStep::RemoveLabel(10),
                MoveStep::Route(RouteKey::Important)
            ]
        );
        // From a category: the sender's verdict becomes "no category".
        assert_eq!(
            steps(plan(
                RouteKey::Important,
                &[101],
                Some("label:101"),
                &splits
            )),
            vec![
                MoveStep::Route(RouteKey::Important),
                MoveStep::CacheSender {
                    key: "shop.example".into(),
                    keyword: String::new()
                },
            ]
        );
        assert_eq!(
            steps(plan(RouteKey::Other, &[], None, &splits)),
            vec![MoveStep::Route(RouteKey::Other)]
        );
    }

    #[test]
    fn shared_mailbox_senders_cache_by_full_address() {
        let s = subject(&[], "pal@gmail.com");
        let p = plan_move(
            RouteKey::Label(101),
            &MoveFacts {
                subject: &s,
                routed_tab: None,
                sender: Some("pal@gmail.com"),
                from: None,
            },
            &[],
            &cats(),
        );
        assert_eq!(
            steps(p),
            vec![
                MoveStep::Route(RouteKey::Label(101)),
                MoveStep::CacheSender {
                    key: "pal@gmail.com".into(),
                    keyword: "ComailAutoMarketing".into()
                },
            ]
        );
    }
}
