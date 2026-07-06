use chrono::{DateTime, Local, NaiveDate, TimeZone};
use rusqlite::{Connection, Result};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Todo {
    pub id: i64,
    pub text: String,
    pub created_at: i64,
    pub due_at: Option<i64>,
    pub done: bool,
    pub parent_id: Option<i64>,
    pub collapsed: bool,
}

impl Todo {
    pub fn created_at_string(&self) -> String {
        format_epoch(self.created_at, "%Y-%m-%d %H:%M")
    }

    pub fn due_string(&self) -> Option<String> {
        self.due_at.map(|e| format_epoch(e, "%Y-%m-%d"))
    }

    pub fn is_overdue(&self, now: i64) -> bool {
        !self.done && self.due_at.is_some_and(|d| d < now)
    }
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open_default() -> Result<Self> {
        let path = default_db_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        Self::open(path)
    }

    pub fn open(path: PathBuf) -> Result<Self> {
        Self::from_connection(Connection::open(path)?)
    }

    fn from_connection(conn: Connection) -> Result<Self> {
        let store = Self { conn };
        store.migrate()?;
        // 마이그레이션(테이블 재구축) 이후에만 외래 키 검사를 켠다.
        store.conn.pragma_update(None, "foreign_keys", true)?;
        Ok(store)
    }

    /// PRAGMA user_version 기반 버전 마이그레이션.
    fn migrate(&self) -> Result<()> {
        let version: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < 1 {
            let tx = self.conn.unchecked_transaction()?;
            migrate_v1(&tx)?;
            tx.pragma_update(None, "user_version", 1)?;
            tx.commit()?;
        }
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<Todo>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, text, created_at, due_at, done, parent_id, collapsed
             FROM todos ORDER BY done ASC, position ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Todo {
                id: row.get(0)?,
                text: row.get(1)?,
                created_at: row.get(2)?,
                due_at: row.get(3)?,
                done: row.get::<_, i64>(4)? != 0,
                parent_id: row.get(5)?,
                collapsed: row.get::<_, i64>(6)? != 0,
            })
        })?;
        let all: Vec<Todo> = rows.collect::<Result<_>>()?;

        let mut out = Vec::with_capacity(all.len());
        for parent in all.iter().filter(|t| t.parent_id.is_none()) {
            out.push(parent.clone());
            for child in all.iter().filter(|c| c.parent_id == Some(parent.id)) {
                out.push(child.clone());
            }
        }
        Ok(out)
    }

    pub fn add(&self, text: &str, due_at: Option<i64>, parent_id: Option<i64>) -> Result<i64> {
        let tx = self.conn.unchecked_transaction()?;
        let id = insert_todo(&tx, text, due_at, parent_id)?;
        tx.commit()?;
        Ok(id)
    }

    /// 하위 목표 추가 + 부모 완료 해제·펼치기를 한 트랜잭션으로 처리한다.
    pub fn add_subtask(&self, text: &str, parent_id: i64) -> Result<i64> {
        let tx = self.conn.unchecked_transaction()?;
        let id = insert_todo(&tx, text, None, Some(parent_id))?;
        tx.execute(
            "UPDATE todos SET done = 0, collapsed = 0 WHERE id = ?1",
            [parent_id],
        )?;
        tx.commit()?;
        Ok(id)
    }

    pub fn update(&self, id: i64, text: &str, due_at: Option<i64>) -> Result<()> {
        self.conn.execute(
            "UPDATE todos SET text = ?1, due_at = ?2 WHERE id = ?3",
            (text, due_at, id),
        )?;
        Ok(())
    }

    /// 두 항목의 position을 한 트랜잭션으로 맞바꾼다.
    pub fn swap_positions(&self, a: i64, b: i64) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let pa: i64 = tx.query_row("SELECT position FROM todos WHERE id = ?1", [a], |r| {
            r.get(0)
        })?;
        let pb: i64 = tx.query_row("SELECT position FROM todos WHERE id = ?1", [b], |r| {
            r.get(0)
        })?;
        tx.execute("UPDATE todos SET position = ?1 WHERE id = ?2", (pb, a))?;
        tx.execute("UPDATE todos SET position = ?1 WHERE id = ?2", (pa, b))?;
        tx.commit()
    }

    /// 여러 항목의 완료 상태를 한 트랜잭션으로 갱신한다(부모-자식 전파용).
    pub fn set_done_many(&self, updates: &[(i64, bool)]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare("UPDATE todos SET done = ?1 WHERE id = ?2")?;
            for &(id, done) in updates {
                stmt.execute((done as i64, id))?;
            }
        }
        tx.commit()
    }

    /// 항목을 부모 밑으로 넣고 맨 뒤 position 부여, 부모 펼침(필요 시 완료 해제)까지 한 트랜잭션.
    pub fn indent(&self, id: i64, parent: i64, reopen_parent: bool) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let pos: i64 = tx.query_row(
            "SELECT COALESCE(MAX(position), 0) + 1 FROM todos",
            [],
            |r| r.get(0),
        )?;
        tx.execute(
            "UPDATE todos SET parent_id = ?1, position = ?2 WHERE id = ?3",
            (parent, pos, id),
        )?;
        tx.execute("UPDATE todos SET collapsed = 0 WHERE id = ?1", [parent])?;
        if reopen_parent {
            tx.execute("UPDATE todos SET done = 0 WHERE id = ?1", [parent])?;
        }
        tx.commit()
    }

    /// 항목을 최상위로 빼고 전달받은 최상위 순서대로 position을 다시 매긴다.
    pub fn outdent(&self, id: i64, top_order: &[i64]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("UPDATE todos SET parent_id = NULL WHERE id = ?1", [id])?;
        {
            let mut stmt = tx.prepare("UPDATE todos SET position = ?1 WHERE id = ?2")?;
            for (i, tid) in top_order.iter().enumerate() {
                stmt.execute((i as i64 + 1, tid))?;
            }
        }
        tx.commit()
    }

    pub fn set_due(&self, id: i64, due_at: Option<i64>) -> Result<()> {
        self.conn
            .execute("UPDATE todos SET due_at = ?1 WHERE id = ?2", (due_at, id))?;
        Ok(())
    }

    pub fn set_collapsed(&self, id: i64, collapsed: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE todos SET collapsed = ?1 WHERE id = ?2",
            (collapsed as i64, id),
        )?;
        Ok(())
    }

    pub fn delete(&self, id: i64) -> Result<()> {
        // 자식 삭제는 parent_id의 ON DELETE CASCADE가 처리한다.
        self.conn.execute("DELETE FROM todos WHERE id = ?1", [id])?;
        Ok(())
    }
}

fn insert_todo(
    conn: &Connection,
    text: &str,
    due_at: Option<i64>,
    parent_id: Option<i64>,
) -> Result<i64> {
    let now = Local::now().timestamp();
    let pos: i64 = conn.query_row(
        "SELECT COALESCE(MAX(position), 0) + 1 FROM todos",
        [],
        |r| r.get(0),
    )?;
    conn.execute(
        "INSERT INTO todos (text, created_at, due_at, done, position, parent_id)
         VALUES (?1, ?2, ?3, 0, ?4, ?5)",
        (text, now, due_at, pos, parent_id),
    )?;
    Ok(conn.last_insert_rowid())
}

/// v1: 최종 스키마로 재구축한다. 레거시 테이블(누락 컬럼 포함)을 흡수하고
/// parent_id에 ON DELETE CASCADE 외래 키를 건다.
fn migrate_v1(conn: &Connection) -> Result<()> {
    let has_table: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'todos')",
        [],
        |r| r.get(0),
    )?;

    if has_table {
        // 레거시 스키마에 없는 컬럼을 먼저 채워 아래 복사 SELECT 목록을 통일한다.
        let existing = column_names(conn)?;
        if !existing.iter().any(|c| c == "due_at") {
            conn.execute("ALTER TABLE todos ADD COLUMN due_at INTEGER", [])?;
        }
        if !existing.iter().any(|c| c == "position") {
            conn.execute(
                "ALTER TABLE todos ADD COLUMN position INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
            conn.execute("UPDATE todos SET position = id", [])?;
        }
        if !existing.iter().any(|c| c == "parent_id") {
            conn.execute("ALTER TABLE todos ADD COLUMN parent_id INTEGER", [])?;
        }
        if !existing.iter().any(|c| c == "collapsed") {
            conn.execute(
                "ALTER TABLE todos ADD COLUMN collapsed INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
    }

    conn.execute_batch(
        "CREATE TABLE todos_v1 (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            text       TEXT    NOT NULL,
            created_at INTEGER NOT NULL,
            due_at     INTEGER,
            done       INTEGER NOT NULL DEFAULT 0,
            position   INTEGER NOT NULL DEFAULT 0,
            parent_id  INTEGER REFERENCES todos_v1(id) ON DELETE CASCADE,
            collapsed  INTEGER NOT NULL DEFAULT 0
        )",
    )?;
    if has_table {
        conn.execute_batch(
            "INSERT INTO todos_v1 (id, text, created_at, due_at, done, position, parent_id, collapsed)
             SELECT id, text, created_at, due_at, done, position, parent_id, collapsed FROM todos;
             DROP TABLE todos;",
        )?;
    }
    conn.execute("ALTER TABLE todos_v1 RENAME TO todos", [])?;
    Ok(())
}

fn column_names(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("PRAGMA table_info(todos)")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    rows.collect()
}

fn format_epoch(epoch: i64, fmt: &str) -> String {
    match DateTime::from_timestamp(epoch, 0) {
        Some(dt) => dt.with_timezone(&Local).format(fmt).to_string(),
        None => "?".to_string(),
    }
}

pub fn parse_due(input: &str) -> crate::error::Result<Option<i64>> {
    use crate::error::Error;

    let s = input.trim();
    if s.is_empty() {
        return Ok(None);
    }
    let date = NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|_| Error::Invalid("날짜 형식은 YYYY-MM-DD 여야 합니다".to_string()))?;
    let naive = date
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| Error::Invalid("잘못된 날짜".to_string()))?;
    match Local.from_local_datetime(&naive).single() {
        Some(dt) => Ok(Some(dt.timestamp())),
        None => Err(Error::Invalid("변환할 수 없는 날짜".to_string())),
    }
}

fn default_db_path() -> PathBuf {
    let mut dir = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    dir.push("todo-tui");
    dir.push("todos.db");
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_store() -> Store {
        Store::from_connection(Connection::open_in_memory().unwrap()).unwrap()
    }

    fn position_of(s: &Store, id: i64) -> i64 {
        s.conn
            .query_row("SELECT position FROM todos WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap()
    }

    #[test]
    fn add_list_update_toggle_delete_roundtrip() {
        let s = mem_store();
        assert!(s.list().unwrap().is_empty());

        let due = parse_due("2026-07-01").unwrap();
        let id = s.add("첫 번째 할 일", due, None).unwrap();
        let todos = s.list().unwrap();
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0].text, "첫 번째 할 일");
        assert_eq!(todos[0].due_at, due);
        assert!(!todos[0].done);
        assert!(todos[0].created_at > 0);

        s.update(id, "수정됨", None).unwrap();
        let t = &s.list().unwrap()[0];
        assert_eq!(t.text, "수정됨");
        assert_eq!(t.due_at, None);

        s.set_done_many(&[(id, true)]).unwrap();
        assert!(s.list().unwrap()[0].done);

        s.delete(id).unwrap();
        assert!(s.list().unwrap().is_empty());
    }

    #[test]
    fn add_assigns_increasing_position() {
        let s = mem_store();
        let a = s.add("a", None, None).unwrap();
        let b = s.add("b", None, None).unwrap();
        let c = s.add("c", None, None).unwrap();
        assert!(position_of(&s, a) < position_of(&s, b));
        assert!(position_of(&s, b) < position_of(&s, c));
        assert_eq!(
            s.list()
                .unwrap()
                .iter()
                .map(|t| t.text.clone())
                .collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
    }

    #[test]
    fn orders_by_position() {
        let s = mem_store();
        s.conn
            .execute(
                "INSERT INTO todos (text, created_at, position) VALUES ('b', 100, 2)",
                [],
            )
            .unwrap();
        s.conn
            .execute(
                "INSERT INTO todos (text, created_at, position) VALUES ('a', 200, 1)",
                [],
            )
            .unwrap();
        let todos = s.list().unwrap();
        assert_eq!(todos[0].text, "a");
        assert_eq!(todos[1].text, "b");
    }

    #[test]
    fn done_items_sink_to_bottom() {
        let s = mem_store();
        let a = s.add("a", None, None).unwrap();
        s.add("b", None, None).unwrap();
        s.add("c", None, None).unwrap();

        s.set_done_many(&[(a, true)]).unwrap();
        let texts: Vec<_> = s.list().unwrap().iter().map(|t| t.text.clone()).collect();
        assert_eq!(texts, ["b", "c", "a"]);

        s.set_done_many(&[(a, false)]).unwrap();
        let texts: Vec<_> = s.list().unwrap().iter().map(|t| t.text.clone()).collect();
        assert_eq!(texts, ["a", "b", "c"]);
    }

    #[test]
    fn done_children_sink_within_parent() {
        let s = mem_store();
        let p = s.add("p", None, None).unwrap();
        let c1 = s.add("c1", None, Some(p)).unwrap();
        s.add("c2", None, Some(p)).unwrap();
        s.add("q", None, None).unwrap();

        s.set_done_many(&[(c1, true)]).unwrap();
        let texts: Vec<_> = s.list().unwrap().iter().map(|t| t.text.clone()).collect();
        assert_eq!(texts, ["p", "c2", "c1", "q"]);
    }

    #[test]
    fn swap_positions_reorders() {
        let s = mem_store();
        let a = s.add("a", None, None).unwrap();
        let b = s.add("b", None, None).unwrap();
        s.swap_positions(a, b).unwrap();
        let todos = s.list().unwrap();
        assert_eq!(todos[0].text, "b");
        assert_eq!(todos[1].text, "a");
    }

    #[test]
    fn migrates_legacy_schema() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE todos (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                text TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                done INTEGER NOT NULL DEFAULT 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO todos (text, created_at) VALUES ('old', 100)",
            [],
        )
        .unwrap();
        let store = Store::from_connection(conn).unwrap();

        let t = &store.list().unwrap()[0];
        assert_eq!(t.text, "old");
        assert_eq!(t.due_at, None);
        assert_eq!(position_of(&store, t.id), t.id);
    }

    #[test]
    fn migrates_v0_full_schema_and_cascades() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE todos (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                text       TEXT    NOT NULL,
                created_at INTEGER NOT NULL,
                due_at     INTEGER,
                done       INTEGER NOT NULL DEFAULT 0,
                position   INTEGER NOT NULL DEFAULT 0,
                parent_id  INTEGER,
                collapsed  INTEGER NOT NULL DEFAULT 0
            );
            INSERT INTO todos (id, text, created_at, position) VALUES (1, 'p', 100, 1);
            INSERT INTO todos (id, text, created_at, position, parent_id)
                VALUES (2, 'c', 100, 2, 1);",
        )
        .unwrap();
        let store = Store::from_connection(conn).unwrap();

        let version: i64 = store
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 1);

        let texts: Vec<_> = store
            .list()
            .unwrap()
            .iter()
            .map(|t| t.text.clone())
            .collect();
        assert_eq!(texts, ["p", "c"]);

        // 재구축된 테이블의 FK CASCADE로 자식까지 지워진다.
        store.delete(1).unwrap();
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn list_nests_children_after_parent() {
        let s = mem_store();
        let a = s.add("a", None, None).unwrap();
        s.add("b", None, None).unwrap();
        s.add("a2", None, Some(a)).unwrap();
        s.add("a1", None, Some(a)).unwrap();
        let texts: Vec<_> = s.list().unwrap().iter().map(|t| t.text.clone()).collect();
        assert_eq!(texts, ["a", "a2", "a1", "b"]);
    }

    #[test]
    fn delete_parent_removes_children() {
        let s = mem_store();
        let p = s.add("p", None, None).unwrap();
        s.add("c1", None, Some(p)).unwrap();
        s.add("c2", None, Some(p)).unwrap();
        assert_eq!(s.list().unwrap().len(), 3);
        s.delete(p).unwrap();
        assert!(s.list().unwrap().is_empty());
    }

    #[test]
    fn parse_due_cases() {
        assert_eq!(parse_due("").unwrap(), None);
        assert_eq!(parse_due("   ").unwrap(), None);
        assert!(parse_due("2026-07-01").unwrap().is_some());
        assert!(parse_due("not-a-date").is_err());
        assert!(parse_due("2026/07/01").is_err());
    }

    #[test]
    fn overdue_detection() {
        let mut t = Todo {
            id: 1,
            text: "x".into(),
            created_at: 0,
            due_at: Some(100),
            done: false,
            parent_id: None,
            collapsed: false,
        };
        assert!(t.is_overdue(200));
        assert!(!t.is_overdue(50));
        t.done = true;
        assert!(!t.is_overdue(200));
    }
}
