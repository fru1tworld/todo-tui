use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, Mode};

#[derive(Clone, Copy)]
pub(crate) enum Action {
    Quit,
    EnterInsert,
    EnterNormal,
    InsertChar(char),
    InsertBackspace,
    CommitInsert,
    Select(isize),
    Reorder(isize),
    Indent,
    Outdent,
    Collapse(bool),
    ToggleDone,
    Delete,
    OpenEdit,
    OpenDue,
    OpenSubtask,
    PopupChar(char),
    PopupBackspace,
    PopupCommit,
    PopupCancel,
}

pub(crate) enum Flow {
    Continue,
    Quit,
}

pub(crate) fn map_key(app: &App, key: KeyEvent) -> Option<Action> {
    use Action::*;
    use KeyCode::{Backspace, Char, Down, Enter, Esc, Left, Right, Up};

    let shift = key.modifiers.contains(KeyModifiers::SHIFT);

    if app.popup.is_some() {
        return Some(match key.code {
            Esc => PopupCancel,
            Enter => PopupCommit,
            Backspace => PopupBackspace,
            Char(c) => PopupChar(c),
            _ => return None,
        });
    }

    let nav = match key.code {
        Up if shift => Some(Reorder(-1)),
        Down if shift => Some(Reorder(1)),
        Left if shift => Some(Indent),
        Right if shift => Some(Outdent),
        Up => Some(Select(-1)),
        Down => Some(Select(1)),
        Left => Some(Collapse(true)),
        Right => Some(Collapse(false)),
        _ => None,
    };
    if nav.is_some() {
        return nav;
    }

    Some(match app.mode {
        Mode::Insert => match key.code {
            Esc => EnterNormal,
            Enter => CommitInsert,
            Backspace => InsertBackspace,
            Char(c) => InsertChar(c),
            _ => return None,
        },
        Mode::Normal => match key.code {
            Char('q') => Quit,
            Char('i' | 'a') | Esc => EnterInsert,
            Char('e') => OpenEdit,
            Char('t') => OpenDue,
            Char('s') => OpenSubtask,
            Char('d') => Delete,
            Char(' ') => ToggleDone,
            Char('j') => Select(1),
            Char('k') => Select(-1),
            Char('l') => Collapse(false),
            Char('h') => Collapse(true),
            _ => return None,
        },
    })
}
