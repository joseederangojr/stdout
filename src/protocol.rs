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

#[cfg(any(target_os = "linux", target_os = "macos"))]
use crate::config::Config;
#[cfg(any(target_os = "linux", target_os = "macos", test))]
use anyhow::Context;
use anyhow::{bail, Result};

pub fn install() -> Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let executable = env::current_exe().context("could not locate stdout executable")?;
        let config_dir = Config::load()?.dir;

        #[cfg(target_os = "linux")]
        return install_linux(&executable, &config_dir);

        #[cfg(target_os = "macos")]
        return install_macos(&executable, &config_dir);
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    bail!("devnull:// protocol registration supports macOS and Linux");
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
        set_plist(&plist, ":LSUIElement", "bool", "true")?;
        run_plist(&plist, "Add :CFBundleURLTypes array")?;
        run_plist(&plist, "Add :CFBundleURLTypes:0 dict")?;
        run_plist(
            &plist,
            "Add :CFBundleURLTypes:0:CFBundleURLName string devnull",
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
