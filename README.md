# dbrust

An approachable Rust terminal database workbench, inspired by [Rainfrog](https://github.com/achristmascarl/rainfrog), [gobang](https://github.com/TaKO8Ki/gobang), and [Redis Insight](https://github.com/redis/RedisInsight). Navigate with arrows, Tab, Enter, and the mouse.

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

- **Connection URL:** enter a PostgreSQL, MySQL, SQLite, Redis, Valkey, or KeyDB URL. The input is masked because URLs may contain passwords.
- **Manual fields:** driver, host, port, database, username, and password. Left/Right on the driver field cycles `postgres`, `mysql`, `sqlite`, `redis`, `rediss`, `valkey`, and `keydb`. Blank ports use 5432 for PostgreSQL or 3306 for MySQL, or 6379 for Redis-compatible servers. Redis Database is a nonnegative logical database number (default 0); username and password are optional. For SQLite, enter a file path or `:memory:` in Database; the other fields are ignored. Manual SQLite connections create the file if needed.
- **Recent connections:** choose a successful connection with Up/Down, then Enter or click Connect. Passwords are requested again when needed.

Tab and Shift+Tab move focus. Enter advances manual fields and connects from the last field or the Connect button. Click any field to place its cursor. Connections have a 15-second timeout.

URL examples:

```text
postgres://alice:password@localhost:5432/app
mysql://alice:password@localhost:3306/app
sqlite::memory:
sqlite://./example.db?mode=rwc
redis://localhost:6379/0
redis://default:password@localhost:6379/2
rediss://default:password@redis.example.com:6379/0
valkey://localhost:6379/0
keydb://localhost:6379/0
```

URL credentials must be percent-encoded. Manual fields accept literal credentials, including punctuation, spaces, and percent signs.

Successful connections are stored in the platform config directory, normally `~/.config/dbrust/connections.json` on Linux (respects `XDG_CONFIG_HOME`). The most recent 20 distinct connections are retained. Password components and password query parameters are removed before saving; Unix history files use mode `0600`. Usernames, database names, hosts, and other connection options are retained. Passwords stay in memory only for connection entry and execution; there is no credential vault integration.

## Work with SQL

The workspace has a table browser, multiline SQL editor, results grid with schema tabs, and Run button. The mint border marks keyboard focus.

| Control | Action |
| --- | --- |
| Tab / Shift+Tab | Cycle Browser → Editor → Results, or reverse; Run is excluded |
| Arrows | Select tables/cells, or move the editor cursor |
| Enter on a table | Load and execute a preview of up to 1,000 rows |
| Enter in the editor | Insert a newline |
| F5 / Ctrl+Enter | Execute from any workspace panel; Run also accepts mouse clicks |
| Enter in results | Open the full cell value, with wrapping and scrolling |
| Shift+arrows | Scroll the focused panel by 1 row or display column |
| Ctrl+arrows / Alt+arrows | Scroll by 10 rows or display columns |
| PageUp / PageDown | Scroll results by a page |
| Home / End in results | First cell / last row |
| Esc | Focus Browser; close full-value/filter popup; return from connection switching |
| F2 | Open connection selection |
| F3 | Refresh tables, or continue/rescan Redis keys |
| F4 in Redis | Name/type/expiry filters and TTL bounds |
| Ctrl+C / Ctrl+V | Copy current text/key/cell; paste into the active text field |
| Ctrl+A / Ctrl+Z / Ctrl+Y | Select all / undo / redo in a text field |
| F6 / Shift+F6 | Next / previous SQL table view |
| 1–5 outside editor / Alt+1–5 anywhere | Records / Columns / Indexes / Foreign keys / DDL |
| Left / Right in SQL Browser | Previous / next table view |
| F1 | Toggle help |
| Ctrl+Q | Quit |

Use Alt+arrows if your terminal intercepts Ctrl+arrows. F5 is available when the terminal sends Ctrl+Enter as plain Enter. The editor supports bracketed paste, Unicode, and undo/redo (Ctrl+Z / Ctrl+Y).

### Mouse

- Click a panel to focus it, a text field or SQL text to position its cursor, or a result cell to select it. Enter opens the full value viewer, where arrows, PageUp/PageDown, and the wheel scroll. Enter, Esc, or a click closes the viewer.
- Click a table or Redis key to open its preview. Click the schema tabs to switch table views. Click Run, Connections, Refresh, Help, or a connection method to activate it.
- Scroll the panel under the pointer using the vertical wheel or horizontal wheel/trackpad events.
- Shift+wheel scrolls horizontally; Ctrl/Alt+wheel scrolls faster.
- Click or drag either result scrollbar to jump through rows or columns.

The terminal must forward mouse events to the application. Some terminal emulators reserve modified wheel gestures for their own zoom or scrolling.

### Query behavior

PostgreSQL, MySQL, and SQLite use native SQLx connections. A persistent connection preserves session state and explicit transactions. Queries run asynchronously while the UI remains responsive, with one database operation at a time. SQL is submitted as written, including writes and multiple statements; no confirmation is added. Earlier statements can succeed before a later statement fails. Use SQL transactions when you need atomic changes.

The grid retains up to 10,000 rows from the last **nonempty** result set. It drains the entire query stream so submitted statements complete, including statements after the display cap. Use SQL `LIMIT` to reduce server work and transfer volume. Multiple result sets are not individually selectable. Empty standalone `SELECT` queries retain their column headers.

Columns have a fixed display width with horizontal scrolling. The selected cell appears in the footer; Enter opens a scrollable full-value viewer. Common scalar, date/time, decimal, JSON, UUID, and binary values are displayed where supported by each driver. Other types show an explicit unsupported-type marker. Query cancellation, autocomplete, syntax highlighting, export, and saved query history are not implemented yet.

## Copy and paste

Ctrl+C copies the selected result cell, full opened value, selected key/table name, or editor/field selection. In a text field without a selection, it copies the whole field. Ctrl+A selects all text; dragging across SQL/command text selects part of it. Ctrl+V pastes into the active editor, connection field, filter, or value-search field. Multiline query text is preserved; single-line fields remove pasted line breaks. Paste replaces a selection and can be undone with Ctrl+Z (Ctrl+Y redoes it).

Desktop clipboard integration supports X11, Wayland, Windows, and macOS through arboard. If no desktop clipboard is available, copied text stays in an internal clipboard so Ctrl+C/Ctrl+V still work within dbrust. Set `DBRUST_CLIPBOARD=internal` to force that mode. Terminal-managed paste (often Ctrl+Shift+V) remains available in remote/headless sessions. Ctrl+Q quits; Ctrl+C copies.

The same shortcuts work in PostgreSQL, MySQL, and SQLite: copy a result cell or table name, then Shift+Tab/Tab to the SQL editor and Ctrl+V. Query selections and multiline SQL can also be copied and pasted. The SQL terminal smoke test copies a query from a result cell, pastes it into the editor, and executes it.

## Table schema views

Select a table and use F6, 1–5 outside the editor, Alt+1–5, or the clickable result tabs. Records previews data; Columns shows types, nullability, defaults, and available driver metadata; Indexes and Foreign keys show definitions or field mappings. DDL shows SQLite table/index/trigger SQL and MySQL `SHOW CREATE TABLE`. PostgreSQL DDL lists catalog-derived column and constraint definitions, including defaults, generated columns, and identities; it is an inspection view, not a complete schema export.

Schema browsing does not replace editor text. Switching back to Records loads the selected table's preview query. Empty metadata views keep their column headers.

## Redis, Valkey, and KeyDB

Standalone Redis-compatible connections use a key browser instead of tables and a Redis command editor instead of SQL. `redis://` is plaintext; `rediss://` uses TLS. `valkey://` and `keydb://` are plaintext aliases; use `rediss://` for TLS with any compatible server. Choose a database number in the connection form or URL. Sentinel and cluster routing are not implemented.

The browser uses `SCAN`, never `KEYS`, and deduplicates names. Each refresh performs at most 5 SCAN calls, using COUNT 200 as a hint, and retains at most 10,000 keys. F3 continues an unfinished scan; after completion it starts a fresh scan. Shift+F3 explicitly discards the scan and starts again. The toolbar distinguishes matched keys, SCAN-returned candidates, and the full database size; returned counts can include duplicates and do not measure all internally visited keys. F4 opens the filter form: combine a server-side glob MATCH pattern such as `users:*`, a server-side key-type filter, expiry state (any / persistent / expiring), and minimum/maximum TTL in whole seconds. TTL conditions use pipelined TYPE/PTTL metadata; they are applied to the scanned snapshot. Tab moves between controls, Left/Right changes type/expiry, Enter applies, and Esc cancels. Invalid TTL ranges keep the form open. The Clear/Reset control removes all filters. SCAN is not a snapshot, so keys may change or expire while browsing.

The Redis screen has a persistent pattern/type/expiry toolbar, a key browser with colored type labels and TTLs, a large value inspector, and a command editor below it. Clicking filter controls opens the same keyboard-accessible form. Empty states distinguish a completed scan with no matches from a partial scan that can continue.

- F6 toggles flat keys and namespace tree views (colon separator). Enter/click drills into a folder; Backspace in Browser moves up. Namespace grouping covers loaded keys, not the entire unseen database.
- F7 cycles sorting by name, type, or TTL; F8 reverses it. Persistent keys sort after expiring keys in ascending TTL order.
- F9 searches the loaded inspector rows, case-insensitively. A blank query restores them. This is a loaded-preview search, not a server-side full-keyspace value search.
- Pattern/type filters run on the server; expiry/TTL filters use metadata collected during the scan. Refresh/rescan updates those snapshots as keys expire or change.

Enter or click a key to view its type, TTL, and data. The inspector shows a type/TTL/length header, multiline JSON/text, and type-specific field/value, member/score, or stream-entry tables. Enter opens the complete loaded string preview or selected cell. Strings preview up to 64 KiB and pretty-print JSON when valid. Lists, hashes, sets, sorted sets, and streams preview up to 200 items, with an explicit limited-preview indicator. Collection SCAN counts are hints and may return a partial preview even for smaller collections. Binary key names and values are shown as hex; underlying key identities remain byte-safe even when displayed in the namespace browser. Enter on a value opens its full preview.

Execute one shell-quoted Redis command per line with F5, Ctrl+Enter, or a mouse click on Run:

```text
SET greeting 'hello world'
GET greeting
HSET user:1 name Alice role admin
EXPIRE user:1 3600
```

Responses are shown in the result grid. Writes execute directly; F3 refreshes the browser afterward. Quoting and incompatible commands are checked before a script starts, but a server error can still leave earlier writes applied. Redis responses have a 15-second timeout. Protocol/session-changing, transaction, and subscription commands are not supported in this editor; choose the logical database through Connections and use `redis-cli` for transactions and streaming subscriptions. Module-specific types can be inspected with commands; they do not yet have dedicated previews.

## Validate

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build --locked
python3 scripts/smoke_tui.py
```

The Python smoke test uses a real Unix pseudo-terminal, an isolated config directory, and SQLite in memory. It exercises connection entry, bracketed paste, multiline SQL execution, function keys, history creation, and clean exit.

Five server integration tests are explicitly ignored in the default run. Run them against dedicated test databases:

```sh
DBRUST_TEST_POSTGRES_URL='postgres://user:password@localhost/testdb' \
  cargo test postgres_session_when_configured -- --ignored
DBRUST_TEST_MYSQL_URL='mysql://user:password@localhost/testdb' \
  cargo test mysql_session_when_configured -- --ignored
DBRUST_TEST_REDIS_URL='redis://:password@localhost:6379/0' \
  cargo test redis_ -- --ignored
DBRUST_TEST_REDIS_URL='redis://:password@localhost:6379/0' \
  python3 scripts/smoke_redis_tui.py
```

SQLite, PostgreSQL 18, Redis 8, Valkey 8, and MariaDB 11 (MySQL protocol) have been exercised against real database engines. KeyDB uses the same standalone RESP path but has not been live-tested separately.

## Code layout

- `src/main.rs`: terminal lifecycle and event loop.
- `src/app.rs`: screens, focus, editor, scrolling, mouse hit testing, background operations, and UI tests.
- `src/connection.rs`: URL construction, credential removal, and history persistence.
- `src/db.rs`: native database connections, schema listing, query streaming, value rendering, and integration tests.
- `src/schema.rs`: driver-specific columns, indexes, foreign keys, and definitions.
- `src/redis_db.rs`: bounded SCAN browser, pipelined metadata, typed previews, Redis commands, and integration tests.
- `src/redis_browser.rs`: key filters, TTL predicates, namespace grouping, and sorting.

The implementation follows standard Cargo layout and Rustfmt/Clippy defaults, using Ratatui, ratatui-textarea, Crossterm, SQLx, redis-rs, and arboard.

The Redis browser interaction design was informed by Redis Insight’s public `KeysHeader`, `KeysSummary`, and `KeyDetailsHeader` components; this TUI uses its own Rust implementation.
