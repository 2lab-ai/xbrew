use anyhow::{anyhow, bail, Result};
use std::path::PathBuf;

use crate::manifest;
use crate::platform::{self, Platform};
use crate::recipe::{self, Recipe};
use crate::state::{Record, State};
use crate::util;
use crate::version;

// ---------------------------------------------------------------------------
// install
// ---------------------------------------------------------------------------

pub fn install(name: &str) -> Result<()> {
    if name == "xbrew" {
        bail!("xbrew installs *other* things — to update xbrew itself, run `xbrew self-update`.");
    }
    let mut state = State::load()?;
    if let Some(rec) = state.packages.get(name) {
        println!("✓ '{name}' is already installed (via {}).", rec.backend);
        return Ok(());
    }

    let plat = platform::detect();
    let recipe = recipe::get(name);

    let record = match plat {
        Platform::MacOS => install_macos(name, recipe.as_ref())?,
        Platform::Arch => install_arch(name, recipe.as_ref())?,
        Platform::Debian => install_debian(name, recipe.as_ref())?,
        Platform::Rhel => install_rhel(name, recipe.as_ref())?,
        Platform::Other => {
            bail!("xbrew supports macOS, Arch, Debian/Ubuntu, and RHEL/Amazon Linux")
        }
    };

    state.packages.insert(name.to_string(), record.clone());
    state.save()?;
    println!(
        "\n✓ '{name}' is set up via {} and tracked by xbrew.",
        record.backend
    );
    Ok(())
}

/// Install several packages in one go; one failure doesn't stop the rest.
pub fn install_many(names: &[String]) -> Result<()> {
    if names.len() == 1 {
        return install(&names[0]);
    }
    let mut failed = Vec::new();
    for name in names {
        println!("\n\x1b[1m── {name} ──\x1b[0m");
        if let Err(e) = install(name) {
            eprintln!("\x1b[31merror:\x1b[0m {e:#}");
            failed.push(name.clone());
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        bail!("failed to install: {}", failed.join(", "))
    }
}

/// Uninstall several packages in one go; one failure doesn't stop the rest.
pub fn uninstall_many(names: &[String]) -> Result<()> {
    if names.len() == 1 {
        return uninstall(&names[0]);
    }
    let mut failed = Vec::new();
    for name in names {
        println!("\n\x1b[1m── {name} ──\x1b[0m");
        if let Err(e) = uninstall(name) {
            eprintln!("\x1b[31merror:\x1b[0m {e:#}");
            failed.push(name.clone());
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        bail!("failed to uninstall: {}", failed.join(", "))
    }
}

/// Arch: a curated recipe is authoritative when it declares an Arch/script
/// backend (that's the whole point of curating it); only names with no such
/// recipe fall back to the generic Homebrew -> pacman chain.
fn install_arch(name: &str, recipe: Option<&Recipe>) -> Result<Record> {
    if let Some(r) = recipe {
        if let Some(pkg) = &r.arch.pacman {
            if pacman_installed(pkg) {
                report_adopted(name, pkg);
            } else {
                util::run_priv("pacman", &["-S", "--needed", "--noconfirm", pkg])?;
            }
            return Ok(Record {
                backend: "pacman".into(),
                reference: pkg.clone(),
                kind: None,
                artifacts: vec![],
            });
        }
        if let Some(aur) = &r.arch.aur {
            if pacman_installed(aur) {
                report_adopted(name, aur);
            } else {
                aur_install(aur)?;
            }
            return Ok(Record {
                backend: "aur".into(),
                reference: aur.clone(),
                kind: None,
                artifacts: vec![],
            });
        }
        if let Some(fp) = &r.arch.flatpak {
            if !util::which("flatpak") {
                bail!("this recipe needs flatpak — install it: sudo pacman -S flatpak");
            }
            if flatpak_installed(fp) {
                report_adopted(name, fp);
            } else {
                util::run("flatpak", &["install", "-y", "flathub", fp])?;
            }
            return Ok(Record {
                backend: "flatpak".into(),
                reference: fp.clone(),
                kind: None,
                artifacts: vec![],
            });
        }
        if let Some(res) = recipe_script_install(name, r) {
            return res;
        }
    }

    if util::which("brew") {
        if let Some(kind) = brew_provides(name) {
            if brew_installed(name, &kind) {
                report_adopted(name, name);
            } else {
                brew_install(name, &kind)?;
            }
            return Ok(Record {
                backend: "brew".into(),
                reference: name.into(),
                kind: Some(kind),
                artifacts: vec![],
            });
        }
    }

    if util::which("pacman") && pacman_provides(name) {
        if pacman_installed(name) {
            report_adopted(name, name);
        } else {
            util::run_priv("pacman", &["-S", "--needed", "--noconfirm", name])?;
        }
        return Ok(Record {
            backend: "pacman".into(),
            reference: name.into(),
            kind: None,
            artifacts: vec![],
        });
    }

    // Generic AUR fallback: any package that exists in the AUR gets the same
    // git-clone + makepkg -si you'd do by hand — no recipe required.
    if aur_exists(name) {
        if pacman_installed(name) {
            report_adopted(name, name);
        } else {
            aur_install(name)?;
        }
        return Ok(Record {
            backend: "aur".into(),
            reference: name.into(),
            kind: None,
            artifacts: vec![],
        });
    }

    bail!("no backend can install '{name}' on Arch (no recipe; not in brew, pacman, or the AUR)")
}

/// macOS: a curated recipe is authoritative when it declares a macOS/script
/// backend; only names with no such recipe fall back to generic Homebrew.
fn install_macos(name: &str, recipe: Option<&Recipe>) -> Result<Record> {
    let has_brew = util::which("brew");

    if let Some(r) = recipe {
        if let Some(cask) = &r.macos.cask {
            if !has_brew {
                bail!("'{name}' installs as a Homebrew cask, but brew isn't installed. Run `xbrew install brew` first.");
            }
            if brew_installed(cask, "cask") {
                report_adopted(name, cask);
            } else {
                brew_install(cask, "cask")?;
            }
            return Ok(Record {
                backend: "brew".into(),
                reference: cask.clone(),
                kind: Some("cask".into()),
                artifacts: vec![],
            });
        }
        if let Some(dmg) = &r.macos.dmg {
            let app = r.macos.app.clone().ok_or_else(|| {
                anyhow!("recipe '{name}' has a dmg but no `app` name for uninstall")
            })?;
            let dest = format!("/Applications/{app}");
            if std::path::Path::new(&dest).exists() {
                report_adopted(name, &app);
            } else {
                dmg_install(dmg, &app)?;
            }
            return Ok(Record {
                backend: "recipe-dmg".into(),
                reference: app,
                kind: None,
                artifacts: vec![dest],
            });
        }
        if let Some(res) = recipe_script_install(name, r) {
            return res;
        }
    }

    if has_brew {
        if let Some(kind) = brew_provides(name) {
            if brew_installed(name, &kind) {
                report_adopted(name, name);
            } else {
                brew_install(name, &kind)?;
            }
            return Ok(Record {
                backend: "brew".into(),
                reference: name.into(),
                kind: Some(kind),
                artifacts: vec![],
            });
        }
        bail!("no backend can install '{name}' on macOS (no recipe, and not in brew)");
    }

    bail!("Homebrew isn't installed and there's no recipe for '{name}'. Run `xbrew install brew` first.")
}

/// Debian/Ubuntu: recipe (apt/flatpak/script) authoritative, then Homebrew, then apt.
fn install_debian(name: &str, recipe: Option<&Recipe>) -> Result<Record> {
    if let Some(r) = recipe {
        if let Some(pkg) = &r.debian.apt {
            if dpkg_installed(pkg) {
                report_adopted(name, pkg);
            } else {
                util::run_priv("apt-get", &["install", "-y", pkg])?;
            }
            return Ok(Record {
                backend: "apt".into(),
                reference: pkg.clone(),
                kind: None,
                artifacts: vec![],
            });
        }
        if let Some(fp) = &r.debian.flatpak {
            return flatpak_install(name, fp);
        }
        if let Some(res) = recipe_script_install(name, r) {
            return res;
        }
    }

    if util::which("brew") {
        if let Some(kind) = brew_provides(name) {
            if brew_installed(name, &kind) {
                report_adopted(name, name);
            } else {
                brew_install(name, &kind)?;
            }
            return Ok(Record {
                backend: "brew".into(),
                reference: name.into(),
                kind: Some(kind),
                artifacts: vec![],
            });
        }
    }

    if util::which("apt-get") && apt_provides(name) {
        if dpkg_installed(name) {
            report_adopted(name, name);
        } else {
            util::run_priv("apt-get", &["install", "-y", name])?;
        }
        return Ok(Record {
            backend: "apt".into(),
            reference: name.into(),
            kind: None,
            artifacts: vec![],
        });
    }

    bail!("no backend can install '{name}' on Debian/Ubuntu (no recipe; not in brew or apt)")
}

/// RHEL/Amazon Linux/Fedora: recipe (dnf/flatpak/script) authoritative, then Homebrew, then dnf/yum.
fn install_rhel(name: &str, recipe: Option<&Recipe>) -> Result<Record> {
    let pm = rhel_pm();

    if let Some(r) = recipe {
        if let Some(pkg) = &r.rhel.dnf {
            if rpm_installed(pkg) {
                report_adopted(name, pkg);
            } else {
                util::run_priv(pm, &["install", "-y", pkg])?;
            }
            return Ok(Record {
                backend: "dnf".into(),
                reference: pkg.clone(),
                kind: None,
                artifacts: vec![],
            });
        }
        if let Some(fp) = &r.rhel.flatpak {
            return flatpak_install(name, fp);
        }
        if let Some(res) = recipe_script_install(name, r) {
            return res;
        }
    }

    if util::which("brew") {
        if let Some(kind) = brew_provides(name) {
            if brew_installed(name, &kind) {
                report_adopted(name, name);
            } else {
                brew_install(name, &kind)?;
            }
            return Ok(Record {
                backend: "brew".into(),
                reference: name.into(),
                kind: Some(kind),
                artifacts: vec![],
            });
        }
    }

    if util::which(pm) && dnf_provides(name, pm) {
        if rpm_installed(name) {
            report_adopted(name, name);
        } else {
            util::run_priv(pm, &["install", "-y", name])?;
        }
        return Ok(Record {
            backend: "dnf".into(),
            reference: name.into(),
            kind: None,
            artifacts: vec![],
        });
    }

    bail!("no backend can install '{name}' on RHEL/Amazon Linux (no recipe; not in brew or {pm})")
}

// ---------------------------------------------------------------------------
// uninstall
// ---------------------------------------------------------------------------

pub fn uninstall(name: &str) -> Result<()> {
    let mut state = State::load()?;
    let rec = state.packages.get(name).cloned().ok_or_else(|| {
        anyhow!("'{name}' is not managed by xbrew — nothing to uninstall.\n(xbrew only removes what it installed; see `xbrew list`.)")
    })?;

    match rec.backend.as_str() {
        "brew" => {
            if rec.kind.as_deref() == Some("cask") {
                util::run("brew", &["uninstall", "--cask", &rec.reference])?;
            } else {
                util::run("brew", &["uninstall", &rec.reference])?;
            }
        }
        "pacman" | "aur" => {
            util::run_priv("pacman", &["-Rns", "--noconfirm", &rec.reference])?;
        }
        "apt" => {
            util::run_priv("apt-get", &["remove", "-y", &rec.reference])?;
        }
        "dnf" => {
            util::run_priv(rhel_pm(), &["remove", "-y", &rec.reference])?;
        }
        "flatpak" => {
            util::run("flatpak", &["uninstall", "-y", &rec.reference])?;
        }
        "recipe-dmg" => {
            for a in &rec.artifacts {
                util::run("rm", &["-rf", a])?;
            }
        }
        "script" => match recipe::get(name).and_then(|r| r.script.uninstall) {
            Some(cmd) => util::run("sh", &["-c", &cmd])?,
            None => println!(
                "note: '{name}' was installed by its own script and has no uninstaller — removing from xbrew tracking only."
            ),
        },
        other => bail!("don't know how to uninstall backend '{other}'"),
    }

    state.packages.remove(name);
    state.save()?;
    println!("\n✓ uninstalled '{name}'.");
    Ok(())
}

// ---------------------------------------------------------------------------
// bundle — install a whole manifest (Brewfile-style), with version constraints
// ---------------------------------------------------------------------------

/// Install everything declared across one or more manifests. Manifests are
/// merged in order (common first, then per-OS); trusted taps are registered
/// before installing so brew-backed formulae resolve. One failure doesn't stop
/// the rest; a version constraint that isn't met is reported as a failure.
pub fn bundle(files: &[PathBuf]) -> Result<()> {
    if files.is_empty() {
        bail!("bundle: give at least one manifest file");
    }

    let mut trust: Vec<String> = Vec::new();
    let mut entries: Vec<manifest::Entry> = Vec::new();
    for f in files {
        let m = manifest::parse_file(f)?;
        for t in m.trust {
            if !trust.contains(&t) {
                trust.push(t);
            }
        }
        for e in m.entries {
            if !entries.iter().any(|x| x.name == e.name) {
                entries.push(e);
            }
        }
    }

    // Register trusted taps first — brew itself is just another xbrew package.
    if !trust.is_empty() {
        if !util::which("brew") {
            println!("\n\x1b[1m── ensuring Homebrew (needed for trusted taps) ──\x1b[0m");
            install("brew")?;
        }
        for t in &trust {
            println!("trust tap: {t}");
            if let Err(e) = util::run("brew", &["tap", t]) {
                eprintln!("\x1b[33m! brew tap {t} failed: {e:#}\x1b[0m");
            }
        }
    }

    let mut ok: Vec<String> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    for e in &entries {
        println!("\n\x1b[1m── {} ──\x1b[0m", e.name);
        if let Err(err) = install(&e.name) {
            eprintln!("\x1b[31merror:\x1b[0m {err:#}");
            failed.push(format!("{} (install failed)", e.name));
            continue;
        }
        match (e.op.as_deref(), e.version.as_deref()) {
            (Some(op), Some(req)) => match version::installed(&e.name) {
                Some(cur) if version::satisfies(&cur, op, req) => {
                    ok.push(format!("{} {cur} (need {op} {req} ✓)", e.name));
                }
                Some(cur) => {
                    println!(
                        "\x1b[33m! {} {cur} does not satisfy {op} {req}\x1b[0m",
                        e.name
                    );
                    failed.push(format!("{} {cur} (need {op} {req})", e.name));
                }
                None => {
                    println!(
                        "\x1b[33m! {} installed but version unreadable (wanted {op} {req})\x1b[0m",
                        e.name
                    );
                    ok.push(format!("{} (version unverified)", e.name));
                }
            },
            _ => {
                let cur = version::installed(&e.name).unwrap_or_default();
                ok.push(if cur.is_empty() {
                    e.name.clone()
                } else {
                    format!("{} {cur}", e.name)
                });
            }
        }
    }

    println!("\n\x1b[1mbundle summary\x1b[0m");
    for o in &ok {
        println!("  \x1b[32m✓\x1b[0m {o}");
    }
    for f in &failed {
        println!("  \x1b[31m✗ {f}\x1b[0m");
    }
    if !failed.is_empty() {
        bail!("{} package(s) failed or unsatisfied", failed.len());
    }
    Ok(())
}

/// Print the installed version of a tracked package (backend-aware).
pub fn version(name: &str) -> Result<()> {
    match version::installed(name) {
        Some(v) => {
            println!("{v}");
            Ok(())
        }
        None => bail!("could not determine the installed version of '{name}'"),
    }
}

/// Re-run the published installer to pull the latest xbrew binary.
pub fn self_update() -> Result<()> {
    println!("updating xbrew (fetching the latest install.sh)…");
    util::run(
        "sh",
        &[
            "-c",
            "curl -fsSL https://raw.githubusercontent.com/2lab-ai/xbrew/HEAD/install.sh | bash",
        ],
    )
}

// ---------------------------------------------------------------------------
// backend helpers
// ---------------------------------------------------------------------------

fn brew_provides(name: &str) -> Option<String> {
    if util::probe("brew", &["info", "--formula", name]) {
        Some("formula".into())
    } else if cfg!(target_os = "macos") && util::probe("brew", &["info", "--cask", name]) {
        // Casks are macOS-only; on Linux `brew info --cask` still succeeds for a
        // mac-only cask, but installing it fails ("requires macOS").
        Some("cask".into())
    } else {
        None
    }
}

fn brew_install(name: &str, kind: &str) -> Result<()> {
    if kind == "cask" {
        util::run("brew", &["install", "--cask", name])
    } else {
        util::run("brew", &["install", name])
    }
}

fn pacman_provides(name: &str) -> bool {
    util::probe("pacman", &["-Si", name])
}

// --- "is it already installed?" probes, so we adopt instead of reinstalling ---

fn pacman_installed(pkg: &str) -> bool {
    util::probe("pacman", &["-Qq", pkg])
}

fn brew_installed(name: &str, kind: &str) -> bool {
    let flag = if kind == "cask" {
        "--cask"
    } else {
        "--formula"
    };
    util::probe("brew", &["list", flag, name])
}

fn flatpak_installed(app_id: &str) -> bool {
    util::probe("flatpak", &["info", app_id])
}

/// Print an "already installed → adopting" line (with the underlying package
/// name when it differs from what the user typed).
fn report_adopted(name: &str, reference: &str) {
    if name == reference {
        println!("✓ '{name}' is already installed — adopting into xbrew.");
    } else {
        println!("✓ '{name}' is already installed ({reference}) — adopting into xbrew.");
    }
}

fn dpkg_installed(pkg: &str) -> bool {
    util::probe("dpkg", &["-s", pkg])
}

fn rpm_installed(pkg: &str) -> bool {
    util::probe("rpm", &["-q", pkg])
}

fn apt_provides(pkg: &str) -> bool {
    util::probe("apt-cache", &["show", pkg])
}

fn dnf_provides(pkg: &str, pm: &str) -> bool {
    util::probe(pm, &["info", pkg])
}

/// dnf on modern systems (Amazon Linux 2023, Fedora), yum on older ones (AL2).
fn rhel_pm() -> &'static str {
    if util::which("dnf") {
        "dnf"
    } else {
        "yum"
    }
}

/// Install (or adopt) a Flathub app id. Shared by every Linux family.
fn flatpak_install(name: &str, app_id: &str) -> Result<Record> {
    if !util::which("flatpak") {
        bail!("this recipe needs flatpak — install it with your package manager first");
    }
    if flatpak_installed(app_id) {
        report_adopted(name, app_id);
    } else {
        util::run("flatpak", &["install", "-y", "flathub", app_id])?;
    }
    Ok(Record {
        backend: "flatpak".into(),
        reference: app_id.into(),
        kind: None,
        artifacts: vec![],
    })
}

/// Last resort: run a recipe's own installer script (brew, claude, …).
/// Returns None when the recipe defines no script.
fn recipe_script_install(name: &str, r: &Recipe) -> Option<Result<Record>> {
    let install = r.script.install.as_ref()?;
    let record = Record {
        backend: "script".into(),
        reference: name.into(),
        kind: None,
        artifacts: vec![],
    };
    if let Some(bin) = &r.script.provides_bin {
        if util::which(bin) {
            println!("✓ '{name}' already present ({bin} on PATH).");
            return Some(Ok(record));
        }
    }
    Some(util::run("sh", &["-c", install]).map(|_| record))
}

/// Does the AUR have a package by this exact name? (Cheap check via `git
/// ls-remote` — no JSON parsing, no extra dependency.)
fn aur_exists(pkg: &str) -> bool {
    if !util::which("git") {
        return false;
    }
    let url = format!("https://aur.archlinux.org/{pkg}.git");
    // The AUR git server returns exit 0 even for a non-existent package (it hands
    // back an empty repo you could push a new package to), so a zero exit isn't
    // enough — require at least one ref.
    !util::capture("git", &["ls-remote", &url]).trim().is_empty()
}

/// Clone the AUR package and build it with makepkg (which installs via pacman,
/// so `pacman -Rns` later removes it cleanly).
fn aur_install(pkg: &str) -> Result<()> {
    if !util::which("makepkg") {
        bail!("makepkg not found — install base-devel: sudo pacman -S --needed base-devel git");
    }
    let cache = util::xbrew_dir()?.join("aur");
    std::fs::create_dir_all(&cache)?;
    let dir = cache.join(pkg);
    if dir.join(".git").exists() {
        util::run_in(&dir, "git", &["pull", "--ff-only"])?;
    } else {
        let url = format!("https://aur.archlinux.org/{pkg}.git");
        util::run("git", &["clone", &url, dir.to_str().unwrap()])?;
    }
    util::run_in(&dir, "makepkg", &["-si", "--needed", "--noconfirm"])?;
    Ok(())
}

/// Download a .dmg, mount it (auto-accepting any license), copy the app into
/// /Applications, strip quarantine, and detach. Returns the installed app path.
fn dmg_install(url: &str, app: &str) -> Result<String> {
    let cache = util::xbrew_dir()?.join("cache");
    std::fs::create_dir_all(&cache)?;
    let dmg = cache.join(format!("{app}.dmg"));
    util::run("curl", &["-L", "-o", dmg.to_str().unwrap(), url])?;

    let mnt = cache.join("mnt");
    std::fs::create_dir_all(&mnt)?;
    util::run(
        "sh",
        &[
            "-c",
            &format!(
                "printf 'Y\\n' | hdiutil attach '{}' -nobrowse -noverify -mountpoint '{}'",
                dmg.display(),
                mnt.display()
            ),
        ],
    )?;

    let dest = format!("/Applications/{app}");
    let src = mnt.join(app);
    let _ = util::run("rm", &["-rf", &dest]);
    util::run("cp", &["-R", src.to_str().unwrap(), "/Applications/"])?;
    let _ = util::run("xattr", &["-dr", "com.apple.quarantine", &dest]);
    let _ = util::run("hdiutil", &["detach", mnt.to_str().unwrap()]);
    Ok(dest)
}

// ---------------------------------------------------------------------------
// list / info / search
// ---------------------------------------------------------------------------

pub fn list() -> Result<()> {
    let state = State::load()?;
    if state.packages.is_empty() {
        println!("nothing installed by xbrew yet. Try: xbrew install <name>");
        return Ok(());
    }
    println!("{:<24} {:<12} REFERENCE", "PACKAGE", "BACKEND");
    for (name, rec) in &state.packages {
        let reference = match &rec.kind {
            Some(k) => format!("{} ({k})", rec.reference),
            None => rec.reference.clone(),
        };
        println!("{:<24} {:<12} {}", name, rec.backend, reference);
    }
    Ok(())
}

pub fn info(name: &str) -> Result<()> {
    let state = State::load()?;
    let plat = platform::detect();

    println!("package: {name}");
    match state.packages.get(name) {
        Some(rec) => println!("status:  installed (via {})", rec.backend),
        None => println!("status:  not installed"),
    }

    match recipe::get(name) {
        Some(r) => {
            if !r.description.is_empty() {
                println!("about:   {}", r.description);
            }
            println!("recipe:  yes");
            match plat {
                Platform::Arch => {
                    if let Some(p) = r.arch.pacman {
                        println!("  arch → pacman: {p}");
                    }
                    if let Some(a) = r.arch.aur {
                        println!("  arch → AUR: {a}");
                    }
                    if let Some(f) = r.arch.flatpak {
                        println!("  arch → flatpak: {f}");
                    }
                }
                Platform::Debian => {
                    if let Some(a) = r.debian.apt {
                        println!("  debian → apt: {a}");
                    }
                    if let Some(f) = r.debian.flatpak {
                        println!("  debian → flatpak: {f}");
                    }
                }
                Platform::Rhel => {
                    if let Some(d) = r.rhel.dnf {
                        println!("  rhel → dnf: {d}");
                    }
                    if let Some(f) = r.rhel.flatpak {
                        println!("  rhel → flatpak: {f}");
                    }
                }
                Platform::MacOS => {
                    if let Some(c) = r.macos.cask {
                        println!("  macos → cask: {c}");
                    }
                    if let Some(d) = r.macos.dmg {
                        println!("  macos → dmg: {d}");
                    }
                }
                Platform::Other => {}
            }
            if let Some(s) = &r.script.install {
                println!("  self-install: {s}");
            }
        }
        None => println!("recipe:  none (would try brew, then the system package manager)"),
    }
    Ok(())
}

pub fn search(query: &str) -> Result<()> {
    let recs = recipe::registry();
    let hits: Vec<&String> = recs.keys().filter(|k| k.contains(query)).collect();
    if !hits.is_empty() {
        println!("== xbrew recipes ==");
        for h in hits {
            println!("  {h}");
        }
        println!();
    }
    if util::which("brew") {
        println!("== brew ==");
        let _ = util::run("brew", &["search", query]);
    }
    match platform::detect() {
        Platform::Arch if util::which("pacman") => {
            println!("== pacman ==");
            let _ = util::run("pacman", &["-Ss", query]);
        }
        Platform::Debian if util::which("apt-cache") => {
            println!("== apt ==");
            let _ = util::run("apt-cache", &["search", query]);
        }
        Platform::Rhel if util::which(rhel_pm()) => {
            println!("== {} ==", rhel_pm());
            let _ = util::run(rhel_pm(), &["search", query]);
        }
        _ => {}
    }
    Ok(())
}
