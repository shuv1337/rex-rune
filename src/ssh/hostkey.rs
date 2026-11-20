//! OpenSSH decides whether a key is known. Scanning only supplies the fingerprint
//! and bytes that the person can choose to append.
use super::{KeyProblem, Target, lookup};
use anyhow::{Context, Result, bail, ensure};
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{Output, Stdio},
    time::Duration,
};

const MANUAL: &str = "Connect once with ssh in a terminal to check and trust this host's key.";

#[derive(Clone, Debug)]
pub struct Offer {
    pub name: String,
    pub key_type: &'static str,
    pub fingerprint: String,
    pub file: PathBuf,
    target: Target,
    record: String,
    make_parent: bool,
}

struct Tools<'a> {
    ssh: &'a OsStr,
    keyscan: &'a OsStr,
    keygen: &'a OsStr,
}

fn system_tools() -> Tools<'static> {
    Tools {
        ssh: OsStr::new("ssh"),
        keyscan: OsStr::new("ssh-keyscan"),
        keygen: OsStr::new("ssh-keygen"),
    }
}

async fn output(
    program: &OsStr,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
) -> Result<Output> {
    tokio::time::timeout(
        Duration::from_secs(15),
        tokio::process::Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .with_context(|| format!("{} timed out", program.to_string_lossy()))?
    .with_context(|| format!("Couldn't run {}", program.to_string_lossy()))
}

struct Config {
    host: String,
    port: u16,
    alias: Option<String>,
    hash: bool,
    files: String,
    proxied: bool,
}

impl Config {
    fn name(&self) -> String {
        if let Some(alias) = &self.alias {
            alias.clone()
        } else if self.port == 22 {
            self.host.clone()
        } else {
            format!("[{}]:{}", self.host, self.port)
        }
    }

    fn file(&self, home: &Path) -> Result<(PathBuf, bool)> {
        ensure!(!self.proxied, "{MANUAL}");
        let mut entries = self.files.split_whitespace();
        let first = entries.next().context(MANUAL)?;
        let absolute = |token: &str| token.starts_with('/') || token.starts_with("~/");
        ensure!(absolute(first) && entries.all(absolute), "{MANUAL}");
        let file = match first.strip_prefix("~/") {
            Some(relative) => {
                ensure!(home.is_absolute(), "{MANUAL}");
                home.join(relative)
            }
            None => PathBuf::from(first),
        };
        let parent = file.parent().context(MANUAL)?;
        let make_parent = home.is_absolute() && parent == home.join(".ssh");
        ensure!(parent.is_dir() || make_parent, "{MANUAL}");
        Ok((file, make_parent))
    }
}

async fn config(target: &Target, ssh: &OsStr) -> Result<Config> {
    crate::platform::local::require_ssh()?;
    let mut args = target.config_options();
    args.extend(["-G".into(), "--".into(), target.destination()]);
    let result = output(ssh, args).await?;
    ensure!(
        result.status.success(),
        "Couldn't read the OpenSSH configuration"
    );
    let text = std::str::from_utf8(&result.stdout)?;
    let get = |name: &str| {
        text.lines()
            .find_map(|line| {
                let (key, value) = line.split_once(' ')?;
                (key == name).then_some(value.trim())
            })
            .filter(|v| !v.is_empty() && *v != "none")
    };
    ensure!(
        !matches!(get("stricthostkeychecking"), Some("no" | "false" | "off")),
        "SSH configuration disables strict host key checking. Enable it before using SSH setup."
    );
    Ok(Config {
        host: get("hostname")
            .context("OpenSSH did not report a host name")?
            .to_ascii_lowercase(),
        port: get("port")
            .context("OpenSSH did not report a port")?
            .parse()?,
        alias: get("hostkeyalias").map(str::to_owned),
        hash: get("hashknownhosts") == Some("yes"),
        files: get("userknownhostsfile").unwrap_or_default().into(),
        proxied: get("proxyjump").is_some() || get("proxycommand").is_some(),
    })
}

pub(super) async fn require_safe_config(target: &Target) -> Result<()> {
    config(target, system_tools().ssh).await?;
    Ok(())
}

/// The name OpenSSH uses in known_hosts, including aliases and non-default ports.
pub async fn known_host_name(target: &Target) -> Result<String> {
    Ok(config(target, system_tools().ssh).await?.name())
}

async fn check(target: &Target, ssh: &OsStr) -> Result<bool> {
    match lookup(target, ssh).await {
        Ok(_) => Ok(true),
        Err(error) if error.downcast_ref::<KeyProblem>() == Some(&KeyProblem::Unknown) => Ok(false),
        Err(error) => Err(error),
    }
}

pub async fn scan(target: &Target) -> Result<Offer> {
    scan_with(target, &system_tools(), &crate::platform::dirs::home()).await
}

async fn scan_with(target: &Target, tools: &Tools<'_>, home: &Path) -> Result<Offer> {
    let config = config(target, tools.ssh).await?;
    let (file, make_parent) = config.file(home)?;
    // A fresh strict lookup, not a scan or the saved status, grants the offer.
    ensure!(
        !check(target, tools.ssh).await?,
        "OpenSSH already trusts this host's key."
    );
    let name = config.name();
    ensure!(
        name.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.-[]:%".contains(&c)),
        "{MANUAL}"
    );
    let keys = output(
        tools.keyscan,
        [
            "-T",
            "10",
            "-t",
            "ed25519,ecdsa,rsa",
            "-p",
            &config.port.to_string(),
            "--",
            &config.host,
        ],
    )
    .await?;
    let keys = std::str::from_utf8(&keys.stdout)?;
    let (algorithm, key_type, key) = [
        ("ssh-ed25519", "ED25519"),
        ("ecdsa-sha2-nistp256", "ECDSA"),
        ("ecdsa-sha2-nistp384", "ECDSA"),
        ("ecdsa-sha2-nistp521", "ECDSA"),
        ("ssh-rsa", "RSA"),
    ]
    .into_iter()
    .find_map(|(algorithm, key_type)| {
        keys.lines().find_map(|line| {
            let mut fields = line.split_ascii_whitespace();
            let _ = fields.next()?;
            if fields.next()? != algorithm {
                return None;
            }
            let key = fields.next()?;
            key.bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"+/=".contains(&c))
                .then_some((algorithm, key_type, key))
        })
    })
    .context("OpenSSH could not scan a supported key for this host.")?;
    let record = format!("{name} {algorithm} {key}");
    let scratch = tempfile::tempdir()?;
    let path = scratch.path().join("key");
    std::fs::write(&path, format!("{record}\n"))?;
    let result = output(tools.keygen, [OsStr::new("-lf"), path.as_os_str()]).await?;
    ensure!(
        result.status.success(),
        "Couldn't read the host key fingerprint."
    );
    let text = std::str::from_utf8(&result.stdout)?;
    let fingerprint = text
        .split_ascii_whitespace()
        .nth(1)
        .filter(|s| s.starts_with("SHA256:"))
        .context("OpenSSH did not report a SHA256 fingerprint")?
        .to_owned();
    let record = if config.hash {
        let result = output(
            tools.keygen,
            [OsStr::new("-H"), OsStr::new("-f"), path.as_os_str()],
        )
        .await?;
        ensure!(result.status.success(), "Couldn't hash the host name.");
        let hashed = std::fs::read_to_string(path)?;
        let (hashed_name, hashed_key) = hashed
            .trim_end()
            .split_once(' ')
            .context("No hashed host name")?;
        ensure!(
            hashed_name.starts_with("|1|") && hashed_key == format!("{algorithm} {key}"),
            "OpenSSH did not hash this key"
        );
        hashed.trim_end().into()
    } else {
        record
    };
    Ok(Offer {
        name,
        key_type,
        fingerprint,
        file,
        target: target.clone(),
        record,
        make_parent,
    })
}

pub async fn trust(offer: &Offer) -> Result<()> {
    trust_with(offer, &system_tools(), &crate::platform::dirs::home()).await
}

async fn trust_with(offer: &Offer, tools: &Tools<'_>, home: &Path) -> Result<()> {
    let config = config(&offer.target, tools.ssh).await?;
    let (file, _) = config.file(home)?;
    ensure!(
        file == offer.file
            && config.name() == offer.name
            && config.hash == offer.record.starts_with("|1|"),
        "The SSH configuration changed. Close this dialog and check again."
    );
    ensure!(
        !check(&offer.target, tools.ssh).await?,
        "OpenSSH already trusts this host's key."
    );
    crate::platform::fs::append_ssh_key(&offer.file, &offer.record, offer.make_parent)
        .with_context(|| format!("Couldn't append the key to {}", offer.file.display()))?;
    if !check(&offer.target, tools.ssh).await.with_context(|| {
        format!(
            "Appended the key to {}, but OpenSSH's follow-up check failed",
            offer.file.display()
        )
    })? {
        bail!(
            "Appended the key to {}, but OpenSSH still does not trust this host. {MANUAL}",
            offer.file.display()
        );
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};

    struct Fixture {
        dir: tempfile::TempDir,
        ssh: PathBuf,
        scan: PathBuf,
        keygen: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let home = dir.path();
            let script = |name: &str, body: &str| {
                let path = home.join(name);
                fs::write(
                    &path,
                    format!("#!/bin/sh\ncd '{}'\n{body}\n", home.display()),
                )
                .unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
                path
            };
            let ssh = script(
                "ssh",
                r#"
case " $* " in
  *" -G "*) cat config; exit 0;;
  *" StrictHostKeyChecking=yes "*) ;;
  *) exit 99;;
esac
if [ -f changed ]; then echo 'REMOTE HOST IDENTIFICATION HAS CHANGED!' >&2; exit 255; fi
if [ -f deny_after_append ] && grep -qs 'ssh-ed25519 cGlu' known; then
  echo 'fixture: Permission denied (publickey).' >&2; exit 255
fi
if [ -f recognized ] && grep -qs 'ssh-ed25519 cGlu' .ssh/known_hosts known; then
  echo 'none tester'; exit 0
fi
echo 'No ED25519 host key is known for fixture and you have requested strict checking.' >&2
echo 'Host key verification failed.' >&2
exit 255"#,
            );
            let scan = script(
                "ssh-keyscan",
                r#"
printf '%s\n' "$*" > scanned
echo 'untrusted ssh-rsa b3RoZXI='
echo 'untrusted ssh-ed25519 cGlu'"#,
            );
            let keygen = script(
                "ssh-keygen",
                r#"
case "$1" in
  -lf) echo '256 SHA256:fixture printable (ED25519)';;
  -H) read -r name rest < "$3"; printf '|1|salt|hash %s\n' "$rest" > "$3";;
  *) exit 99;;
esac"#,
            );
            Self {
                dir,
                ssh,
                scan,
                keygen,
            }
        }

        fn tools(&self) -> Tools<'_> {
            Tools {
                ssh: self.ssh.as_os_str(),
                keyscan: self.scan.as_os_str(),
                keygen: self.keygen.as_os_str(),
            }
        }

        fn config(&self, files: &str, extra: &str) {
            fs::write(self.dir.path().join("config"), format!(
                "hostname parsed.example\nport 2201\nstricthostkeychecking ask\nuserknownhostsfile {files}\n{extra}"
            )).unwrap();
        }

        async fn offer(&self) -> Result<Offer> {
            let target = Target::parse("ssh://tester@alias").unwrap().unwrap();
            scan_with(&target, &self.tools(), self.dir.path()).await
        }

        async fn trust(&self, offer: &Offer) -> Result<()> {
            trust_with(offer, &self.tools(), self.dir.path()).await
        }
    }

    #[tokio::test]
    async fn auth_failure_after_append_names_the_modified_file() {
        let f = Fixture::new();
        let known = f.dir.path().join("known");
        f.config(known.to_str().unwrap(), "");
        fs::write(f.dir.path().join("deny_after_append"), "").unwrap();
        let offer = f.offer().await.unwrap();
        let error = format!("{:#}", f.trust(&offer).await.unwrap_err());
        assert!(error.contains(&format!("Appended the key to {}", known.display())));
        assert!(error.contains("Permission denied (publickey)"));
        assert_eq!(
            fs::read_to_string(known).unwrap(),
            "[parsed.example]:2201 ssh-ed25519 cGlu\n"
        );
    }

    #[tokio::test]
    async fn default_alias_hash_and_permissions() {
        let f = Fixture::new();
        f.config(
            "~/.ssh/known_hosts ~/.ssh/known_hosts2",
            "hostkeyalias exact-alias\nhashknownhosts yes\n",
        );
        fs::write(f.dir.path().join("recognized"), "").unwrap();
        let offer = f.offer().await.unwrap();
        assert_eq!(offer.name, "exact-alias");
        assert_eq!(offer.key_type, "ED25519");
        assert_eq!(offer.fingerprint, "SHA256:fixture");
        assert_eq!(
            fs::read_to_string(f.dir.path().join("scanned"))
                .unwrap()
                .trim(),
            "-T 10 -t ed25519,ecdsa,rsa -p 2201 -- parsed.example"
        );
        f.trust(&offer).await.unwrap();
        assert_eq!(
            fs::read_to_string(&offer.file).unwrap(),
            "|1|salt|hash ssh-ed25519 cGlu\n"
        );
        assert_eq!(
            fs::metadata(&offer.file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(offer.file.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert!(
            f.trust(&offer).await.is_err(),
            "a second press must not append"
        );
    }

    #[tokio::test]
    async fn custom_append_and_immediate_checks() {
        let f = Fixture::new();
        let known = f.dir.path().join("known");
        f.config(known.to_str().unwrap(), "");
        fs::write(&known, "# do not replace me").unwrap();
        let offer = f.offer().await.unwrap();
        assert_eq!(offer.name, "[parsed.example]:2201");
        fs::write(f.dir.path().join("changed"), "").unwrap();
        assert_eq!(
            f.trust(&offer)
                .await
                .unwrap_err()
                .downcast_ref::<KeyProblem>(),
            Some(&KeyProblem::Changed)
        );
        assert_eq!(fs::read_to_string(&known).unwrap(), "# do not replace me");
        fs::remove_file(f.dir.path().join("changed")).unwrap();
        let error = f.trust(&offer).await.unwrap_err().to_string();
        assert!(error.contains(known.to_str().unwrap()) && error.contains("still does not trust"));
        assert_eq!(
            fs::read_to_string(&known).unwrap(),
            "# do not replace me\n[parsed.example]:2201 ssh-ed25519 cGlu\n"
        );
    }

    #[tokio::test]
    async fn refuses_ambiguous_missing_parent_and_proxy_before_scan() {
        let f = Fixture::new();
        let work = f.dir.path().display();
        // Both the quoted path and three separate tokens produce this same `ssh -G` line.
        for files in [
            format!("{work}/known primary {work}/extra"),
            format!("{work}/absent/known"),
        ] {
            f.config(&files, "");
            assert_eq!(f.offer().await.unwrap_err().to_string(), MANUAL);
        }
        f.config(&format!("{work}/known {work}/primary {work}/extra"), "");
        assert_eq!(f.offer().await.unwrap().file, f.dir.path().join("known"));
        fs::remove_file(f.dir.path().join("scanned")).unwrap();
        for config in ["proxyjump bastion\n", "proxycommand ssh bastion\n"] {
            f.config(&format!("{work}/known"), config);
            super::config(
                &Target::parse("ssh://alias").unwrap().unwrap(),
                f.tools().ssh,
            )
            .await
            .expect("setup may use a proxy whose key is already trusted");
            assert_eq!(f.offer().await.unwrap_err().to_string(), MANUAL);
        }
        f.config(&format!("{work}/known"), "stricthostkeychecking no\n");
        let text = fs::read_to_string(f.dir.path().join("config"))
            .unwrap()
            .replace("stricthostkeychecking ask\n", "");
        fs::write(f.dir.path().join("config"), text).unwrap();
        assert!(
            f.offer()
                .await
                .unwrap_err()
                .to_string()
                .contains("disables strict")
        );
        assert!(!f.dir.path().join("scanned").exists());
    }
}
