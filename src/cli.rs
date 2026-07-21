use clap::{Parser, Subcommand};

use crate::db::{Store, parse_due};

#[derive(Parser)]
#[command(name = "todo-tui", about = "TUI 할 일 관리 + CLI")]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// 할 일 목록 조회
    List {
        /// 프로젝트 이름 (생략 시 전체)
        #[arg(short, long)]
        project: Option<String>,
        /// JSON 출력
        #[arg(long)]
        json: bool,
    },
    /// 할 일 추가
    Add {
        /// 할 일 내용
        text: String,
        /// 프로젝트 이름 (생략 시 기본)
        #[arg(short, long)]
        project: Option<String>,
        /// 상위 할 일 ID
        #[arg(long)]
        parent: Option<i64>,
        /// 마감일 (YYYY-MM-DD)
        #[arg(short, long)]
        due: Option<String>,
    },
    /// 하위 목표 추가
    Subtask {
        /// 상위 할 일 ID
        parent_id: i64,
        /// 할 일 내용
        text: String,
    },
    /// 완료 처리
    Done {
        /// 할 일 ID
        id: i64,
    },
    /// 미완료 처리
    Undone {
        /// 할 일 ID
        id: i64,
    },
    /// 삭제
    #[command(name = "rm")]
    Delete {
        /// 할 일 ID
        id: i64,
    },
    /// 내용 수정
    Edit {
        /// 할 일 ID
        id: i64,
        /// 새 내용
        text: String,
    },
    /// 프로젝트 목록
    Projects {
        /// JSON 출력
        #[arg(long)]
        json: bool,
    },
    /// 프로젝트 추가
    AddProject {
        /// 프로젝트 이름
        name: String,
    },
}

pub(crate) fn run(cmd: Command) -> anyhow::Result<()> {
    let store = Store::open_default()?;

    match cmd {
        Command::List { project, json } => {
            let todos = match project {
                Some(name) => store.list(resolve_project(&store, &name)?)?,
                None => store.list_all()?,
            };
            if json {
                println!("{}", serde_json::to_string_pretty(&todos)?);
            } else {
                for t in &todos {
                    let check = if t.done { "x" } else { " " };
                    let indent = if t.parent_id.is_some() { "  " } else { "" };
                    let due = t
                        .due_string()
                        .map(|d| format!(" (마감: {d})"))
                        .unwrap_or_default();
                    println!("{indent}[{check}] #{} {}{due}", t.id, t.text);
                }
            }
        }
        Command::Add {
            text,
            project,
            parent,
            due,
        } => {
            let pid = match project {
                Some(name) => resolve_project(&store, &name)?,
                None => store.list_projects()?[0].id,
            };
            let due_at = due.map(|d| parse_due(&d)).transpose()?.flatten();
            println!("{}", store.add(&text, due_at, parent, pid)?);
        }
        Command::Subtask { parent_id, text } => {
            println!("{}", store.add_subtask(&text, parent_id)?);
        }
        Command::Done { id } => store.set_done_many(&[(id, true)])?,
        Command::Undone { id } => store.set_done_many(&[(id, false)])?,
        Command::Delete { id } => store.delete(id)?,
        Command::Edit { id, text } => store.update(id, &text, None)?,
        Command::Projects { json } => {
            let projects = store.list_projects()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&projects)?);
            } else {
                for p in &projects {
                    println!("#{} {}", p.id, p.name);
                }
            }
        }
        Command::AddProject { name } => {
            println!("{}", store.add_project(&name)?);
        }
    }
    Ok(())
}

fn resolve_project(store: &Store, name: &str) -> anyhow::Result<i64> {
    store
        .list_projects()?
        .iter()
        .find(|p| p.name == name)
        .map(|p| p.id)
        .ok_or_else(|| anyhow::anyhow!("프로젝트 '{name}'을(를) 찾을 수 없습니다"))
}
