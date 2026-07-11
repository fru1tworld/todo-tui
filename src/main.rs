mod action;
mod app;
mod db;
mod error;
mod ui;

use ratatui::{
    DefaultTerminal,
    crossterm::{
        cursor::SetCursorStyle,
        event::{
            self, Event, KeyCode, KeyEventKind, KeyboardEnhancementFlags,
            PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
        },
        execute,
        terminal::supports_keyboard_enhancement,
    },
};

use action::{Flow, map_key};
use app::App;
use db::Store;
use ui::ui;

fn main() -> anyhow::Result<()> {
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
    let _ = execute!(std::io::stdout(), SetCursorStyle::BlinkingBar);

    let result = run(&mut terminal, &mut app);

    let _ = execute!(std::io::stdout(), SetCursorStyle::DefaultUserShape);
    if enhanced {
        let _ = execute!(std::io::stdout(), PopKeyboardEnhancementFlags);
    }
    ratatui::restore();
    result
}

fn run(terminal: &mut DefaultTerminal, app: &mut App) -> anyhow::Result<()> {
    loop {
        terminal.draw(|f| ui(f, app))?;

        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        // Tab을 누르면 '보내기 모드': ←→ 가 메모를 옆 탭으로 옮기고,
        // 그 외 키를 누르면 즉시 해제된다(Tab 뗌은 터미널이 보고하지 않음).
        if key.code == KeyCode::Tab {
            app.tab_held = true;
            app.status = "←→ 메모를 옆 탭으로 보내기 · 다른 키를 누르면 해제".to_string();
            continue;
        }
        if app.tab_held && !matches!(key.code, KeyCode::Left | KeyCode::Right) {
            app.tab_held = false;
        }
        if let Some(action) = map_key(app, key)
            && let Flow::Quit = app.apply(action)?
        {
            return Ok(());
        }
    }
}
