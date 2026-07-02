mod db;

use chrono::Local;
use ratatui::{
    DefaultTerminal, Frame,
    crossterm::{
        event::{
            self, Event, KeyCode, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
            PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
        },
        execute,
        terminal::supports_keyboard_enhancement,
    },
    layout::{Constraint, Flex, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph},
};

use db::{Store, Todo, parse_due};

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Insert,
    Normal,
}

#[derive(Clone, Copy)]
enum PopupKind {
    Edit { id: i64 },
    Due { id: i64 },
    Subtask { parent_id: i64 },
}

impl PopupKind {
    fn label(self) -> &'static str {
        match self {
            PopupKind::Edit { .. } => "내용 편집 (Enter 저장 · Esc 취소)",
            PopupKind::Due { .. } => "마감 (YYYY-MM-DD, 비우면 해제)",
            PopupKind::Subtask { .. } => "하위 목표 (Enter 저장 · Esc 취소)",
        }
    }
}

struct Popup {
    kind: PopupKind,
    input: String,
}

struct App {
    store: Store,
    todos: Vec<Todo>,
    visible: Vec<usize>,
    state: ListState,
    mode: Mode,
    input: String,
    popup: Option<Popup>,
    status: String,
}

impl App {
    fn new(store: Store) -> rusqlite::Result<Self> {
        let mut app = Self {
            store,
            todos: Vec::new(),
            visible: Vec::new(),
            state: ListState::default(),
            mode: Mode::Insert,
            input: String::new(),
            popup: None,
            status: String::new(),
        };
        app.todos = app.store.list()?;
        app.rebuild_visible();
        app.select_id_or_first(None);
        Ok(app)
    }

    fn reload(&mut self) -> rusqlite::Result<()> {
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

    fn move_selected(&mut self, delta: isize) -> rusqlite::Result<()> {
        let Some(cur) = self.selected() else {
            return Ok(());
        };
        let (id, parent_id) = (cur.id, cur.parent_id);

        let siblings: Vec<(i64, i64)> = self
            .todos
            .iter()
            .filter(|t| t.parent_id == parent_id)
            .map(|t| (t.id, t.position))
            .collect();
        let Some(idx) = siblings.iter().position(|(sid, _)| *sid == id) else {
            return Ok(());
        };
        let j = idx as isize + delta;
        if j < 0 || j as usize >= siblings.len() {
            return Ok(());
        }
        let (a_id, a_pos) = siblings[idx];
        let (b_id, b_pos) = siblings[j as usize];
        self.store.set_position(a_id, b_pos)?;
        self.store.set_position(b_id, a_pos)?;
        self.reload()?;
        self.status = "순서 이동됨".to_string();
        Ok(())
    }

    fn children_done(&self, parent_id: i64) -> (usize, usize) {
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

    fn toggle_done(&mut self) -> rusqlite::Result<()> {
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

        if !child_ids.is_empty() {
            let new = !done;
            self.store.set_done(id, new)?;
            for cid in &child_ids {
                self.store.set_done(*cid, new)?;
            }
        } else if let Some(pid) = parent_id {
            self.store.set_done(id, !done)?;
            let all_done = self
                .todos
                .iter()
                .filter(|c| c.parent_id == Some(pid))
                .all(|c| if c.id == id { !done } else { c.done });
            self.store.set_done(pid, all_done)?;
        } else {
            self.store.set_done(id, !done)?;
        }
        self.reload()?;
        Ok(())
    }

    fn set_collapse_toggle(&mut self, force: Option<bool>) -> rusqlite::Result<()> {
        let Some(t) = self.selected() else {
            return Ok(());
        };
        let id = t.id;
        let has_children = self.todos.iter().any(|c| c.parent_id == Some(id));
        if !has_children {
            return Ok(());
        }
        let next = force.unwrap_or(!t.collapsed);
        if next == t.collapsed {
            return Ok(());
        }
        self.store.set_collapsed(id, next)?;
        self.reload()?;
        Ok(())
    }

    fn indent_selected(&mut self) -> rusqlite::Result<()> {
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

        let others_done = self
            .todos
            .iter()
            .filter(|c| c.parent_id == Some(new_parent))
            .all(|c| c.done);
        let parent_done = others_done && done;

        let pos = self.store.next_position()?;
        self.store.set_parent(id, Some(new_parent))?;
        self.store.set_position(id, pos)?;
        self.store.set_collapsed(new_parent, false)?;
        self.store.set_done(new_parent, parent_done)?;
        self.reload()?;
        self.select_id_or_first(Some(id));
        self.status = "하위로 넣음".to_string();
        Ok(())
    }

    fn outdent_selected(&mut self) -> rusqlite::Result<()> {
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

        self.store.set_parent(id, None)?;
        for (i, tid) in order.iter().enumerate() {
            self.store.set_position(*tid, i as i64 + 1)?;
        }

        let remaining: Vec<bool> = self
            .todos
            .iter()
            .filter(|c| c.parent_id == Some(pid) && c.id != id)
            .map(|c| c.done)
            .collect();
        if !remaining.is_empty() {
            self.store.set_done(pid, remaining.iter().all(|&d| d))?;
        }

        self.reload()?;
        self.select_id_or_first(Some(id));
        self.status = "최상위로 뺌".to_string();
        Ok(())
    }

    fn delete_selected(&mut self) -> rusqlite::Result<()> {
        if let Some(id) = self.selected_id() {
            self.store.delete(id)?;
            self.reload()?;
            self.status = "삭제됨".to_string();
        }
        Ok(())
    }

    fn commit_insert(&mut self) -> rusqlite::Result<()> {
        let text = self.input.trim();
        if !text.is_empty() {
            let id = self.store.add(text, None, None)?;
            self.reload()?;
            self.select_id_or_first(Some(id));
            self.status = "추가됨".to_string();
        }
        self.input.clear();
        Ok(())
    }

    fn open_popup(&mut self, kind: PopupKind, input: String) {
        self.status = "Enter 저장  Esc 취소".to_string();
        self.popup = Some(Popup { kind, input });
    }

    fn open_subtask(&mut self) {
        let Some(t) = self.selected() else {
            return;
        };
        let parent_id = t.parent_id.unwrap_or(t.id);
        self.open_popup(PopupKind::Subtask { parent_id }, String::new());
    }

    fn commit_subtask(
        &mut self,
        parent_id: i64,
        text: &str,
    ) -> rusqlite::Result<Result<(), String>> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(Err("내용을 입력하세요".to_string()));
        }
        let id = self.store.add(text, None, Some(parent_id))?;
        self.store.set_done(parent_id, false)?;
        self.store.set_collapsed(parent_id, false)?;
        self.reload()?;
        self.select_id_or_first(Some(id));
        self.status = "하위 목표 추가됨".to_string();
        Ok(Ok(()))
    }

    fn commit_edit(&mut self, id: i64, text: &str) -> rusqlite::Result<Result<(), String>> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(Err("내용을 입력하세요".to_string()));
        }
        let due = self
            .todos
            .iter()
            .find(|t| t.id == id)
            .and_then(|t| t.due_at);
        self.store.update(id, text, due)?;
        self.reload()?;
        self.status = "수정됨".to_string();
        Ok(Ok(()))
    }

    fn commit_due(&mut self, id: i64, input: &str) -> rusqlite::Result<Result<(), String>> {
        match parse_due(input) {
            Ok(value) => {
                self.store.set_due(id, value)?;
                self.reload()?;
                self.status = if value.is_some() {
                    "마감 설정됨"
                } else {
                    "마감 해제됨"
                }
                .to_string();
                Ok(Ok(()))
            }
            Err(e) => Ok(Err(e)),
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let store = Store::open_default()?;
    let mut app = App::new(store)?;

    let mut terminal = ratatui::init();

    let enhanced = supports_keyboard_enhancement().unwrap_or(false);
    if enhanced {
        let _ = execute!(
            std::io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
    }

    let result = run(&mut terminal, &mut app);

    if enhanced {
        let _ = execute!(std::io::stdout(), PopKeyboardEnhancementFlags);
    }
    ratatui::restore();
    result
}

fn run(terminal: &mut DefaultTerminal, app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        terminal.draw(|f| ui(f, app))?;

        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        if app.popup.is_some() {
            handle_popup(app, key.code)?;
            continue;
        }

        match app.mode {
            Mode::Insert => match key.code {
                KeyCode::Esc => app.mode = Mode::Normal,
                KeyCode::Enter => app.commit_insert()?,
                KeyCode::Backspace => {
                    app.input.pop();
                }
                KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    app.move_selected(-1)?
                }
                KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    app.move_selected(1)?
                }
                KeyCode::Left if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    app.indent_selected()?
                }
                KeyCode::Right if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    app.outdent_selected()?
                }
                KeyCode::Right => app.set_collapse_toggle(Some(false))?,
                KeyCode::Left => app.set_collapse_toggle(Some(true))?,
                KeyCode::Up => app.move_selection(-1),
                KeyCode::Down => app.move_selection(1),
                KeyCode::Char(c) => app.input.push(c),
                _ => {}
            },
            Mode::Normal => match key.code {
                KeyCode::Char('q') => return Ok(()),
                KeyCode::Char('i') | KeyCode::Char('a') | KeyCode::Esc => app.mode = Mode::Insert,
                KeyCode::Char('e') => {
                    if let Some(t) = app.selected() {
                        app.open_popup(PopupKind::Edit { id: t.id }, t.text.clone());
                    }
                }
                KeyCode::Char('t') => {
                    if let Some(t) = app.selected() {
                        let input = t.due_string().unwrap_or_default();
                        app.open_popup(PopupKind::Due { id: t.id }, input);
                    }
                }
                KeyCode::Char('s') => app.open_subtask(),
                KeyCode::Char('d') => app.delete_selected()?,
                KeyCode::Char(' ') => app.toggle_done()?,
                KeyCode::Left if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    app.indent_selected()?
                }
                KeyCode::Right if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    app.outdent_selected()?
                }
                KeyCode::Right | KeyCode::Char('l') => app.set_collapse_toggle(Some(false))?,
                KeyCode::Left | KeyCode::Char('h') => app.set_collapse_toggle(Some(true))?,
                KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    app.move_selected(-1)?
                }
                KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    app.move_selected(1)?
                }
                KeyCode::Down | KeyCode::Char('j') => app.move_selection(1),
                KeyCode::Up | KeyCode::Char('k') => app.move_selection(-1),
                _ => {}
            },
        }
    }
}

fn handle_popup(app: &mut App, code: KeyCode) -> rusqlite::Result<()> {
    let Some(mut popup) = app.popup.take() else {
        return Ok(());
    };
    match code {
        KeyCode::Esc => {
            app.status = "취소됨".to_string();
            return Ok(());
        }
        KeyCode::Enter => {
            let committed = match popup.kind {
                PopupKind::Edit { id } => app.commit_edit(id, &popup.input)?,
                PopupKind::Due { id } => app.commit_due(id, &popup.input)?,
                PopupKind::Subtask { parent_id } => app.commit_subtask(parent_id, &popup.input)?,
            };
            match committed {
                Ok(()) => return Ok(()),
                Err(msg) => app.status = msg,
            }
        }
        KeyCode::Backspace => {
            popup.input.pop();
        }
        KeyCode::Char(c) => popup.input.push(c),
        _ => {}
    }
    app.popup = Some(popup);
    Ok(())
}

fn ui(f: &mut Frame, app: &mut App) {
    let area = f.area();

    // 안내바는 커맨드 단위로 줄바꿈되므로, 필요한 줄 수만큼 높이를 잡는다.
    let inner = area.width.saturating_sub(2);
    let bottom = bottom_panel(app, inner);
    let bottom_height = bottom.line_count(inner).max(3) as u16;

    let [top, mid, bot] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(bottom_height),
    ])
    .areas(area);

    let (tag, tag_color) = match app.mode {
        Mode::Insert => ("-- INSERT --", Color::Green),
        Mode::Normal => ("-- NORMAL --", Color::Blue),
    };
    let top_level = app.todos.iter().filter(|t| t.parent_id.is_none()).count();
    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            format!(" To-Do  ({top_level}개)  "),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            tag,
            Style::default().fg(tag_color).add_modifier(Modifier::BOLD),
        ),
    ]))
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, top);

    let now = Local::now().timestamp();
    let items: Vec<ListItem> = app
        .visible
        .iter()
        .map(|&i| {
            let t = &app.todos[i];
            match t.parent_id {
                Some(pid) => {
                    let is_last = app
                        .todos
                        .iter()
                        .rfind(|c| c.parent_id == Some(pid))
                        .map(|c| c.id)
                        == Some(t.id);
                    child_line(t, now, is_last)
                }
                None => {
                    let (done, total) = app.children_done(t.id);
                    parent_line(t, now, done, total)
                }
            }
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" 목록  [ ] 생성시각: 내용 "),
        )
        .highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");
    f.render_stateful_widget(list, mid, &mut app.state);

    f.render_widget(bottom, bot);

    if let Some(popup) = &app.popup {
        render_popup(f, popup);
    }
}

/// 안내바 커맨드. 그룹(동작 / 이동·구조)마다 항상 새 줄에서 시작한다.
const HELP_GROUPS: &[&[&str]] = &[
    &[
        "i 입력",
        "s 하위추가",
        "e 편집",
        "t 마감",
        "space 완료",
        "d 삭제",
    ],
    &[
        "↑↓ 이동",
        "← 접기",
        "→ 펼치기",
        "Shift+← 넣기",
        "Shift+→ 빼기",
        "Shift+↑↓ 순서",
        "q 종료",
    ],
];

fn bottom_panel(app: &App, inner_width: u16) -> Paragraph<'static> {
    match app.mode {
        Mode::Insert => input_box("새 할 일 (Enter 추가 · Esc 명령모드)", &app.input, true),
        Mode::Normal => {
            let mut lines = wrap_commands(HELP_GROUPS, inner_width);
            if !app.status.is_empty() {
                lines.push(Line::from(Span::styled(
                    app.status.clone(),
                    Style::default().fg(Color::Yellow),
                )));
            }
            Paragraph::new(lines)
                .style(Style::default().fg(Color::Gray))
                .block(Block::default().borders(Borders::ALL).title(" 안내 "))
        }
    }
}

/// 커맨드 토큰을 폭에 맞춰 줄로 묶는다. 토큰 내부는 절대 쪼개지 않고, 그룹 경계에서는
/// 남는 폭과 상관없이 항상 새 줄로 넘어간다.
fn wrap_commands(groups: &[&[&str]], width: u16) -> Vec<Line<'static>> {
    use unicode_width::UnicodeWidthStr;

    const SEP: &str = "   ";
    let width = width as usize;
    let sep_w = SEP.width();

    let mut lines = Vec::new();
    for group in groups {
        let mut cur = String::new();
        let mut cur_w = 0usize;
        for cmd in *group {
            let w = cmd.width();
            if cur.is_empty() {
                cur.push_str(cmd);
                cur_w = w;
            } else if cur_w + sep_w + w <= width {
                cur.push_str(SEP);
                cur.push_str(cmd);
                cur_w += sep_w + w;
            } else {
                lines.push(Line::from(std::mem::take(&mut cur)));
                cur.push_str(cmd);
                cur_w = w;
            }
        }
        if !cur.is_empty() {
            lines.push(Line::from(cur));
        }
    }
    if lines.is_empty() {
        lines.push(Line::from(""));
    }
    lines
}

fn content_style(done: bool) -> Style {
    if done {
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::CROSSED_OUT)
    } else {
        Style::default()
    }
}

fn push_due(spans: &mut Vec<Span<'static>>, t: &Todo, now: i64) {
    if let Some(due) = t.due_string() {
        let style = if t.is_overdue(now) {
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Magenta)
        };
        spans.push(Span::styled(format!("   ⏳ {due}"), style));
    }
}

fn parent_line(
    t: &Todo,
    now: i64,
    done_children: usize,
    total_children: usize,
) -> ListItem<'static> {
    let mark = if t.done { "[x]" } else { "[ ]" };
    let mut spans = Vec::new();

    if total_children > 0 {
        let caret = if t.collapsed { "▸ " } else { "▾ " };
        spans.push(Span::styled(caret, Style::default().fg(Color::Yellow)));
    } else {
        spans.push(Span::raw("  "));
    }

    spans.push(Span::styled(
        format!("{mark} "),
        Style::default().fg(Color::Green),
    ));
    spans.push(Span::styled(
        t.created_at_string(),
        Style::default().fg(Color::DarkGray),
    ));
    spans.push(Span::styled(": ", Style::default().fg(Color::DarkGray)));
    spans.push(Span::styled(t.text.clone(), content_style(t.done)));

    if total_children > 0 {
        let style = if done_children == total_children {
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Cyan)
        };
        spans.push(Span::styled(
            format!("  ({done_children}/{total_children})"),
            style,
        ));
    }

    push_due(&mut spans, t, now);
    ListItem::new(Line::from(spans))
}

fn child_line(t: &Todo, now: i64, is_last: bool) -> ListItem<'static> {
    let mark = if t.done { "[x]" } else { "[ ]" };
    let branch = if is_last { "    └ " } else { "    ├ " };
    let mut spans = vec![
        Span::styled(branch, Style::default().fg(Color::DarkGray)),
        Span::styled(format!("{mark} "), Style::default().fg(Color::Green)),
        Span::styled(t.created_at_string(), Style::default().fg(Color::DarkGray)),
        Span::styled(": ", Style::default().fg(Color::DarkGray)),
        Span::styled(t.text.clone(), content_style(t.done)),
    ];
    push_due(&mut spans, t, now);
    ListItem::new(Line::from(spans))
}

fn render_popup(f: &mut Frame, popup: &Popup) {
    let area = centered_rect(60, 3, f.area());
    f.render_widget(Clear, area);
    f.render_widget(input_box(popup.kind.label(), &popup.input, true), area);
}

fn input_box<'a>(label: &'a str, value: &str, active: bool) -> Paragraph<'a> {
    let border_style = if active {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let shown = if active {
        format!("{value}▏")
    } else {
        value.to_string()
    };
    Paragraph::new(shown).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(border_style)
            .title(format!(" {label} ")),
    )
}

fn centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    let vertical = Layout::vertical([Constraint::Length(height)]).flex(Flex::Center);
    let horizontal = Layout::horizontal([Constraint::Percentage(percent_x)]).flex(Flex::Center);
    let [a] = vertical.areas(area);
    let [a] = horizontal.areas(a);
    a
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

    #[test]
    fn insert_adds_todo() {
        let mut app = app_with_todos(0);
        app.input = "장보기".to_string();
        app.commit_insert().unwrap();
        let todos = app.store.list().unwrap();
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0].text, "장보기");
        assert!(app.input.is_empty());
    }

    #[test]
    fn edit_updates_selected_text() {
        let mut app = app_with_todos(2);
        app.move_selection(1);
        let id = app.selected_id().unwrap();
        app.commit_edit(id, "수정됨").unwrap().unwrap();
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
    fn toggle_and_delete() {
        let mut app = app_with_todos(1);
        app.toggle_done().unwrap();
        assert!(app.selected().unwrap().done);
        app.delete_selected().unwrap();
        assert!(app.todos.is_empty());
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
    fn parent_autocompletes_when_all_children_done() {
        let mut app = app_with_subtasks();
        let p0 = app.todos[0].id;
        let a = app.todos.iter().find(|t| t.text == "child A").unwrap().id;
        let b = app.todos.iter().find(|t| t.text == "child B").unwrap().id;

        app.select_id_or_first(Some(a));
        app.toggle_done().unwrap();
        assert!(!app.todos.iter().find(|t| t.id == p0).unwrap().done);

        app.select_id_or_first(Some(b));
        app.toggle_done().unwrap();
        assert!(app.todos.iter().find(|t| t.id == p0).unwrap().done);
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
        app.set_collapse_toggle(None).unwrap();
        assert_eq!(app.visible.len(), 2);
        app.set_collapse_toggle(None).unwrap();
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
        let t1 = app.todos.iter().find(|t| t.text == "todo 1").unwrap().id;
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
        let _ = t1;
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
    fn indent_reopens_completed_new_parent() {
        let mut app = app_with_todos(2);
        let t0 = app.todos[0].id;
        app.select_id_or_first(Some(t0));
        app.toggle_done().unwrap();
        assert!(app.todos.iter().find(|t| t.id == t0).unwrap().done);
        let t1 = app.todos.iter().find(|t| t.text == "todo 1").unwrap().id;
        app.select_id_or_first(Some(t1));
        app.indent_selected().unwrap();
        assert!(!app.todos.iter().find(|t| t.id == t0).unwrap().done);
    }

    #[test]
    fn adding_subtask_reopens_completed_parent() {
        let mut app = app_with_todos(1);
        let p = app.todos[0].id;
        app.select_id_or_first(Some(p));
        app.toggle_done().unwrap();
        assert!(app.todos.iter().find(|t| t.id == p).unwrap().done);
        app.commit_subtask(p, "새 하위").unwrap().unwrap();
        assert!(!app.todos.iter().find(|t| t.id == p).unwrap().done);
    }

    fn line_text(l: &Line) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn help_wraps_by_command_and_group() {
        // 넉넉한 폭이면 그룹마다 한 줄 → 총 2줄.
        let wide = wrap_commands(HELP_GROUPS, 200);
        assert_eq!(wide.len(), 2);

        // 좁은 폭이라도 커맨드 토큰은 절대 쪼개지지 않는다.
        let narrow = wrap_commands(HELP_GROUPS, 12);
        for cmd in HELP_GROUPS.iter().flat_map(|g| g.iter()) {
            assert!(
                narrow.iter().any(|l| line_text(l).contains(cmd)),
                "커맨드가 쪼개짐: {cmd}"
            );
        }
    }
}
