use anyhow::{anyhow, bail, Result};

use crate::platform::{self, Platform};
use crate::recipe::{self, Recipe};
use crate::state::{Record, State};
use crate::util;

// ---------------------------------------------------------------------------
// install
// ---------------------------------------------------------------------------

pub fn install(name: &str) -> Result<()> {
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
        Platform::Other => bail!("nobrew supports only macOS and Arch Linux"),
    };

    state.packages.insert(name.to_string(), record.clone());
    state.save()?;
    println!("\n✓ installed '{name}' via {} — tracked by nobrew.", record.backend);
    Ok(())
}

/// Arch order: Homebrew (ordinary CLIs) -> pacman -> recipe (AUR/flatpak).
fn install_arch(name: &str, recipe: Option<&Recipe>) -> Result<Record> {
    if util::which("brew") {
        if let Some(kind) = brew_provides(name) {
            brew_install(name, &kind)?;
            return Ok(Record {
                backend: "brew".into(),
                reference: name.into(),
                kind: Some(kind),
                artifacts: vec![],
            });
        }
    }

    if util::which("pacman") && pacman_provides(name) {
        util::run("sudo", &["pacman", "-S", "--needed", "--noconfirm", name])?;
        return Ok(Record {
            backend: "pacman".into(),
            reference: name.into(),
            kind: None,
            artifacts: vec![],
        });
    }

    if let Some(r) = recipe {
        if let Some(aur) = &r.arch.aur {
            aur_install(aur)?;
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
            util::run("flatpak", &["install", "-y", "flathub", fp])?;
            return Ok(Record {
                backend: "flatpak".into(),
                reference: fp.clone(),
                kind: None,
                artifacts: vec![],
            });
        }
    }

    bail!("no backend can install '{name}' on Arch (not in brew or pacman, and no recipe with an Arch source)")
}

/// macOS order: brew formula/cask -> recipe (cask override / dmg).
fn install_macos(name: &str, recipe: Option<&Recipe>) -> Result<Record> {
    if !util::which("brew") {
        bail!(
            "Homebrew is required on macOS. Install it first:\n  /bin/bash -c \"$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)\""
        );
    }

    if let Some(kind) = brew_provides(name) {
        brew_install(name, &kind)?;
        return Ok(Record {
            backend: "brew".into(),
            reference: name.into(),
            kind: Some(kind),
            artifacts: vec![],
        });
    }

    if let Some(r) = recipe {
        if let Some(cask) = &r.macos.cask {
            brew_install(cask, "cask")?;
            return Ok(Record {
                backend: "brew".into(),
                reference: cask.clone(),
                kind: Some("cask".into()),
                artifacts: vec![],
            });
        }
        if let Some(dmg) = &r.macos.dmg {
            let app = r
                .macos
                .app
                .clone()
                .ok_or_else(|| anyhow!("recipe '{name}' has a dmg but no `app` name for uninstall"))?;
            let path = dmg_install(dmg, &app)?;
            return Ok(Record {
                backend: "recipe-dmg".into(),
                reference: app,
                kind: None,
                artifacts: vec![path],
            });
        }
    }

    bail!("no backend can install '{name}' on macOS (not in brew, and no recipe with a macOS source)")
}

// ---------------------------------------------------------------------------
// uninstall
// ---------------------------------------------------------------------------

pub fn uninstall(name: &str) -> Result<()> {
    let mut state = State::load()?;
    let rec = state.packages.get(name).cloned().ok_or_else(|| {
        anyhow!("'{name}' is not managed by nobrew — nothing to uninstall.\n(nobrew only removes what it installed; see `nobrew list`.)")
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
        other => bail!("don't know how to uninstall backend '{other}'"),
    }

    state.packages.remove(name);
    state.save()?;
    println!("\n✓ uninstalled '{name}'.");
    Ok(())
}

// ---------------------------------------------------------------------------
// backend helpers
// ---------------------------------------------------------------------------

fn brew_provides(name: &str) -> Option<String> {
    if util::probe("brew", &["info", "--formula", name]) {
        Some("formula".into())
    } else if util::probe("brew", &["info", "--cask", name]) {
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

/// Clone the AUR package and build it with makepkg (which installs via pacman,
/// so `pacman -Rns` later removes it cleanly).
fn aur_install(pkg: &str) -> Result<()> {
    if !util::which("makepkg") {
        bail!("makepkg not found — install base-devel: sudo pacman -S --needed base-devel git");
    }
    let cache = util::nobrew_dir()?.join("aur");
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
    let cache = util::nobrew_dir()?.join("cache");
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
        println!("nothing installed by nobrew yet. Try: nobrew install <name>");
        return Ok(());
    }
    println!("{:<24} {:<12} {}", "PACKAGE", "BACKEND", "REFERENCE");
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
        }
        None => println!("recipe:  none (would try brew, then pacman)"),
    }
    Ok(())
}

pub fn search(query: &str) -> Result<()> {
    let recs = recipe::registry();
    let hits: Vec<&String> = recs.keys().filter(|k| k.contains(query)).collect();
    if !hits.is_empty() {
        println!("== nobrew recipes ==");
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
