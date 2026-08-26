use anyhow::{anyhow, bail, Context, Result};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::manifest;
use crate::platform::{self, Platform};
use crate::recipe::{self, BinaryArtifact, BinarySpec, Recipe};
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

    // If the recipe names a binary and it's already on PATH (installed by any
    // means — e.g. the tool's own installer), adopt it instead of reinstalling
    // through a package backend. Keeps a self-installed `claude` from being
    // re-fetched as a brew cask, etc.
    if let Some(bin) = recipe
        .as_ref()
        .and_then(|r| r.script.provides_bin.as_deref())
    {
        if util::which(bin) {
            println!("✓ '{name}' already present ({bin} on PATH) — adopting into xbrew.");
            let record = Record {
                backend: "script".into(),
                reference: name.into(),
                kind: None,
                artifacts: vec![],
            };
            state.packages.insert(name.to_string(), record);
            state.save()?;
            return Ok(());
        }
    }

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
        // Extra repo packages the backend won't pull on its own (e.g. a build
        // tool an AUR PKGBUILD uses but forgets to declare as makedepends).
        if !r.arch.deps.is_empty() {
            let mut args = vec!["-S", "--needed", "--noconfirm"];
            args.extend(r.arch.deps.iter().map(String::as_str));
            util::run_priv("pacman", &args)?;
        }
        if let Some(binary) = &r.arch.binary {
            return install_arch_binary(name, binary);
        }
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
        if let Some(url) = &r.arch.pkgbuild {
            let (dir, pkg) = pkgbuild_spec(name, r)?;
            if pacman_installed(pkg) {
                report_adopted(name, pkg);
            } else {
                pkgbuild_install(url, dir, pkg)?;
            }
            return Ok(Record {
                backend: "pkgbuild".into(),
                reference: pkg.into(),
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

/// Which macOS backend a recipe selects. The precedence is fixed —
/// cask -> formula -> dmg -> script, first one declared wins — and lives in one
/// pure function so the order is a testable fact rather than the incidental
/// shape of an if-chain that spawns package managers.
#[derive(Debug, PartialEq, Eq)]
enum MacBackend<'a> {
    Cask(&'a str),
    Formula(&'a str),
    Dmg(&'a str),
    Script,
}

fn macos_backend(r: &Recipe) -> Option<MacBackend<'_>> {
    if let Some(cask) = &r.macos.cask {
        return Some(MacBackend::Cask(cask));
    }
    if let Some(formula) = &r.macos.formula {
        return Some(MacBackend::Formula(formula));
    }
    if let Some(dmg) = &r.macos.dmg {
        return Some(MacBackend::Dmg(dmg));
    }
    if r.script.install.is_some() {
        return Some(MacBackend::Script);
    }
    None
}

/// A `macos.formula` token is **data**: xbrew hands it to brew as exactly one
/// argv item and never through a shell. Accept only Homebrew's own grammar — a
/// plain name, or a fully-qualified `user/tap/formula` — so a recipe cannot
/// smuggle in a second argument (whitespace), a flag (leading dash), a path
/// (`.`, `..`, an extra or empty component) or a shell metacharacter. Checked
/// before brew is ever handed the token. (`install_macos` has by then run
/// `util::which("brew")`, which spawns a fixed `sh -c "command -v brew"` that
/// never sees the token — so this is "brew never receives it", not "no process
/// has started".)
pub fn validate_formula_token(token: &str) -> Result<()> {
    let reject = |why: &str| anyhow!("invalid Homebrew formula token '{token}': {why}");
    let parts: Vec<&str> = token.split('/').collect();
    // 1 = a formula in a tapped/core repo, 3 = user/tap/formula. Nothing else
    // is a Homebrew reference.
    if !matches!(parts.len(), 1 | 3) {
        return Err(reject("expected either `formula` or `user/tap/formula`"));
    }
    for part in parts {
        if let Some(why) = reference_component_fault(part) {
            return Err(reject(why));
        }
    }
    Ok(())
}

/// A tap reference is `user/tap` — the same per-component grammar as a formula,
/// one component shorter. Checked before `brew tap`/`brew trust` is handed the
/// token, for the same reason `validate_formula_token` exists: manifest `trust:`
/// entries are user data, and `- --force` must not arrive as a brew flag.
pub fn validate_tap_token(token: &str) -> Result<()> {
    let reject = |why: &str| anyhow!("invalid Homebrew tap token '{token}': {why}");
    let parts: Vec<&str> = token.split('/').collect();
    if parts.len() != 2 {
        return Err(reject("expected `user/tap`"));
    }
    for part in parts {
        if let Some(why) = reference_component_fault(part) {
            return Err(reject(why));
        }
    }
    Ok(())
}

/// The tap that owns a fully-qualified formula token: its first two components.
/// `None` for a plain (core/already-tapped) name, which belongs to no tap and
/// so grants nothing to trust.
pub fn tap_of_formula(token: &str) -> Option<String> {
    match token.split('/').collect::<Vec<&str>>()[..] {
        [user, tap, _formula] => Some(format!("{user}/{tap}")),
        _ => None,
    }
}

/// Why this component is not a usable piece of a Homebrew reference, or `None`
/// when it is. Shared by the formula and tap validators so the two can never
/// drift apart.
fn reference_component_fault(part: &str) -> Option<&'static str> {
    if part.is_empty() {
        return Some("empty name component");
    }
    if part.starts_with('-') {
        return Some("a leading dash would be read as a brew flag");
    }
    if part.starts_with('.') {
        return Some("name components may not start with a dot");
    }
    if !part
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"-_.+@".contains(&byte))
    {
        return Some("only letters, digits and the characters -_.+@ are allowed");
    }
    None
}

/// Trust a tap so Homebrew will load *everything* in it, not just the one item
/// we name.
///
/// `brew install user/tap/formula` grants implicit trust to that single named
/// formula and to nothing else (Homebrew `cmd/install.rb` passes only
/// `args.named` to `Trust.trust_fully_qualified_items!`), so the first
/// dependency the formula pulls from its own tap is refused — "Refusing to load
/// formula … from untrusted tap …". Trusting the tap covers every item in it
/// (`Homebrew::Trust.trusted?` short-circuits on `trusted_tap?`), which is why
/// this is a tap-level grant and not an enumeration of dependencies: xbrew
/// cannot know what a formula requires, and shouldn't have to.
///
/// Best effort on purpose: a Homebrew old enough to have no `trust` subcommand
/// has no trust gate to satisfy either, and a newer one that fails to record
/// the grant will refuse the install a moment later with its own instructions.
/// Failing hard here would break the first case to warn about the second.
fn brew_trust_tap(tap: &str) -> Result<()> {
    validate_tap_token(tap)?;
    if let Err(e) = util::run("brew", &["trust", "--tap", tap]) {
        eprintln!("\x1b[33m! brew trust --tap {tap} failed: {e:#}\x1b[0m");
    }
    Ok(())
}

/// macOS: a curated recipe is authoritative when it declares a macOS/script
/// backend; only names with no such recipe fall back to generic Homebrew.
fn install_macos(name: &str, recipe: Option<&Recipe>) -> Result<Record> {
    let has_brew = util::which("brew");

    if let Some(r) = recipe {
        match macos_backend(r) {
            Some(MacBackend::Cask(cask)) => {
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
                    reference: cask.to_string(),
                    kind: Some("cask".into()),
                    artifacts: vec![],
                });
            }
            Some(MacBackend::Formula(formula)) => {
                validate_formula_token(formula).with_context(|| {
                    format!("recipe '{name}' declares an unusable macOS formula")
                })?;
                if !has_brew {
                    bail!("'{name}' installs as a Homebrew formula, but brew isn't installed. Run `xbrew install brew` first.");
                }
                if brew_installed(formula, "formula") {
                    // Nothing is installed and no dependency is resolved, so
                    // there is nothing to trust for — leave the trust store
                    // alone.
                    report_adopted(name, formula);
                } else {
                    // Trust the tap *before* the install, because the install
                    // is what resolves the dependencies that implicit
                    // per-formula trust would refuse. No `brew tap` here:
                    // `brew install` already calls `tap&.ensure_installed!` for
                    // a fully-qualified name, so tapping would only duplicate
                    // it (the manifest `trust:` path does need it — see
                    // `bundle` — because plain names have no tap to install).
                    if let Some(tap) = tap_of_formula(formula) {
                        brew_trust_tap(&tap)?;
                    }
                    brew_install(formula, "formula")?;
                }
                // Recorded like any other brew formula, so uninstall/update/
                // version route through the generic brew paths unchanged.
                return Ok(Record {
                    backend: "brew".into(),
                    reference: formula.to_string(),
                    kind: Some("formula".into()),
                    artifacts: vec![],
                });
            }
            Some(MacBackend::Dmg(dmg)) => {
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
            Some(MacBackend::Script) | None => {
                if let Some(res) = recipe_script_install(name, r) {
                    return res;
                }
            }
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
        // pkgbuild builds register in the pacman DB exactly like AUR ones.
        "pacman" | "aur" | "pkgbuild" => {
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
        "recipe-binary" => {
            let destination = binary_destination(&rec.reference)?;
            if rec
                .artifacts
                .iter()
                .any(|artifact| Path::new(artifact) != destination)
            {
                bail!("refusing to remove a recipe-binary artifact outside ~/.xbrew/bin");
            }
            if destination.exists() {
                std::fs::remove_file(&destination)?;
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
    // Every declared tap is checked before anything runs, so a manifest that
    // can't express a tap fails closed instead of installing its entries
    // against whatever did register.
    for t in &trust {
        validate_tap_token(t)
            .with_context(|| format!("manifest declares an unusable tap '{t}'"))?;
    }
    if !trust.is_empty() {
        if !util::which("brew") {
            println!("\n\x1b[1m── ensuring Homebrew (needed for trusted taps) ──\x1b[0m");
            install("brew")?;
        }
        for t in &trust {
            println!("trust tap: {t}");
            // Two distinct things, both needed: `brew tap` clones the tap so a
            // plain formula name declared in this manifest resolves at all,
            // and `brew trust --tap` lets Homebrew actually load what it finds
            // there. Tapping without trusting is what made this path
            // misleading before — it looked like trust and wasn't.
            if let Err(e) = util::run("brew", &["tap", t]) {
                eprintln!("\x1b[33m! brew tap {t} failed: {e:#}\x1b[0m");
            }
            brew_trust_tap(t)?;
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

// ---------------------------------------------------------------------------
// update — show installed vs latest for tracked packages, upgrade on y/n/all
// ---------------------------------------------------------------------------

struct UpdateRow {
    name: String,
    backend: String,
    installed: String,
    latest: String,
    outdated: bool,
}

/// Check tracked packages (all, or the named ones) for newer versions and,
/// for each that is behind, prompt y / n / a(ll) / q(uit) to upgrade it.
pub fn update(names: &[String]) -> Result<()> {
    let state = State::load()?;
    if state.packages.is_empty() {
        println!("nothing installed by xbrew yet. Try: xbrew install <name>");
        return Ok(());
    }

    let explicit = !names.is_empty();
    let targets: Vec<String> = if names.is_empty() {
        state.packages.keys().cloned().collect()
    } else {
        for n in names {
            if !state.packages.contains_key(n) {
                bail!("'{n}' is not tracked by xbrew — see `xbrew list`");
            }
        }
        names.to_vec()
    };

    println!("checking {} package(s) for updates…\n", targets.len());

    let mut rows: Vec<UpdateRow> = Vec::new();
    for name in &targets {
        let installed = version::installed(name).unwrap_or_default();
        let latest = version::latest_available(name);
        let backend = state.packages[name].backend.clone();
        let vcs = backend == "pkgbuild";
        let pinned_binary = backend == "recipe-binary";
        let outdated = match (&latest, installed.is_empty()) {
            (Some(l), false) => version::compare(l, &installed) == std::cmp::Ordering::Greater,
            // VCS packages and immutable binary recipes have no remote registry
            // to query. Do not churn on a bare update; explicitly naming one
            // rebuilds or revalidates it against the recipe shipped by xbrew.
            _ => (vcs || pinned_binary) && explicit,
        };
        rows.push(UpdateRow {
            name: name.clone(),
            backend,
            installed: if installed.is_empty() {
                "?".into()
            } else {
                installed
            },
            latest: latest.unwrap_or_else(|| {
                if vcs {
                    "HEAD".into()
                } else if pinned_binary {
                    "recipe".into()
                } else {
                    "?".into()
                }
            }),
            outdated,
        });
    }

    println!(
        "{:<22} {:<9} {:<15} {:<15} STATUS",
        "PACKAGE", "BACKEND", "INSTALLED", "LATEST"
    );
    for r in &rows {
        let mark = if r.outdated {
            "\x1b[33mupdate\x1b[0m"
        } else if r.backend == "pkgbuild" {
            // Whether HEAD moved can't be known without building it, so don't
            // claim "ok" — `xbrew update <name>` forces the rebuild.
            "\x1b[2mgit\x1b[0m"
        } else if r.backend == "recipe-binary" && !r.outdated {
            "\x1b[2mpinned\x1b[0m"
        } else if r.latest == "?" {
            "\x1b[2m?\x1b[0m"
        } else {
            "\x1b[32mok\x1b[0m"
        };
        println!(
            "{:<22} {:<9} {:<15} {:<15} {mark}",
            r.name, r.backend, r.installed, r.latest
        );
    }

    let outdated: Vec<&UpdateRow> = rows.iter().filter(|r| r.outdated).collect();
    if outdated.is_empty() {
        println!("\n\x1b[32m✓\x1b[0m everything up to date.");
        return Ok(());
    }
    println!("\n{} update(s) available.", outdated.len());

    let mut all = false;
    let mut sudo_primed = false;
    let mut updated = 0usize;
    let mut failed: Vec<String> = Vec::new();
    for r in &outdated {
        if !all {
            match ask_char(&format!(
                "update {} {} → {}? [y/N/a=all/q=quit] ",
                r.name, r.installed, r.latest
            ))? {
                'y' => {}
                'a' => all = true,
                'q' => break,
                _ => {
                    println!("  skipped {}", r.name);
                    continue;
                }
            }
        }
        // Prime + keep sudo alive before the first privileged upgrade, so a
        // sequence of long AUR/pacman rebuilds doesn't hit a stale-password
        // timeout on a later `pacman -U`.
        if !sudo_primed
            && matches!(
                r.backend.as_str(),
                "pacman" | "aur" | "pkgbuild" | "dnf" | "apt"
            )
        {
            util::keep_sudo_alive();
            sudo_primed = true;
        }
        println!("\n\x1b[1m── updating {} ──\x1b[0m", r.name);
        match upgrade(&r.name) {
            Ok(()) => updated += 1,
            Err(e) => {
                eprintln!("\x1b[31merror:\x1b[0m {e:#}");
                failed.push(r.name.clone());
            }
        }
    }

    println!("\n{updated} updated.");
    if !failed.is_empty() {
        bail!("failed to update: {}", failed.join(", "));
    }
    Ok(())
}

/// Upgrade one tracked package via whatever backend installed it.
fn upgrade(name: &str) -> Result<()> {
    let state = State::load()?;
    let rec = state
        .packages
        .get(name)
        .ok_or_else(|| anyhow!("'{name}' is not tracked by xbrew"))?;
    match rec.backend.as_str() {
        "brew" => {
            if rec.kind.as_deref() == Some("cask") {
                util::run("brew", &["upgrade", "--cask", &rec.reference])
            } else {
                // `brew upgrade` has the same named-only trust as `brew
                // install` — Homebrew `cmd/upgrade.rb` calls
                // `Trust.trust_fully_qualified_items!` on `args.named` and
                // only then resolves the formulae — so an upgrade that pulls
                // in a new same-tap dependency is refused unless the tap is
                // trusted. This also repairs machines that adopted the formula
                // before any of this existed: adoption trusts nothing, so the
                // first upgrade is where they would otherwise break.
                if let Some(tap) = tap_of_formula(&rec.reference) {
                    brew_trust_tap(&tap)?;
                }
                util::run("brew", &["upgrade", &rec.reference])
            }
        }
        "pacman" => {
            eprintln!(
                "note: upgrading one Arch package — a full `sudo pacman -Syu` is the supported path."
            );
            util::run_priv("pacman", &["-S", "--noconfirm", &rec.reference])
        }
        "aur" => aur_install(&rec.reference), // git pull + makepkg -si rebuilds latest
        // A VCS PKGBUILD has no published release to move to — rebuilding it
        // against the current upstream HEAD *is* the update.
        "pkgbuild" => {
            let r = recipe::get(name).ok_or_else(|| {
                anyhow!("'{name}' was built from a PKGBUILD but no recipe declares it any more")
            })?;
            let url = r
                .arch
                .pkgbuild
                .clone()
                .ok_or_else(|| anyhow!("recipe '{name}' no longer declares a pkgbuild"))?;
            let (dir, pkg) = pkgbuild_spec(name, &r)?;
            pkgbuild_install(&url, dir, pkg)
        }
        "recipe-binary" => {
            let r = recipe::get(name).ok_or_else(|| {
                anyhow!("'{name}' was installed from a binary but no recipe declares it any more")
            })?;
            let binary = r
                .arch
                .binary
                .ok_or_else(|| anyhow!("recipe '{name}' no longer declares an Arch binary"))?;
            install_arch_binary(name, &binary).map(|_| ())
        }
        "flatpak" => util::run("flatpak", &["update", "-y", &rec.reference]),
        other => bail!("don't know how to update backend '{other}' — update it with its own tool"),
    }
}

/// Read a single y/n/a/q answer from the terminal (defaults to 'n' on empty).
fn ask_char(prompt: &str) -> Result<char> {
    use std::io::Write;
    print!("{prompt}");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line
        .trim()
        .chars()
        .next()
        .unwrap_or('n')
        .to_ascii_lowercase())
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
    // Check the *version output*, not the exit code: after `brew uninstall
    // --cask`, a leftover empty Caskroom dir can make `brew list --cask <name>`
    // still exit 0, which would make xbrew wrongly "adopt" a missing package.
    // `--versions` prints "<name> <ver>" only when actually installed.
    !util::capture("brew", &["list", flag, "--versions", name])
        .trim()
        .is_empty()
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
    // Build with system /usr/bin ahead of everything else on PATH. AUR PKGBUILDs
    // expect Arch tools; a linuxbrew (or other) shim of the same name — e.g. a
    // Homebrew `asar` (a SNES assembler) shadowing extra/asar (the Electron tool)
    // — otherwise silently gets used and breaks prepare()/build().
    let cmd = format!(
        "cd '{}' && PATH=\"/usr/bin:/usr/local/bin:$PATH\" makepkg -si --needed --noconfirm",
        dir.display()
    );
    util::run("sh", &["-c", &cmd])?;
    Ok(())
}

/// The `dir` + `pkg` an `arch.pkgbuild` recipe needs; both are mandatory, since
/// without them there is nothing to build and nothing to uninstall later.
fn pkgbuild_spec<'a>(name: &str, r: &'a Recipe) -> Result<(&'a str, &'a str)> {
    let dir =
        r.arch.dir.as_deref().ok_or_else(|| {
            anyhow!("recipe '{name}' has a pkgbuild but no `dir` to build it from")
        })?;
    let pkg =
        r.arch.pkg.as_deref().ok_or_else(|| {
            anyhow!("recipe '{name}' has a pkgbuild but no `pkg` name for uninstall")
        })?;
    Ok((dir, pkg))
}

/// Build a PKGBUILD that lives in an upstream git repo rather than the AUR.
/// Identical to `aur_install` past the clone URL: makepkg installs through
/// pacman, so `pacman -Rns` removes it cleanly and `pacman -Q` reports it.
fn pkgbuild_install(url: &str, dir: &str, pkg: &str) -> Result<()> {
    if !util::which("makepkg") {
        bail!("makepkg not found — install base-devel: sudo pacman -S --needed base-devel git");
    }
    let cache = util::xbrew_dir()?.join("pkgbuild");
    std::fs::create_dir_all(&cache)?;
    let repo = cache.join(pkg);
    if repo.join(".git").exists() {
        util::run_in(&repo, "git", &["pull", "--ff-only"])?;
    } else {
        // Shallow: only the PKGBUILD is read here. Any source the PKGBUILD
        // fetches is its own clone, under makepkg's control.
        util::run("git", &["clone", "--depth=1", url, repo.to_str().unwrap()])?;
    }
    let build = repo.join(dir);
    if !build.join("PKGBUILD").exists() {
        bail!("no PKGBUILD at '{dir}' in {url}");
    }
    // Same /usr/bin-first PATH as aur_install: PKGBUILDs expect Arch tools, and
    // a linuxbrew shim of the same name would otherwise shadow them.
    let cmd = format!(
        "cd '{}' && PATH=\"/usr/bin:/usr/local/bin:$PATH\" makepkg -si --needed --noconfirm",
        build.display()
    );
    util::run("sh", &["-c", &cmd])?;
    Ok(())
}

fn install_arch_binary(recipe_name: &str, binary: &BinarySpec) -> Result<Record> {
    let artifact = binary_asset_for_arch(binary, std::env::consts::ARCH)?;
    validate_binary_artifact(binary, artifact)?;

    let destination = binary_destination(&binary.name)?;
    if destination.exists() && sha256_file(&destination)? == artifact.sha256 {
        std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o755))?;
        report_adopted(recipe_name, destination.to_string_lossy().as_ref());
        return Ok(binary_record(binary, destination));
    }

    let root = util::xbrew_dir()?;
    let cache = root.join("cache");
    std::fs::create_dir_all(&cache)?;
    let temporary = cache.join(format!(
        ".{}-{}-{}.download",
        recipe_name,
        binary.name,
        std::process::id()
    ));
    let temporary_text = temporary
        .to_str()
        .ok_or_else(|| anyhow!("xbrew cache path is not valid UTF-8"))?;

    let result = (|| -> Result<()> {
        util::run(
            "curl",
            &[
                "--fail",
                "--location",
                "--retry",
                "3",
                "--output",
                temporary_text,
                &artifact.url,
            ],
        )?;
        let actual = sha256_file(&temporary)?;
        if actual != artifact.sha256 {
            bail!(
                "checksum mismatch for '{}': expected {}, got {}",
                binary.name,
                artifact.sha256,
                actual
            );
        }
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o755))?;
        let parent = destination
            .parent()
            .ok_or_else(|| anyhow!("binary destination has no parent"))?;
        std::fs::create_dir_all(parent)?;
        std::fs::rename(&temporary, &destination)?;
        Ok(())
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result?;
    Ok(binary_record(binary, destination))
}

fn binary_asset_for_arch<'a>(binary: &'a BinarySpec, arch: &str) -> Result<&'a BinaryArtifact> {
    match arch {
        "aarch64" => Ok(&binary.aarch64),
        "x86_64" => Ok(&binary.x86_64),
        other => bail!(
            "recipe binary '{}' does not support architecture '{other}'",
            binary.name
        ),
    }
}

fn validate_binary_artifact(binary: &BinarySpec, artifact: &BinaryArtifact) -> Result<()> {
    let valid_name = !binary.name.is_empty()
        && binary.name != "."
        && binary.name != ".."
        && binary
            .name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte));
    if !valid_name {
        bail!("recipe binary name must be a single safe filename");
    }
    if !artifact.url.starts_with("https://") {
        bail!("recipe binary URL must use HTTPS");
    }
    if !valid_sha256(&artifact.sha256) {
        bail!("recipe binary SHA-256 must be 64 lowercase hexadecimal characters");
    }
    Ok(())
}

fn valid_sha256(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn sha256_file(path: &Path) -> Result<String> {
    let path = path
        .to_str()
        .ok_or_else(|| anyhow!("binary path is not valid UTF-8"))?;
    let output = util::capture("sha256sum", &[path]);
    let digest = output
        .split_whitespace()
        .next()
        .ok_or_else(|| anyhow!("sha256sum failed for {path}"))?;
    if !valid_sha256(digest) {
        bail!("sha256sum returned an invalid digest for {path}");
    }
    Ok(digest.to_string())
}

fn binary_destination(name: &str) -> Result<PathBuf> {
    let valid_name = !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte));
    if !valid_name {
        bail!("recipe binary name must be a single safe filename");
    }
    Ok(util::xbrew_dir()?.join("bin").join(name))
}

fn binary_record(binary: &BinarySpec, destination: PathBuf) -> Record {
    Record {
        backend: "recipe-binary".into(),
        reference: binary.name.clone(),
        kind: None,
        artifacts: vec![destination.to_string_lossy().into_owned()],
    }
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
                    if let Some(p) = r.arch.pkgbuild {
                        let pkg = r.arch.pkg.as_deref().unwrap_or("?");
                        let dir = r.arch.dir.as_deref().unwrap_or("?");
                        println!("  arch → PKGBUILD: {pkg} (built from {p}, {dir})");
                    }
                    if let Some(f) = r.arch.flatpak {
                        println!("  arch → flatpak: {f}");
                    }
                    if let Some(binary) = r.arch.binary {
                        println!(
                            "  arch → verified binary: {} ({})",
                            binary.name,
                            std::env::consts::ARCH
                        );
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
                    if let Some(f) = r.macos.formula {
                        println!("  macos → formula: {f}");
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::{MacSpec, ScriptSpec};

    /// A recipe carrying exactly the macOS backends named, so the precedence
    /// between them can be read off without installing anything.
    fn mac_recipe(cask: Option<&str>, formula: Option<&str>, dmg: bool, script: bool) -> Recipe {
        Recipe {
            name: "probe".into(),
            macos: MacSpec {
                cask: cask.map(str::to_string),
                formula: formula.map(str::to_string),
                dmg: dmg.then(|| "https://example.test/App.dmg".to_string()),
                app: dmg.then(|| "App.app".to_string()),
            },
            script: ScriptSpec {
                install: script.then(|| "true".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn macos_precedence_is_cask_then_formula_then_dmg_then_script() {
        assert_eq!(
            macos_backend(&mac_recipe(Some("app"), Some("cli"), true, true)),
            Some(MacBackend::Cask("app"))
        );
        assert_eq!(
            macos_backend(&mac_recipe(None, Some("cli"), true, true)),
            Some(MacBackend::Formula("cli"))
        );
        assert_eq!(
            macos_backend(&mac_recipe(None, None, true, true)),
            Some(MacBackend::Dmg("https://example.test/App.dmg"))
        );
        assert_eq!(
            macos_backend(&mac_recipe(None, None, false, true)),
            Some(MacBackend::Script)
        );
        assert_eq!(macos_backend(&mac_recipe(None, None, false, false)), None);
    }

    #[test]
    fn formula_tokens_accept_the_homebrew_grammar() {
        for token in [
            "somawork",
            "somawork-preview",
            "node@20",
            "libc++",
            "2lab-ai/tap/somawork",
            "2lab-ai/tap/somawork-preview",
            "stablyai/orca/orca",
        ] {
            validate_formula_token(token)
                .unwrap_or_else(|e| panic!("{token} should be accepted: {e:#}"));
        }
    }

    /// Everything that could turn one argv item into something else: a second
    /// argument, a flag, a path, or a shell word.
    #[test]
    fn formula_tokens_reject_anything_that_is_not_a_reference() {
        for token in [
            "",
            "   ",
            "somawork --build-from-source",
            "somawork\tsecond",
            "--formula",
            "-somawork",
            "2lab-ai/-tap/somawork",
            "somawork;id",
            "somawork|id",
            "somawork&&id",
            "$(id)",
            "`id`",
            "somawork\nid",
            "some'work",
            "some\"work",
            "some*work",
            "some>work",
            "../../../etc/passwd",
            "2lab-ai/tap/../etc",
            "2lab-ai/tap/.",
            "/absolute/tap/somawork",
            "2lab-ai//somawork",
            "2lab-ai/tap",
            "2lab-ai/tap/somawork/extra",
        ] {
            assert!(
                validate_formula_token(token).is_err(),
                "{token:?} must be rejected"
            );
        }
    }

    #[test]
    fn a_tap_is_the_first_two_components_of_a_fully_qualified_formula() {
        assert_eq!(
            tap_of_formula("2lab-ai/tap/somawork-preview").as_deref(),
            Some("2lab-ai/tap")
        );
        assert_eq!(
            tap_of_formula("stablyai/orca/orca").as_deref(),
            Some("stablyai/orca")
        );
        // A plain name belongs to no tap, so there is nothing to trust.
        assert_eq!(tap_of_formula("somawork"), None);
        assert_eq!(tap_of_formula("node@20"), None);
    }

    /// Every tap derived from a token that `validate_formula_token` accepted is
    /// itself a valid tap token — so the install path can never hand brew a tap
    /// it did not check.
    #[test]
    fn a_tap_derived_from_an_accepted_formula_is_itself_valid() {
        for token in [
            "2lab-ai/tap/somawork",
            "2lab-ai/tap/somawork-preview",
            "stablyai/orca/orca",
            "vendor/brews/cli",
        ] {
            validate_formula_token(token).expect("token should be accepted");
            let tap = tap_of_formula(token).expect("fully-qualified token has a tap");
            validate_tap_token(&tap)
                .unwrap_or_else(|e| panic!("tap {tap} derived from {token} was rejected: {e:#}"));
        }
    }

    #[test]
    fn tap_tokens_accept_only_user_slash_tap() {
        for tap in [
            "2lab-ai/tap",
            "homebrew/core",
            "oven-sh/bun",
            "vendor/brews",
        ] {
            validate_tap_token(tap).unwrap_or_else(|e| panic!("{tap} should be accepted: {e:#}"));
        }
        for tap in [
            "",
            "   ",
            "2lab-ai",
            "2lab-ai/tap/somawork",
            "2lab-ai/",
            "/tap",
            "2lab-ai//tap",
            "--force",
            "2lab-ai/-tap",
            "-2lab-ai/tap",
            "2lab-ai/.",
            "2lab-ai/tap extra",
            "2lab-ai/tap;id",
            "$(id)/tap",
        ] {
            assert!(validate_tap_token(tap).is_err(), "{tap:?} must be rejected");
        }
    }

    fn binary_spec() -> BinarySpec {
        BinarySpec {
            name: "dbotter".into(),
            aarch64: BinaryArtifact {
                url: "https://example.test/dbotter-aarch64".into(),
                sha256: "a".repeat(64),
            },
            x86_64: BinaryArtifact {
                url: "https://example.test/dbotter-x86_64".into(),
                sha256: "b".repeat(64),
            },
        }
    }

    #[test]
    fn binary_recipe_selects_the_matching_architecture() {
        let binary = binary_spec();
        assert!(binary_asset_for_arch(&binary, "aarch64")
            .expect("aarch64 asset")
            .url
            .ends_with("aarch64"));
        assert!(binary_asset_for_arch(&binary, "x86_64")
            .expect("x86_64 asset")
            .url
            .ends_with("x86_64"));
        assert!(binary_asset_for_arch(&binary, "riscv64").is_err());
    }

    #[test]
    fn binary_recipe_rejects_unsafe_metadata() {
        let mut binary = binary_spec();
        binary.name = "../dbotter".into();
        assert!(validate_binary_artifact(&binary, &binary.aarch64).is_err());

        let mut binary = binary_spec();
        binary.aarch64.url = "http://example.test/dbotter".into();
        assert!(validate_binary_artifact(&binary, &binary.aarch64).is_err());

        let mut binary = binary_spec();
        binary.aarch64.sha256 = "not-a-sha".into();
        assert!(validate_binary_artifact(&binary, &binary.aarch64).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn sha256_file_measures_the_downloaded_bytes() {
        let path =
            std::env::temp_dir().join(format!("xbrew-sha256-contract-{}", std::process::id()));
        std::fs::write(&path, b"abc").expect("write checksum fixture");
        let digest = sha256_file(&path).expect("measure checksum fixture");
        let _ = std::fs::remove_file(path);
        assert_eq!(
            digest,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
