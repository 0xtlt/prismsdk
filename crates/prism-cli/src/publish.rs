//! `prism publish <target>`: builds the store package, checks it locally,
//! then uploads it (Google Play, App Store Connect) or notarizes it (macOS
//! Developer ID). `--dry-run` does everything but the network calls.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::appstore::{self, AscKey, Expected, NotaryAuth};
use crate::config::Project;
use crate::play;
use crate::{Options, Target, flag};

pub struct PublishOptions {
    pub dry_run: bool,
    /// An already built package instead of building one.
    pub file: Option<PathBuf>,
    pub track: Option<String>,
    pub rollout: Option<f64>,
    pub draft: bool,
    pub notes: Option<String>,
    pub service_account: Option<String>,
    /// macOS: Developer ID + notarization instead of the Mac App Store.
    pub direct: bool,
}

impl PublishOptions {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        Ok(PublishOptions {
            dry_run: args.iter().any(|a| a == "--dry-run"),
            file: flag(args, "--file").map(PathBuf::from),
            track: flag(args, "--track"),
            rollout: flag(args, "--rollout")
                .map(|r| {
                    r.parse()
                        .map_err(|_| format!("--rollout {r}: a fraction like 0.1"))
                })
                .transpose()?,
            draft: args.iter().any(|a| a == "--draft"),
            notes: flag(args, "--notes"),
            service_account: flag(args, "--service-account"),
            direct: args.iter().any(|a| a == "--direct"),
        })
    }
}

/// The SDK directory an Apple target's device builds go to.
pub fn apple_sdk(target: Target) -> &'static str {
    match target {
        Target::Tvos => "appletvos",
        Target::Visionos => "xros",
        Target::Watchos => "watchos",
        _ => "iphoneos",
    }
}

/// Where `prism build <target> --store/--aab/--direct` writes its package.
pub fn package_path(p: &Project, target: Target, direct: bool) -> PathBuf {
    match target {
        Target::Android => p.target_dir.join(format!("android/release/{}.aab", p.name)),
        Target::Macos => p.target_dir.join(format!(
            "macos/{}.{}",
            p.name,
            if direct { "dmg" } else { "pkg" }
        )),
        t => p.target_dir.join(format!(
            "{}/{}/{}.ipa",
            crate::os_name(t),
            apple_sdk(t),
            p.name
        )),
    }
}

pub fn publish(p: &Project, target: Target, o: &PublishOptions) -> Result<(), String> {
    if matches!(target, Target::Web | Target::Linux | Target::Windows) {
        return Err(format!(
            "nothing to publish for {}: prism build {0} makes the package, ship it yourself (see the docs)",
            crate::os_name(target)
        ));
    }
    if o.direct && target != Target::Macos {
        return Err("--direct is for macOS (Developer ID, notarization)".into());
    }
    let package = match &o.file {
        Some(f) => f.clone(),
        None => {
            let build = Options {
                release: true,
                aab: target == Target::Android,
                store: target != Target::Android && !o.direct,
                direct: o.direct,
                ..Options::default()
            };
            crate::build_target(p, target, &build)?;
            package_path(p, target, o.direct)
        }
    };
    if !package.exists() {
        return Err(format!("{} does not exist", package.display()));
    }
    match target {
        Target::Android => android(p, &package, o),
        Target::Macos if o.direct => notarize(p, &package, o),
        _ => app_store(p, target, &package, o),
    }
}

fn heading(dry_run: bool, what: &str) {
    println!(
        "\n{}{what}",
        if dry_run {
            "Dry run (no network): "
        } else {
            ""
        }
    );
}

// --- Google Play ---------------------------------------------------------------------

fn service_account(o: &PublishOptions) -> Result<play::ServiceAccount, String> {
    // A command line is visible to every user of the machine (`ps`): the
    // flag takes a path, the environment variable a path or the key.
    if o.service_account
        .as_deref()
        .is_some_and(|s| s.trim_start().starts_with('{'))
    {
        return Err("--service-account takes the JSON key's path, not its contents (a command line is visible to other users): pass a file, or set PRISM_PLAY_SERVICE_ACCOUNT to the path or the JSON".into());
    }
    let source = o
        .service_account
        .clone()
        .or_else(|| std::env::var("PRISM_PLAY_SERVICE_ACCOUNT").ok())
        .filter(|s| !s.is_empty())
        .ok_or(
            "set PRISM_PLAY_SERVICE_ACCOUNT to the service account's JSON key (a path or its contents), or pass its path with --service-account",
        )?;
    let json = if source.trim_start().starts_with('{') {
        source
    } else {
        std::fs::read_to_string(&source).map_err(|e| format!("{source}: {e}"))?
    };
    play::ServiceAccount::parse(&json)
}

fn android(p: &Project, aab: &Path, o: &PublishOptions) -> Result<(), String> {
    let platform = crate::embedded::extract(&p.target_dir)?;
    let info = play::inspect(aab, &platform.join("scripts/bundletool.sh"))?;
    heading(o.dry_run, &format!("Google Play: {}", aab.display()));
    println!(
        "  bundle   {} {} ({}), API {}–{}, signed by {}",
        info.package,
        info.version_name,
        info.version_code,
        info.min_sdk,
        info.target_sdk,
        info.signer.as_deref().unwrap_or("nobody Play takes")
    );
    info.check(&p.bundle_id)?;
    let status = match (o.draft, o.rollout) {
        (true, Some(_)) => return Err("--draft and --rollout do not go together".into()),
        (true, None) => "draft",
        (false, Some(_)) => "inProgress",
        (false, None) => "completed",
    };
    let release = play::Release {
        package: info.package.clone(),
        version_code: info.version_code,
        version_name: info.version_name.clone(),
        track: o
            .track
            .clone()
            .or_else(|| p.play_track.clone())
            .unwrap_or_else(|| "internal".into()),
        status: status.into(),
        user_fraction: o.rollout,
        notes: o.notes.clone(),
    };
    release.check()?;
    let account = service_account(o)?;
    let endpoints = play::Endpoints::google();
    // Signing the token request proves the key works before any upload.
    account.assertion(crate::jwt::now(), &endpoints.token)?;
    println!("  account  {}", account.client_email);
    println!("  release  {} on the {} track", status, release.track);
    let size = std::fs::metadata(aab).map(|m| m.len()).unwrap_or(0);
    if o.dry_run {
        println!("  would send:");
        for step in endpoints.plan(&release, size) {
            println!("    {step}");
        }
        println!("Everything checks out. Run without --dry-run to upload.");
        return Ok(());
    }
    let edit = play::publish(&account, &endpoints, &release, aab)?;
    println!(
        "Released {} ({}) on the {} track (edit {edit}).",
        release.version_name, release.version_code, release.track
    );
    if status == "draft" {
        println!("It is a draft: roll it out in the Play Console.");
    }
    Ok(())
}

// --- Apple ---------------------------------------------------------------------------

fn expected<'a>(
    p: &'a Project,
    build: &'a str,
    identities: &'a [&'a str],
    o: &PublishOptions,
) -> Expected<'a> {
    Expected {
        bundle_id: &p.bundle_id,
        version: &p.version,
        build,
        identities,
        strict: !o.dry_run,
    }
}

fn finish(report: &appstore::Report) -> Result<(), String> {
    report.print();
    if report.problems.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} problem(s): fix them first",
            report.problems.len()
        ))
    }
}

fn xcrun_line(args: &[String]) -> String {
    let quoted: Vec<String> = args
        .iter()
        .map(|a| {
            if a.contains(' ') {
                format!("'{a}'")
            } else {
                a.clone()
            }
        })
        .collect();
    format!("xcrun {}", quoted.join(" "))
}

fn asc_key() -> Result<AscKey, String> {
    let key = AscKey::from_env()?.ok_or(
        "set PRISM_ASC_KEY_ID, PRISM_ASC_ISSUER_ID and PRISM_ASC_KEY (an App Store Connect API key: Users and Access › Integrations)",
    )?;
    key.check()?;
    Ok(key)
}

/// A Mac package's app, unpacked into a temporary directory.
fn expand_pkg(pkg: &Path, dir: &Path) -> Result<PathBuf, String> {
    let ok = Command::new("pkgutil")
        .arg("--expand-full")
        .arg(pkg)
        .arg(dir)
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        return Err(format!("{}: pkgutil --expand-full failed", pkg.display()));
    }
    find_app(dir, 3).ok_or_else(|| format!("{}: no app inside", pkg.display()))
}

fn find_app(dir: &Path, depth: u32) -> Option<PathBuf> {
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let path = e.path();
        if path.extension().is_some_and(|x| x == "app") {
            return Some(path);
        }
        if depth > 0
            && path.is_dir()
            && let Some(app) = find_app(&path, depth - 1)
        {
            return Some(app);
        }
    }
    None
}

fn app_store(
    p: &Project,
    target: Target,
    package: &Path,
    o: &PublishOptions,
) -> Result<(), String> {
    let build = p.build.clone().unwrap_or_else(|| "1".into());
    heading(
        o.dry_run,
        &format!("App Store Connect: {}", package.display()),
    );
    let key = asc_key()?;
    println!("  api key  {} (issuer {})", key.id, key.issuer);
    let report = if target == Target::Macos {
        // Removed on every return.
        let scratch = appstore::private_dir()?;
        let dir = scratch.path().join("pkg");
        let app = expand_pkg(package, &dir)?;
        let e = expected(
            p,
            &build,
            &["Apple Distribution", "3rd Party Mac Developer Application"],
            o,
        );
        let r = appstore::check_pkg(
            package,
            &app,
            &e,
            &[
                "3rd Party Mac Developer Installer",
                "Mac Installer Distribution",
            ],
        );
        r?
    } else {
        let e = expected(p, &build, &["Apple Distribution", "iPhone Distribution"], o);
        let mut r = appstore::check_ipa(package, &e)?;
        if target == Target::Watchos {
            r.warnings.push(
                "watch-only app: Xcode wraps these in an iOS stub app, which Prism does not make; App Store Connect may refuse this .ipa (untested)"
                    .into(),
            );
        }
        r
    };
    finish(&report)?;
    let args = appstore::altool_upload(package, &key);
    if o.dry_run {
        println!("  would run: {}", xcrun_line(&args));
        println!("Everything checks out. Run without --dry-run to upload.");
        return Ok(());
    }
    let status = Command::new("xcrun")
        .args(&args)
        .status()
        .map_err(|e| format!("xcrun altool: {e}"))?;
    if !status.success() {
        return Err(format!("altool --upload-package failed ({status})"));
    }
    println!(
        "Uploaded {} ({build}). App Store Connect processes it, then it shows in TestFlight.",
        p.version
    );
    Ok(())
}

/// Mounts a disk image read-only; returns the mount point.
fn attach(dmg: &Path, mount: &Path) -> Result<(), String> {
    let _ = std::fs::create_dir_all(mount);
    let ok = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-quiet", "-mountpoint"])
        .arg(mount)
        .arg(dmg)
        .status()
        .is_ok_and(|s| s.success());
    ok.then_some(())
        .ok_or_else(|| format!("hdiutil attach {} failed", dmg.display()))
}

fn notarize(p: &Project, dmg: &Path, o: &PublishOptions) -> Result<(), String> {
    let build = p.build.clone().unwrap_or_else(|| "1".into());
    heading(o.dry_run, &format!("Notarization: {}", dmg.display()));
    let auth = match std::env::var("PRISM_NOTARY_PROFILE")
        .ok()
        .filter(|v| !v.is_empty())
    {
        Some(profile) => {
            println!("  keychain profile {profile}");
            NotaryAuth::Profile(profile)
        }
        None => {
            let key = asc_key()?;
            println!("  api key  {} (issuer {})", key.id, key.issuer);
            NotaryAuth::Key(key)
        }
    };
    let scratch = appstore::private_dir()?;
    let mount = scratch.path().join("mount");
    attach(dmg, &mount)?;
    let report = find_app(&mount, 0)
        .ok_or_else(|| "the disk image holds no app".to_string())
        .and_then(|app| {
            let e = expected(p, &build, &["Developer ID Application"], o);
            appstore::check_dmg(dmg, &app, &e)
        });
    let detached = Command::new("hdiutil")
        .args(["detach", "-quiet"])
        .arg(&mount)
        .status()
        .is_ok_and(|s| s.success());
    if !detached {
        // Still mounted: the directory stays, so the image's files are
        // never removed with it.
        let _ = scratch.keep();
    }
    finish(&report?)?;
    let submit = appstore::notarytool_submit(dmg, &auth);
    let staple: Vec<String> = vec!["stapler".into(), "staple".into(), dmg.display().to_string()];
    if o.dry_run {
        println!("  would run: {}", xcrun_line(&submit));
        println!("  then:      {}", xcrun_line(&staple));
        println!("Everything checks out. Run without --dry-run to notarize.");
        return Ok(());
    }
    println!("Submitting to Apple's notary service (this takes a few minutes)…");
    let out = Command::new("xcrun")
        .args(&submit)
        .output()
        .map_err(|e| format!("xcrun notarytool: {e}"))?;
    let (status, id) = appstore::notary_result(&String::from_utf8_lossy(&out.stdout))?;
    if status != "Accepted" {
        return Err(format!(
            "notarization: {status} (details: xcrun notarytool log {id} …same credentials)"
        ));
    }
    for args in [
        staple,
        vec![
            "stapler".into(),
            "validate".into(),
            dmg.display().to_string(),
        ],
    ] {
        let ok = Command::new("xcrun")
            .args(&args)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            return Err(format!("{} failed", xcrun_line(&args)));
        }
    }
    println!(
        "Notarized and stapled {} (submission {id}): ready to download from your site.",
        dmg.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// S36: a command line is visible to every user of the machine (`ps`),
    /// so `--service-account` takes the key's path, never the key itself.
    #[test]
    fn s36_the_service_account_key_is_never_on_the_command_line() {
        let inline = r#"{"type":"service_account","private_key":"S36-SECRET"}"#;
        let o = PublishOptions::parse(&["--service-account".into(), inline.into()]).unwrap();
        let err = service_account(&o).unwrap_err();
        assert!(err.contains("path"), "{err}");
        assert!(!err.contains("S36-SECRET"), "{err}");
        // A path is read (here: a missing file).
        let o =
            PublishOptions::parse(&["--service-account".into(), "/nonexistent/key.json".into()])
                .unwrap();
        assert!(
            service_account(&o)
                .unwrap_err()
                .contains("/nonexistent/key.json")
        );
    }

    #[test]
    fn parses_options() {
        let args: Vec<String> = [
            "android",
            "--dry-run",
            "--track",
            "beta",
            "--rollout",
            "0.2",
            "--notes",
            "Hi",
        ]
        .map(String::from)
        .to_vec();
        let o = PublishOptions::parse(&args).unwrap();
        assert!(o.dry_run && !o.draft && !o.direct);
        assert_eq!(o.track.as_deref(), Some("beta"));
        assert_eq!(o.rollout, Some(0.2));
        assert_eq!(o.notes.as_deref(), Some("Hi"));
        assert!(PublishOptions::parse(&["--rollout".into(), "most".into()]).is_err());
    }

    #[test]
    fn quotes_commands() {
        assert_eq!(
            xcrun_line(&[
                "altool".into(),
                "--upload-package".into(),
                "/a b/App.ipa".into()
            ]),
            "xcrun altool --upload-package '/a b/App.ipa'"
        );
    }
}
