# stdout

Rust terminal client for [dev-null.sh](https://dev-null.sh). Project and CLI name: `stdout`; config: `~/.config/stdout`. Uses the documented `/api/v1` JSON API, not website cookies or HTML.

```sh
cargo run --release
# or: cargo install --path . && stdout

# Opt in to the in-memory API request/response log
cargo run --release -- --log
# or: stdout -l
```

Latest opens by default. On terminals 100 columns and wider, layout uses 20% navigation, 60% feed, and 20% invisible right spacer; below that, the responsive two-column/narrow layout remains. Sidebar: `/dev/null` text logo (white first slash and letters, orange second slash), `@current-user`, Log, Notification, Profile, Starred, Settings. Log tabs: Latest, Trending, Following. Generous spacing; no framed section titles or inverted selection. Focus uses a subtle accent marker. Accent and lane colors follow dev-null.sh's light/dark palettes; theme detection queries OSC 11 with `COLORFGBG` fallback. Terminal foreground/background stay inherited.

Rendering is text-only: no remote image/avatar fetching, terminal graphics, media cards, or syntax highlighting. Image attachments and link previews retain their text metadata and URLs; visible links, including image links, open on terminal mouse click (or `o` opens the selected commit on the web). Quoted commits render nested inside their parent, with the quoted author's lane color. Fenced code remains plain text. Upstream supplies at most one Open Graph preview per commit; no website scraping or embedded players. `Alt+Enter` inserts a composer newline.

## Controls

- `j/k` select Log items; `↑/↓` move focus; `h/l` switch panes; `Tab` cycles panes.
- On the Log surface only: `1` Latest, `2` Trending, `3` Following. `g l` is the only chord to jump to a log; it opens Latest. Initial view: Latest.
- `Enter` opens a thread/profile; `o` opens the selected commit/user on the web; `Alt+Enter` inserts a composer newline; `g p` opens your profile; `g P` opens the selected commit author's profile; `g s` opens Starred. Profile tabs: `a` commits, `e` replies.
- `s` star (no-op if already starred); `S` unstar. `GET /api/v1/starred` backs Starred. `r` reply; `Q` quote; `Space` expand/collapse nested replies; `]` load more.
- `q` quits outside text-entry modes (`Ctrl+C` also quits); `Q` quotes. `f` follow/unfollow; `m` mute/unmute; `B` block/unblock with confirmation.
- `:` opens the searchable command center. `?` shows keyboard shortcuts. `/` and `Ctrl+K` search via the official website because `/api/v1` has no developer/commit search endpoint. `Super+K` submits in the composer; `Super+Shift+P` previews. `Super+K` is not a search shortcut and `Super+P` does not open the command center.
- `stdout --log` or `stdout -l` enables an in-memory log of dev-null.sh API methods, paths, status codes, durations, byte counts, and errors (last 100 requests). `Ctrl+Shift+L` opens the popup (`Ctrl+L` also works in terminals that collapse Shift); the command center offers “View API request log” only while logging is enabled. Request/response bodies, auth headers, and signatures are never logged.
- `Alt+S` Settings; `N` Notifications; `L` sign in; `R` refresh; `y` copy link.

`/api/v1` supports star/unstar with `POST`/`DELETE /api/v1/commits/{id}/star`; `GET /api/v1/starred` lists starred commits. Upstream checked: `origin/production` at `aa319e3`. Notifications/profile/new-commit counts poll read-only every 60s; listing `/api/v1/notifications` marks notifications read, so polling never uses that endpoint. New unread notifications show a popup.

Composer supports text, replies, quotes and local preview. Image upload, polls and scheduling remain visibly unavailable because the current API does not support them.

## Sign in and local files

`L` starts device approval. The app creates an ed25519 key under `~/.config/stdout/device_ed25519`; `devnull://` handler registration remains opt-in via Enter. No OS handler is changed automatically. Manual approval fallback: browser Network → `POST /mobile/authorize` 303 → copy `Location` and paste it in the TUI. Signed requests use `ssh-keygen -Y sign -n devnull-api`.

To use a local upstream server, set `STDOUT_API_ORIGIN=http://localhost:8000` when launching `stdout`; this also opens device approval on the local server. The default remains `https://dev-null.sh`. Only that production origin or HTTP loopback addresses are accepted. The upstream `DatabaseSeeder` generates local sample records; this is not a copy of production user data. Use a dedicated local database for server setup and seeding.

On startup, an existing `~/.config/dev-null-tui/device_ed25519` and `config.toml` move to `~/.config/stdout/` only when the new destination is absent; the new directory/key/config are restricted to the current user. If you reinstall/move the binary, explicitly re-register the callback through `L` → Enter. Existing OS associations are not changed by startup migration.

Override bindings in `~/.config/stdout/config.toml`:

```toml
[keymap]
reply = "R"
refresh = "r"
go_latest = "g z"
go_starred = "g b"
search_ctrl = "Alt+K"
```

Run checks: `cargo fmt --check && cargo test && cargo clippy --all-targets -- -D warnings`. Signed actions and account-specific profile data need a real approved device for live verification.
