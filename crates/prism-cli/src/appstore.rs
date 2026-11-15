//! Apple distribution: checks store packages (.ipa, .pkg) and Developer ID
//! disk images, then uploads them with `xcrun altool --upload-package` or
//! notarizes them with `xcrun notarytool submit --wait` and `stapler`.
//!
//! Credentials: an App Store Connect API key (Users and Access › Integrations):
//! `PRISM_ASC_KEY_ID`, `PRISM_ASC_ISSUER_ID`, `PRISM_ASC_KEY` (the .p8 file).
//! Notarization can also use a `notarytool store-credentials` profile
//! (`PRISM_NOTARY_PROFILE`).

use std::path::{Path, PathBuf};
use std::process::Command;

/// An App Store Connect API key.
#[derive(Debug, Clone, PartialEq)]
pub struct AscKey {
    pub id: String,
    pub issuer: String,
    pub path: PathBuf,
}

impl AscKey {
    /// From `PRISM_ASC_KEY_ID`, `PRISM_ASC_ISSUER_ID` and `PRISM_ASC_KEY`.
    pub fn from_env() -> Result<Option<Self>, String> {
        Self::from_vars(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
    }

    pub fn from_vars(var: impl Fn(&str) -> Option<String>) -> Result<Option<Self>, String> {
        let (id, issuer, path) = (
            var("PRISM_ASC_KEY_ID"),
            var("PRISM_ASC_ISSUER_ID"),
            var("PRISM_ASC_KEY"),
        );
        match (id, issuer, path) {
            (None, None, None) => Ok(None),
            (Some(id), Some(issuer), Some(path)) => Ok(Some(AscKey {
                id,
                issuer,
                path: path.into(),
            })),
            _ => Err(
                "set all of PRISM_ASC_KEY_ID, PRISM_ASC_ISSUER_ID and PRISM_ASC_KEY (the AuthKey_<id>.p8 file)"
                    .into(),
            ),
        }
    }

    /// Checks the ids' shape and that the key signs an App Store Connect token.
    pub fn check(&self) -> Result<(), String> {
        if self.id.len() < 8 || !self.id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(format!(
                "PRISM_ASC_KEY_ID {:?} is not an API key id (10 letters and digits)",
                self.id
            ));
        }
        let uuid_shape = self.issuer.len() == 36
            && self.issuer.char_indices().all(|(i, c)| {
                if [8, 13, 18, 23].contains(&i) {
                    c == '-'
                } else {
                    c.is_ascii_hexdigit()
                }
            });
        if !uuid_shape {
            return Err(format!(
                "PRISM_ASC_ISSUER_ID {:?} is not an issuer id (a UUID)",
                self.issuer
            ));
        }
        let pem = std::fs::read_to_string(&self.path)
            .map_err(|e| format!("PRISM_ASC_KEY {}: {e}", self.path.display()))?;
        let der = crate::jwt::pkcs8_der(&pem).map_err(|e| format!("PRISM_ASC_KEY: {e}"))?;
        let now = crate::jwt::now();
        crate::jwt::es256(
            &der,
            &serde_json::json!({"alg": "ES256", "kid": self.id, "typ": "JWT"}),
            &serde_json::json!({"iss": self.issuer, "iat": now, "exp": now + 1200, "aud": "appstoreconnect-v1"}),
        )
        .map_err(|e| format!("PRISM_ASC_KEY: {e} (App Store Connect keys are P-256)"))?;
        Ok(())
    }
}

/// `xcrun altool --upload-package` (altool 27: the app is found from the package).
pub fn altool_upload(package: &Path, key: &AscKey) -> Vec<String> {
    vec![
        "altool".into(),
        "--upload-package".into(),
        package.display().to_string(),
        "--api-key".into(),
        key.id.clone(),
        "--api-issuer".into(),
        key.issuer.clone(),
        "--p8-file-path".into(),
        key.path.display().to_string(),
        "--output-format".into(),
        "json".into(),
    ]
}

/// How notarytool signs in.
#[derive(Debug, Clone, PartialEq)]
pub enum NotaryAuth {
    Key(AscKey),
    /// `xcrun notarytool store-credentials <name>`.
    Profile(String),
}

/// `xcrun notarytool submit <file> --wait`, JSON output.
pub fn notarytool_submit(file: &Path, auth: &NotaryAuth) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "notarytool".into(),
        "submit".into(),
        file.display().to_string(),
    ];
    match auth {
        NotaryAuth::Key(k) => args.extend([
            "--key".into(),
            k.path.display().to_string(),
            "--key-id".into(),
            k.id.clone(),
            "--issuer".into(),
            k.issuer.clone(),
        ]),
        NotaryAuth::Profile(p) => args.extend(["--keychain-profile".into(), p.clone()]),
    }
    args.extend(["--wait", "--timeout", "2h", "--output-format", "json"].map(String::from));
    args
}

/// The status in notarytool's JSON output (`Accepted`, `Invalid`…) and the id.
pub fn notary_result(json: &str) -> Result<(String, String), String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|_| format!("notarytool: {}", json.trim()))?;
    Ok((
        v["status"].as_str().unwrap_or("?").to_string(),
        v["id"].as_str().unwrap_or("?").to_string(),
    ))
}

/// Results of the local checks: problems stop an upload, warnings don't.
#[derive(Default, Debug)]
pub struct Report {
    pub ok: Vec<String>,
    pub warnings: Vec<String>,
    pub problems: Vec<String>,
}

impl Report {
    fn check(&mut self, ok: bool, good: impl Into<String>, bad: impl Into<String>) {
        if ok {
            self.ok.push(good.into());
        } else {
            self.problems.push(bad.into());
        }
    }

    pub fn print(&self) {
        for m in &self.ok {
            println!("  ok    {m}");
        }
        for m in &self.warnings {
            println!("  warn  {m}");
        }
        for m in &self.problems {
            println!("  FAIL  {m}");
        }
    }
}

/// What a package should declare.
pub struct Expected<'a> {
    pub bundle_id: &'a str,
    pub version: &'a str,
    pub build: &'a str,
    /// Accepted signing identities (name prefixes).
    pub identities: &'a [&'a str],
    /// Upload or notarization will happen: wrong identities are problems.
    pub strict: bool,
}

/// `Authority=` lines of `codesign -dvv` (leaf first).
pub fn authorities(codesign_dvv: &str) -> Vec<String> {
    codesign_dvv
        .lines()
        .filter_map(|l| l.strip_prefix("Authority="))
        .map(str::to_string)
        .collect()
}

/// Checks the identity that signed something.
fn identity(r: &mut Report, what: &str, signer: Option<&str>, e: &Expected) {
    match signer {
        Some(s) if e.identities.iter().any(|p| s.starts_with(p)) => {
            r.ok.push(format!("{what} signed by {s}"))
        }
        Some(s) => {
            let msg = format!(
                "{what} signed by {s:?}, not {}: Apple would refuse it",
                e.identities.join(" / ")
            );
            if e.strict {
                r.problems.push(msg)
            } else {
                r.warnings.push(msg)
            }
        }
        None => {
            let msg = format!("{what} is not signed: Apple would refuse it");
            if e.strict {
                r.problems.push(msg)
            } else {
                r.warnings.push(msg)
            }
        }
    }
}

/// How an app is distributed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dist {
    /// iOS, iPadOS, tvOS, visionOS, watchOS (.ipa).
    AppStore,
    /// macOS (.pkg).
    MacAppStore,
    /// macOS outside the store (.dmg, notarized).
    DeveloperId,
}

/// Checks an app's Info.plist (as JSON) for its distribution.
pub fn check_info(r: &mut Report, info: &serde_json::Value, e: &Expected, dist: Dist) {
    let s = |k: &str| info[k].as_str().unwrap_or("");
    r.check(
        s("CFBundleIdentifier") == e.bundle_id,
        format!("bundle id {}", e.bundle_id),
        format!(
            "bundle id is {:?}, expected {}",
            s("CFBundleIdentifier"),
            e.bundle_id
        ),
    );
    r.check(
        s("CFBundleShortVersionString") == e.version && s("CFBundleVersion") == e.build,
        format!("version {} ({})", e.version, e.build),
        format!(
            "version is {} ({}), expected {} ({})",
            s("CFBundleShortVersionString"),
            s("CFBundleVersion"),
            e.version,
            e.build
        ),
    );
    let short_ok = e.version.split('.').count() <= 3
        && e.version
            .split('.')
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    if !short_ok {
        r.problems.push(format!(
            "version {:?}: App Store versions are up to three numbers (1.2.3)",
            e.version
        ));
    }
    let missing: Vec<&str> = ["DTSDKName", "DTPlatformVersion", "DTXcode", "DTXcodeBuild"]
        .into_iter()
        .filter(|k| info[*k].is_null())
        .collect();
    r.check(
        missing.is_empty(),
        format!(
            "built with {} (Xcode {})",
            s("DTSDKName"),
            s("DTXcodeBuild")
        ),
        format!("Info.plist lacks {}", missing.join(", ")),
    );
    let icon = !info["CFBundleIcons"].is_null()
        || !info["CFBundleIconName"].is_null()
        || !info["CFBundleIconFile"].is_null();
    if !icon {
        r.problems
            .push("no app icon (Prism.toml [app] icon)".into());
    }
    if dist == Dist::MacAppStore && info["LSApplicationCategoryType"].is_null() {
        r.problems
            .push("no LSApplicationCategoryType (Prism.toml [app] category)".into());
    }
    if dist != Dist::DeveloperId && info["ITSAppUsesNonExemptEncryption"].is_null() {
        r.warnings.push(
            "export compliance is asked on each upload (Prism.toml [app] uses_encryption = false answers it)"
                .into(),
        );
    }
}

/// A provisioning profile's kind for distribution.
pub fn check_profile(r: &mut Report, profile: &serde_json::Value, bundle_id: &str) {
    let ent = &profile["Entitlements"];
    let app_id = ent["application-identifier"]
        .as_str()
        .or(ent["com.apple.application-identifier"].as_str())
        .unwrap_or("");
    let devices = profile["ProvisionedDevices"]
        .as_array()
        .is_some_and(|d| !d.is_empty())
        || profile["ProvisionsAllDevices"] == true;
    let store = !devices && ent["get-task-allow"] != true;
    r.check(
        store,
        format!(
            "App Store profile {:?}",
            profile["Name"].as_str().unwrap_or("?")
        ),
        format!(
            "profile {:?} is a development or ad hoc profile: use an App Store one",
            profile["Name"].as_str().unwrap_or("?")
        ),
    );
    let matches = app_id.split_once('.').is_some_and(|(_, id)| {
        id == bundle_id || (id.ends_with('*') && bundle_id.starts_with(&id[..id.len() - 1]))
    });
    r.check(
        matches,
        format!("profile covers {bundle_id}"),
        format!("profile is for {app_id}, not {bundle_id}"),
    );
}

fn run(cmd: &str, args: &[&str]) -> Result<std::process::Output, String> {
    Command::new(cmd)
        .args(args)
        .output()
        .map_err(|e| format!("{cmd}: {e}"))
}

/// A plist as JSON (plistlib: profiles hold dates and data, which `plutil
/// -convert json` refuses).
fn plist_json(path: &Path) -> Result<serde_json::Value, String> {
    let p = path.to_string_lossy();
    let out = run(
        "python3",
        &[
            "-c",
            "import json, plistlib, sys; print(json.dumps(plistlib.load(open(sys.argv[1], 'rb')), default=str))",
            &p,
        ],
    )?;
    serde_json::from_slice(&out.stdout).map_err(|_| format!("{p}: unreadable plist"))
}

fn profile_json(path: &Path) -> Result<serde_json::Value, String> {
    let out = run("security", &["cms", "-D", "-i", &path.to_string_lossy()])?;
    let tmp = std::env::temp_dir().join(format!("prism-profile-{}.plist", std::process::id()));
    std::fs::write(&tmp, &out.stdout).map_err(|e| e.to_string())?;
    let v = plist_json(&tmp);
    let _ = std::fs::remove_file(&tmp);
    v
}

/// `codesign --verify --deep --strict`, then the signing authority.
fn signature(r: &mut Report, what: &str, path: &Path, e: &Expected) -> String {
    let p = path.to_string_lossy();
    let verify = run("codesign", &["--verify", "--deep", "--strict", &p]);
    r.check(
        verify.as_ref().is_ok_and(|o| o.status.success()),
        format!("{what}: signature valid"),
        format!(
            "{what}: invalid signature ({})",
            verify
                .map(|o| String::from_utf8_lossy(&o.stderr).trim().to_string())
                .unwrap_or_else(|e| e)
        ),
    );
    let details = run("codesign", &["-dvv", &p])
        .map(|o| String::from_utf8_lossy(&o.stderr).into_owned())
        .unwrap_or_default();
    identity(
        r,
        what,
        authorities(&details).first().map(String::as_str),
        e,
    );
    details
}

/// The bundle's entitlements as JSON.
fn entitlements(app: &Path) -> serde_json::Value {
    let tmp = std::env::temp_dir().join(format!("prism-ent-{}.plist", std::process::id()));
    let ok = Command::new("codesign")
        .args(["-d", "--entitlements"])
        .arg(&tmp)
        .args(["--xml"])
        .arg(app)
        .output()
        .is_ok_and(|o| o.status.success());
    let v = if ok {
        plist_json(&tmp).unwrap_or_default()
    } else {
        serde_json::Value::Null
    };
    let _ = std::fs::remove_file(&tmp);
    v
}

/// Checks an .ipa (iOS, iPadOS, tvOS, visionOS, watchOS).
pub fn check_ipa(ipa: &Path, e: &Expected) -> Result<Report, String> {
    let mut r = Report::default();
    let dir = std::env::temp_dir().join(format!("prism-ipa-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let out = run(
        "ditto",
        &["-x", "-k", &ipa.to_string_lossy(), &dir.to_string_lossy()],
    )?;
    if !out.status.success() {
        return Err(format!("{}: not a zip archive", ipa.display()));
    }
    let app = std::fs::read_dir(dir.join("Payload"))
        .ok()
        .and_then(|d| {
            d.flatten()
                .map(|e| e.path())
                .find(|p| p.extension().is_some_and(|x| x == "app"))
        })
        .ok_or("the .ipa has no Payload/<App>.app")?;
    r.ok.push(format!(
        "Payload/{}",
        app.file_name().unwrap_or_default().to_string_lossy()
    ));
    check_info(
        &mut r,
        &plist_json(&app.join("Info.plist"))?,
        e,
        Dist::AppStore,
    );
    signature(&mut r, "app", &app, e);
    match profile_json(&app.join("embedded.mobileprovision")) {
        Ok(p) => check_profile(&mut r, &p, e.bundle_id),
        Err(_) => r.problems.push("no embedded.mobileprovision".into()),
    }
    let appex = app.join("PlugIns/PrismWidgets.appex");
    if appex.exists() {
        signature(&mut r, "widget extension", &appex, e);
        match profile_json(&appex.join("embedded.mobileprovision")) {
            Ok(p) => check_profile(&mut r, &p, &format!("{}.widgets", e.bundle_id)),
            Err(_) => r
                .problems
                .push("the widget extension has no profile".into()),
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    Ok(r)
}

/// Checks a Mac App Store package and the app it installs.
pub fn check_pkg(
    pkg: &Path,
    app: &Path,
    e: &Expected,
    installer: &[&str],
) -> Result<Report, String> {
    let mut r = Report::default();
    let out = run("pkgutil", &["--check-signature", &pkg.to_string_lossy()])?;
    let text = String::from_utf8_lossy(&out.stdout);
    // "   1. 3rd Party Mac Developer Installer: Name (TEAM)"
    let signer = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("1. "))
        .map(str::to_string);
    let installer_expected = Expected {
        identities: installer,
        ..*e
    };
    identity(&mut r, "package", signer.as_deref(), &installer_expected);
    check_info(
        &mut r,
        &plist_json(&app.join("Contents/Info.plist"))?,
        e,
        Dist::MacAppStore,
    );
    signature(&mut r, "app", app, e);
    let ent = entitlements(app);
    r.check(
        ent["com.apple.security.app-sandbox"] == true,
        "app sandbox",
        "the app is not sandboxed (the Mac App Store requires it)",
    );
    match profile_json(&app.join("Contents/embedded.provisionprofile")) {
        Ok(p) => check_profile(&mut r, &p, e.bundle_id),
        Err(_) => r.problems.push("no embedded.provisionprofile".into()),
    }
    Ok(r)
}

/// Checks a Developer ID disk image and its app before notarization.
pub fn check_dmg(dmg: &Path, app: &Path, e: &Expected) -> Result<Report, String> {
    let mut r = Report::default();
    signature(&mut r, "disk image", dmg, e);
    check_info(
        &mut r,
        &plist_json(&app.join("Contents/Info.plist"))?,
        e,
        Dist::DeveloperId,
    );
    let details = signature(&mut r, "app", app, e);
    r.check(
        details.contains("(runtime)"),
        "hardened runtime",
        "the app is not signed with the hardened runtime (--options runtime)",
    );
    if details.lines().any(|l| l.starts_with("Timestamp=")) {
        r.ok.push("secure timestamp".into());
    } else {
        let msg = "no secure timestamp: notarization requires one";
        if e.strict {
            r.problems.push(msg.into())
        } else {
            r.warnings.push(msg.into())
        }
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expected() -> Expected<'static> {
        Expected {
            bundle_id: "dev.prism.counter",
            version: "1.2.0",
            build: "7",
            identities: &["Apple Distribution"],
            strict: false,
        }
    }

    #[test]
    fn reads_keys_from_the_environment() {
        let none = AscKey::from_vars(|_| None).unwrap();
        assert_eq!(none, None);
        assert!(AscKey::from_vars(|k| (k == "PRISM_ASC_KEY_ID").then(|| "X".into())).is_err());
        let key = AscKey::from_vars(|k| Some(format!("v-{k}")))
            .unwrap()
            .unwrap();
        assert_eq!(key.path, PathBuf::from("v-PRISM_ASC_KEY"));
    }

    #[test]
    fn checks_api_keys() {
        let pem = crate::jwt::tests::openssl_key(&[
            "-algorithm",
            "EC",
            "-pkeyopt",
            "ec_paramgen_curve:P-256",
        ]);
        let path = std::env::temp_dir().join(format!("prism-asc-{}.p8", std::process::id()));
        std::fs::write(&path, pem).unwrap();
        let mut key = AscKey {
            id: "ABCDE12345".into(),
            issuer: "57246542-96fe-1a63-e053-0824d011072a".into(),
            path: path.clone(),
        };
        assert_eq!(key.check(), Ok(()));
        key.issuer = "nope".into();
        assert!(key.check().unwrap_err().contains("UUID"));
        key.issuer = "57246542-96fe-1a63-e053-0824d011072a".into();
        let rsa = crate::jwt::tests::openssl_key(&[
            "-algorithm",
            "RSA",
            "-pkeyopt",
            "rsa_keygen_bits:2048",
        ]);
        std::fs::write(&path, rsa).unwrap();
        assert!(key.check().unwrap_err().contains("P-256"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn builds_tool_commands() {
        let key = AscKey {
            id: "ABCDE12345".into(),
            issuer: "i".into(),
            path: "/k/AuthKey.p8".into(),
        };
        assert_eq!(
            altool_upload(Path::new("/o/App.ipa"), &key).join(" "),
            "altool --upload-package /o/App.ipa --api-key ABCDE12345 --api-issuer i --p8-file-path /k/AuthKey.p8 --output-format json"
        );
        assert_eq!(
            notarytool_submit(Path::new("/o/App.dmg"), &NotaryAuth::Key(key)).join(" "),
            "notarytool submit /o/App.dmg --key /k/AuthKey.p8 --key-id ABCDE12345 --issuer i --wait --timeout 2h --output-format json"
        );
        assert_eq!(
            notarytool_submit(Path::new("a.dmg"), &NotaryAuth::Profile("prism".into())).join(" "),
            "notarytool submit a.dmg --keychain-profile prism --wait --timeout 2h --output-format json"
        );
        assert_eq!(
            notary_result(r#"{"id":"2efe","message":"Processing complete","status":"Accepted"}"#),
            Ok(("Accepted".into(), "2efe".into()))
        );
    }

    #[test]
    fn checks_info_plists() {
        let mut info = serde_json::json!({
            "CFBundleIdentifier": "dev.prism.counter",
            "CFBundleShortVersionString": "1.2.0",
            "CFBundleVersion": "7",
            "DTSDKName": "iphoneos27.2", "DTPlatformVersion": "27.2",
            "DTXcode": "2720", "DTXcodeBuild": "27B52",
            "CFBundleIcons": {},
            "ITSAppUsesNonExemptEncryption": false,
        });
        let mut r = Report::default();
        check_info(&mut r, &info, &expected(), Dist::AppStore);
        assert!(r.problems.is_empty(), "{r:?}");
        let mut r = Report::default();
        check_info(&mut r, &info, &expected(), Dist::MacAppStore);
        assert_eq!(
            r.problems,
            ["no LSApplicationCategoryType (Prism.toml [app] category)"]
        );
        info["CFBundleVersion"] = "8".into();
        info.as_object_mut().unwrap().remove("DTXcode");
        info.as_object_mut().unwrap().remove("CFBundleIcons");
        let mut r = Report::default();
        check_info(&mut r, &info, &expected(), Dist::AppStore);
        assert_eq!(r.problems.len(), 3, "{r:?}");
    }

    #[test]
    fn checks_profiles_and_identities() {
        let store = serde_json::json!({"Name": "Store", "Entitlements": {"application-identifier": "T.dev.prism.counter"}});
        let mut r = Report::default();
        check_profile(&mut r, &store, "dev.prism.counter");
        assert!(r.problems.is_empty(), "{r:?}");
        let dev = serde_json::json!({"Name": "Dev", "ProvisionedDevices": ["x"],
            "Entitlements": {"application-identifier": "T.*", "get-task-allow": true}});
        let mut r = Report::default();
        check_profile(&mut r, &dev, "dev.prism.counter");
        assert_eq!(r.problems.len(), 1, "{r:?}");

        assert_eq!(
            authorities(
                "Identifier=x\nAuthority=Apple Distribution: A (T)\nAuthority=Apple Worldwide Developer Relations Certification Authority\n"
            ),
            [
                "Apple Distribution: A (T)",
                "Apple Worldwide Developer Relations Certification Authority"
            ]
        );
        let mut r = Report::default();
        identity(&mut r, "app", Some("Prism Test Distribution"), &expected());
        assert_eq!((r.warnings.len(), r.problems.len()), (1, 0));
        let mut r = Report::default();
        identity(
            &mut r,
            "app",
            Some("Prism Test Distribution"),
            &Expected {
                strict: true,
                ..expected()
            },
        );
        assert_eq!(r.problems.len(), 1);
    }
}
