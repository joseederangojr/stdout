use std::{
    fs,
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const SOCKET: &str = "callback.sock";

pub fn listen(dir: &Path) -> Result<UnixListener, String> {
    let path = dir.join(SOCKET);
    if path.exists() {
        if UnixStream::connect(&path).is_ok() {
            return Err("Another /dev/null login is already waiting for a callback".into());
        }
        fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    let listener = UnixListener::bind(&path).map_err(|e| e.to_string())?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    Ok(listener)
}

pub fn socket_path(key: &Path) -> PathBuf {
    key.parent()
        .expect("key has a parent directory")
        .join(SOCKET)
}

pub fn receive(key: &Path, url: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| e.to_string())?;
    if parsed.scheme() != "devnull"
        || parsed.host_str() != Some("auth")
        || parsed.path() != "/callback"
    {
        return Err("Unexpected authorization callback".into());
    }
    if url.len() > 4096 {
        return Err("Authorization callback too long".into());
    }
    UnixStream::connect(socket_path(key))
        .map_err(|_| "No running TUI is waiting for a login callback".to_string())?
        .write_all(url.as_bytes())
        .map_err(|e| e.to_string())
}

pub fn poll(listener: &UnixListener) -> Result<Option<String>, String> {
    let (stream, _) = match listener.accept() {
        Ok(pair) => pair,
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(None),
        Err(e) => return Err(format!("accept: {e}")),
    };
    stream.set_nonblocking(true).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    let start = Instant::now();
    let mut chunk = [0u8; 1024];
    loop {
        match (&stream).read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => bytes.extend_from_slice(&chunk[..count]),
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    && start.elapsed() < Duration::from_millis(500) =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => return Err(format!("read: {e}")),
        }
        if bytes.len() > 4096 {
            return Err("Authorization callback too long".into());
        }
    }
    if bytes.len() > 4096 {
        return Err("Authorization callback too long".into());
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|e| e.to_string())
}

#[cfg(target_os = "macos")]
pub fn install(key: &Path) -> Result<(), String> {
    use std::process::Command;

    const BUNDLE_ID: &str = "sh.dev-null.tui.callback";
    const REGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";
    let dir = key.parent().ok_or("Missing key directory")?;
    let app = dir.join("Callback.app");
    let existing = handler()?;
    if !existing.is_empty() && existing != BUNDLE_ID {
        return Err(format!("devnull:// belongs to {existing}; refusing to replace it. Use your browser's Network → 303 Location instead."));
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let path = exe.to_str().ok_or("Executable path is not UTF-8")?;
    if path.contains(['\n', '\r']) {
        return Err("Executable path contains newline".into());
    }
    let sh_path = format!("'{}'", path.replace('\'', "'\\''"));
    let osa_path = sh_path.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!("do shell script \"{osa_path} --callback \" & quoted form of theURL");
    let staged = dir.join(format!("Callback-{}.app", rand::random::<u64>()));
    run(Command::new("/usr/bin/osacompile")
        .arg("-o")
        .arg(&staged)
        .args([
            "-e",
            "on open location theURL",
            "-e",
            &script,
            "-e",
            "end open location",
        ]))?;
    let plist = staged.join("Contents/Info.plist");
    let configured = run(Command::new("/usr/libexec/PlistBuddy")
        .args([
            "-c",
            &format!("Add :CFBundleIdentifier string {BUNDLE_ID}"),
            "-c",
            "Add :CFBundleURLTypes array",
            "-c",
            "Add :CFBundleURLTypes:0 dict",
            "-c",
            "Add :CFBundleURLTypes:0:CFBundleURLName string stdout",
            "-c",
            "Add :CFBundleURLTypes:0:CFBundleURLSchemes array",
            "-c",
            "Add :CFBundleURLTypes:0:CFBundleURLSchemes:0 string devnull",
        ])
        .arg(&plist));
    if let Err(error) = configured {
        let _ = fs::remove_dir_all(&staged);
        return Err(error);
    }
    run(Command::new("/usr/bin/codesign")
        .args(["--force", "--deep", "--sign", "-"])
        .arg(&staged))?;
    if app.exists() {
        fs::remove_dir_all(&app).map_err(|e| e.to_string())?;
    }
    fs::rename(&staged, &app).map_err(|e| e.to_string())?;
    run(Command::new(REGISTER).arg("-f").arg(&app))?;
    let scheme = "let scheme = CFStringCreateWithCString(kCFAllocatorDefault, \"devnull\", CFStringBuiltInEncodings.UTF8.rawValue)!";
    let bundle = format!("let bundle = CFStringCreateWithCString(kCFAllocatorDefault, \"{BUNDLE_ID}\", CFStringBuiltInEncodings.UTF8.rawValue)!");
    let code = format!("import CoreServices\n{scheme}\n{bundle}\nlet result = LSSetDefaultHandlerForURLScheme(scheme, bundle)\nif result != noErr {{ fatalError(\"Launch Services: \\(result)\") }}");
    run(Command::new("/usr/bin/swift").arg("-e").arg(code))?;
    if handler()? != BUNDLE_ID {
        return Err("macOS did not register the devnull:// handler".into());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn install(_key: &Path) -> Result<(), String> {
    use std::process::Command;

    const DESKTOP: &str = "dev-null-tui.desktop";
    let existing = Command::new("xdg-mime")
        .args(["query", "default", "x-scheme-handler/devnull"])
        .output()
        .map_err(|e| format!("xdg-mime: {e}"))?;
    if !existing.status.success() {
        return Err(format!(
            "xdg-mime: {}",
            String::from_utf8_lossy(&existing.stderr)
        ));
    }
    let current = String::from_utf8_lossy(&existing.stdout).trim().to_string();
    if !current.is_empty() && current != DESKTOP {
        return Err(format!("devnull:// belongs to {current}; refusing to replace it. Copy the 303 Location instead."));
    }
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(home).join(".local/share"));
    let apps = data.join("applications");
    fs::create_dir_all(&apps).map_err(|e| e.to_string())?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.to_str().ok_or("Executable path is not UTF-8")?;
    if exe.contains(['\n', '\r']) {
        return Err("Executable path contains newline".into());
    }
    let exe = exe
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
        .replace('`', "\\`");
    fs::write(apps.join(DESKTOP), format!("[Desktop Entry]\nType=Application\nName=stdout callback\nExec=\"{exe}\" --callback %u\nNoDisplay=true\nTerminal=false\nMimeType=x-scheme-handler/devnull;\n"))
        .map_err(|e| e.to_string())?;
    let set = Command::new("xdg-mime")
        .args(["default", DESKTOP, "x-scheme-handler/devnull"])
        .output()
        .map_err(|e| format!("xdg-mime: {e}"))?;
    if !set.status.success() {
        return Err(format!(
            "xdg-mime: {}",
            String::from_utf8_lossy(&set.stderr)
        ));
    }
    let verify = Command::new("xdg-mime")
        .args(["query", "default", "x-scheme-handler/devnull"])
        .output()
        .map_err(|e| e.to_string())?;
    if !verify.status.success() || String::from_utf8_lossy(&verify.stdout).trim() != DESKTOP {
        return Err(
            "The desktop did not register the devnull:// handler; use manual approval".into(),
        );
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn handler() -> Result<String, String> {
    let code = "import CoreServices\nlet scheme = CFStringCreateWithCString(kCFAllocatorDefault, \"devnull\", CFStringBuiltInEncodings.UTF8.rawValue)!\nif let value = LSCopyDefaultHandlerForURLScheme(scheme)?.takeRetainedValue() { print(value) }";
    let output = std::process::Command::new("/usr/bin/swift")
        .arg("-e")
        .arg(code)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(target_os = "macos")]
fn run(command: &mut std::process::Command) -> Result<(), String> {
    let output = command.output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_is_delivered_without_browser_or_disk_secrets() {
        let dir = std::env::temp_dir().join(format!("dev-null-socket-{}", rand::random::<u64>()));
        fs::create_dir(&dir).unwrap();
        let key = dir.join("device_ed25519");
        let listener = listen(&dir).unwrap();
        let url = "devnull://auth/callback?code=sample&state=sample";
        receive(&key, url).unwrap();
        assert_eq!(poll(&listener).unwrap().as_deref(), Some(url));
        assert!(receive(&key, "https://evil.test/?code=x").is_err());
        drop(listener);
        fs::remove_dir_all(dir).unwrap();
    }
}
