//! Google Play: checks an app bundle against Play's rules and uploads it with
//! the Google Play Developer API (edits.insert → edits.bundles.upload →
//! edits.tracks.update → edits.commit), authenticated as a service account.
//!
//! <https://developers.google.com/android-publisher/api-ref/rest/v3/edits>

use std::path::Path;
use std::time::Duration;

const SCOPE: &str = "https://www.googleapis.com/auth/androidpublisher";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
/// From August 31, 2026, new apps and updates target Android 16 (API 36).
pub const MIN_TARGET_SDK: u32 = 36;

/// A service account's JSON key (Google Cloud › IAM › Service accounts › Keys).
#[derive(Debug)]
pub struct ServiceAccount {
    pub client_email: String,
    pub key_id: Option<String>,
    private_key: Vec<u8>,
}

impl ServiceAccount {
    pub fn parse(json: &str) -> Result<Self, String> {
        let v: serde_json::Value = serde_json::from_str(json)
            .map_err(|e| format!("service account: invalid JSON ({e})"))?;
        if v["type"] != "service_account" {
            return Err(
                "service account: \"type\" is not \"service_account\" (download a JSON key for a service account)"
                    .into(),
            );
        }
        let field = |k: &str| {
            v[k].as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .ok_or_else(|| format!("service account: no \"{k}\""))
        };
        let token_uri = field("token_uri")?;
        if token_uri != TOKEN_URL {
            return Err(format!(
                "service account: token_uri is {token_uri}, expected {TOKEN_URL}"
            ));
        }
        let private_key = crate::jwt::pkcs8_der(&field("private_key")?)
            .map_err(|e| format!("service account: {e}"))?;
        Ok(ServiceAccount {
            client_email: field("client_email")?,
            key_id: v["private_key_id"].as_str().map(str::to_string),
            private_key,
        })
    }

    /// The signed JWT exchanged for an access token (valid one hour).
    pub fn assertion(&self, now: u64, audience: &str) -> Result<String, String> {
        let mut header = serde_json::json!({"alg": "RS256", "typ": "JWT"});
        if let Some(kid) = &self.key_id {
            header["kid"] = kid.clone().into();
        }
        let claims = serde_json::json!({
            "iss": self.client_email,
            "scope": SCOPE,
            "aud": audience,
            "iat": now,
            "exp": now + 3600,
        });
        crate::jwt::rs256(&self.private_key, &header, &claims)
    }
}

/// What a bundle declares (from `bundletool dump manifest`), its native
/// libraries, and who signed it.
#[derive(Debug, PartialEq)]
pub struct BundleInfo {
    pub package: String,
    pub version_code: u32,
    pub version_name: String,
    pub min_sdk: u32,
    pub target_sdk: u32,
    pub debuggable: bool,
    /// Subject of the one certificate that signed every entry, or why the
    /// signature is not one Play takes ([`bundle_signer`]).
    pub signer: Result<String, String>,
    /// Each native library (`base/lib/arm64-v8a/libapp.so`) with the
    /// smallest alignment of its loadable segments ([`load_alignment`]).
    pub native_libs: Vec<(String, Option<u64>)>,
    /// The bundle carries native debug symbols (Play Console's crashes).
    pub debug_symbols: bool,
}

/// The page size Android 15+ devices may use, which Google Play requires
/// 64-bit native libraries to be aligned for.
pub const PAGE_16K: u64 = 16384;

/// The smallest `p_align` of a 64-bit little-endian ELF file's loadable
/// segments (`PT_LOAD`); `None` for anything else (32-bit libraries, which
/// 16 KB pages don't concern).
pub fn load_alignment(elf: &[u8]) -> Option<u64> {
    let u16_at = |o: usize| Some(u16::from_le_bytes(elf.get(o..o + 2)?.try_into().ok()?));
    let u32_at = |o: usize| Some(u32::from_le_bytes(elf.get(o..o + 4)?.try_into().ok()?));
    let u64_at = |o: usize| Some(u64::from_le_bytes(elf.get(o..o + 8)?.try_into().ok()?));
    if elf.get(..6)? != b"\x7fELF\x02\x01" {
        return None;
    }
    let phoff = usize::try_from(u64_at(0x20)?).ok()?;
    let (size, count) = (usize::from(u16_at(0x36)?), usize::from(u16_at(0x38)?));
    (0..count)
        .map(|i| phoff + i * size)
        .filter(|&h| u32_at(h) == Some(1))
        .map(|h| u64_at(h + 48))
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .min()
}

fn attr<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let start = xml.find(&format!(" {name}=\""))? + name.len() + 3;
    let len = xml[start..].find('"')?;
    Some(&xml[start..start + len])
}

impl BundleInfo {
    /// Reads the manifest XML bundletool prints.
    pub fn from_manifest(xml: &str) -> Result<Self, String> {
        let num = |k: &str| attr(xml, k).and_then(|v| v.parse().ok()).unwrap_or(0);
        Ok(BundleInfo {
            package: attr(xml, "package")
                .ok_or("the bundle's manifest has no package")?
                .to_string(),
            version_code: num("android:versionCode"),
            version_name: attr(xml, "android:versionName").unwrap_or("").to_string(),
            min_sdk: num("android:minSdkVersion"),
            target_sdk: num("android:targetSdkVersion"),
            debuggable: attr(xml, "android:debuggable") == Some("true"),
            signer: Err(NOT_SIGNED.into()),
            native_libs: Vec::new(),
            debug_symbols: false,
        })
    }

    /// Play's rules for an upload; the first broken one.
    pub fn check(&self, package: &str) -> Result<(), String> {
        if self.package != package {
            return Err(format!(
                "the bundle is {}, the project is {package} (Prism.toml bundle_id)",
                self.package
            ));
        }
        crate::config::parse_version_code(&self.version_code.to_string())?;
        if self.target_sdk < MIN_TARGET_SDK {
            return Err(format!(
                "targetSdkVersion {} is too low: Google Play takes API {MIN_TARGET_SDK}+ (install platforms;android-{MIN_TARGET_SDK})",
                self.target_sdk
            ));
        }
        if self.debuggable {
            return Err("the bundle is debuggable: Play rejects it".into());
        }
        if let Some((lib, Some(align))) = self
            .native_libs
            .iter()
            .find(|(_, a)| a.is_some_and(|a| a < PAGE_16K))
        {
            return Err(format!(
                "{lib} is aligned for {align}-byte pages: Google Play requires 16 KB (Android 15+ devices); rebuild it with cargo-ndk 4+ or NDK r28+"
            ));
        }
        match &self.signer {
            Err(e) => Err(e.clone()),
            Ok(s) if s.contains("CN=Android Debug") => Err(
                "the bundle is signed with the debug key: set PRISM_ANDROID_KEYSTORE, PRISM_ANDROID_KEYSTORE_PASS and PRISM_ANDROID_KEY_ALIAS to your upload key"
                    .into(),
            ),
            Ok(_) => Ok(()),
        }
    }
}

const NOT_SIGNED: &str = "the bundle is not signed (jarsigner)";

/// The one signer of a bundle (its certificate's subject) from
/// `jarsigner -verify -strict -verbose -certs`, in English: its exit
/// `code` (`None`: stopped by a signal) and what it printed. Every entry
/// must be signed, and by one key. The strict check's "chain not validated"
/// (4: a self-signed certificate, as upload keys are; or an expired one)
/// is the only failure taken: Play checks the upload key itself.
pub fn signer_from_jarsigner(code: Option<i32>, output: &str) -> Result<String, String> {
    const CHAIN_NOT_VALIDATED: i32 = 4;
    const UNSIGNED_ENTRIES: i32 = 16;
    let code = code.ok_or("jarsigner was stopped")?;
    if code & UNSIGNED_ENTRIES != 0 {
        return Err(
            "some of the bundle's entries are not signed (changed after signing?): build and sign it again"
                .into(),
        );
    }
    if code & !CHAIN_NOT_VALIDATED != 0 {
        return Err(format!(
            "the bundle's signature does not verify (jarsigner -verify -strict: exit {code})"
        ));
    }
    if !output.lines().any(|l| l.starts_with("jar verified")) {
        return Err(NOT_SIGNED.into());
    }
    let subject = |l: &str, prefix: &str| {
        l.trim()
            .strip_prefix(prefix)
            .map(|s| s.trim_end_matches('"').to_string())
    };
    // One line per signer ...
    let mut signers: Vec<String> = output
        .lines()
        .filter_map(|l| subject(l, "- Signed by \""))
        .collect();
    // ... or, from older JDKs, each entry's signer certificates (the
    // signer's own first).
    if signers.is_empty() {
        let mut lines = output.lines();
        while lines.any(|l| l.trim() == ">>> Signer") {
            signers.extend(lines.find_map(|l| subject(l, "X.509, ")));
        }
    }
    signers.sort();
    signers.dedup();
    match signers.as_slice() {
        [] => Err(NOT_SIGNED.into()),
        [one] => Ok(one.clone()),
        several => Err(format!(
            "the bundle is signed by several keys ({}): sign it with your upload key only",
            several.join("; ")
        )),
    }
}

/// Who signed a bundle: one key, every entry ([`signer_from_jarsigner`]).
pub fn bundle_signer(aab: &Path) -> Result<String, String> {
    let out = std::process::Command::new("jarsigner")
        // Its messages in English, whatever the machine's language.
        .args(["-J-Duser.language=en", "-J-Duser.country=US"])
        .args(["-verify", "-strict", "-verbose", "-certs"])
        .arg(aab)
        .output()
        .map_err(|e| format!("jarsigner (Java): {e}"))?;
    signer_from_jarsigner(out.status.code(), &String::from_utf8_lossy(&out.stdout))
}

/// Reads and checks a bundle with bundletool and jarsigner.
pub fn inspect(aab: &Path, bundletool: &Path) -> Result<BundleInfo, String> {
    let out = std::process::Command::new("bash")
        .arg(bundletool)
        .arg("dump")
        .arg("manifest")
        .arg(format!("--bundle={}", aab.display()))
        .output()
        .map_err(|e| format!("bundletool: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "bundletool dump manifest: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let mut info = BundleInfo::from_manifest(&String::from_utf8_lossy(&out.stdout))?;
    info.signer = bundle_signer(aab);
    let list = std::process::Command::new("unzip")
        .arg("-Z1")
        .arg(aab)
        .output()
        .map_err(|e| format!("unzip: {e}"))?;
    for entry in String::from_utf8_lossy(&list.stdout).lines() {
        if entry.starts_with("BUNDLE-METADATA/com.android.tools.build.debugsymbols/") {
            info.debug_symbols = true;
        } else if entry.contains("/lib/") && entry.ends_with(".so") {
            let lib = std::process::Command::new("unzip")
                .arg("-p")
                .arg(aab)
                .arg(entry)
                .output()
                .map_err(|e| format!("unzip: {e}"))?;
            info.native_libs
                .push((entry.to_string(), load_alignment(&lib.stdout)));
        }
    }
    Ok(info)
}

/// A release on a track.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub package: String,
    pub version_code: u32,
    pub version_name: String,
    /// internal, alpha, beta, production (or a custom/form-factor track).
    pub track: String,
    /// draft, inProgress, halted, completed.
    pub status: String,
    /// The share of users of a staged rollout (inProgress, halted).
    pub user_fraction: Option<f64>,
    /// "What's new" (en-US).
    pub notes: Option<String>,
}

impl Release {
    pub fn check(&self) -> Result<(), String> {
        if self.track.is_empty()
            || !self
                .track
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_:".contains(c))
        {
            return Err(format!("invalid track {:?}", self.track));
        }
        match (self.status.as_str(), self.user_fraction) {
            ("draft" | "completed", None) => Ok(()),
            ("inProgress" | "halted", Some(f)) if f > 0.0 && f < 1.0 => Ok(()),
            ("inProgress" | "halted", _) => {
                Err("a staged rollout needs --rollout between 0 and 1 (exclusive)".into())
            }
            ("draft" | "completed", Some(_)) => {
                Err("--rollout only applies to a staged (inProgress) release".into())
            }
            (s, _) => Err(format!(
                "release status {s:?}: use draft, inProgress, halted or completed"
            )),
        }?;
        if self.notes.as_ref().is_some_and(|n| n.chars().count() > 500) {
            return Err("release notes are limited to 500 characters".into());
        }
        Ok(())
    }

    /// The edits.tracks.update body.
    pub fn track_body(&self) -> serde_json::Value {
        let mut release = serde_json::json!({
            "name": format!("{} ({})", self.version_name, self.version_code),
            "versionCodes": [self.version_code.to_string()],
            "status": self.status,
        });
        if let Some(f) = self.user_fraction {
            release["userFraction"] = f.into();
        }
        if let Some(n) = &self.notes {
            release["releaseNotes"] = serde_json::json!([{"language": "en-US", "text": n}]);
        }
        serde_json::json!({"track": self.track, "releases": [release]})
    }
}

/// Where the requests go: Google, or a local server in tests.
pub struct Endpoints {
    pub token: String,
    pub api: String,
    pub upload: String,
}

impl Endpoints {
    pub fn google() -> Self {
        Endpoints {
            token: TOKEN_URL.into(),
            api: "https://androidpublisher.googleapis.com/androidpublisher/v3".into(),
            upload: "https://androidpublisher.googleapis.com/upload/androidpublisher/v3".into(),
        }
    }

    /// The requests `publish` makes, for `--dry-run`.
    pub fn plan(&self, r: &Release, bytes: u64) -> Vec<String> {
        let app = format!("applications/{}", r.package);
        vec![
            format!("POST {} (service account JWT → access token)", self.token),
            format!("POST {}/{app}/edits", self.api),
            format!(
                "POST {}/{app}/edits/<edit>/bundles?uploadType=media ({bytes} bytes)",
                self.upload
            ),
            format!(
                "PUT  {}/{app}/edits/<edit>/tracks/{} {}",
                self.api,
                r.track,
                r.track_body()
            ),
            format!("POST {}/{app}/edits/<edit>:commit", self.api),
        ]
    }
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build()
        .into()
}

/// The JSON body of a response, or its error.
fn json(
    what: &str,
    res: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
) -> Result<serde_json::Value, String> {
    let mut res = res.map_err(|e| format!("{what}: {e}"))?;
    let status = res.status().as_u16();
    let text = res
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("{what}: {e}"))?;
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
    if !(200..300).contains(&status) {
        let msg = v["error"]["message"]
            .as_str()
            .or(v["error_description"].as_str())
            .map(str::to_string)
            .unwrap_or(text);
        return Err(format!("{what}: HTTP {status}: {msg}"));
    }
    Ok(v)
}

/// Uploads the bundle and releases it on the track. Returns the edit id.
pub fn publish(
    account: &ServiceAccount,
    endpoints: &Endpoints,
    release: &Release,
    aab: &Path,
) -> Result<String, String> {
    let http = agent(Duration::from_secs(60));
    let assertion = account.assertion(crate::jwt::now(), &endpoints.token)?;
    let token = json(
        "access token",
        http.post(&endpoints.token).send_form([
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", assertion.as_str()),
        ]),
    )?;
    let bearer = format!(
        "Bearer {}",
        token["access_token"]
            .as_str()
            .ok_or("access token: no access_token in the response")?
    );
    let app = format!("applications/{}", release.package);
    let edit = json(
        "edits.insert",
        http.post(format!("{}/{app}/edits", endpoints.api))
            .header("Authorization", &bearer)
            .header("Content-Type", "application/json")
            .send("{}"),
    )?;
    let id = edit["id"]
        .as_str()
        .ok_or("edits.insert: no edit id")?
        .to_string();
    let steps = || -> Result<(), String> {
        let file = std::fs::File::open(aab).map_err(|e| format!("{}: {e}", aab.display()))?;
        let len = file.metadata().map_err(|e| e.to_string())?.len();
        // Large bundles take a while: allow an hour.
        let bundle = json(
            "edits.bundles.upload",
            agent(Duration::from_secs(3600))
                .post(format!(
                    "{}/{app}/edits/{id}/bundles?uploadType=media",
                    endpoints.upload
                ))
                .header("Authorization", &bearer)
                .header("Content-Type", "application/octet-stream")
                .header("Content-Length", len.to_string())
                .send(file),
        )?;
        if bundle["versionCode"].as_u64() != Some(release.version_code.into()) {
            return Err(format!(
                "edits.bundles.upload: Play read versionCode {}, expected {}",
                bundle["versionCode"], release.version_code
            ));
        }
        json(
            "edits.tracks.update",
            http.put(format!(
                "{}/{app}/edits/{id}/tracks/{}",
                endpoints.api, release.track
            ))
            .header("Authorization", &bearer)
            .header("Content-Type", "application/json")
            .send(release.track_body().to_string()),
        )?;
        json(
            "edits.commit",
            http.post(format!("{}/{app}/edits/{id}:commit", endpoints.api))
                .header("Authorization", &bearer)
                .send_empty(),
        )?;
        Ok(())
    };
    if let Err(e) = steps() {
        // Leave nothing half-done in the Play Console.
        let _ = http
            .delete(format!("{}/{app}/edits/{id}", endpoints.api))
            .header("Authorization", &bearer)
            .call();
        return Err(e);
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// `None` where there is no openssl to make the RSA key: the test skips.
    fn account_json() -> Option<String> {
        let pem = crate::jwt::tests::rsa_key()?;
        let json = serde_json::json!({
            "type": "service_account",
            "project_id": "p",
            "private_key_id": "k1",
            "private_key": pem,
            "client_email": "publisher@p.iam.gserviceaccount.com",
            "token_uri": "https://oauth2.googleapis.com/token",
        });
        Some(json.to_string())
    }

    fn release() -> Release {
        Release {
            package: "dev.prism.counter".into(),
            version_code: 7,
            version_name: "1.2.0".into(),
            track: "internal".into(),
            status: "completed".into(),
            user_fraction: None,
            notes: Some("Faster".into()),
        }
    }

    #[test]
    fn parses_service_accounts() {
        let Some(json) = account_json() else { return };
        let account = ServiceAccount::parse(&json).unwrap();
        assert_eq!(account.client_email, "publisher@p.iam.gserviceaccount.com");
        let jwt = account.assertion(1000, TOKEN_URL).unwrap();
        let claims = jwt.split('.').nth(1).unwrap();
        let claims: serde_json::Value = serde_json::from_slice(
            &base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, claims)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(claims["scope"], SCOPE);
        assert_eq!(claims["aud"], TOKEN_URL);
        assert_eq!(claims["exp"], 4600);
        assert!(
            ServiceAccount::parse(r#"{"type":"authorized_user"}"#)
                .unwrap_err()
                .contains("service_account")
        );
        assert!(ServiceAccount::parse("nope").is_err());
    }

    #[test]
    fn reads_bundletool_manifests() {
        let xml = r#"<manifest xmlns:android="http://schemas.android.com/apk/res/android" android:compileSdkVersion="36" android:versionCode="7" android:versionName="1.2.0" package="dev.prism.counter">
  <uses-sdk android:minSdkVersion="29" android:targetSdkVersion="36"/>"#;
        let mut info = BundleInfo::from_manifest(xml).unwrap();
        assert_eq!(
            (info.version_code, info.min_sdk, info.target_sdk),
            (7, 29, 36)
        );
        assert!(
            info.check("dev.prism.counter")
                .unwrap_err()
                .contains("not signed")
        );
        info.signer = Ok("CN=Android Debug, O=Android, C=US".into());
        assert!(
            info.check("dev.prism.counter")
                .unwrap_err()
                .contains("debug key")
        );
        info.signer = Ok("CN=Upload".into());
        assert_eq!(info.check("dev.prism.counter"), Ok(()));
        assert!(info.check("dev.prism.other").is_err());
        info.target_sdk = 34;
        assert!(
            info.check("dev.prism.counter")
                .unwrap_err()
                .contains("API 36")
        );
    }

    /// A 64-bit ELF header with PT_LOAD segments aligned to `aligns`.
    fn elf(aligns: &[u64]) -> Vec<u8> {
        let mut e = vec![0u8; 64 + 56 * aligns.len()];
        e[..6].copy_from_slice(b"\x7fELF\x02\x01");
        e[0x20..0x28].copy_from_slice(&64u64.to_le_bytes());
        e[0x36..0x38].copy_from_slice(&56u16.to_le_bytes());
        e[0x38..0x3a].copy_from_slice(&(aligns.len() as u16).to_le_bytes());
        for (i, a) in aligns.iter().enumerate() {
            let h = 64 + 56 * i;
            e[h..h + 4].copy_from_slice(&1u32.to_le_bytes());
            e[h + 48..h + 56].copy_from_slice(&a.to_le_bytes());
        }
        e
    }

    #[test]
    fn checks_native_library_alignment() {
        assert_eq!(load_alignment(&elf(&[16384, 16384])), Some(16384));
        assert_eq!(load_alignment(&elf(&[16384, 4096])), Some(4096));
        assert_eq!(load_alignment(b"\x7fELF\x01\x01"), None);
        assert_eq!(load_alignment(&elf(&[16384])[..80]), None);
        let xml = r#"<manifest android:versionCode="7" android:versionName="1.2.0" package="dev.prism.counter">
  <uses-sdk android:minSdkVersion="29" android:targetSdkVersion="36"/>"#;
        let mut info = BundleInfo::from_manifest(xml).unwrap();
        info.signer = Ok("CN=Upload".into());
        info.native_libs = vec![("base/lib/arm64-v8a/libcounter.so".into(), Some(16384))];
        assert_eq!(info.check("dev.prism.counter"), Ok(()));
        info.native_libs
            .push(("base/lib/x86_64/libcounter.so".into(), Some(4096)));
        let err = info.check("dev.prism.counter").unwrap_err();
        assert!(err.contains("x86_64") && err.contains("16 KB"), "{err}");
    }

    #[test]
    fn reads_jarsigner_output() {
        let out = "s  1384 Sun Nov 15 META-INF/UPLOAD.RSA\n\n      >>> Signer\n      X.509, CN=Prism Test Upload, O=Prism, C=FR\n      [certificate is valid]\n\njar verified.\n";
        assert_eq!(
            signer_from_jarsigner(Some(0), out).as_deref(),
            Ok("CN=Prism Test Upload, O=Prism, C=FR")
        );
        // A self-signed upload key: the strict check's 4.
        let out = "- Signed by \"CN=Upload, O=Prism\"\n    Digest algorithm: SHA-256\njar verified, with signer errors.\n";
        assert_eq!(
            signer_from_jarsigner(Some(4), out).as_deref(),
            Ok("CN=Upload, O=Prism")
        );
        let err = |code, out| signer_from_jarsigner(code, out).unwrap_err();
        assert!(err(Some(0), "jar is unsigned.\n").contains("not signed"));
        // S40: unsigned entries (16), any other failure, two signers.
        assert!(err(Some(20), out).contains("entries are not signed"));
        assert!(err(Some(1), out).contains("does not verify"));
        assert!(err(Some(4 | 8), out).contains("does not verify"));
        assert!(err(None, out).contains("stopped"));
        let two = format!("- Signed by \"CN=Other\"\n{out}");
        assert!(err(Some(4), &two).contains("several keys"));
        let two = ">>> Signer\n X.509, CN=A\n X.509, CN=Root CA\n>>> Signer\n X.509, CN=B\njar verified.\n";
        assert!(err(Some(0), two).contains("several keys"));
        // A chain's other certificates are not signers.
        let chain = ">>> Signer\n X.509, CN=A\n X.509, CN=Root CA\n>>> Signer\n X.509, CN=A\njar verified.\n";
        assert_eq!(signer_from_jarsigner(Some(0), chain).as_deref(), Ok("CN=A"));
    }

    /// Whether the JDK's tools run (macOS has stubs that fail without one).
    fn jdk() -> bool {
        std::process::Command::new("jarsigner")
            .arg("-help")
            .output()
            .is_ok_and(|out| out.status.success())
    }

    /// S40: the Play preflight takes a bundle signed in full by one key; not
    /// one with an entry added after signing, nor one signed by two keys.
    /// Real bundles and test keys, made with the JDK's tools (skipped
    /// without a JDK).
    #[test]
    fn s40_only_a_bundle_signed_in_full_by_one_key_passes() {
        if !jdk() {
            eprintln!("no JDK: bundles are not signed");
            return;
        }
        let dir = crate::temp_dir::TempDir::new("play-s40");
        std::fs::create_dir_all(&dir).unwrap();
        let d: &std::path::Path = &dir;
        let run = |tool: &str, args: &[&str]| {
            let out = std::process::Command::new(tool)
                .args(args)
                .current_dir(d)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{tool} {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        for key in ["a", "b"] {
            let pass = format!("secret-{key}");
            run(
                "keytool",
                &[
                    "-genkeypair",
                    "-keystore",
                    &format!("{key}.p12"),
                    "-storetype",
                    "PKCS12",
                    "-storepass",
                    &pass,
                    "-keypass",
                    &pass,
                    "-alias",
                    key,
                    "-keyalg",
                    "RSA",
                    "-keysize",
                    "2048",
                    "-validity",
                    "1000",
                    "-dname",
                    &format!("CN=Key {key}, O=Test"),
                ],
            );
        }
        let sign = |key: &str, from: &str, to: &str| {
            let pass = format!("secret-{key}");
            let store = format!("{key}.p12");
            run(
                "jarsigner",
                &[
                    "-keystore",
                    &store,
                    "-storepass",
                    &pass,
                    "-signedjar",
                    to,
                    from,
                    key,
                ],
            );
        };
        for (path, data) in [
            ("src/base/manifest/AndroidManifest.xml", "manifest"),
            ("src/base/dex/classes.dex", "dex"),
            ("extra/base/root/added.txt", "added after signing"),
        ] {
            let path = d.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, data).unwrap();
        }
        run("jar", &["cfM", "unsigned.aab", "-C", "src", "base"]);
        sign("a", "unsigned.aab", "signed.aab");
        // A packaging step adds an entry after signing...
        std::fs::copy(d.join("signed.aab"), d.join("partial.aab")).unwrap();
        run(
            "jar",
            &["uf", "partial.aab", "-C", "extra", "base/root/added.txt"],
        );
        // ... and signs again with another key (which covers the new entry).
        sign("b", "partial.aab", "mixed.aab");
        sign("b", "signed.aab", "both.aab");

        assert_eq!(
            bundle_signer(&d.join("signed.aab")),
            Ok("CN=Key a, O=Test".into())
        );
        for (bundle, why) in [
            ("unsigned.aab", "not signed"),
            ("partial.aab", "entries are not signed"),
            ("mixed.aab", "several keys"),
            ("both.aab", "several keys"),
        ] {
            let r = bundle_signer(&d.join(bundle));
            assert!(
                r.as_ref().is_err_and(|e| e.contains(why)),
                "{bundle}: {r:?}"
            );
        }
    }

    #[test]
    fn checks_releases() {
        let mut r = release();
        assert_eq!(r.check(), Ok(()));
        assert_eq!(
            r.track_body(),
            serde_json::json!({"track": "internal", "releases": [{
                "name": "1.2.0 (7)", "versionCodes": ["7"], "status": "completed",
                "releaseNotes": [{"language": "en-US", "text": "Faster"}]}]})
        );
        r.status = "inProgress".into();
        assert!(r.check().is_err());
        r.user_fraction = Some(0.1);
        assert_eq!(r.check(), Ok(()));
        assert_eq!(r.track_body()["releases"][0]["userFraction"], 0.1);
        r.status = "completed".into();
        assert!(r.check().is_err());
        r.user_fraction = None;
        r.track = "wear:production".into();
        assert_eq!(r.check(), Ok(()));
        r.track = "a b".into();
        assert!(r.check().is_err());
    }

    /// A fake Google: records requests and answers like the real API.
    fn mock(fail_commit: bool) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let log = Arc::new(Mutex::new(Vec::new()));
        let seen = log.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let (mut len, mut auth) = (0usize, String::new());
                loop {
                    let mut h = String::new();
                    reader.read_line(&mut h).unwrap();
                    if h.trim().is_empty() {
                        break;
                    }
                    let (k, v) = h.split_once(':').unwrap();
                    match k.to_ascii_lowercase().as_str() {
                        "content-length" => len = v.trim().parse().unwrap(),
                        "authorization" => auth = v.trim().to_string(),
                        _ => {}
                    }
                }
                let mut body = vec![0; len];
                reader.read_exact(&mut body).unwrap();
                let request = line
                    .split_whitespace()
                    .take(2)
                    .collect::<Vec<_>>()
                    .join(" ");
                let body_text = String::from_utf8_lossy(&body).to_string();
                let (status, reply) = if request.starts_with("POST /token") {
                    assert!(body_text.contains(
                        "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer"
                    ));
                    assert!(body_text.contains("assertion=ey"));
                    (
                        200,
                        r#"{"access_token":"t0k","expires_in":3600,"token_type":"Bearer"}"#
                            .to_string(),
                    )
                } else {
                    assert_eq!(auth, "Bearer t0k", "{request}");
                    if request.ends_with("/edits") {
                        (200, r#"{"id":"e1","expiryTimeSeconds":"9"}"#.into())
                    } else if request.contains("/bundles?uploadType=media") {
                        assert_eq!(body, b"AAB!");
                        (200, r#"{"versionCode":7,"sha256":"x"}"#.into())
                    } else if request.starts_with("PUT") {
                        assert!(body_text.contains(r#""versionCodes":["7"]"#));
                        (200, body_text.clone())
                    } else if request.ends_with(":commit") && fail_commit {
                        (403, r#"{"error":{"code":403,"message":"The caller does not have permission"}}"#.into())
                    } else {
                        (200, r#"{"id":"e1"}"#.into())
                    }
                };
                seen.lock().unwrap().push(request);
                write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                )
                .unwrap();
            }
        });
        (base, log)
    }

    fn endpoints(base: &str) -> Endpoints {
        Endpoints {
            token: format!("{base}/token"),
            api: format!("{base}/androidpublisher/v3"),
            upload: format!("{base}/upload/androidpublisher/v3"),
        }
    }

    #[test]
    fn publishes_through_an_edit() {
        let Some(json) = account_json() else { return };
        let account = ServiceAccount::parse(&json).unwrap();
        let dir = crate::temp_dir::TempDir::new("play");
        std::fs::create_dir_all(&dir).unwrap();
        let aab = dir.join("app.aab");
        std::fs::write(&aab, b"AAB!").unwrap();

        let (base, log) = mock(false);
        let id = publish(&account, &endpoints(&base), &release(), &aab).unwrap();
        assert_eq!(id, "e1");
        let app = "/androidpublisher/v3/applications/dev.prism.counter";
        assert_eq!(
            *log.lock().unwrap(),
            [
                "POST /token".to_string(),
                format!("POST {app}/edits"),
                "POST /upload/androidpublisher/v3/applications/dev.prism.counter/edits/e1/bundles?uploadType=media".into(),
                format!("PUT {app}/edits/e1/tracks/internal"),
                format!("POST {app}/edits/e1:commit"),
            ]
        );

        // A failed step deletes the edit and reports Google's message.
        let (base, log) = mock(true);
        let err = publish(&account, &endpoints(&base), &release(), &aab).unwrap_err();
        assert!(
            err.contains("HTTP 403: The caller does not have permission"),
            "{err}"
        );
        assert_eq!(
            log.lock().unwrap().last().unwrap(),
            &format!("DELETE {app}/edits/e1")
        );
    }
}
