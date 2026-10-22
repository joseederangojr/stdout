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

Unauthenticated startup shows a full-screen trusted-device sign-in. Press `o` to open the authorization URL, approve `stdout` in the browser, then paste the complete `devnull://auth/callback?...` URI and press Enter. Press `r` for a fresh authorization or Esc to quit.

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

`auth login` validates the local key against the configured API and prints `Hi @username (email) [device name]` if already authenticated. Otherwise it prompts for a trusted-device name (Enter accepts the saved name or `stdout on <hostname>`), opens browser authorization, and starts the sign-in TUI. `--device-name` supplies the name without prompting. Username, email (when returned by the API), and device name are cached in private `identity.json`; `auth logout` removes that cache and the local key. No device-revoke endpoint is available, so server approval remains until revoked in dev-null.sh settings. Press `q` or `Ctrl-C` to quit.

After sign-in, the TUI currently renders only plain vertical dividers at the 20% / 80% boundaries. Feed content, pane input, and mouse interactions remain deferred; future interaction should be keyboard-first with mouse click and wheel support.

## Structure

```text
src/
├── main.rs         # startup, TUI loop, callback invocation
├── protocol.rs     # macOS/Linux devnull:// handler registration
├── app.rs          # input state and actions
├── auth.rs         # PKCE, browser URL, callback validation
├── api.rs          # SSH-signed public API requests
├── config.rs       # API origin and private config directory
├── device.rs       # local Ed25519 device key and SSHSIG signing
├── event.rs        # terminal keyboard/paste events
├── terminal.rs     # terminal setup and restoration
└── ui.rs           # full-screen sign-in and divider-only signed-in view
```

## Development

```sh
cargo fmt --check
cargo test
cargo clippy -- -D warnings
```
