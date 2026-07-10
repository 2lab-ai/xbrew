use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("HOME environment variable is not set"))
}

/// `~/.xbrew`, created if missing.
pub fn xbrew_dir() -> Result<PathBuf> {
    let d = home()?.join(".xbrew");
    std::fs::create_dir_all(&d).with_context(|| format!("creating {}", d.display()))?;
    Ok(d)
}

pub fn which(bin: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {bin} >/dev/null 2>&1"))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Run a command, inheriting stdio so the user sees progress (and sudo prompts).
pub fn run(program: &str, args: &[&str]) -> Result<()> {
    eprintln!("\x1b[2m$ {program} {}\x1b[0m", args.join(" "));
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("failed to launch `{program}`"))?;
    if !status.success() {
        return Err(anyhow!("`{program}` exited with {status}"));
    }
    Ok(())
}

/// Same as `run`, but from a working directory.
pub fn run_in(dir: &Path, program: &str, args: &[&str]) -> Result<()> {
    eprintln!(
        "\x1b[2m$ (cd {}) {program} {}\x1b[0m",
        dir.display(),
        args.join(" ")
    );
    let status = Command::new(program)
        .args(args)
        .current_dir(dir)
        .status()
        .with_context(|| format!("failed to launch `{program}`"))?;
    if !status.success() {
        return Err(anyhow!("`{program}` exited with {status}"));
    }
    Ok(())
}

/// Quietly probe whether a command succeeds (capability checks).
pub fn probe(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Run a command and capture its stdout (empty string on non-zero exit).
pub fn capture(program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// Are we running as root? (containers, servers — then we skip `sudo`.)
pub fn is_root() -> bool {
    capture("id", &["-u"]).trim() == "0"
}

/// Prime sudo now and keep its timestamp fresh in the background, so a sequence
/// of long privileged steps (e.g. several AUR rebuilds in `xbrew update`) doesn't
/// hit a stale-password timeout partway through. No-op when already root.
/// Best-effort: if the initial prompt fails, we just don't start the keep-alive.
pub fn keep_sudo_alive() {
    if is_root() {
        return;
    }
    if run("sudo", &["-v"]).is_err() {
        return;
    }
    // Detached refresher: `sudo -n -v` never prompts; it extends the timestamp
    // while it's still valid and fails (ending the loop) once it lapses.
    std::thread::spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
        if !probe("sudo", &["-n", "-v"]) {
            break;
        }
    });
}

/// Run a privileged command: prefixed with `sudo` unless already root.
pub fn run_priv(program: &str, args: &[&str]) -> Result<()> {
    if is_root() {
        run(program, args)
    } else {
        let mut full = Vec::with_capacity(args.len() + 1);
        full.push(program);
        full.extend_from_slice(args);
        run("sudo", &full)
    }
}
