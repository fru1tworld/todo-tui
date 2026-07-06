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
    pub position: i64,
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
        let conn = Connection::open(path)?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS todos (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                text       TEXT    NOT NULL,
                created_at INTEGER NOT NULL,
                due_at     INTEGER,
                done       INTEGER NOT NULL DEFAULT 0,
                position   INTEGER NOT NULL DEFAULT 0,
                parent_id  INTEGER,
                collapsed  INTEGER NOT NULL DEFAULT 0
            )",
            [],
        )?;

        let existing = self.column_names()?;
        if !existing.iter().any(|c| c == "due_at") {
            self.conn
                .execute("ALTER TABLE todos ADD COLUMN due_at INTEGER", [])?;
        }
        if !existing.iter().any(|c| c == "position") {
            self.conn.execute(
                "ALTER TABLE todos ADD COLUMN position INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
            self.conn.execute("UPDATE todos SET position = id", [])?;
        }
        if !existing.iter().any(|c| c == "parent_id") {
            self.conn
                .execute("ALTER TABLE todos ADD COLUMN parent_id INTEGER", [])?;
        }
        if !existing.iter().any(|c| c == "collapsed") {
            self.conn.execute(
                "ALTER TABLE todos ADD COLUMN collapsed INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        Ok(())
    }

    fn column_names(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("PRAGMA table_info(todos)")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
        rows.collect()
    }

    pub fn list(&self) -> Result<Vec<Todo>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, text, created_at, due_at, done, position, parent_id, collapsed
             FROM todos ORDER BY done ASC, position ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Todo {
                id: row.get(0)?,
                text: row.get(1)?,
                created_at: row.get(2)?,
                due_at: row.get(3)?,
                done: row.get::<_, i64>(4)? != 0,
                position: row.get(5)?,
                parent_id: row.get(6)?,
                collapsed: row.get::<_, i64>(7)? != 0,
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

    pub fn next_position(&self) -> Result<i64> {
        self.conn.query_row(
            "SELECT COALESCE(MAX(position), 0) + 1 FROM todos",
            [],
            |r| r.get(0),
        )
    }

    pub fn add(&self, text: &str, due_at: Option<i64>, parent_id: Option<i64>) -> Result<i64> {
        let now = Local::now().timestamp();
        let next_pos = self.next_position()?;
        self.conn.execute(
            "INSERT INTO todos (text, created_at, due_at, done, position, parent_id)
             VALUES (?1, ?2, ?3, 0, ?4, ?5)",
            (text, now, due_at, next_pos, parent_id),
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn update(&self, id: i64, text: &str, due_at: Option<i64>) -> Result<()> {
        self.conn.execute(
            "UPDATE todos SET text = ?1, due_at = ?2 WHERE id = ?3",
            (text, due_at, id),
        )?;
        Ok(())
    }

    pub fn set_position(&self, id: i64, position: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE todos SET position = ?1 WHERE id = ?2",
            (position, id),
        )?;
        Ok(())
    }

    pub fn set_parent(&self, id: i64, parent_id: Option<i64>) -> Result<()> {
        self.conn.execute(
            "UPDATE todos SET parent_id = ?1 WHERE id = ?2",
            (parent_id, id),
        )?;
        Ok(())
    }

    pub fn set_due(&self, id: i64, due_at: Option<i64>) -> Result<()> {
        self.conn
            .execute("UPDATE todos SET due_at = ?1 WHERE id = ?2", (due_at, id))?;
        Ok(())
    }

    pub fn set_done(&self, id: i64, done: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE todos SET done = ?1 WHERE id = ?2",
            (done as i64, id),
        )?;
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
        self.conn
            .execute("DELETE FROM todos WHERE id = ?1 OR parent_id = ?1", [id])?;
        Ok(())
    }
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
        let conn = Connection::open_in_memory().unwrap();
        let store = Store { conn };
        store.migrate().unwrap();
        store
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

        s.set_done(id, true).unwrap();
        assert!(s.list().unwrap()[0].done);

        s.delete(id).unwrap();
        assert!(s.list().unwrap().is_empty());
    }

    #[test]
    fn add_assigns_increasing_position() {
        let s = mem_store();
        s.add("a", None, None).unwrap();
        s.add("b", None, None).unwrap();
        s.add("c", None, None).unwrap();
        let todos = s.list().unwrap();
        assert!(todos[0].position < todos[1].position);
        assert!(todos[1].position < todos[2].position);
        assert_eq!(
            todos.iter().map(|t| t.text.clone()).collect::<Vec<_>>(),
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

        s.set_done(a, true).unwrap();
        let texts: Vec<_> = s.list().unwrap().iter().map(|t| t.text.clone()).collect();
        assert_eq!(texts, ["b", "c", "a"]);

        s.set_done(a, false).unwrap();
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

        s.set_done(c1, true).unwrap();
        let texts: Vec<_> = s.list().unwrap().iter().map(|t| t.text.clone()).collect();
        assert_eq!(texts, ["p", "c2", "c1", "q"]);
    }

    #[test]
    fn set_position_reorders() {
        let s = mem_store();
        let a = s.add("a", None, None).unwrap();
        let b = s.add("b", None, None).unwrap();
        let (pa, pb) = {
            let todos = s.list().unwrap();
            (todos[0].position, todos[1].position)
        };
        s.set_position(a, pb).unwrap();
        s.set_position(b, pa).unwrap();
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
        let store = Store { conn };
        store.migrate().unwrap();

        let t = &store.list().unwrap()[0];
        assert_eq!(t.text, "old");
        assert_eq!(t.due_at, None);
        assert_eq!(t.position, t.id);
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
            position: 1,
            parent_id: None,
            collapsed: false,
        };
        assert!(t.is_overdue(200));
        assert!(!t.is_overdue(50));
        t.done = true;
        assert!(!t.is_overdue(200));
    }
}
