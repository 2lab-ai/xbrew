use anyhow::{anyhow, bail, Result};

use crate::platform::{self, Platform};
use crate::recipe::{self, Recipe};
use crate::state::{Record, State};
use crate::util;

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
        Platform::Other => bail!("xbrew supports only macOS and Arch Linux"),
    };

    state.packages.insert(name.to_string(), record.clone());
    state.save()?;
    println!(
        "\n✓ '{name}' is set up via {} and tracked by xbrew.",
        record.backend
    );
    Ok(())
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
                util::run("sudo", &["pacman", "-S", "--needed", "--noconfirm", pkg])?;
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
            util::run("sudo", &["pacman", "-S", "--needed", "--noconfirm", name])?;
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
            util::run("sudo", &["pacman", "-Rns", "--noconfirm", &rec.reference])?;
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
    util::probe("git", &["ls-remote", &url])
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
                    if let Some(a) = r.arch.aur {
                        println!("  arch → AUR: {a}");
                    }
                    if let Some(f) = r.arch.flatpak {
                        println!("  arch → flatpak: {f}");
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
        None => println!("recipe:  none (would try brew, then pacman)"),
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
    if platform::detect() == Platform::Arch && util::which("pacman") {
        println!("== pacman ==");
        let _ = util::run("pacman", &["-Ss", query]);
    }
    Ok(())
}
