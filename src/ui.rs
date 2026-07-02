use chrono::Local;
use ratatui::{
    Frame,
    layout::{Constraint, Flex, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph},
};

use crate::app::{App, Mode, Popup};
use crate::db::Todo;

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

pub(crate) fn ui(f: &mut Frame, app: &mut App) {
    let area = f.area();

    let inner = area.width.saturating_sub(2);
    let bottom = bottom_panel(app, inner);
    let bottom_height = bottom.line_count(inner).max(3) as u16;

    let [top, mid, bot] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(bottom_height),
    ])
    .areas(area);

    f.render_widget(title_bar(app), top);
    f.render_stateful_widget(todo_list(app), mid, &mut app.state);
    f.render_widget(bottom, bot);

    if let Some(popup) = &app.popup {
        render_popup(f, popup);
    }
}

fn title_bar(app: &App) -> Paragraph<'static> {
    let (tag, tag_color) = match app.mode {
        Mode::Insert => ("-- INSERT --", Color::Green),
        Mode::Normal => ("-- NORMAL --", Color::Blue),
    };
    let top_level = app.todos.iter().filter(|t| t.parent_id.is_none()).count();
    Paragraph::new(Line::from(vec![
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
    .block(Block::default().borders(Borders::ALL))
}

fn todo_list(app: &App) -> List<'static> {
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

    List::new(items)
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
        .highlight_symbol("▶ ")
}

fn bottom_panel(app: &App, inner_width: u16) -> Paragraph<'static> {
    match app.mode {
        Mode::Insert => input_box("새 할 일 (Enter 추가 · Esc 명령모드)", &app.input),
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

fn parent_line(
    t: &Todo,
    now: i64,
    done_children: usize,
    total_children: usize,
) -> ListItem<'static> {
    let has_children = total_children > 0;
    let mut spans = Vec::new();

    if has_children {
        let caret = if t.collapsed { "▸ " } else { "▾ " };
        spans.push(Span::styled(caret, Style::default().fg(Color::Yellow)));
    } else {
        spans.push(Span::raw("  "));
    }

    spans.push(checkbox(t.done));
    spans.push(Span::styled(
        t.created_at_string(),
        Style::default().fg(Color::DarkGray),
    ));
    spans.push(Span::styled(": ", Style::default().fg(Color::DarkGray)));
    spans.push(Span::styled(t.text.clone(), content_style(t.done)));

    if has_children {
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
    let branch = if is_last { "    └ " } else { "    ├ " };
    let mut spans = vec![
        Span::styled(branch, Style::default().fg(Color::DarkGray)),
        checkbox(t.done),
        Span::styled(t.created_at_string(), Style::default().fg(Color::DarkGray)),
        Span::styled(": ", Style::default().fg(Color::DarkGray)),
        Span::styled(t.text.clone(), content_style(t.done)),
    ];
    push_due(&mut spans, t, now);
    ListItem::new(Line::from(spans))
}

fn checkbox(done: bool) -> Span<'static> {
    let mark = if done { "[x] " } else { "[ ] " };
    Span::styled(mark, Style::default().fg(Color::Green))
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

fn render_popup(f: &mut Frame, popup: &Popup) {
    let area = centered_rect(60, 3, f.area());
    f.render_widget(Clear, area);
    f.render_widget(input_box(popup.kind.label(), &popup.input), area);
}

fn input_box(label: &str, value: &str) -> Paragraph<'static> {
    Paragraph::new(format!("{value}▏")).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Yellow))
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

    fn line_text(l: &Line) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn help_wraps_by_command_and_group() {
        let wide = wrap_commands(HELP_GROUPS, 200);
        assert_eq!(wide.len(), 2);

        let narrow = wrap_commands(HELP_GROUPS, 12);
        for cmd in HELP_GROUPS.iter().flat_map(|g| g.iter()) {
            assert!(
                narrow.iter().any(|l| line_text(l).contains(cmd)),
                "커맨드가 쪼개짐: {cmd}"
            );
        }
    }
}
