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
                position   INTEGER NOT NULL DEFAULT 0
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
            // 기존 항목은 생성 순서(=id)를 초기 우선순위로 둔다.
            self.conn.execute("UPDATE todos SET position = id", [])?;
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
            "SELECT id, text, created_at, due_at, done, position
             FROM todos ORDER BY position ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Todo {
                id: row.get(0)?,
                text: row.get(1)?,
                created_at: row.get(2)?,
                due_at: row.get(3)?,
                done: row.get::<_, i64>(4)? != 0,
                position: row.get(5)?,
            })
        })?;
        rows.collect()
    }

    pub fn add(&self, text: &str, due_at: Option<i64>) -> Result<i64> {
        let now = Local::now().timestamp();
        let next_pos: i64 =
            self.conn
                .query_row("SELECT COALESCE(MAX(position), 0) + 1 FROM todos", [], |r| {
                    r.get(0)
                })?;
        self.conn.execute(
            "INSERT INTO todos (text, created_at, due_at, done, position) VALUES (?1, ?2, ?3, 0, ?4)",
            (text, now, due_at, next_pos),
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
        self.conn
            .execute("UPDATE todos SET position = ?1 WHERE id = ?2", (position, id))?;
        Ok(())
    }

    pub fn set_due(&self, id: i64, due_at: Option<i64>) -> Result<()> {
        self.conn
            .execute("UPDATE todos SET due_at = ?1 WHERE id = ?2", (due_at, id))?;
        Ok(())
    }

    pub fn set_done(&self, id: i64, done: bool) -> Result<()> {
        self.conn
            .execute("UPDATE todos SET done = ?1 WHERE id = ?2", (done as i64, id))?;
        Ok(())
    }

    pub fn delete(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM todos WHERE id = ?1", [id])?;
        Ok(())
    }
}

fn format_epoch(epoch: i64, fmt: &str) -> String {
    match DateTime::from_timestamp(epoch, 0) {
        Some(dt) => dt.with_timezone(&Local).format(fmt).to_string(),
        None => "?".to_string(),
    }
}

pub fn parse_due(input: &str) -> std::result::Result<Option<i64>, String> {
    let s = input.trim();
    if s.is_empty() {
        return Ok(None);
    }
    let date = NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|_| "날짜 형식은 YYYY-MM-DD 여야 합니다".to_string())?;
    let naive = date
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| "잘못된 날짜".to_string())?;
    match Local.from_local_datetime(&naive).single() {
        Some(dt) => Ok(Some(dt.timestamp())),
        None => Err("변환할 수 없는 날짜".to_string()),
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
        let id = s.add("첫 번째 할 일", due).unwrap();
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
        s.add("a", None).unwrap();
        s.add("b", None).unwrap();
        s.add("c", None).unwrap();
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
            .execute("INSERT INTO todos (text, created_at, position) VALUES ('b', 100, 2)", [])
            .unwrap();
        s.conn
            .execute("INSERT INTO todos (text, created_at, position) VALUES ('a', 200, 1)", [])
            .unwrap();
        let todos = s.list().unwrap();
        assert_eq!(todos[0].text, "a");
        assert_eq!(todos[1].text, "b");
    }

    #[test]
    fn set_position_reorders() {
        let s = mem_store();
        let a = s.add("a", None).unwrap();
        let b = s.add("b", None).unwrap();
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
        conn.execute("INSERT INTO todos (text, created_at) VALUES ('old', 100)", [])
            .unwrap();
        let store = Store { conn };
        store.migrate().unwrap();

        let t = &store.list().unwrap()[0];
        assert_eq!(t.text, "old");
        assert_eq!(t.due_at, None);
        assert_eq!(t.position, t.id);
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
        };
        assert!(t.is_overdue(200));
        assert!(!t.is_overdue(50));
        t.done = true;
        assert!(!t.is_overdue(200));
    }
}
