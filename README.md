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

Without `XDG_CONFIG_HOME`, config lives under `$HOME/.config/stdout/config.toml`. Plain HTTP is restricted to loopback origins.

## Sign in

Unauthenticated startup shows a full-screen trusted-device sign-in. Press `o` to open the authorization URL, approve `stdout` in the browser, then paste the complete `devnull://auth/callback?...` URI and press Enter. Press `r` for a fresh authorization or Esc to quit.

For automatic callback delivery, run `stdout --install-protocol` once. It registers a per-user `devnull://` handler on macOS and Linux; Linux requires `xdg-mime`, macOS uses the built-in AppleScript and Launch Services tools. The handler invokes `stdout --callback <uri>`, which forwards the URI to the running TUI for state validation and code redemption. Registration captures the executable and config directory; rerun it after moving the binary or changing `XDG_CONFIG_HOME`. Manual paste remains available. OpenSSH `ssh-keygen` is required; the Ed25519 device key is stored locally with owner-only permissions.

## Run

Requires Rust/Cargo and an interactive UTF-8 terminal.

```sh
cargo run
```

Press `q` or `Ctrl-C` to quit.

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
