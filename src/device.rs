use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};

pub struct DeviceKey {
    private_path: PathBuf,
    fingerprint: String,
}

impl DeviceKey {
    pub fn load_or_create(dir: &Path) -> Result<Self> {
        set_dir_permissions(dir)?;
        let private_path = dir.join("device_ed25519");
        let public_path = dir.join("device_ed25519.pub");

        if !private_path.exists() {
            if public_path.exists() {
                fs::remove_file(&public_path).context("removing orphaned device public key")?;
            }
            let output = Command::new("ssh-keygen")
                .args([
                    "-q",
                    "-t",
                    "ed25519",
                    "-N",
                    "",
                    "-C",
                    "stdout trusted device",
                    "-f",
                ])
                .arg(&private_path)
                .output()
                .context("ssh-keygen is required to create the trusted-device key")?;
            if !output.status.success() {
                bail!("ssh-keygen could not create the trusted-device key");
            }
        }

        set_private_permissions(&private_path)?;
        if !public_path.exists() {
            let output = Command::new("ssh-keygen")
                .arg("-y")
                .arg("-f")
                .arg(&private_path)
                .output()
                .context("ssh-keygen is required to restore the trusted-device public key")?;
            if !output.status.success() {
                bail!("could not derive the trusted-device public key from its private key");
            }
            fs::write(&public_path, output.stdout)
                .with_context(|| format!("restoring {}", public_path.display()))?;
        }
        let output = Command::new("ssh-keygen")
            .arg("-lf")
            .arg(&public_path)
            .args(["-E", "sha256"])
            .output()
            .context("ssh-keygen is required to inspect the trusted-device key")?;
        if !output.status.success() {
            bail!("ssh-keygen could not read the trusted-device public key");
        }

        let fingerprint = String::from_utf8(output.stdout)?
            .split_whitespace()
            .find(|part| part.starts_with("SHA256:"))
            .context("ssh-keygen returned no SHA256 fingerprint")?
            .to_owned();

        Ok(Self {
            private_path,
            fingerprint,
        })
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub fn sign(&self, message: &[u8], namespace: &str) -> Result<String> {
        let suffix = random_suffix()?;
        let temp_dir = self
            .private_path
            .parent()
            .context("trusted-device key has no parent directory")?;
        let input_path = temp_dir.join(format!(".stdout-{suffix}.message"));
        let signature_path = PathBuf::from(format!("{}.sig", input_path.display()));
        let _cleanup = TempFiles {
            input: input_path.clone(),
            signature: signature_path.clone(),
        };

        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut input = options.open(&input_path)?;
        input.write_all(message)?;
        input.sync_all()?;

        let output = Command::new("ssh-keygen")
            .args(["-q", "-Y", "sign", "-f"])
            .arg(&self.private_path)
            .args(["-n", namespace, "-O", "hashalg=sha512"])
            .arg(&input_path)
            .output()
            .context("ssh-keygen is required to sign API requests")?;
        if !output.status.success() {
            bail!("ssh-keygen could not sign the API request");
        }

        let signature = fs::read_to_string(&signature_path)
            .with_context(|| "ssh-keygen did not produce an SSH signature")?;
        let encoded: String = signature
            .lines()
            .filter(|line| !line.starts_with("-----"))
            .collect();
        let raw = STANDARD
            .decode(encoded)
            .context("ssh-keygen produced a malformed signature")?;
        Ok(STANDARD.encode(raw))
    }
}

struct TempFiles {
    input: PathBuf,
    signature: PathBuf,
}

impl Drop for TempFiles {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.input);
        let _ = fs::remove_file(&self.signature);
    }
}

fn random_suffix() -> Result<String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|error| anyhow::anyhow!("getting secure random bytes: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn set_dir_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn set_private_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::Write,
        path::PathBuf,
        process::{Command, Stdio},
    };

    use base64::{engine::general_purpose::STANDARD, Engine};

    use super::{random_suffix, DeviceKey};

    #[test]
    fn generated_key_signs_ssh_signature_messages() {
        if Command::new("ssh-keygen").arg("-?").output().is_err() {
            return;
        }

        let dir = test_dir();
        fs::create_dir_all(&dir).unwrap();
        let key = DeviceKey::load_or_create(&dir).unwrap();
        let message = b"test signed request";
        let signature = key.sign(message, "devnull-api").unwrap();
        let decoded = STANDARD.decode(&signature).unwrap();
        assert!(decoded.starts_with(b"SSHSIG"));
        assert!(key.fingerprint().starts_with("SHA256:"));

        let public_key = fs::read_to_string(dir.join("device_ed25519.pub")).unwrap();
        let allowed_signers = dir.join("allowed_signers");
        fs::write(&allowed_signers, format!("stdout {}", public_key.trim())).unwrap();
        let signature_file = dir.join("test.sig");
        fs::write(
            &signature_file,
            format!("-----BEGIN SSH SIGNATURE-----\n{signature}\n-----END SSH SIGNATURE-----\n"),
        )
        .unwrap();

        let mut verifier = Command::new("ssh-keygen")
            .args(["-Y", "verify", "-f"])
            .arg(allowed_signers)
            .args(["-I", "stdout", "-n", "devnull-api", "-s"])
            .arg(signature_file)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        verifier.stdin.take().unwrap().write_all(message).unwrap();
        let output = verifier.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_public_key_is_rebuilt_from_private_key() {
        if Command::new("ssh-keygen").arg("-?").output().is_err() {
            return;
        }

        let dir = test_dir();
        fs::create_dir_all(&dir).unwrap();
        let original = DeviceKey::load_or_create(&dir)
            .unwrap()
            .fingerprint()
            .to_owned();
        fs::remove_file(dir.join("device_ed25519.pub")).unwrap();

        let restored = DeviceKey::load_or_create(&dir).unwrap();
        assert_eq!(original, restored.fingerprint());
        assert!(dir.join("device_ed25519.pub").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    fn test_dir() -> PathBuf {
        std::env::temp_dir().join(format!("stdout-device-test-{}", random_suffix().unwrap()))
    }
}
