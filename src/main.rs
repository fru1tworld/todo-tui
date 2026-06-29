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
    layout::{Constraint, Direction, Flex, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

use db::{Store, Todo, parse_due};

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Insert,
    Normal,
}

struct Form {
    id: i64,
    text: String,
}

struct DueInput {
    id: i64,
    input: String,
}

enum Popup {
    Edit(Form),
    Due(DueInput),
}

struct App {
    store: Store,
    todos: Vec<Todo>,
    state: ListState,
    mode: Mode,
    input: String,
    popup: Option<Popup>,
    status: String,
}

impl App {
    fn new(store: Store) -> rusqlite::Result<Self> {
        let todos = store.list()?;
        let mut state = ListState::default();
        if !todos.is_empty() {
            state.select(Some(0));
        }
        Ok(Self {
            store,
            todos,
            state,
            mode: Mode::Insert,
            input: String::new(),
            popup: None,
            status: String::new(),
        })
    }

    fn reload(&mut self) -> rusqlite::Result<()> {
        let prev = self.selected_id();
        self.todos = self.store.list()?;
        let new_index = prev
            .and_then(|id| self.todos.iter().position(|t| t.id == id))
            .or_else(|| (!self.todos.is_empty()).then_some(0));
        self.state.select(new_index);
        Ok(())
    }

    fn selected_id(&self) -> Option<i64> {
        self.state.selected().and_then(|i| self.todos.get(i)).map(|t| t.id)
    }

    fn selected(&self) -> Option<&Todo> {
        self.state.selected().and_then(|i| self.todos.get(i))
    }

    fn move_selection(&mut self, delta: isize) {
        if self.todos.is_empty() {
            return;
        }
        let len = self.todos.len() as isize;
        let cur = self.state.selected().unwrap_or(0) as isize;
        self.state.select(Some((cur + delta).rem_euclid(len) as usize));
    }

    fn move_selected(&mut self, delta: isize) -> rusqlite::Result<()> {
        let Some(i) = self.state.selected() else {
            return Ok(());
        };
        let j = i as isize + delta;
        if j < 0 || j as usize >= self.todos.len() {
            return Ok(());
        }
        let (a, b) = (&self.todos[i], &self.todos[j as usize]);
        self.store.set_position(a.id, b.position)?;
        self.store.set_position(b.id, a.position)?;
        self.reload()?;
        self.status = "순서 이동됨".to_string();
        Ok(())
    }

    fn toggle_done(&mut self) -> rusqlite::Result<()> {
        if let Some(t) = self.selected() {
            let (id, done) = (t.id, t.done);
            self.store.set_done(id, !done)?;
            self.reload()?;
        }
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
            self.store.add(text, None)?;
            self.reload()?;
            if !self.todos.is_empty() {
                self.state.select(Some(self.todos.len() - 1));
            }
            self.status = "추가됨".to_string();
        }
        self.input.clear();
        Ok(())
    }

    fn commit_form(&mut self, form: &Form) -> rusqlite::Result<Result<(), String>> {
        let text = form.text.trim();
        if text.is_empty() {
            return Ok(Err("내용을 입력하세요".to_string()));
        }
        let due = self.todos.iter().find(|t| t.id == form.id).and_then(|t| t.due_at);
        self.store.update(form.id, text, due)?;
        self.reload()?;
        self.status = "수정됨".to_string();
        Ok(Ok(()))
    }

    fn commit_due(&mut self, due: &DueInput) -> rusqlite::Result<Result<(), String>> {
        match parse_due(&due.input) {
            Ok(value) => {
                self.store.set_due(due.id, value)?;
                self.reload()?;
                self.status = if value.is_some() { "마감 설정됨" } else { "마감 해제됨" }.to_string();
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

    // 터미널이 지원하면 키보드 향상 플래그를 켜서 Shift+화살표 같은
    // 수정자 조합을 받을 수 있게 한다. (미지원 터미널에서는 JK 로 동작)
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
                        app.popup = Some(Popup::Edit(Form { id: t.id, text: t.text.clone() }));
                        app.status = "Enter 저장  Esc 취소".to_string();
                    }
                }
                KeyCode::Char('t') => {
                    if let Some(t) = app.selected() {
                        app.popup = Some(Popup::Due(DueInput {
                            id: t.id,
                            input: t.due_string().unwrap_or_default(),
                        }));
                        app.status = "마감일 입력 (비우면 해제)  Enter 저장  Esc 취소".to_string();
                    }
                }
                KeyCode::Char('d') => app.delete_selected()?,
                KeyCode::Char(' ') => app.toggle_done()?,
                KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    app.move_selected(-1)?
                }
                KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    app.move_selected(1)?
                }
                KeyCode::Char('K') => app.move_selected(-1)?,
                KeyCode::Char('J') => app.move_selected(1)?,
                KeyCode::Down | KeyCode::Char('j') => app.move_selection(1),
                KeyCode::Up | KeyCode::Char('k') => app.move_selection(-1),
                _ => {}
            },
        }
    }
}

fn handle_popup(app: &mut App, code: KeyCode) -> rusqlite::Result<()> {
    let Some(popup) = app.popup.take() else {
        return Ok(());
    };
    match popup {
        Popup::Edit(mut form) => {
            let mut keep = true;
            match code {
                KeyCode::Esc => {
                    app.status = "취소됨".to_string();
                    keep = false;
                }
                KeyCode::Enter => match app.commit_form(&form)? {
                    Ok(()) => keep = false,
                    Err(msg) => app.status = msg,
                },
                KeyCode::Backspace => {
                    form.text.pop();
                }
                KeyCode::Char(c) => form.text.push(c),
                _ => {}
            }
            if keep {
                app.popup = Some(Popup::Edit(form));
            }
        }
        Popup::Due(mut due) => {
            let mut keep = true;
            match code {
                KeyCode::Esc => {
                    app.status = "취소됨".to_string();
                    keep = false;
                }
                KeyCode::Enter => match app.commit_due(&due)? {
                    Ok(()) => keep = false,
                    Err(msg) => app.status = msg,
                },
                KeyCode::Backspace => {
                    due.input.pop();
                }
                KeyCode::Char(c) => due.input.push(c),
                _ => {}
            }
            if keep {
                app.popup = Some(Popup::Due(due));
            }
        }
    }
    Ok(())
}

fn ui(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(3),
        ])
        .split(f.area());

    let (tag, tag_color) = match app.mode {
        Mode::Insert => ("-- INSERT --", Color::Green),
        Mode::Normal => ("-- NORMAL --", Color::Blue),
    };
    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            format!(" To-Do  ({}개)  ", app.todos.len()),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::styled(tag, Style::default().fg(tag_color).add_modifier(Modifier::BOLD)),
    ]))
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    let now = Local::now().timestamp();
    let items: Vec<ListItem> = app.todos.iter().map(|t| todo_line(t, now)).collect();
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
    f.render_stateful_widget(list, chunks[1], &mut app.state);

    let bottom = match app.mode {
        Mode::Insert => input_box("새 할 일 (Enter 추가 · Esc 명령모드)", &app.input, true),
        Mode::Normal => Paragraph::new(format!(
            "i 입력  e 편집  t 마감  space 완료  d 삭제  ↑↓ 이동  ⇧↑↓/JK 순서  q 종료    {}",
            app.status
        ))
        .style(Style::default().fg(Color::Gray))
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title(" 안내 ")),
    };
    f.render_widget(bottom, chunks[2]);

    match &app.popup {
        Some(Popup::Edit(form)) => render_form(f, form),
        Some(Popup::Due(due)) => render_due(f, due),
        None => {}
    }
}

fn todo_line(t: &Todo, now: i64) -> ListItem<'static> {
    let mark = if t.done { "[x]" } else { "[ ]" };
    let content_style = if t.done {
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::CROSSED_OUT)
    } else {
        Style::default()
    };

    let mut spans = vec![
        Span::styled(format!("{mark} "), Style::default().fg(Color::Green)),
        Span::styled(t.created_at_string(), Style::default().fg(Color::DarkGray)),
        Span::styled(": ", Style::default().fg(Color::DarkGray)),
        Span::styled(t.text.clone(), content_style),
    ];

    if let Some(due) = t.due_string() {
        let style = if t.is_overdue(now) {
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Magenta)
        };
        spans.push(Span::styled(format!("   ⏳ {due}"), style));
    }

    ListItem::new(Line::from(spans))
}

fn render_form(f: &mut Frame, form: &Form) {
    let area = centered_rect(60, 3, f.area());
    f.render_widget(Clear, area);
    f.render_widget(input_box("내용 편집 (Enter 저장 · Esc 취소)", &form.text, true), area);
}

fn render_due(f: &mut Frame, due: &DueInput) {
    let area = centered_rect(50, 3, f.area());
    f.render_widget(Clear, area);
    f.render_widget(input_box("마감 (YYYY-MM-DD, 비우면 해제)", &due.input, true), area);
}

fn input_box<'a>(label: &'a str, value: &str, active: bool) -> Paragraph<'a> {
    let border_style = if active {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let shown = if active { format!("{value}▏") } else { value.to_string() };
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
            store.add(&format!("todo {i}"), None).unwrap();
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
        app.commit_form(&Form { id, text: "수정됨".to_string() }).unwrap().unwrap();
        let todos = app.store.list().unwrap();
        assert_eq!(todos[1].text, "수정됨");
    }

    #[test]
    fn move_selected_reorders_and_keeps_selection() {
        let mut app = app_with_todos(3); // todo 0, todo 1, todo 2
        app.state.select(Some(0));
        app.move_selected(1).unwrap();
        let texts: Vec<_> = app.todos.iter().map(|t| t.text.clone()).collect();
        assert_eq!(texts, ["todo 1", "todo 0", "todo 2"]);
        // 이동한 항목을 계속 선택 중이어야 한다.
        assert_eq!(app.selected().unwrap().text, "todo 0");
        assert_eq!(app.state.selected(), Some(1));
    }

    #[test]
    fn move_selected_clamps_at_edges() {
        let mut app = app_with_todos(2);
        app.state.select(Some(0));
        app.move_selected(-1).unwrap(); // 맨 위에서 위로 → 변화 없음
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
}
