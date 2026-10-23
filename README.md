# stdout

Rust TUI client for [dev-null.sh](https://github.com/nunomaduro/dev-null.sh). Built with Ratatui and Crossterm. Generated executable: `stdout`. Server access uses the public API only.

## API origin

Resolution order:

1. `DEVNULL_API_ORIGIN` environment variable.
2. `api_origin` in `$XDG_CONFIG_HOME/stdout/config.toml`.
3. `https://dev-null.sh`.

Example local config:

```toml
api_origin = "http://localhost:8000"
```

Without `XDG_CONFIG_HOME`, config lives under `$HOME/.config/stdout/config.toml`. Pass `--config <dir>` to use a different directory for `config.toml` and local credentials. Plain HTTP is restricted to loopback origins.

## Sign in

Unauthenticated startup shows a centered, three-step trusted-device sign-in: **Name device**, **Browser approval**, and **Finish**. The name field starts empty with a saved-name/hostname suggestion; `auth login --device-name "..."` pre-fills it. Enter prepares the authorization URL; use `o` to open or `y` to copy it in Browser approval. A `devnull://` callback can complete automatically, or paste it in Finish. Normal mode uses `j`/`k` to navigate and `i` to edit; Esc returns to Normal. Clicking a step selects it in Visual mode. See the [onboarding PRD](docs/auth-onboarding-prd.md) for details.

On the first TUI launch, `stdout` registers a per-user `devnull://` handler on macOS and Linux. Linux requires `xdg-mime`; macOS uses AppleScript and Launch Services. The handler invokes `stdout --callback <uri>` and forwards the URI to the TUI for state validation and code redemption. Registration refreshes automatically if the executable or config directory changes; `stdout --install-protocol` forces re-registration. Windows currently uses manual callback paste. OpenSSH `ssh-keygen` is required; the Ed25519 device key is stored locally with owner-only permissions.

## Run

Requires Rust/Cargo and an interactive UTF-8 terminal.

```sh
cargo run
cargo run -- auth login
cargo run -- auth login --device-name "Work Mac"
cargo run -- auth logout
cargo run -- --help
```

`auth login` validates the local key and prints `Hi @username (email) [device name]` when authenticated; otherwise it opens onboarding. `--device-name` pre-fills the name field. Username, optional email, and device name are cached in private `identity.json`; `auth logout` removes that cache and the local key. Server approval remains until revoked in dev-null.sh settings. In Normal mode, `q` quits unless configured as a key-sequence prefix; `Ctrl-C` always quits.

After sign-in, the TUI uses four borderless columns at 15%/15%/40%/30%, without visible separators. The first 15% is blank, navigation occupies the second 15%, the center content occupies 40%, and the right 30% remains blank. The center column has a global `// /dev/null` header and “A social network for devs” tagline. On Log, sticky **Following [1]**, **Latest [2]**, and **Trending [3]** tabs sit one blank row below it, stacking when the center column is too narrow to fit them side by side. Two blank rows separate tabs from the feed. Signed `/api/v1/log/{following|latest|trending}` pages load asynchronously; pagination uses the API's opaque `meta.next_cursor`. Following can be empty when you follow nobody; suggestions are separate and not shown. Only API-returned top-level commits appear: no fabricated entries or cached fallback on error. Cards use relaxed spacing between metadata, body, and action row; metadata right-aligns the commit hash. The action row shows Star and Pastel as icons only, Quote and Replies with icons and labels, and copy/report as icons. Each card shows a pending indicator inline when applicable, a 🧵 icon when replies exist, and two blank rows between cards. Valid API `created_at` timestamps display as relative time and refresh each minute; missing, invalid, or future values display as returned. Feed load/error/empty status appears in the viewport. Switching tabs discards stale responses. `j`/`k` or arrow keys move focus and reveal its card; mouse wheel scrolls the Log viewport without changing focus. Actions and hints appear only for a focused card. A fixed screen footer carries hotkeys only on Log with a focused commit; it is hidden while a dialog is open.

Navigation defaults: `g l` Log, `g n` Notifications, `g r` Remotes ↗, `g p` Profile, `g S` Starred, `g s` Settings. `g ,` opens `config.toml` in the default application; `Ctrl+,` opens it in `$EDITOR` (creating it if missing and restoring the TUI afterward). Remotes opens `$API_ORIGIN/remotes` in your browser; it does not use a Remotes API client. Override any binding in `config.toml`:

```toml
[keymap]
log = "g l"
notifications = "g n"
remotes = "g r"
profile = "g p"
starred = "g S"
settings = "g s"
config = "g ,"
```

Bindings are unique two-key sequences; a pending navigation prefix takes precedence over commit shortcuts. Log tabs and scrolling are active only on Log. With a focused Log card, `s` toggles star, `p` opens a Pastel confirmation (Enter sends one from your daily quota; Esc cancels), `r` opens a reply draft, `q` opens a quote draft, `y` copies its API `html_url`, `R` opens a report dialog, `o` opens that URL in a browser, and Enter opens commit detail. `q` with no focus quits (unless a configured navigation prefix takes precedence); `Ctrl-C` always quits. Copy/open accepts only same-origin `http(s)` commit URLs matching the selected ID; clipboard support uses the platform helper and can fail when no clipboard service is available. URL failures do not launch a browser. Clicking an underlined URL in a visible Log card body opens its `http(s)` destination only when no dialog is active; sidebar, tabs, gaps, and non-link text do nothing. Links with credentials or control characters are ignored. Clicks and wheel scrolling require terminal mouse-reporting support; without it, use `o` on a focused card to open its verified commit URL. Browser launch failures appear in the Log status. The detail dialog displays commit body and supports `j`/`k` scrolling, `r`/`q`/`R` actions, Esc close. Drafts accept typing, paste and Backspace (4,000 characters max); Enter submits a nonempty reply/quote. Reports require `1`–`4` reason then two Enter presses (confirm and send). Esc cancels; opening a dialog never writes. Requests run off the UI thread, with status/errors displayed; a failed write keeps its draft for retry. Onboarding step selection also uses mouse clicks.

Current limits: detail fetch loads commit data, but ancestors and replies are not displayed as a full thread; no replies pagination or inline star count is shown. Successful reply/quote does not synthesize a feed card: revisit the tab to reload it. API moderation may leave new commits pending; first-ever posting requires accepting server rules on the website. Report is limited to eligible approved commits by other authors; invalid writes surface API errors. See [signed Log API contract](docs/log-api-contract.md) and [onboarding/Log behavior](docs/auth-onboarding-prd.md).

## Structure

```text
src/
├── main.rs         # startup, TUI loop, callback invocation
├── protocol.rs     # macOS/Linux devnull:// handler registration
├── app.rs          # sign-in input state and actions
├── navigation.rs   # signed-in sections and key sequences
├── auth.rs         # PKCE, browser URL, callback validation
├── api.rs          # SSH-signed public API requests
├── clipboard.rs    # cross-platform URL clipboard copy helper
├── config.rs       # API origin, keymap, and private config directory
├── device.rs       # local Ed25519 device key and SSHSIG signing
├── event.rs        # terminal keyboard, mouse, and paste events
├── terminal.rs     # terminal setup and restoration
└── ui.rs           # sign-in flow and signed-in navigation sidebar
```

## Development

```sh
cargo fmt --check
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo build
git diff --check
```
