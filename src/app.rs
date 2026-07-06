use ratatui::widgets::ListState;
use tui_input::Input;

use crate::action::{Action, Flow};
use crate::db::{Store, Todo, parse_due};
use crate::error::{Error, Result};

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Mode {
    Insert,
    Normal,
}

#[derive(Clone, Copy)]
pub(crate) enum PopupKind {
    Edit { id: i64 },
    Due { id: i64 },
    Subtask { parent_id: i64 },
}

impl PopupKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            PopupKind::Edit { .. } => "내용 편집 (Enter 저장 · Esc 취소)",
            PopupKind::Due { .. } => "마감 (YYYY-MM-DD, 비우면 해제)",
            PopupKind::Subtask { .. } => "하위 목표 (Enter 저장 · Esc 취소)",
        }
    }
}

pub(crate) struct Popup {
    pub(crate) kind: PopupKind,
    pub(crate) input: Input,
}

pub(crate) struct App {
    pub(crate) store: Store,
    pub(crate) todos: Vec<Todo>,
    pub(crate) visible: Vec<usize>,
    pub(crate) state: ListState,
    pub(crate) mode: Mode,
    pub(crate) input: Input,
    pub(crate) popup: Option<Popup>,
    pub(crate) status: String,
}

impl App {
    pub(crate) fn new(store: Store) -> Result<Self> {
        let mut app = Self {
            store,
            todos: Vec::new(),
            visible: Vec::new(),
            state: ListState::default(),
            mode: Mode::Insert,
            input: Input::default(),
            popup: None,
            status: String::new(),
        };
        app.todos = app.store.list()?;
        app.rebuild_visible();
        app.select_id_or_first(None);
        Ok(app)
    }

    pub(crate) fn apply(&mut self, action: Action) -> Result<Flow> {
        match action {
            Action::Quit => return Ok(Flow::Quit),
            Action::EnterInsert => self.mode = Mode::Insert,
            Action::EnterNormal => self.mode = Mode::Normal,
            Action::Input(req) => {
                self.input.handle(req);
            }
            Action::CommitInsert => self.commit_insert()?,
            Action::Select(delta) => self.move_selection(delta),
            Action::Reorder(delta) => self.move_selected(delta)?,
            Action::Indent => self.indent_selected()?,
            Action::Outdent => self.outdent_selected()?,
            Action::Collapse(collapsed) => self.set_collapse(collapsed)?,
            Action::ToggleDone => self.toggle_done()?,
            Action::Delete => self.delete_selected()?,
            Action::OpenEdit => self.open_edit(),
            Action::OpenDue => self.open_due(),
            Action::OpenSubtask => self.open_subtask(),
            Action::PopupInput(req) => {
                if let Some(popup) = &mut self.popup {
                    popup.input.handle(req);
                }
            }
            Action::PopupCommit => self.popup_commit()?,
            Action::PopupCancel => self.popup_cancel(),
        }
        Ok(Flow::Continue)
    }

    pub(crate) fn children_done(&self, parent_id: i64) -> (usize, usize) {
        let mut done = 0;
        let mut total = 0;
        for c in self.todos.iter().filter(|c| c.parent_id == Some(parent_id)) {
            total += 1;
            if c.done {
                done += 1;
            }
        }
        (done, total)
    }

    fn reload(&mut self) -> Result<()> {
        let prev = self.selected_id();
        self.todos = self.store.list()?;
        self.rebuild_visible();
        self.select_id_or_first(prev);
        Ok(())
    }

    fn rebuild_visible(&mut self) {
        let mut vis = Vec::with_capacity(self.todos.len());
        for (i, t) in self.todos.iter().enumerate() {
            let show = match t.parent_id {
                None => true,
                Some(pid) => !self.todos.iter().any(|p| p.id == pid && p.collapsed),
            };
            if show {
                vis.push(i);
            }
        }
        self.visible = vis;
    }

    fn select_id_or_first(&mut self, id: Option<i64>) {
        let idx = id
            .and_then(|id| self.visible.iter().position(|&i| self.todos[i].id == id))
            .or_else(|| (!self.visible.is_empty()).then_some(0));
        self.state.select(idx);
    }

    fn selected(&self) -> Option<&Todo> {
        let v = self.state.selected()?;
        let &i = self.visible.get(v)?;
        self.todos.get(i)
    }

    fn selected_id(&self) -> Option<i64> {
        self.selected().map(|t| t.id)
    }

    fn move_selection(&mut self, delta: isize) {
        if self.visible.is_empty() {
            return;
        }
        let len = self.visible.len() as isize;
        let cur = self.state.selected().unwrap_or(0) as isize;
        self.state
            .select(Some((cur + delta).rem_euclid(len) as usize));
    }

    fn move_selected(&mut self, delta: isize) -> Result<()> {
        let Some(cur) = self.selected() else {
            return Ok(());
        };
        let (id, parent_id) = (cur.id, cur.parent_id);

        let siblings: Vec<(i64, bool)> = self
            .todos
            .iter()
            .filter(|t| t.parent_id == parent_id)
            .map(|t| (t.id, t.done))
            .collect();
        let Some(idx) = siblings.iter().position(|(sid, _)| *sid == id) else {
            return Ok(());
        };
        let j = idx as isize + delta;
        if j < 0 || j as usize >= siblings.len() {
            return Ok(());
        }
        let (a_id, a_done) = siblings[idx];
        let (b_id, b_done) = siblings[j as usize];
        if a_done != b_done {
            self.status = "완료 항목은 미완료 항목과 자리를 바꿀 수 없어요".to_string();
            return Ok(());
        }
        self.store.swap_positions(a_id, b_id)?;
        self.reload()?;
        self.status = "순서 이동됨".to_string();
        Ok(())
    }

    fn toggle_done(&mut self) -> Result<()> {
        let Some(t) = self.selected() else {
            return Ok(());
        };
        let (id, done, parent_id) = (t.id, t.done, t.parent_id);
        let child_ids: Vec<i64> = self
            .todos
            .iter()
            .filter(|c| c.parent_id == Some(id))
            .map(|c| c.id)
            .collect();

        let new = !done;
        let mut updates = vec![(id, new)];
        if !child_ids.is_empty() {
            // 부모 체크는 자식 전체에 전파된다.
            updates.extend(child_ids.iter().map(|&cid| (cid, new)));
        } else if let Some(pid) = parent_id
            && !new
        {
            // 자식 해제는 완료된 부모를 다시 연다.
            updates.push((pid, false));
        }
        self.store.set_done_many(&updates)?;
        self.reload()?;
        Ok(())
    }

    fn set_collapse(&mut self, collapsed: bool) -> Result<()> {
        let Some(t) = self.selected() else {
            return Ok(());
        };
        let id = t.id;
        let has_children = self.todos.iter().any(|c| c.parent_id == Some(id));
        if !has_children || t.collapsed == collapsed {
            return Ok(());
        }
        self.store.set_collapsed(id, collapsed)?;
        self.reload()
    }

    fn indent_selected(&mut self) -> Result<()> {
        let Some(t) = self.selected() else {
            return Ok(());
        };
        let (id, done) = (t.id, t.done);
        if t.parent_id.is_some() {
            self.status = "이미 하위 목표예요".to_string();
            return Ok(());
        }
        if self.todos.iter().any(|c| c.parent_id == Some(id)) {
            self.status = "하위 목표가 있는 항목은 넣을 수 없어요".to_string();
            return Ok(());
        }
        let tops: Vec<i64> = self
            .todos
            .iter()
            .filter(|x| x.parent_id.is_none())
            .map(|x| x.id)
            .collect();
        let idx = tops.iter().position(|&x| x == id).unwrap();
        if idx == 0 {
            self.status = "위에 넣을 상위 항목이 없어요".to_string();
            return Ok(());
        }
        let new_parent = tops[idx - 1];

        self.store.indent(id, new_parent, !done)?;
        self.reload()?;
        self.select_id_or_first(Some(id));
        self.status = "하위로 넣음".to_string();
        Ok(())
    }

    fn outdent_selected(&mut self) -> Result<()> {
        let Some(t) = self.selected() else {
            return Ok(());
        };
        let Some(pid) = t.parent_id else {
            self.status = "이미 최상위 항목이에요".to_string();
            return Ok(());
        };
        let id = t.id;

        let mut order: Vec<i64> = self
            .todos
            .iter()
            .filter(|x| x.parent_id.is_none())
            .map(|x| x.id)
            .collect();
        let at = order.iter().position(|&x| x == pid).unwrap();
        order.insert(at + 1, id);

        self.store.outdent(id, &order)?;
        self.reload()?;
        self.select_id_or_first(Some(id));
        self.status = "최상위로 뺌".to_string();
        Ok(())
    }

    fn delete_selected(&mut self) -> Result<()> {
        if let Some(id) = self.selected_id() {
            self.store.delete(id)?;
            self.reload()?;
            self.status = "삭제됨".to_string();
        }
        Ok(())
    }

    fn commit_insert(&mut self) -> Result<()> {
        let text = self.input.value().trim().to_string();
        if !text.is_empty() {
            let id = self.store.add(&text, None, None)?;
            self.reload()?;
            self.select_id_or_first(Some(id));
            self.status = "추가됨".to_string();
        }
        self.input.reset();
        Ok(())
    }

    fn open_popup(&mut self, kind: PopupKind, input: String) {
        self.status = "Enter 저장  Esc 취소".to_string();
        self.popup = Some(Popup {
            kind,
            input: Input::new(input),
        });
    }

    fn open_edit(&mut self) {
        if let Some(t) = self.selected() {
            self.open_popup(PopupKind::Edit { id: t.id }, t.text.clone());
        }
    }

    fn open_due(&mut self) {
        if let Some(t) = self.selected() {
            let input = t.due_string().unwrap_or_default();
            self.open_popup(PopupKind::Due { id: t.id }, input);
        }
    }

    fn open_subtask(&mut self) {
        let Some(t) = self.selected() else {
            return;
        };
        let parent_id = t.parent_id.unwrap_or(t.id);
        self.open_popup(PopupKind::Subtask { parent_id }, String::new());
    }

    fn popup_cancel(&mut self) {
        self.status = "취소됨".to_string();
        self.popup = None;
    }

    fn popup_commit(&mut self) -> Result<()> {
        let Some(popup) = self.popup.take() else {
            return Ok(());
        };
        let committed = match popup.kind {
            PopupKind::Edit { id } => self.commit_edit(id, popup.input.value()),
            PopupKind::Due { id } => self.commit_due(id, popup.input.value()),
            PopupKind::Subtask { parent_id } => self.commit_subtask(parent_id, popup.input.value()),
        };
        match committed {
            Ok(()) => Ok(()),
            // 검증 실패는 상태 표시줄에 알리고 팝업을 유지한다.
            Err(Error::Invalid(msg)) => {
                self.status = msg;
                self.popup = Some(popup);
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    fn commit_subtask(&mut self, parent_id: i64, text: &str) -> Result<()> {
        let text = text.trim();
        if text.is_empty() {
            return Err(Error::Invalid("내용을 입력하세요".to_string()));
        }
        let id = self.store.add_subtask(text, parent_id)?;
        self.reload()?;
        self.select_id_or_first(Some(id));
        self.status = "하위 목표 추가됨".to_string();
        Ok(())
    }

    fn commit_edit(&mut self, id: i64, text: &str) -> Result<()> {
        let text = text.trim();
        if text.is_empty() {
            return Err(Error::Invalid("내용을 입력하세요".to_string()));
        }
        let due = self
            .todos
            .iter()
            .find(|t| t.id == id)
            .and_then(|t| t.due_at);
        self.store.update(id, text, due)?;
        self.reload()?;
        self.status = "수정됨".to_string();
        Ok(())
    }

    fn commit_due(&mut self, id: i64, input: &str) -> Result<()> {
        let value = parse_due(input)?;
        self.store.set_due(id, value)?;
        self.reload()?;
        self.status = if value.is_some() {
            "마감 설정됨"
        } else {
            "마감 해제됨"
        }
        .to_string();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with_todos(n: usize) -> App {
        let store = Store::open(std::path::PathBuf::from(":memory:")).unwrap();
        for i in 0..n {
            store.add(&format!("todo {i}"), None, None).unwrap();
        }
        App::new(store).unwrap()
    }

    fn app_with_subtasks() -> App {
        let mut app = app_with_todos(2);
        let p0 = app.todos[0].id;
        app.store.add("child A", None, Some(p0)).unwrap();
        app.store.add("child B", None, Some(p0)).unwrap();
        app.reload().unwrap();
        app
    }

    #[test]
    fn insert_adds_todo() {
        let mut app = app_with_todos(0);
        app.input = Input::new("장보기".to_string());
        app.commit_insert().unwrap();
        let todos = app.store.list().unwrap();
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0].text, "장보기");
        assert!(app.input.value().is_empty());
    }

    #[test]
    fn edit_updates_selected_text() {
        let mut app = app_with_todos(2);
        app.move_selection(1);
        let id = app.selected_id().unwrap();
        app.commit_edit(id, "수정됨").unwrap();
        let todos = app.store.list().unwrap();
        assert_eq!(todos[1].text, "수정됨");
    }

    #[test]
    fn move_selected_reorders_and_keeps_selection() {
        let mut app = app_with_todos(3);
        app.state.select(Some(0));
        app.move_selected(1).unwrap();
        let texts: Vec<_> = app.todos.iter().map(|t| t.text.clone()).collect();
        assert_eq!(texts, ["todo 1", "todo 0", "todo 2"]);
        assert_eq!(app.selected().unwrap().text, "todo 0");
        assert_eq!(app.state.selected(), Some(1));
    }

    #[test]
    fn move_selected_clamps_at_edges() {
        let mut app = app_with_todos(2);
        app.state.select(Some(0));
        app.move_selected(-1).unwrap();
        let texts: Vec<_> = app.todos.iter().map(|t| t.text.clone()).collect();
        assert_eq!(texts, ["todo 0", "todo 1"]);
    }

    #[test]
    fn toggle_done_sinks_and_restores() {
        let mut app = app_with_todos(3);
        app.state.select(Some(0));
        app.toggle_done().unwrap();
        let texts: Vec<_> = app.todos.iter().map(|t| t.text.clone()).collect();
        assert_eq!(texts, ["todo 1", "todo 2", "todo 0"]);
        assert_eq!(app.selected().unwrap().text, "todo 0");

        app.toggle_done().unwrap();
        let texts: Vec<_> = app.todos.iter().map(|t| t.text.clone()).collect();
        assert_eq!(texts, ["todo 0", "todo 1", "todo 2"]);
        assert_eq!(app.selected().unwrap().text, "todo 0");
    }

    #[test]
    fn reorder_refused_across_done_boundary() {
        let mut app = app_with_todos(3);
        app.state.select(Some(2));
        app.toggle_done().unwrap();
        assert_eq!(app.selected().unwrap().text, "todo 2");

        app.move_selected(-1).unwrap();
        let texts: Vec<_> = app.todos.iter().map(|t| t.text.clone()).collect();
        assert_eq!(texts, ["todo 0", "todo 1", "todo 2"]);
    }

    #[test]
    fn toggle_and_delete() {
        let mut app = app_with_todos(1);
        app.toggle_done().unwrap();
        assert!(app.selected().unwrap().done);
        app.delete_selected().unwrap();
        assert!(app.todos.is_empty());
    }

    #[test]
    fn subtask_moves_with_parent() {
        let mut app = app_with_subtasks();
        let p0 = app.todos[0].id;
        app.select_id_or_first(Some(p0));
        app.move_selected(1).unwrap();
        let texts: Vec<_> = app.todos.iter().map(|t| t.text.clone()).collect();
        assert_eq!(texts, ["todo 1", "todo 0", "child A", "child B"]);
        assert_eq!(app.selected().unwrap().text, "todo 0");
    }

    #[test]
    fn subtask_reorders_only_among_siblings() {
        let mut app = app_with_subtasks();
        let a = app.todos.iter().find(|t| t.text == "child A").unwrap().id;
        app.select_id_or_first(Some(a));
        app.move_selected(1).unwrap();
        let texts: Vec<_> = app.todos.iter().map(|t| t.text.clone()).collect();
        assert_eq!(texts, ["todo 0", "child B", "child A", "todo 1"]);
        app.move_selected(1).unwrap();
        let texts: Vec<_> = app.todos.iter().map(|t| t.text.clone()).collect();
        assert_eq!(texts, ["todo 0", "child B", "child A", "todo 1"]);
    }

    #[test]
    fn parent_stays_open_when_all_children_done() {
        let mut app = app_with_subtasks();
        let p0 = app.todos[0].id;
        let a = app.todos.iter().find(|t| t.text == "child A").unwrap().id;
        let b = app.todos.iter().find(|t| t.text == "child B").unwrap().id;

        app.select_id_or_first(Some(a));
        app.toggle_done().unwrap();
        assert!(!app.todos.iter().find(|t| t.id == p0).unwrap().done);

        app.select_id_or_first(Some(b));
        app.toggle_done().unwrap();
        assert!(!app.todos.iter().find(|t| t.id == p0).unwrap().done);
    }

    #[test]
    fn unchecking_child_reopens_done_parent() {
        let mut app = app_with_subtasks();
        let p0 = app.todos[0].id;
        let a = app.todos.iter().find(|t| t.text == "child A").unwrap().id;

        app.select_id_or_first(Some(p0));
        app.toggle_done().unwrap();
        assert!(app.todos.iter().find(|t| t.id == p0).unwrap().done);

        app.select_id_or_first(Some(a));
        app.toggle_done().unwrap();
        assert!(!app.todos.iter().find(|t| t.id == p0).unwrap().done);
    }

    #[test]
    fn toggling_parent_cascades_to_children() {
        let mut app = app_with_subtasks();
        let p0 = app.todos[0].id;
        app.select_id_or_first(Some(p0));
        app.toggle_done().unwrap();
        assert!(
            app.todos
                .iter()
                .filter(|t| t.parent_id == Some(p0))
                .all(|t| t.done)
        );
        assert!(app.todos.iter().find(|t| t.id == p0).unwrap().done);
    }

    #[test]
    fn collapse_hides_children_from_visible() {
        let mut app = app_with_subtasks();
        let p0 = app.todos[0].id;
        assert_eq!(app.visible.len(), 4);
        app.select_id_or_first(Some(p0));
        app.set_collapse(true).unwrap();
        assert_eq!(app.visible.len(), 2);
        app.set_collapse(false).unwrap();
        assert_eq!(app.visible.len(), 4);
    }

    #[test]
    fn indent_nests_under_item_above() {
        let mut app = app_with_todos(2);
        let t1 = app.todos[1].id;
        app.select_id_or_first(Some(t1));
        app.indent_selected().unwrap();
        assert_eq!(
            app.todos.iter().find(|t| t.id == t1).unwrap().parent_id,
            Some(app.todos[0].id)
        );
        let texts: Vec<_> = app.todos.iter().map(|t| t.text.clone()).collect();
        assert_eq!(texts, ["todo 0", "todo 1"]);
        assert_eq!(app.selected().unwrap().id, t1);
    }

    #[test]
    fn indent_refused_at_top_or_with_children() {
        let mut app = app_with_todos(2);
        let t0 = app.todos[0].id;
        app.select_id_or_first(Some(t0));
        app.indent_selected().unwrap();
        assert!(
            app.todos
                .iter()
                .find(|t| t.id == t0)
                .unwrap()
                .parent_id
                .is_none()
        );
        app.store.add("child", None, Some(t0)).unwrap();
        app.reload().unwrap();
        app.select_id_or_first(Some(t0));
        app.indent_selected().unwrap();
        assert!(
            app.todos
                .iter()
                .find(|t| t.id == t0)
                .unwrap()
                .parent_id
                .is_none()
        );
    }

    #[test]
    fn outdent_promotes_child_after_parent_block() {
        let mut app = app_with_subtasks();
        let a = app.todos.iter().find(|t| t.text == "child A").unwrap().id;
        app.select_id_or_first(Some(a));
        app.outdent_selected().unwrap();
        assert!(
            app.todos
                .iter()
                .find(|t| t.id == a)
                .unwrap()
                .parent_id
                .is_none()
        );
        let texts: Vec<_> = app.todos.iter().map(|t| t.text.clone()).collect();
        assert_eq!(texts, ["todo 0", "child B", "child A", "todo 1"]);
        assert_eq!(app.selected().unwrap().id, a);
    }

    #[test]
    fn indent_refused_when_done_item_sank_below() {
        let mut app = app_with_todos(2);
        let t0 = app.todos[0].id;
        app.select_id_or_first(Some(t0));
        app.toggle_done().unwrap();
        assert!(app.todos.iter().find(|t| t.id == t0).unwrap().done);
        assert_eq!(app.todos[1].id, t0);

        let t1 = app.todos.iter().find(|t| t.text == "todo 1").unwrap().id;
        app.select_id_or_first(Some(t1));
        app.indent_selected().unwrap();
        assert!(
            app.todos
                .iter()
                .find(|t| t.id == t1)
                .unwrap()
                .parent_id
                .is_none()
        );
    }

    #[test]
    fn adding_subtask_reopens_completed_parent() {
        let mut app = app_with_todos(1);
        let p = app.todos[0].id;
        app.select_id_or_first(Some(p));
        app.toggle_done().unwrap();
        assert!(app.todos.iter().find(|t| t.id == p).unwrap().done);
        app.commit_subtask(p, "새 하위").unwrap();
        assert!(!app.todos.iter().find(|t| t.id == p).unwrap().done);
    }
}
