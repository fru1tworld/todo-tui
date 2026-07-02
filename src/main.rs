mod action;
mod app;
mod db;
mod ui;

use ratatui::{
    DefaultTerminal,
    crossterm::{
        cursor::SetCursorStyle,
        event::{
            self, Event, KeyEventKind, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
            PushKeyboardEnhancementFlags,
        },
        execute,
        terminal::supports_keyboard_enhancement,
    },
};

use action::{Flow, map_key};
use app::App;
use db::Store;
use ui::ui;

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
    let _ = execute!(std::io::stdout(), SetCursorStyle::BlinkingBar);

    let result = run(&mut terminal, &mut app);

    let _ = execute!(std::io::stdout(), SetCursorStyle::DefaultUserShape);
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
        if let Some(action) = map_key(app, key)
            && let Flow::Quit = app.apply(action)?
        {
            return Ok(());
        }
    }
}
