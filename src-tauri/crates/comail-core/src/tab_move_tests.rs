//! `apply_tab_move` / `revert_tab_move` against a real schema: label and star
//! actions are queued for IMAP, routes and sender verdicts are written, undo
//! restores.

use super::*;
use crate::db::testutil;
use crate::models::{RouteKey, RuleCondition, SplitRuleQuery, TextOp};
use rusqlite::{Connection, params};

struct Fixture {
    conn: Connection,
    finance: i64,
    travel: i64,
    finance_tab: i64,
    travel_tab: i64,
    /// A view tab of starred mail (kept in the inbox).
    starred_tab: i64,
    marketing: i64,
}

fn split(c: &Connection, name: &str, position: i64, cond: &RuleCondition, exclusive: bool) -> i64 {
    repo::splits::save(
        c,
        None,
        name,
        position,
        &SplitRuleQuery::default(),
        Some(cond),
        None,
        None,
        false,
        exclusive,
    )
    .unwrap()
    .id
}

fn fixture() -> Fixture {
    let conn = testutil::conn();
    testutil::seed_account(&conn);
    let finance = repo::labels::save(&conn, None, "Finance", "#111111", 0)
        .unwrap()
        .id;
    let travel = repo::labels::save(&conn, None, "Travel", "#222222", 1)
        .unwrap()
        .id;
    let finance_tab = split(
        &conn,
        "Finance",
        0,
        &RuleCondition::Label { id: finance },
        true,
    );
    let travel_tab = split(
        &conn,
        "Travel",
        1,
        &RuleCondition::Label { id: travel },
        true,
    );
    let starred_tab = split(
        &conn,
        "Starred",
        2,
        &RuleCondition::IsStarred { value: true },
        false,
    );
    let marketing = conn
        .query_row(
            "SELECT id FROM labels WHERE keyword = 'ComailAutoMarketing'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    Fixture {
        conn,
        finance,
        travel,
        finance_tab,
        travel_tab,
        starred_tab,
        marketing,
    }
}

/// A thread from `from`, carrying `labels`, stored in tab `routed`.
fn thread(c: &Connection, from: &str, labels: &[i64], routed: Option<&str>) -> i64 {
    let (tid, mid) = testutil::seed_message(c, from, "hello", false);
    for &l in labels {
        repo::labels::add_to_message(c, mid, l).unwrap();
    }
    c.execute(
        "UPDATE threads SET routed_tab = ?2 WHERE id = ?1",
        params![tid, routed],
    )
    .unwrap();
    tid
}

fn routed(c: &Connection, tid: i64) -> Option<String> {
    c.query_row(
        "SELECT routed_tab FROM threads WHERE id = ?1",
        params![tid],
        |r| r.get(0),
    )
    .unwrap()
}

fn labels_of(c: &Connection, tid: i64) -> Vec<i64> {
    let mut v: Vec<i64> = c
        .prepare(
            "SELECT DISTINCT ml.label_id FROM message_labels ml
             JOIN messages m ON m.id = ml.message_id WHERE m.thread_id = ?1",
        )
        .unwrap()
        .query_map(params![tid], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    v.sort();
    v
}

fn is_starred(c: &Connection, tid: i64) -> bool {
    route::RuleSubject::load(c, tid).unwrap().is_starred
}

/// Queued IMAP actions for a thread as (kind, label keyword).
fn queued(c: &Connection, tid: i64) -> Vec<(String, Option<String>)> {
    c.prepare(
        "SELECT kind, json_extract(payload, '$.keyword') FROM pending_actions
         WHERE thread_id = ?1 ORDER BY id",
    )
    .unwrap()
    .query_map(params![tid], |r| Ok((r.get(0)?, r.get(1)?)))
    .unwrap()
    .collect::<rusqlite::Result<_>>()
    .unwrap()
}

fn action(kind: &str, keyword: Option<&str>) -> (String, Option<String>) {
    (kind.into(), keyword.map(Into::into))
}

fn cached(c: &Connection, key: &str) -> Option<String> {
    c.query_row(
        "SELECT route_key FROM route_cache WHERE sender_domain = ?1",
        params![key],
        |r| r.get(0),
    )
    .optional()
    .unwrap()
}

#[test]
fn moving_between_label_tabs_swaps_the_labels_through_imap_actions() {
    let mut f = fixture();
    let finance_key = format!("split:{}", f.finance_tab);
    let t = thread(&f.conn, "Bank@Acme.test", &[f.finance], Some(&finance_key));

    let (out, accounts) =
        apply_tab_move(&mut f.conn, &[t], RouteKey::Split(f.travel_tab), None).unwrap();
    assert_eq!(out.moved, vec![t]);
    assert_eq!(accounts, vec![1]);
    assert_eq!(labels_of(&f.conn, t), vec![f.travel]);
    assert_eq!(routed(&f.conn, t), Some(format!("split:{}", f.travel_tab)));
    assert_eq!(
        queued(&f.conn, t),
        vec![
            action("add_label", Some("Travel")),
            action("remove_label", Some("Finance")),
        ]
    );
    let (touched, _) = revert_tab_move(&mut f.conn, &out.undo).unwrap();
    assert_eq!(touched, vec![t]);
    assert_eq!(labels_of(&f.conn, t), vec![f.finance]);
    assert_eq!(routed(&f.conn, t), Some(finance_key));
}

#[test]
fn moving_into_a_category_routes_and_teaches_the_sender() {
    let mut f = fixture();
    f.conn
        .execute(
            "INSERT INTO route_cache (sender_domain, route_key) VALUES ('acme.test', '')",
            [],
        )
        .unwrap();
    let t = thread(&f.conn, "deals@acme.test", &[], None);

    let (out, _) = apply_tab_move(&mut f.conn, &[t], RouteKey::Label(f.marketing), None).unwrap();
    assert_eq!(routed(&f.conn, t), Some(format!("label:{}", f.marketing)));
    assert_eq!(labels_of(&f.conn, t), vec![f.marketing]);
    assert_eq!(
        cached(&f.conn, "acme.test").as_deref(),
        Some("ComailAutoMarketing")
    );
    // Categories are local-only: nothing goes to IMAP.
    assert!(queued(&f.conn, t).is_empty());

    revert_tab_move(&mut f.conn, &out.undo).unwrap();
    assert_eq!(routed(&f.conn, t), None);
    assert!(labels_of(&f.conn, t).is_empty());
    assert_eq!(cached(&f.conn, "acme.test").as_deref(), Some(""));
}

#[test]
fn moving_to_important_drops_the_tab_label_and_pins_the_route() {
    let mut f = fixture();
    let t = thread(
        &f.conn,
        "friend@acme.test",
        &[f.finance, f.travel],
        Some(&format!("split:{}", f.finance_tab)),
    );
    let (out, _) = apply_tab_move(&mut f.conn, &[t], RouteKey::Important, None).unwrap();
    assert!(labels_of(&f.conn, t).is_empty());
    assert_eq!(routed(&f.conn, t).as_deref(), Some("important"));
    // Undo restores both labels, not only the one the tab showed.
    revert_tab_move(&mut f.conn, &out.undo).unwrap();
    assert_eq!(labels_of(&f.conn, t), vec![f.finance, f.travel]);
}

#[test]
fn a_sender_rule_holds_its_mail_and_sender_tabs_refuse_drops() {
    let mut f = fixture();
    let github = split(
        &f.conn,
        "GitHub",
        -1,
        &RuleCondition::Sender {
            op: TextOp::EndsWith,
            value: "@github.test".into(),
        },
        true,
    );
    let held = thread(
        &f.conn,
        "bot@github.test",
        &[],
        Some(&format!("split:{github}")),
    );
    let free = thread(&f.conn, "pal@acme.test", &[], None);

    let (out, _) = apply_tab_move(
        &mut f.conn,
        &[held, free],
        RouteKey::Split(f.travel_tab),
        None,
    )
    .unwrap();
    assert_eq!(out.moved, vec![free]);
    assert_eq!(out.held.len(), 1);
    assert_eq!(
        (out.held[0].thread_id, out.held[0].split_id),
        (held, github)
    );
    assert!(labels_of(&f.conn, held).is_empty());

    assert!(apply_tab_move(&mut f.conn, &[free], RouteKey::Split(github), None).is_err());
}

#[test]
fn a_view_tab_drop_stars_and_keeps_the_thread_in_important() {
    let mut f = fixture();
    // A label no tab shows: the drop keeps it.
    let misc = repo::labels::save(&f.conn, None, "Misc", "#333333", 2)
        .unwrap()
        .id;
    let t = thread(&f.conn, "pal@acme.test", &[misc], Some("important"));

    let (out, _) = apply_tab_move(
        &mut f.conn,
        &[t],
        RouteKey::Split(f.starred_tab),
        Some(RouteKey::Important),
    )
    .unwrap();
    assert_eq!(out.moved, vec![t]);
    assert!(is_starred(&f.conn, t));
    assert_eq!(routed(&f.conn, t).as_deref(), Some("important"));
    assert_eq!(labels_of(&f.conn, t), vec![misc]);
    assert_eq!(queued(&f.conn, t), vec![action("star", None)]);

    revert_tab_move(&mut f.conn, &out.undo).unwrap();
    assert!(!is_starred(&f.conn, t));
    assert_eq!(routed(&f.conn, t).as_deref(), Some("important"));
}

#[test]
fn dragging_out_of_the_starred_tab_unstars_and_undo_restars() {
    let mut f = fixture();
    let t = thread(&f.conn, "pal@acme.test", &[], Some("important"));
    f.conn
        .execute(
            "UPDATE messages SET is_starred = 1 WHERE thread_id = ?1",
            [t],
        )
        .unwrap();

    let (out, _) = apply_tab_move(
        &mut f.conn,
        &[t],
        RouteKey::Split(f.travel_tab),
        Some(RouteKey::Split(f.starred_tab)),
    )
    .unwrap();
    assert!(!is_starred(&f.conn, t));
    assert_eq!(labels_of(&f.conn, t), vec![f.travel]);
    assert_eq!(routed(&f.conn, t), Some(format!("split:{}", f.travel_tab)));

    revert_tab_move(&mut f.conn, &out.undo).unwrap();
    assert!(is_starred(&f.conn, t));
    assert!(labels_of(&f.conn, t).is_empty());
    assert_eq!(routed(&f.conn, t).as_deref(), Some("important"));
}
