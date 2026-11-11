# dbrust

An approachable Rust terminal SQL workbench, inspired by [Rainfrog](https://github.com/achristmascarl/rainfrog). Navigate with arrows, Tab, Enter, and the mouse.

## Run

Requires Rust 1.88 or newer and a terminal of at least **55 × 26** characters. A larger terminal, such as 120 × 40, gives the editor and results more room.

```sh
cargo run --release --locked
```

Install directly from GitHub:

```sh
cargo install --git https://github.com/nhanhoang1405/dbrust --locked
dbrust
```

To install the executable from a local checkout:

```sh
cargo install --path . --locked
dbrust
```

Start without a database server using `sqlite::memory:` in the connection URL field.

## Connect

The startup screen always offers three methods. Use **F2**, click a method tab, or Tab to the method selector and use Left/Right.

- **Connection URL:** enter a PostgreSQL, MySQL, or SQLite URL. The input is masked because URLs may contain passwords.
- **Manual fields:** driver, host, port, database, username, and password. Left/Right on the driver field cycles `postgres`, `mysql`, and `sqlite`. Blank ports use 5432 for PostgreSQL or 3306 for MySQL. For SQLite, enter a file path or `:memory:` in Database; the other fields are ignored. Manual SQLite connections create the file if needed.
- **Recent connections:** choose a successful connection with Up/Down, then Enter or click Connect. Passwords are requested again when needed.

Tab and Shift+Tab move focus. Enter advances manual fields and connects from the last field or the Connect button. Click any field to place its cursor. Connections have a 15-second timeout.

URL examples:

```text
postgres://alice:password@localhost:5432/app
mysql://alice:password@localhost:3306/app
sqlite::memory:
sqlite://./example.db?mode=rwc
```

URL credentials must be percent-encoded. Manual fields accept literal credentials, including punctuation, spaces, and percent signs.

Successful connections are stored in the platform config directory, normally `~/.config/dbrust/connections.json` on Linux (respects `XDG_CONFIG_HOME`). The most recent 20 distinct connections are retained. Password components and password query parameters are removed before saving; Unix history files use mode `0600`. Usernames, database names, hosts, and other connection options are retained. Passwords stay in memory only for connection entry and execution; there is no credential vault integration.

## Work with SQL

The workspace has a table browser, multiline SQL editor, results grid, and Run button. The mint border marks keyboard focus.

| Control | Action |
| --- | --- |
| Tab / Shift+Tab | Cycle Tables → Editor → Results → Run, or reverse |
| Arrows | Select tables/cells, or move the editor cursor |
| Enter on a table | Load and execute a preview of up to 1,000 rows |
| Enter in the editor | Insert a newline |
| Enter on Run | Execute the editor contents |
| F5 / Ctrl+Enter | Execute from any workspace panel |
| Enter in results | Return to the editor |
| Shift+arrows | Scroll the focused panel by 1 row or display column |
| Ctrl+arrows / Alt+arrows | Scroll by 10 rows or display columns |
| PageUp / PageDown | Scroll results by a page |
| Home / End in results | First cell / last row |
| Esc | Focus Run; return from connection switching |
| F2 | Open connection selection |
| F3 | Refresh tables after schema changes |
| F1 | Toggle help |
| Ctrl+Q | Quit |

Use Alt+arrows if your terminal intercepts Ctrl+arrows. F5 is available when the terminal sends Ctrl+Enter as plain Enter. The editor supports bracketed paste, Unicode, and undo/redo (Ctrl+Z / Ctrl+Y).

### Mouse

- Click a panel to focus it, a text field or SQL text to position its cursor, or a result cell to inspect its full value in the footer.
- Click a table to execute its preview. Click Run, Connections, Refresh, Help, or a connection method to activate it.
- Scroll the panel under the pointer using the vertical wheel or horizontal wheel/trackpad events.
- Shift+wheel scrolls horizontally; Ctrl/Alt+wheel scrolls faster.
- Click or drag either result scrollbar to jump through rows or columns.

The terminal must forward mouse events to the application. Some terminal emulators reserve modified wheel gestures for their own zoom or scrolling.

### Query behavior

PostgreSQL, MySQL, and SQLite use native SQLx connections. A persistent connection preserves session state and explicit transactions. Queries run asynchronously while the UI remains responsive, with one database operation at a time. SQL is submitted as written, including writes and multiple statements; no confirmation is added. Earlier statements can succeed before a later statement fails. Use SQL transactions when you need atomic changes.

The grid retains up to 10,000 rows from the last **nonempty** result set. It drains the entire query stream so submitted statements complete, including statements after the display cap. Use SQL `LIMIT` to reduce server work and transfer volume. Multiple result sets are not individually selectable. Empty standalone `SELECT` queries retain their column headers.

Columns have a fixed display width with horizontal scrolling. The full selected cell appears in the footer. Common scalar, date/time, decimal, JSON, UUID, and binary values are displayed where supported by each driver. Other types show an explicit unsupported-type marker. Query cancellation, autocomplete, syntax highlighting, export, and saved query history are not implemented yet.

## Validate

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build --locked
python3 scripts/smoke_tui.py
```

The Python smoke test uses a real Unix pseudo-terminal, an isolated config directory, and SQLite in memory. It exercises connection entry, bracketed paste, multiline SQL execution, function keys, history creation, and clean exit.

Two server integration tests are explicitly ignored in the default run. Run them against dedicated test databases:

```sh
DBRUST_TEST_POSTGRES_URL='postgres://user:password@localhost/testdb' \
  cargo test postgres_session_when_configured -- --ignored
DBRUST_TEST_MYSQL_URL='mysql://user:password@localhost/testdb' \
  cargo test mysql_session_when_configured -- --ignored
```

PostgreSQL and SQLite have been exercised against real database engines during implementation. MySQL compiles but requires a running server for its integration check.

## Code layout

- `src/main.rs`: terminal lifecycle and event loop.
- `src/app.rs`: screens, focus, editor, scrolling, mouse hit testing, background operations, and UI tests.
- `src/connection.rs`: URL construction, credential removal, and history persistence.
- `src/db.rs`: native database connections, schema listing, query streaming, value rendering, and integration tests.

The implementation follows standard Cargo layout and Rustfmt/Clippy defaults. Rainfrog is the interaction inspiration; this project implements its own application code using Ratatui, ratatui-textarea, Crossterm, and SQLx.
