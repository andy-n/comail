//! Manual moves between inbox tabs: mail dragged onto a tab, bundle or sidebar
//! row. [`plan_move`] is pure: from one thread's rule facts and the tab
//! definitions it lists the steps that make the thread land in the target tab,
//! replaying the same first-match-wins evaluation routing uses
//! ([`super::decide`]) on a simulated copy of the thread. `Core` executes the
//! steps through the normal label-action path (IMAP keyword sync, reroute).
//!
//! Extension points for follow-ups:
//! - a rule that a drop can satisfy or undo is a [`Claim`]; another kind of
//!   claim (e.g. an `isStarred` condition: drop = star, drag out = unstar) is a
//!   new variant in [`claim_of`] plus its join/release steps in [`plan_move`];
//! - a rule a drop can't undo (a sender or subject rule) yields
//!   [`MovePlan::HeldBy`], and a non-claim split target is refused with
//!   [`NotDroppable`]. Learning the sender ("sender equals <from>" appended to
//!   the target, excluded from the holder) replaces those two outcomes with
//!   condition-edit steps, using [`MoveFacts::sender`].

use super::decide::{RuleSubject, gate};
use crate::models::{RouteKey, RuleCondition, SplitRule};
use serde::{Deserialize, Serialize};

/// What puts a thread into a split's tab by hand, and what takes it out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Claim {
    /// The split matches threads carrying any of these user labels.
    Labels(Vec<i64>),
}

/// Label ids of a condition made only of labels: a bare `label`, or an `any`
/// of labels, also inside a one-child `all` (how a legacy label query reads).
/// Anything else (sender, subject, nested logic) is `None`.
fn label_only(cond: &RuleCondition) -> Option<Vec<i64>> {
    match cond {
        RuleCondition::All { of } if of.len() == 1 => label_only(&of[0]),
        RuleCondition::Label { id } => Some(vec![*id]),
        RuleCondition::Any { of } if !of.is_empty() => of
            .iter()
            .map(|c| match c {
                RuleCondition::Label { id } => Some(*id),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

/// How a drop can satisfy or undo this split, if at all. Splits with an AI
/// check never qualify: their verdict isn't the drop's to make.
pub fn claim_of(rule: &SplitRule) -> Option<Claim> {
    if rule.ai.is_some() {
        return None;
    }
    label_only(&rule.effective_condition()).map(Claim::Labels)
}

/// Whether mail can be dropped onto the tab `to` at all. Mirrors the UI's
/// drop-target check (`src/lib/tabDrop.ts`).
pub fn droppable(to: RouteKey, splits: &[SplitRule], categories: &[Category]) -> bool {
    match to {
        RouteKey::Important | RouteKey::Other => true,
        RouteKey::Label(id) => categories.iter().any(|c| c.id == id),
        RouteKey::Split(id) => splits
            .iter()
            .find(|s| s.id == id)
            .is_some_and(|s| s.target.is_none() && claim_of(s).is_some()),
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
    /// Rule facts, labels included (the planner simulates label changes on a copy).
    pub subject: &'a RuleSubject,
    /// `threads.routed_tab` as stored.
    pub routed_tab: Option<&'a str>,
    /// Lowercased From of the newest inbound message; `None` without one.
    pub sender: Option<&'a str>,
}

/// One change that moves a thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveStep {
    AddLabel(i64),
    RemoveLabel(i64),
    /// Write the route directly (categories, Important, Other): no split rule
    /// decides those.
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
    let target = to.to_string();
    if facts.routed_tab == Some(target.as_str()) {
        return Ok(MovePlan::AlreadyThere);
    }

    let mut sim = facts.subject.clone();
    let mut steps = Vec::new();
    // Joining a split tab: satisfy its claim.
    let mut keep: Option<i64> = None;
    if let RouteKey::Split(id) = to {
        let rule = splits.iter().find(|s| s.id == id).ok_or(NotDroppable)?;
        match claim_of(rule).ok_or(NotDroppable)? {
            Claim::Labels(ids) => {
                if !ids.iter().any(|l| sim.label_ids.contains(l)) {
                    steps.push(MoveStep::AddLabel(ids[0]));
                    sim.label_ids.insert(ids[0]);
                }
                keep = ids.iter().copied().find(|l| sim.label_ids.contains(l));
            }
        }
    }

    // Replay first-match-wins: every split ahead of the target that would
    // still claim the thread must let go of it.
    let mut by_rule = false;
    for sp in splits {
        if sp.route_key() == to {
            // The target itself, or a rule already routing into the target.
            by_rule = gate(sp, &sim) && sp.ai.is_none();
            if matches!(to, RouteKey::Split(_)) || by_rule {
                break;
            }
            continue;
        }
        if !gate(sp, &sim) {
            continue;
        }
        // An AI split only holds the thread if it already said yes.
        if sp.ai.is_some() && facts.routed_tab != Some(sp.route_key().to_string().as_str()) {
            continue;
        }
        match claim_of(sp) {
            Some(Claim::Labels(ids)) => {
                for l in ids {
                    if Some(l) == keep {
                        // The target's own label also feeds this earlier split.
                        return Ok(MovePlan::HeldBy(sp.id));
                    }
                    if sim.label_ids.remove(&l) {
                        steps.push(MoveStep::RemoveLabel(l));
                    }
                }
            }
            None => return Ok(MovePlan::HeldBy(sp.id)),
        }
    }

    // Write the route as well: the replay above shows the rules now agree (or
    // no rule decides, for categories / Important / Other), and a thread whose
    // labels already fit would otherwise keep a stale route.
    steps.push(MoveStep::Route(to));
    if matches!(to, RouteKey::Split(_)) || by_rule {
        return Ok(MovePlan::Steps(steps));
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
    Ok(MovePlan::Steps(steps))
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

/// Inverse of a move: labels to take back, routes and sender verdicts to restore.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveUndo {
    pub threads: Vec<ThreadUndo>,
    pub senders: Vec<SenderUndo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadUndo {
    pub thread_id: i64,
    /// `routed_tab` before the move.
    pub routed_tab: Option<String>,
    pub added_labels: Vec<i64>,
    pub removed_labels: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SenderUndo {
    pub key: String,
    /// The cached verdict before the move; `None` = there was none.
    pub keyword: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AiCheck, SplitRuleQuery, TextOp};

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
        }
    }

    fn label(id: i64) -> RuleCondition {
        RuleCondition::Label { id }
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

    fn plan(
        to: RouteKey,
        labels: &[i64],
        routed: Option<&str>,
        splits: &[SplitRule],
    ) -> Result<MovePlan, NotDroppable> {
        let s = subject(labels, "ann@shop.example");
        plan_move(
            to,
            &MoveFacts {
                subject: &s,
                routed_tab: routed,
                sender: Some("ann@shop.example"),
            },
            splits,
            &cats(),
        )
    }

    fn steps(p: Result<MovePlan, NotDroppable>) -> Vec<MoveStep> {
        match p {
            Ok(MovePlan::Steps(s)) => s,
            other => panic!("expected steps, got {other:?}"),
        }
    }

    #[test]
    fn label_tabs_accept_bare_and_any_of_labels_only() {
        let any = rule(
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
        let splits = [
            rule(1, label(10)),
            any,
            mixed,
            legacy,
            ai,
            routed,
            sender_rule(7, "@x.com"),
        ];
        let ok = |id| droppable(RouteKey::Split(id), &splits, &cats());
        assert!(ok(1) && ok(2) && ok(4));
        assert!(!ok(3) && !ok(5) && !ok(6) && !ok(7) && !ok(99));
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
            steps(plan(
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
            steps(plan(RouteKey::Split(1), &[30], Some("split:3"), &splits)),
            vec![MoveStep::AddLabel(10), MoveStep::Route(RouteKey::Split(1))]
        );
    }

    #[test]
    fn any_of_tab_reuses_a_label_the_thread_already_has() {
        let splits = [
            rule(1, label(10)),
            rule(
                2,
                RuleCondition::Any {
                    of: vec![label(20), label(21)],
                },
            ),
        ];
        assert_eq!(
            steps(plan(
                RouteKey::Split(2),
                &[21, 10],
                Some("split:1"),
                &splits
            )),
            vec![
                MoveStep::RemoveLabel(10),
                MoveStep::Route(RouteKey::Split(2))
            ]
        );
        assert_eq!(
            steps(plan(RouteKey::Split(2), &[], None, &splits)),
            vec![MoveStep::AddLabel(20), MoveStep::Route(RouteKey::Split(2))]
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
            steps(plan(RouteKey::Split(2), &[], Some("split:1"), &splits)),
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
            steps(plan(RouteKey::Split(2), &[], None, &splits)),
            vec![MoveStep::AddLabel(20), MoveStep::Route(RouteKey::Split(2))]
        );
        assert_eq!(
            plan(RouteKey::Split(2), &[], Some("split:1"), &splits),
            Ok(MovePlan::HeldBy(1))
        );
    }

    #[test]
    fn target_label_feeding_an_earlier_tab_is_held() {
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
