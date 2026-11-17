//! A [`RuleCondition`] compiled to a SQL predicate over one thread id, so a
//! view tab (a split with `exclusive == false`) can be listed, counted and
//! bundled in SQL instead of loading every inbox thread into memory.
//!
//! Each leaf mirrors [`super::decide::eval`] over the facts
//! [`super::decide::RuleSubject::load`] reads: the same messages (inbound only
//! for sender/subject/automated, non-drafts for attachments, visible mail
//! outside Trash/Spam for stars) and the same normalization, through the
//! `comail_*` SQL functions [`crate::db::register_functions`] installs on every
//! connection (address normalization incl. IDN, accent folding and Unicode
//! lowercasing are not expressible with SQLite's ASCII-only built-ins). Every
//! fragment evaluates to 0 or 1, never NULL, so `NOT` keeps two-valued logic.

use super::decide::{SubjectNeedle, fold_subject_needle};
use crate::models::{RuleCondition, TextOp};
use rusqlite::types::ToSql;

/// Positional parameters collected while compiling, `?N` numbered in order.
pub type Binds = Vec<Box<dyn ToSql>>;

fn param(bind: &mut Binds, v: impl ToSql + 'static) -> String {
    bind.push(Box::new(v));
    format!("?{}", bind.len())
}

/// `hay <op> needle` on already-normalized text, with Rust's `str` semantics.
fn text_op(op: TextOp, hay: &str, p: &str) -> String {
    match op {
        TextOp::Contains => format!("instr({hay}, {p}) > 0"),
        TextOp::Equals => format!("{hay} = {p}"),
        TextOp::StartsWith => format!("substr({hay}, 1, length({p})) = {p}"),
        TextOp::EndsWith => format!("substr({hay}, -length({p})) = {p}"),
    }
}

/// An inbound (non-draft, non-outgoing) message of thread `tid` passes `pred`.
fn any_inbound(tid: &str, pred: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM messages rm WHERE rm.thread_id = {tid}
                 AND rm.is_draft = 0 AND rm.is_outgoing = 0 AND {pred})"
    )
}

/// Normalized To/Cc address list of `rm`, one per line (`comail_addrs`).
const RECIPIENT_LISTS: [&str; 2] = [
    "comail_addrs(COALESCE(rm.to_json, '[]'))",
    "comail_addrs(COALESCE(rm.cc_json, '[]'))",
];

/// One address of a newline-joined list matches: the list is wrapped in
/// newlines so each op can anchor on address boundaries.
fn list_op(op: TextOp, list: &str, p: &str) -> String {
    let nl = "char(10)";
    match op {
        TextOp::Contains => format!("instr({list}, {p}) > 0"),
        TextOp::Equals => format!("instr({nl} || {list} || {nl}, {nl} || {p} || {nl}) > 0"),
        TextOp::StartsWith => format!("instr({nl} || {list}, {nl} || {p}) > 0"),
        TextOp::EndsWith => format!("instr({list} || {nl}, {p} || {nl}) > 0"),
    }
}

fn leaf(c: &RuleCondition, tid: &str, bind: &mut Binds) -> String {
    match c {
        RuleCondition::All { of } if of.is_empty() => "1".into(),
        RuleCondition::Any { of } if of.is_empty() => "0".into(),
        RuleCondition::All { of } | RuleCondition::Any { of } => {
            let joiner = if matches!(c, RuleCondition::All { .. }) {
                " AND "
            } else {
                " OR "
            };
            let parts: Vec<String> = of.iter().map(|c| leaf(c, tid, bind)).collect();
            format!("({})", parts.join(joiner))
        }
        RuleCondition::Not { cond } => format!("NOT ({})", leaf(cond, tid, bind)),
        RuleCondition::Sender { op, value } => {
            let v = crate::addr::normalize(value);
            if v.is_empty() {
                return "0".into();
            }
            let p = param(bind, v);
            any_inbound(
                tid,
                &text_op(*op, "comail_addr(COALESCE(rm.from_addr, ''))", &p),
            )
        }
        RuleCondition::Recipient { op, value } => {
            let v = crate::addr::normalize(value);
            if v.is_empty() {
                return "0".into();
            }
            // No normalized address contains a line break, and one in the
            // needle must not straddle two addresses of the joined list.
            let mut tests: Vec<String> = Vec::new();
            let p = param(bind, v.clone());
            if !v.contains('\n') {
                tests.extend(RECIPIENT_LISTS.iter().map(|l| list_op(*op, l, &p)));
            }
            // `contains` also sees display names in the raw To/Cc JSON.
            if *op == TextOp::Contains {
                tests.push(format!(
                    "instr(comail_lower(COALESCE(rm.to_json, '[]')), {p}) > 0"
                ));
                tests.push(format!(
                    "instr(comail_lower(COALESCE(rm.cc_json, '[]')), {p}) > 0"
                ));
            }
            if tests.is_empty() {
                return "0".into();
            }
            format!(
                "EXISTS (SELECT 1 FROM messages rm WHERE rm.thread_id = {tid} AND ({}))",
                tests.join(" OR ")
            )
        }
        RuleCondition::Subject { op, value } => {
            let subject = "COALESCE(rm.local_subject_prefix, '') || COALESCE(rm.subject, '')";
            let (hay, v) = match fold_subject_needle(value) {
                SubjectNeedle::Folded(v) => (format!("comail_fold({subject})"), v),
                SubjectNeedle::Raw(v) => (format!("comail_lower({subject})"), v),
            };
            if v.is_empty() {
                return "0".into();
            }
            let p = param(bind, v);
            any_inbound(tid, &text_op(*op, &hay, &p))
        }
        RuleCondition::Label { id } => {
            let p = param(bind, *id);
            format!(
                "EXISTS (SELECT 1 FROM message_labels rl JOIN messages rm ON rm.id = rl.message_id
                         WHERE rm.thread_id = {tid} AND rl.label_id = {p})"
            )
        }
        RuleCondition::IsAutomated { value } => {
            // No inbound message: NULL IS v is 0 for either value, like `None`.
            let p = param(bind, *value as i64);
            format!(
                "((SELECT rm.is_automated FROM messages rm
                   WHERE rm.thread_id = {tid} AND rm.is_draft = 0 AND rm.is_outgoing = 0
                   ORDER BY rm.date DESC, rm.id DESC LIMIT 1) IS {p})"
            )
        }
        RuleCondition::HasAttachment { value } => {
            let exists = format!(
                "EXISTS (SELECT 1 FROM messages rm WHERE rm.thread_id = {tid}
                         AND rm.is_draft = 0 AND rm.has_attachments = 1)"
            );
            if *value {
                exists
            } else {
                format!("NOT {exists}")
            }
        }
        RuleCondition::IsStarred { value } => {
            let exists = format!(
                "EXISTS (SELECT 1 FROM visible_messages rm LEFT JOIN folders rf ON rf.id = rm.folder_id
                         WHERE rm.thread_id = {tid} AND rm.is_starred = 1
                           AND COALESCE(rf.role, '') NOT IN ('trash', 'spam'))"
            );
            if *value {
                exists
            } else {
                format!("NOT {exists}")
            }
        }
    }
}

/// `c` as a predicate on the thread id expression `tid` (e.g. `"t.id"`), with
/// [`super::decide::matches`] semantics: an unmatchable condition (no positive
/// criterion) matches nothing. Parameters are appended to `bind`.
pub fn compile(c: &RuleCondition, tid: &str, bind: &mut Binds) -> String {
    if !c.is_matchable() {
        return "0".into();
    }
    leaf(c, tid, bind)
}

/// `t.id IN (…)`: inbox threads matching `c`, for a view tab's list and badge.
/// The condition is evaluated only for threads in the inbox: the inner query
/// walks the inbox thread set (built once through the folder index, like the
/// inbox scope in `threads::list`), never every thread in the database.
pub fn view_tab_clause(c: &RuleCondition, bind: &mut Binds) -> String {
    let pred = compile(c, "vt.id", bind);
    format!(
        "t.id IN (SELECT vt.id FROM threads vt
                  WHERE vt.id IN (SELECT im.thread_id FROM visible_messages im
                                  JOIN folders imf ON imf.id = im.folder_id
                                  WHERE imf.role = 'inbox')
                    AND {pred})"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testutil;
    use crate::route::decide::{RuleSubject, matches};
    use rusqlite::{Connection, params};

    fn ids_sql(c: &Connection, cond: &RuleCondition) -> Vec<i64> {
        let mut bind = Binds::new();
        let pred = compile(cond, "t.id", &mut bind);
        let sql = format!("SELECT t.id FROM threads t WHERE {pred} ORDER BY t.id");
        let params: Vec<&dyn ToSql> = bind.iter().map(|b| b.as_ref()).collect();
        let mut stmt = c.prepare(&sql).unwrap();
        stmt.query_map(params.as_slice(), |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    fn ids_decide(c: &Connection, cond: &RuleCondition) -> Vec<i64> {
        let mut stmt = c.prepare("SELECT id FROM threads ORDER BY id").unwrap();
        let all: Vec<i64> = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        all.into_iter()
            .filter(|&t| matches(cond, &RuleSubject::load(c, t).unwrap()))
            .collect()
    }

    /// Insert a message; `thread` is created on first use.
    #[allow(clippy::too_many_arguments)]
    fn msg(
        c: &Connection,
        thread: i64,
        folder: i64,
        from: &str,
        to_json: &str,
        subject: &str,
        date: i64,
        flags: &[&str],
    ) -> i64 {
        c.execute(
            "INSERT OR IGNORE INTO threads (id, account_id, subject_norm, last_message_at)
             VALUES (?1, 1, 's', ?2)",
            params![thread, date],
        )
        .unwrap();
        let f = |k: &str| flags.contains(&k) as i64;
        c.execute(
            "INSERT INTO messages (thread_id, account_id, folder_id, subject, from_addr, to_json,
             cc_json, date, is_read, is_automated, is_draft, is_outgoing, has_attachments,
             is_starred, local_subject_prefix)
             VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                thread,
                folder,
                subject,
                from,
                to_json,
                if f("cc") == 1 {
                    r#"[{"name":"Ops Team","email":"ops@Corp.example"}]"#
                } else {
                    "[]"
                },
                date,
                f("auto"),
                f("draft"),
                f("out"),
                f("attach"),
                f("star"),
                if f("prefix") == 1 { "[Invoice] " } else { "" },
            ],
        )
        .unwrap();
        c.last_insert_rowid()
    }

    /// Threads covering every fact a leaf reads, including the edge cases
    /// where a naive SQL translation would diverge from `decide`.
    fn fixture() -> Connection {
        let c = testutil::conn();
        testutil::seed_account(&c);
        c.execute_batch(
            "INSERT INTO folders (id, account_id, imap_name, role) VALUES
               (2,1,'Trash','trash'), (3,1,'Sent','sent');
             INSERT INTO labels (id, name, color, keyword, position) VALUES
               (7,'Receipts','#888','Receipts',0), (8,'Travel','#888','Travel',1);",
        )
        .unwrap();
        let to = |e: &str| format!(r#"[{{"name":"Dev Team","email":"{e}"}}]"#);
        // 1: GitHub notification, automated, labelled Receipts.
        let m = msg(
            &c,
            1,
            1,
            "NoReply@GitHub.com",
            &to("me@acme.io"),
            "[CI] Build failed",
            100,
            &["auto"],
        );
        c.execute(
            "INSERT INTO message_labels (message_id, label_id) VALUES (?1, 7)",
            params![m],
        )
        .unwrap();
        // 2: a person (IDN domain) with an accented subject, then my reply
        //    from my company and a newer automated draft-only message.
        msg(
            &c,
            2,
            1,
            "Rechnung@XN--MLLER-KVA.DE",
            &to("me@acme.io"),
            "HÓA ĐƠN tháng 3",
            200,
            &["attach"],
        );
        msg(
            &c,
            2,
            3,
            "me@mycompany.com",
            &to("Rechnung@müller.de"),
            "Re: HÓA ĐƠN",
            300,
            &["out", "star"],
        );
        msg(
            &c,
            2,
            1,
            "me@mycompany.com",
            "[]",
            "draft",
            400,
            &["draft", "attach", "auto"],
        );
        // 3: CJK subject, CC'd team, AI prefix, starred only in Trash.
        msg(
            &c,
            3,
            2,
            "billing@shop.example",
            &to("me@acme.io"),
            "您的电费账单",
            150,
            &["cc", "prefix", "star"],
        );
        // 4: only outgoing mail (no inbound: automated is neither yes nor no).
        msg(
            &c,
            4,
            3,
            "me@mycompany.com",
            &to("bob@client.io"),
            "Proposal",
            120,
            &["out"],
        );
        // 5: human reply after an automated message; starred in the inbox.
        msg(
            &c,
            5,
            1,
            "alerts@bank.example",
            &to("me@acme.io"),
            "Alert",
            100,
            &["auto"],
        );
        msg(
            &c,
            5,
            1,
            "Banker@Bank.example",
            &to("me@acme.io"),
            "Re: Alert",
            200,
            &["star"],
        );
        c
    }

    fn text(kind: &str, op: TextOp, v: &str) -> RuleCondition {
        let value = v.to_string();
        match kind {
            "sender" => RuleCondition::Sender { op, value },
            "recipient" => RuleCondition::Recipient { op, value },
            _ => RuleCondition::Subject { op, value },
        }
    }

    #[test]
    fn every_leaf_matches_decide_on_a_fixture() {
        let c = fixture();
        let ops = [
            TextOp::Contains,
            TextOp::Equals,
            TextOp::StartsWith,
            TextOp::EndsWith,
        ];
        let mut conds: Vec<RuleCondition> = Vec::new();
        let needles = [
            (
                "sender",
                vec![
                    "github",
                    "noreply@github.com",
                    "NOREPLY",
                    "@github.com",
                    "@müller.de",
                    "@xn--mller-kva.de",
                    "rechnung@müller.de",
                    "mycompany",
                    "bank.example",
                    "banker@bank.example",
                    "  ",
                    "@",
                ],
            ),
            (
                "recipient",
                vec![
                    "dev team",
                    "me@acme.io",
                    "@acme.io",
                    "ops@corp.example",
                    "ops",
                    "rechnung@müller.de",
                    "@müller.de",
                    "bob",
                    "client.io",
                    "team",
                ],
            ),
            (
                "subject",
                vec![
                    "hoa don",
                    "HÓA",
                    "[ci]",
                    "failed",
                    "电费",
                    "安",
                    "[invoice]",
                    "re: alert",
                    "alert",
                    "draft",
                    "账单",
                    "tháng 3",
                ],
            ),
        ];
        for (kind, values) in &needles {
            for op in ops {
                for v in values {
                    conds.push(text(kind, op, v));
                }
            }
        }
        for id in [7, 8, 99] {
            conds.push(RuleCondition::Label { id });
        }
        for value in [true, false] {
            conds.push(RuleCondition::IsAutomated { value });
            conds.push(RuleCondition::HasAttachment { value });
            conds.push(RuleCondition::IsStarred { value });
        }
        // Negations and groups, including unmatchable trees.
        let leaves = conds.clone();
        for l in &leaves {
            conds.push(RuleCondition::Not {
                cond: Box::new(l.clone()),
            });
            conds.push(RuleCondition::All {
                of: vec![
                    RuleCondition::Any {
                        of: vec![RuleCondition::Label { id: 7 }, l.clone()],
                    },
                    RuleCondition::Not {
                        cond: Box::new(RuleCondition::Any {
                            of: vec![text("sender", TextOp::Equals, "alerts@bank.example")],
                        }),
                    },
                ],
            });
        }
        conds.push(RuleCondition::All { of: vec![] });
        conds.push(RuleCondition::Any { of: vec![] });
        conds.push(RuleCondition::Not {
            cond: Box::new(RuleCondition::Not {
                cond: Box::new(RuleCondition::IsStarred { value: true }),
            }),
        });

        let mut nonempty = 0;
        for cond in &conds {
            let want = ids_decide(&c, cond);
            assert_eq!(ids_sql(&c, cond), want, "{cond:?}");
            nonempty += !want.is_empty() as usize;
        }
        // The fixture must actually exercise matches, not just agree on nothing.
        assert!(nonempty > conds.len() / 3, "{nonempty} of {}", conds.len());
    }

    #[test]
    fn view_tab_clause_scopes_to_the_inbox() {
        let c = fixture();
        let mut bind = Binds::new();
        let starred = RuleCondition::IsStarred { value: true };
        let clause = view_tab_clause(&starred, &mut bind);
        let sql = format!("SELECT t.id FROM threads t WHERE {clause} ORDER BY t.id");
        let params: Vec<&dyn ToSql> = bind.iter().map(|b| b.as_ref()).collect();
        let ids: Vec<i64> = c
            .prepare(&sql)
            .unwrap()
            .query_map(params.as_slice(), |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        // Thread 2's star sits on a sent copy, but the thread is in the
        // inbox; thread 3's star is in Trash; thread 5 is starred in the inbox.
        assert_eq!(ids, vec![2, 5]);
    }

    /// The condition runs once per inbox thread: the planner must walk the
    /// inbox thread set and look threads up by rowid, never scan `threads`
    /// (100k+ rows on a real mailbox). Measured on 50k threads / 50 in the
    /// inbox: a view-tab page costs ~0.3 ms (Important: ~0.2 ms) and its badge
    /// adds ~0.6 ms to `unread_counts`.
    #[test]
    fn view_tab_plan_walks_the_inbox_not_every_thread() {
        let c = fixture();
        let cond = RuleCondition::Any {
            of: vec![
                RuleCondition::IsStarred { value: true },
                text("sender", TextOp::Equals, "a@b.c"),
            ],
        };
        let mut bind = Binds::new();
        let clause = view_tab_clause(&cond, &mut bind);
        let sql = format!(
            "EXPLAIN QUERY PLAN SELECT t.id FROM threads t LEFT JOIN snoozes s ON s.thread_id = t.id
             WHERE {clause} AND s.thread_id IS NULL ORDER BY t.last_message_at DESC LIMIT 51"
        );
        let params: Vec<&dyn ToSql> = bind.iter().map(|b| b.as_ref()).collect();
        let plan: Vec<String> = c
            .prepare(&sql)
            .unwrap()
            .query_map(params.as_slice(), |r| r.get(3))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let has = |p: &str| plan.iter().any(|l| l.starts_with(p));
        assert!(has("SEARCH vt USING INTEGER PRIMARY KEY"), "{plan:#?}");
        assert!(!has("SCAN vt") && !has("SCAN t "), "{plan:#?}");
    }
}
