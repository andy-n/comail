//! `apply_tab_move` / `revert_tab_move` against a real schema: label actions
//! are queued for IMAP, routes and sender verdicts are written, undo restores.

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
    marketing: i64,
}

fn label_split(c: &Connection, name: &str, position: i64, label: i64) -> i64 {
    repo::splits::save(
        c,
        None,
        name,
        position,
        &SplitRuleQuery::default(),
        Some(&RuleCondition::Label { id: label }),
        None,
        None,
        false,
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
    let finance_tab = label_split(&conn, "Finance", 0, finance);
    let travel_tab = label_split(&conn, "Travel", 1, travel);
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

/// Queued IMAP label actions for a thread as (kind, keyword).
fn queued(c: &Connection, tid: i64) -> Vec<(String, String)> {
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
    let t = thread(&f.conn, "bank@acme.test", &[f.finance], Some(&finance_key));

    let (out, accounts) = apply_tab_move(&mut f.conn, &[t], RouteKey::Split(f.travel_tab)).unwrap();
    assert_eq!(out.moved, vec![t]);
    assert_eq!(accounts, vec![1]);
    assert_eq!(labels_of(&f.conn, t), vec![f.travel]);
    assert_eq!(routed(&f.conn, t), Some(format!("split:{}", f.travel_tab)));
    assert_eq!(
        queued(&f.conn, t),
        vec![
            ("add_label".into(), "Travel".into()),
            ("remove_label".into(), "Finance".into())
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

    let (out, _) = apply_tab_move(&mut f.conn, &[t], RouteKey::Label(f.marketing)).unwrap();
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
    let (out, _) = apply_tab_move(&mut f.conn, &[t], RouteKey::Important).unwrap();
    assert!(labels_of(&f.conn, t).is_empty());
    assert_eq!(routed(&f.conn, t).as_deref(), Some("important"));
    // Undo restores both labels, not only the one the tab showed.
    revert_tab_move(&mut f.conn, &out.undo).unwrap();
    assert_eq!(labels_of(&f.conn, t), vec![f.finance, f.travel]);
}

#[test]
fn a_sender_rule_holds_its_mail_and_sender_tabs_refuse_drops() {
    let mut f = fixture();
    let github = repo::splits::save(
        &f.conn,
        None,
        "GitHub",
        -1,
        &SplitRuleQuery::default(),
        Some(&RuleCondition::Sender {
            op: TextOp::EndsWith,
            value: "@github.test".into(),
        }),
        None,
        None,
        false,
    )
    .unwrap()
    .id;
    let held = thread(
        &f.conn,
        "bot@github.test",
        &[],
        Some(&format!("split:{github}")),
    );
    let free = thread(&f.conn, "pal@acme.test", &[], None);

    let (out, _) =
        apply_tab_move(&mut f.conn, &[held, free], RouteKey::Split(f.travel_tab)).unwrap();
    assert_eq!(out.moved, vec![free]);
    assert_eq!(out.held.len(), 1);
    assert_eq!(
        (out.held[0].thread_id, out.held[0].split_id),
        (held, github)
    );
    assert!(labels_of(&f.conn, held).is_empty());

    assert!(apply_tab_move(&mut f.conn, &[free], RouteKey::Split(github)).is_err());
}
