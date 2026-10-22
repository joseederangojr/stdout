use std::{
    io::Write,
    process::{Command, Stdio},
};

use anyhow::{bail, Context, Result};

#[cfg(target_os = "macos")]
pub fn copy(value: &str) -> Result<()> {
    copy_with_commands(value, &[("pbcopy", &[])])
}

#[cfg(target_os = "linux")]
pub fn copy(value: &str) -> Result<()> {
    copy_with_commands(
        value,
        &[
            ("wl-copy", &[]),
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
        ],
    )
}

#[cfg(target_os = "windows")]
pub fn copy(value: &str) -> Result<()> {
    copy_with_commands(value, &[("clip", &[])])
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub fn copy(_value: &str) -> Result<()> {
    bail!("clipboard copy is unsupported on this platform")
}

fn copy_with_commands(value: &str, commands: &[(&str, &[&str])]) -> Result<()> {
    let mut failures = Vec::with_capacity(commands.len());
    for (command, args) in commands {
        match run_copy_command(command, args, value) {
            Ok(()) => return Ok(()),
            Err(error) => failures.push(format!("{command}: {error:#}")),
        }
    }

    bail!(
        "no clipboard command succeeded; tried {}",
        failures.join("; ")
    )
}

fn run_copy_command(command: &str, args: &[&str], value: &str) -> Result<()> {
    let mut child = Command::new(command)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("launching {command}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(value.as_bytes())
            .with_context(|| format!("writing clipboard payload for {command}"))?;
    }

    let output = child
        .wait_with_output()
        .with_context(|| format!("waiting for {command}"))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if stderr.is_empty() {
        bail!("exited with status {}", output.status);
    }

    bail!("{stderr}")
}
