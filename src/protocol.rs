#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::env;

#[cfg(all(test, not(any(target_os = "linux", target_os = "macos"))))]
use std::path::Path;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use crate::config::Config;
#[cfg(any(target_os = "linux", target_os = "macos", test))]
use anyhow::Context;
use anyhow::{bail, Result};
#[cfg(target_os = "macos")]
use std::os::raw::{c_char, c_void};

pub fn install(config: &Config) -> Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        install_for_current_process(config)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    bail!("devnull:// protocol registration supports macOS and Linux");
}

pub fn ensure_installed(config: &Config) -> Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let executable = env::current_exe().context("could not locate stdout executable")?;
        let marker = config.dir.join("protocol-handler");
        let expected = format!("{}\n{}\n", executable.display(), config.dir.display());
        if fs::read(&marker).is_ok_and(|installed| installed == expected.as_bytes()) {
            return Ok(());
        }
        install_for_current_process(config)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = config;
        Ok(())
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn install_for_current_process(config: &Config) -> Result<()> {
    let executable = env::current_exe().context("could not locate stdout executable")?;

    #[cfg(target_os = "linux")]
    install_linux(&executable, &config.dir)?;

    #[cfg(target_os = "macos")]
    install_macos(&executable, &config.dir)?;

    let expected = format!("{}\n{}\n", executable.display(), config.dir.display());
    config.write_private(&config.dir.join("protocol-handler"), expected.as_bytes())
}

#[cfg(any(target_os = "linux", test))]
fn linux_entry(executable: &Path, config_dir: &Path) -> Result<String> {
    Ok(format!(
        "[Desktop Entry]\nType=Application\nName=stdout callback handler\nNoDisplay=true\nTerminal=false\nMimeType=x-scheme-handler/devnull;\nExec={} --callback %u --config-dir {}\n",
        desktop_argument(executable)?,
        desktop_argument(config_dir)?
    ))
}

#[cfg(any(target_os = "linux", test))]
fn desktop_argument(path: &Path) -> Result<String> {
    let value = path
        .to_str()
        .context("stdout protocol paths must be valid UTF-8")?;
    if value.contains(['\n', '\r']) {
        bail!("stdout protocol paths cannot contain newlines");
    }

    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('`', "\\`")
        .replace('$', "\\$")
        .replace('%', "%%");
    Ok(format!("\"{escaped}\""))
}

#[cfg(target_os = "linux")]
fn install_linux(executable: &Path, config_dir: &Path) -> Result<()> {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")?;
    let data_home = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path: &std::path::PathBuf| path.is_absolute())
        .unwrap_or_else(|| home.join(".local/share"));
    let applications = data_home.join("applications");
    fs::create_dir_all(&applications)?;

    let desktop_id = "stdout-devnull.desktop";
    fs::write(
        applications.join(desktop_id),
        linux_entry(executable, config_dir)?,
    )?;
    run(
        "xdg-mime",
        &["default", desktop_id, "x-scheme-handler/devnull"],
    )
    .context("could not register the devnull:// handler with xdg-mime")?;

    println!("Registered devnull:// for this user.");
    Ok(())
}

#[cfg(target_os = "macos")]
fn install_macos(executable: &Path, config_dir: &Path) -> Result<()> {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")?;
    let applications = home.join("Applications");
    fs::create_dir_all(&applications)?;

    let app = applications.join("stdout URL Handler.app");
    let temporary_app =
        applications.join(format!(".stdout-url-handler-{}.app", std::process::id()));
    let script = applications.join(format!(
        ".stdout-url-handler-{}.applescript",
        std::process::id()
    ));
    if temporary_app.exists() {
        fs::remove_dir_all(&temporary_app)?;
    }
    let result: Result<()> = (|| {
        fs::write(&script, applescript_source(executable, config_dir)?)?;
        run(
            "osacompile",
            &["-o", path_arg(&temporary_app)?, path_arg(&script)?],
        )
        .context("could not build the macOS URL handler with osacompile")?;
        fs::remove_file(&script)?;

        let plist = temporary_app.join("Contents/Info.plist");
        set_plist(
            &plist,
            ":CFBundleIdentifier",
            "string",
            "sh.devnull.stdout-url-handler",
        )?;
        set_plist(&plist, ":CFBundleName", "string", "stdout")?;
        set_plist(&plist, ":LSUIElement", "bool", "true")?;
        run_plist(&plist, "Add :CFBundleURLTypes array")?;
        run_plist(&plist, "Add :CFBundleURLTypes:0 dict")?;
        run_plist(
            &plist,
            "Add :CFBundleURLTypes:0:CFBundleURLName string devnull",
        )?;
        run_plist(
            &plist,
            "Add :CFBundleURLTypes:0:CFBundleTypeRole string Viewer",
        )?;
        run_plist(&plist, "Add :CFBundleURLTypes:0:CFBundleURLSchemes array")?;
        run_plist(
            &plist,
            "Add :CFBundleURLTypes:0:CFBundleURLSchemes:0 string devnull",
        )?;

        if app.exists() {
            fs::remove_dir_all(&app).context("could not replace existing URL handler app")?;
        }
        fs::rename(&temporary_app, &app)?;
        let lsregister = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";
        run(lsregister, &["-f", path_arg(&app)?])
            .context("could not register the macOS URL handler with Launch Services")?;
        set_default_url_handler("sh.devnull.stdout-url-handler")?;
        Ok(())
    })();

    let _ = fs::remove_file(&script);
    if temporary_app.exists() {
        let _ = fs::remove_dir_all(&temporary_app);
    }
    result?;

    println!("Registered devnull:// for this user at {}.", app.display());
    Ok(())
}

#[cfg(target_os = "macos")]
fn applescript_source(executable: &Path, config_dir: &Path) -> Result<String> {
    let executable = applescript_path(executable)?;
    let config_dir = applescript_path(config_dir)?;
    Ok(format!(
        "on run\nend run\n\non open location callbackURI\n    set stdoutPath to \"{executable}\"\n    set configPath to \"{config_dir}\"\n    set callbackCommand to quoted form of stdoutPath & \" --callback \" & quoted form of callbackURI & \" --config-dir \" & quoted form of configPath\n    do shell script callbackCommand\nend open location\n"
    ))
}

#[cfg(target_os = "macos")]
fn applescript_path(path: &Path) -> Result<String> {
    let value = path
        .to_str()
        .context("stdout protocol paths must be valid UTF-8")?;
    if value.contains(['\n', '\r']) {
        bail!("stdout protocol paths cannot contain newlines");
    }
    Ok(value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(target_os = "macos")]
fn set_plist(plist: &Path, key: &str, kind: &str, value: &str) -> Result<()> {
    run_plist(plist, &format!("Set {key} {value}"))
        .or_else(|_| run_plist(plist, &format!("Add {key} {kind} {value}")))
}

#[cfg(target_os = "macos")]
fn run_plist(plist: &Path, command: &str) -> Result<()> {
    run(
        "/usr/libexec/PlistBuddy",
        &["-c", command, path_arg(plist)?],
    )
}

#[cfg(target_os = "macos")]
fn path_arg(path: &Path) -> Result<&str> {
    path.to_str().context("path is not valid UTF-8")
}

#[cfg(target_os = "macos")]
fn set_default_url_handler(bundle_identifier: &str) -> Result<()> {
    use std::ffi::CString;

    let scheme = CString::new("devnull")?;
    let bundle_identifier = CString::new(bundle_identifier)?;
    unsafe {
        let scheme = CFStringCreateWithCString(std::ptr::null(), scheme.as_ptr(), 0x08000100);
        let bundle_identifier =
            CFStringCreateWithCString(std::ptr::null(), bundle_identifier.as_ptr(), 0x08000100);
        if scheme.is_null() || bundle_identifier.is_null() {
            if !scheme.is_null() {
                CFRelease(scheme);
            }
            if !bundle_identifier.is_null() {
                CFRelease(bundle_identifier);
            }
            bail!("could not create Launch Services strings");
        }

        let status = LSSetDefaultHandlerForURLScheme(scheme, bundle_identifier);
        CFRelease(scheme);
        CFRelease(bundle_identifier);
        if status != 0 {
            bail!("Launch Services could not set the devnull:// handler (status {status})");
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithCString(
        allocator: *const c_void,
        c_string: *const c_char,
        encoding: u32,
    ) -> *const c_void;
    fn CFRelease(value: *const c_void);
}

#[cfg(target_os = "macos")]
#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn LSSetDefaultHandlerForURLScheme(
        scheme: *const c_void,
        bundle_identifier: *const c_void,
    ) -> i32;
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn run(program: &str, args: &[&str]) -> Result<()> {
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("could not run {program}"))?;
    if !output.status.success() {
        bail!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::linux_entry;

    #[test]
    fn linux_desktop_entry_escapes_exec_path_and_passes_uri() {
        let entry = linux_entry(
            Path::new("/home/test user/$stdout%"),
            Path::new("/home/test/.config/stdout"),
        )
        .unwrap();
        assert!(entry.contains(
            r#"Exec="/home/test user/\$stdout%%" --callback %u --config-dir "/home/test/.config/stdout""#
        ));
        assert!(entry.contains("MimeType=x-scheme-handler/devnull;"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_handler_shell_quotes_executable_and_uri() {
        let source = super::applescript_source(
            Path::new("/tmp/stdout\"; touch /tmp/pwn"),
            Path::new("/tmp/config dir/stdout"),
        )
        .unwrap();
        assert!(source.contains(r#"/tmp/stdout\"; touch /tmp/pwn"#));
        assert!(source.contains("quoted form of stdoutPath"));
        assert!(source.contains("quoted form of callbackURI"));
        assert!(source.contains("quoted form of configPath"));
    }
}
