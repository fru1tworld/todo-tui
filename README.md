# todo-tui

생성 시각(timestamp)을 함께 기록하는 터미널 To-Do 앱.
Rust + [ratatui](https://ratatui.rs) + SQLite([rusqlite], bundled) 로 구현했습니다.

## 실행

```sh
cargo run            # 디버그 실행
cargo build --release && ./target/release/todo-tui   # 릴리스 빌드 후 실행
```

## 화면 구성

```
┌ To-Do (2개)  -- INSERT -- ──────────────────────────┐
├ 목록  [ ] 생성시각: 내용 ────────────────────────────┤
│  [ ] 2026-06-29 11:17: 장보기   ⏳ 2026-07-01        │
│ ▶[x] 2026-06-28 09:30: 운동                          │
├ 새 할 일 (Enter 추가 · Esc 명령모드) ────────────────┤
└─────────────────────────────────────────────────────┘
```

## 조작 (vim 스타일 모달)

기본은 **Insert 모드**라 실행 직후 바로 타이핑해 할 일을 추가할 수 있습니다.
`Esc` 로 **Normal 모드(명령)** 와 전환합니다.

**Insert 모드** — 빠른 추가

| 키 | 동작 |
|----|------|
| 타이핑 + `Enter` | 입력한 내용으로 새 할 일 추가 |
| `↑` / `↓` | 항목 이동 |
| `Esc` | Normal 모드로 전환 |

**Normal 모드** — 명령

| 키 | 동작 |
|----|------|
| `i` / `a` / `Esc` | Insert 모드로 전환 |
| `e` | 선택 항목 내용 편집 |
| `t` | 선택 항목 마감일 설정/해제 (`YYYY-MM-DD`, 비우면 해제) |
| `Space` | 완료/미완료 토글 |
| `d` | 삭제 |
| `↑`/`k`, `↓`/`j` | 선택 이동 |
| `Shift`+`↑`/`↓` (또는 `K`/`J`) | **우선순위 순서 이동** (위/아래로 재정렬) |
| `q` | 종료 |

> 항목은 직접 정한 우선순위 순서로 정렬·저장됩니다. 새 할 일은 맨 아래에 추가됩니다.

각 항목에는 추가한 **생성 시각**이 `YYYY-MM-DD HH:MM` 형식(로컬 타임존)으로 좌측 정렬되어 `[ ] 생성시각: 내용` 형태로 표시되고, 마감이 있으면 `⏳`로 함께 보입니다(지난 마감은 빨간색).

## 데이터 저장

SQLite DB 파일에 영구 저장됩니다 (앱을 껐다 켜도 유지).

- macOS: `~/Library/Application Support/todo-tui/todos.db`
- Linux: `~/.local/share/todo-tui/todos.db`

스키마:

```sql
CREATE TABLE todos (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    text       TEXT    NOT NULL,
    created_at INTEGER NOT NULL,  -- UTC epoch seconds
    due_at     INTEGER,           -- 마감 (epoch seconds, 없으면 NULL)
    done       INTEGER NOT NULL DEFAULT 0,
    position   INTEGER NOT NULL DEFAULT 0  -- 우선순위 정렬 순서
);
```

[rusqlite]: https://github.com/rusqlite/rusqlite
