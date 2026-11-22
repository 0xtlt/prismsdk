//! Code signing for `prism build windows`: the executable (Authenticode),
//! and the MSIX package, with the developer's certificate:
//!
//! - `PRISM_WINDOWS_CERTIFICATE`: the `.pfx` (PKCS #12) file;
//! - `PRISM_WINDOWS_CERTIFICATE_PASS`: its password (never in Prism.toml);
//! - `PRISM_WINDOWS_TIMESTAMP_URL` (optional): an RFC 3161 time-stamping
//!   server, so the signature outlives the certificate.
//!
//! The password never goes on a command line, where any process of the
//! user (and process-creation logs) could read it:
//!
//! - `osslsigncode` (`PRISM_OSSLSIGNCODE`, else on `PATH`), first on every
//!   OS, signs executables and MSIX packages with the password in a file
//!   only the user reads (`-readpass`);
//! - else, on Windows, `signtool` (`PRISM_SIGNTOOL`, `PATH`, the Windows
//!   SDK's, or the `Microsoft.Windows.SDK.BuildTools` NuGet package's),
//!   which takes a password only as `/p`: Windows PowerShell opens the
//!   `.pfx` with the password read from its standard input, its key held
//!   in memory only (`EphemeralKeySet`), and writes it again without a
//!   password into a private temporary folder (this user only), which
//!   signtool reads and which is deleted right after. Nothing goes into a
//!   certificate store. signtool leaves the key it imported in the user's
//!   key store (`%APPDATA%\Microsoft\Crypto`): the files it added are
//!   deleted after it signed.
//!
//! An MSIX's `Publisher` must be the certificate's subject, as Windows
//! writes it (`CN=Example Ltd, O=Example Ltd, C=FR`): the executable is
//! signed first, and the subject read back from its signature
//! ([`signer_subject`]), so no tool (and no certificate store) is needed
//! to read the `.pfx`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The developer's certificate, from the environment.
pub struct Certificate {
    pfx: PathBuf,
    password: String,
    timestamp: Option<String>,
}

impl std::fmt::Debug for Certificate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Certificate")
            .field("pfx", &self.pfx)
            .field("password", &"<hidden>")
            .field("timestamp", &self.timestamp)
            .finish()
    }
}

pub const CERTIFICATE_VAR: &str = "PRISM_WINDOWS_CERTIFICATE";
pub const PASSWORD_VAR: &str = "PRISM_WINDOWS_CERTIFICATE_PASS";
pub const TIMESTAMP_VAR: &str = "PRISM_WINDOWS_TIMESTAMP_URL";

impl Certificate {
    /// `PRISM_WINDOWS_CERTIFICATE` and its password, if set.
    pub fn from_env() -> Result<Option<Certificate>, String> {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let Some(pfx) = var(CERTIFICATE_VAR) else {
            return Ok(None);
        };
        let pfx = PathBuf::from(pfx);
        if !pfx.is_file() {
            return Err(format!(
                "{CERTIFICATE_VAR}: {} is not a file (a .pfx)",
                pfx.display()
            ));
        }
        let timestamp = var(TIMESTAMP_VAR);
        if let Some(url) = &timestamp
            && !url.starts_with("http://")
            && !url.starts_with("https://")
        {
            return Err(format!("{TIMESTAMP_VAR}: {url:?} is not an http(s) URL"));
        }
        Ok(Some(Certificate {
            pfx,
            password: std::env::var(PASSWORD_VAR).unwrap_or_default(),
            timestamp,
        }))
    }

    /// Signs `file` (an `.exe` or an `.msix`) in place.
    pub fn sign(&self, file: &Path) -> Result<(), String> {
        let tool = Tool::find()?;
        let what = file.display();
        let dir = private_dir()?;
        match tool {
            Tool::Signtool(signtool) => {
                let pfx = dir.path().join("key.pfx");
                unprotect_pfx(&self.pfx, &self.password, &pfx)?;
                let before = key_files();
                let result = run(
                    self.signtool_command(&signtool, &pfx, file),
                    &format!("signtool sign {what}"),
                );
                let _ = std::fs::remove_file(&pfx);
                remove_new_key_files(&before);
                result
            }
            Tool::Osslsigncode(tool) => {
                let pass = dir.path().join("pass");
                write_private(&pass, &self.password)?;
                let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("bin");
                let out = dir.path().join(format!("signed.{ext}"));
                let cmd = self.osslsigncode_command(&tool, &pass, file, &out);
                let result = run(cmd, &format!("osslsigncode sign {what}"));
                let _ = std::fs::remove_file(&pass);
                result?;
                std::fs::copy(&out, file).map_err(|e| format!("{what}: {e}"))?;
                Ok(())
            }
        }
    }

    /// signtool with the password-less copy of the `.pfx`: no `/p`.
    fn signtool_command(&self, signtool: &Path, pfx: &Path, file: &Path) -> Command {
        let mut cmd = Command::new(signtool);
        cmd.args(["sign", "/fd", "SHA256", "/f"]).arg(pfx);
        if let Some(url) = &self.timestamp {
            cmd.args(["/tr", url, "/td", "SHA256"]);
        }
        cmd.arg(file);
        cmd.env_remove(PASSWORD_VAR);
        cmd
    }

    /// osslsigncode with the password in the private file `pass`.
    fn osslsigncode_command(&self, tool: &Path, pass: &Path, file: &Path, out: &Path) -> Command {
        let mut cmd = Command::new(tool);
        cmd.args(["sign", "-pkcs12"])
            .arg(&self.pfx)
            .arg("-readpass")
            .arg(pass)
            .args(["-h", "sha256"]);
        if let Some(url) = &self.timestamp {
            cmd.args(["-ts", url]);
        }
        cmd.arg("-in").arg(file).arg("-out").arg(out);
        cmd.env_remove(PASSWORD_VAR);
        cmd
    }
}

/// A new temporary folder only this user can open: 0700 on Unix
/// (`tempfile`), its inherited permissions replaced by this user's alone
/// on Windows.
fn private_dir() -> Result<tempfile::TempDir, String> {
    let dir = tempfile::Builder::new()
        .prefix("prism-sign-")
        .tempdir()
        .map_err(|e| format!("a private folder: {e}"))?;
    if cfg!(windows) {
        // By SID: `USERDOMAIN\USERNAME` does not always resolve (a
        // workgroup PC over SSH names the domain `WORKGROUP`).
        let sid = user_sid()?;
        let mut cmd = Command::new("icacls");
        cmd.arg(dir.path())
            .args(["/inheritance:r", "/grant:r"])
            .arg(format!("*{sid}:(OI)(CI)F"));
        run(cmd, &format!("icacls {}", dir.path().display()))?;
    }
    Ok(dir)
}

/// The current user's SID, from `whoami /user /fo csv /nh`.
fn user_sid() -> Result<String, String> {
    let out = Command::new("whoami")
        .args(["/user", "/fo", "csv", "/nh"])
        .output()
        .map_err(|e| format!("whoami: {e}"))?;
    sid_of(&String::from_utf8_lossy(&out.stdout))
        .ok_or_else(|| "whoami /user gave no SID".to_string())
}

/// The SID in a `whoami /user /fo csv /nh` line: `"pc\user","S-1-5-…"`.
fn sid_of(csv: &str) -> Option<String> {
    let sid = csv.trim().rsplit(',').next()?.trim_matches('"');
    (sid.starts_with("S-1-") && sid[4..].chars().all(|c| c.is_ascii_digit() || c == '-'))
        .then(|| sid.to_string())
}

/// Windows PowerShell: the `.pfx` in `PRISM_PFX_IN` opened with the
/// password from standard input (UTF-8), its private key kept in memory
/// (`EphemeralKeySet`: nothing goes into a certificate or key store), and
/// written again to `PRISM_PFX_OUT` without a password.
const UNPROTECT_PS1: &str = r#"$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
try {
    $in = [Console]::OpenStandardInput()
    $buffer = New-Object IO.MemoryStream
    $in.CopyTo($buffer)
    $password = [Text.Encoding]::UTF8.GetString($buffer.ToArray())
    $flags = [Security.Cryptography.X509Certificates.X509KeyStorageFlags]'Exportable, EphemeralKeySet'
    $certs = New-Object Security.Cryptography.X509Certificates.X509Certificate2Collection
    $certs.Import([IO.File]::ReadAllBytes($env:PRISM_PFX_IN), $password, $flags)
    $type = [Security.Cryptography.X509Certificates.X509ContentType]::Pkcs12
    [IO.File]::WriteAllBytes($env:PRISM_PFX_OUT, $certs.Export($type))
} catch {
    $e = $_.Exception
    while ($e.InnerException) { $e = $e.InnerException }
    [Console]::Error.WriteLine($e.Message.Trim())
    exit 1
}
"#;

/// The PowerShell command that writes the password-less copy: the
/// script as `-EncodedCommand`, the paths in the environment, nothing
/// secret in its arguments (the password goes to its standard input).
fn unprotect_command(pfx: &Path, out: &Path) -> Command {
    use base64::Engine as _;
    let utf16: Vec<u8> = UNPROTECT_PS1
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let mut cmd = Command::new("powershell");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-EncodedCommand",
    ])
    .arg(base64::engine::general_purpose::STANDARD.encode(utf16))
    .env("PRISM_PFX_IN", pfx)
    .env("PRISM_PFX_OUT", out)
    .env_remove(PASSWORD_VAR)
    .stdin(std::process::Stdio::piped())
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped());
    cmd
}

/// Writes `pfx` again at `out` without a password, for signtool.
fn unprotect_pfx(pfx: &Path, password: &str, out: &Path) -> Result<(), String> {
    use std::io::Write;
    let what = format!("opening {} with Windows PowerShell", pfx.display());
    let mut child = unprotect_command(pfx, out)
        .spawn()
        .map_err(|e| format!("{what}: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(password.as_bytes())
            .map_err(|e| format!("{what}: {e}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("{what}: {e}"))?;
    if output.status.success() && out.is_file() {
        return Ok(());
    }
    // PowerShell may add progress records (CLIXML) to its error output.
    let stderr = String::from_utf8_lossy(&output.stderr);
    let why: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("#< CLIXML") && !l.starts_with("<Objs"))
        .collect();
    Err(format!(
        "{what} failed (a wrong {PASSWORD_VAR}?): {}",
        why.join(" ")
    ))
}

/// The user's key store folders signtool's import writes to: CNG keys
/// (`Crypto\Keys`) and CryptoAPI ones (`Crypto\RSA\<SID>`).
fn key_dirs() -> Vec<PathBuf> {
    let Some(appdata) = std::env::var_os("APPDATA").filter(|_| cfg!(windows)) else {
        return Vec::new();
    };
    let crypto = PathBuf::from(appdata).join("Microsoft").join("Crypto");
    let mut dirs = vec![crypto.join("Keys")];
    if let Ok(sids) = std::fs::read_dir(crypto.join("RSA")) {
        dirs.extend(sids.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
    }
    dirs
}

/// The files in the key store folders now.
fn key_files() -> std::collections::HashSet<PathBuf> {
    key_dirs()
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flat_map(|entries| entries.flatten().map(|e| e.path()))
        .collect()
}

/// Deletes the key files that appeared since `before`: the key signtool
/// imported from the password-less `.pfx` and left behind.
fn remove_new_key_files(before: &std::collections::HashSet<PathBuf>) {
    for path in key_files().difference(before) {
        let _ = std::fs::remove_file(path);
    }
}

fn write_private(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    let mut f = o
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(text.as_bytes())
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn run(mut cmd: Command, what: &str) -> Result<(), String> {
    let out = cmd.output().map_err(|e| format!("{what}: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    Err(format!("{what} failed ({}):\n{}", out.status, text.trim()))
}

enum Tool {
    Signtool(PathBuf),
    Osslsigncode(PathBuf),
}

impl Tool {
    fn find() -> Result<Tool, String> {
        let runs = |tool: &Path, arg: &str| {
            Command::new(tool)
                .arg(arg)
                .output()
                .is_ok_and(|o| o.status.success() || !o.stdout.is_empty() || !o.stderr.is_empty())
        };
        let env = |k: &str| {
            std::env::var_os(k)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        // osslsigncode first, also on Windows: it reads the password from a
        // file; signtool takes the `.pfx` without one (`unprotect_pfx`).
        if let Some(t) = env("PRISM_OSSLSIGNCODE") {
            return Ok(Tool::Osslsigncode(t));
        }
        if cfg!(windows)
            && let Some(t) = env("PRISM_SIGNTOOL")
        {
            return Ok(Tool::Signtool(t));
        }
        let path = PathBuf::from("osslsigncode");
        if runs(&path, "--version") {
            return Ok(Tool::Osslsigncode(path));
        }
        if cfg!(windows) {
            let path = PathBuf::from("signtool");
            if runs(&path, "/?") {
                return Ok(Tool::Signtool(path));
            }
            if let Some(t) = windows_kits_signtool() {
                return Ok(Tool::Signtool(t));
            }
        }
        Err(if cfg!(windows) {
            "no osslsigncode or signtool: install osslsigncode or the Windows SDK, or unzip the \
             Microsoft.Windows.SDK.BuildTools NuGet package and set PRISM_SIGNTOOL to its \
             bin\\<version>\\x64\\signtool.exe"
                .into()
        } else {
            "no osslsigncode (it signs executables and MSIX packages): `brew install \
             osslsigncode`, `apt install osslsigncode`, or PRISM_OSSLSIGNCODE"
                .into()
        })
    }
}

/// The Windows SDK's newest `signtool.exe` for this PC.
fn windows_kits_signtool() -> Option<PathBuf> {
    let base = PathBuf::from(std::env::var_os("ProgramFiles(x86)")?)
        .join("Windows Kits")
        .join("10")
        .join("bin");
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86" => "x86",
        _ => "x64",
    };
    let mut versions: Vec<PathBuf> = std::fs::read_dir(&base)
        .ok()?
        .flatten()
        .map(|e| e.path().join(arch).join("signtool.exe"))
        .filter(|p| p.is_file())
        .collect();
    versions.sort();
    versions.pop()
}

// --- The signer's subject, from a signed executable ------------------------------------

/// The subject of the certificate that signed `exe` (Authenticode), as
/// Windows writes it: an MSIX's `Publisher` must be this.
pub fn signer_subject(exe: &[u8]) -> Result<String, String> {
    let pkcs7 = pe_signature(exe).ok_or("the executable has no Authenticode signature")?;
    let subject = pkcs7_signer_subject(pkcs7).ok_or("the signature holds no signer certificate")?;
    Ok(subject)
}

/// The PKCS #7 `SignedData` of a PE file's certificate table.
fn pe_signature(pe: &[u8]) -> Option<&[u8]> {
    let u16_at = |i: usize| Some(u16::from_le_bytes(pe.get(i..i + 2)?.try_into().ok()?));
    let u32_at = |i: usize| Some(u32::from_le_bytes(pe.get(i..i + 4)?.try_into().ok()?));
    if pe.get(..2)? != b"MZ" {
        return None;
    }
    let nt = u32_at(0x3C)? as usize;
    if pe.get(nt..nt + 4)? != b"PE\0\0" {
        return None;
    }
    let opt = nt + 24;
    // Data directories: after 96 bytes (PE32) or 112 (PE32+); the 5th is
    // the certificate table (a file offset, not an RVA).
    let dirs = match u16_at(opt)? {
        0x10b => opt + 96,
        0x20b => opt + 112,
        _ => return None,
    };
    let offset = u32_at(dirs + 4 * 8)? as usize;
    let size = u32_at(dirs + 4 * 8 + 4)? as usize;
    if offset == 0 || size < 8 {
        return None;
    }
    // WIN_CERTIFICATE: length, revision, type 2 (PKCS_SIGNED_DATA).
    let length = u32_at(offset)? as usize;
    if u16_at(offset + 6)? != 2 || length < 8 || length > size {
        return None;
    }
    pe.get(offset + 8..offset + length)
}

/// A DER element: its tag, its content and what follows it.
fn der(b: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let tag = *b.first()?;
    let first = *b.get(1)? as usize;
    let (len, head) = if first < 0x80 {
        (first, 2)
    } else {
        let n = first & 0x7F;
        if n == 0 || n > 4 {
            return None;
        }
        let mut len = 0usize;
        for i in 0..n {
            len = (len << 8) | *b.get(2 + i)? as usize;
        }
        (len, 2 + n)
    };
    let end = head.checked_add(len)?;
    Some((tag, b.get(head..end)?, b.get(end..)?))
}

/// The elements of a constructed DER value.
fn children(mut b: &[u8]) -> Vec<(u8, &[u8], &[u8])> {
    let mut out = Vec::new();
    while !b.is_empty() {
        let Some((tag, content, rest)) = der(b) else {
            break;
        };
        // The whole element (for comparing names).
        let whole = &b[..b.len() - rest.len()];
        out.push((tag, content, whole));
        b = rest;
    }
    out
}

/// The signer's certificate subject in a PKCS #7 `ContentInfo`: the
/// certificate whose issuer and serial number the first `SignerInfo`
/// names (else the first certificate).
fn pkcs7_signer_subject(pkcs7: &[u8]) -> Option<String> {
    let (_, info, _) = der(pkcs7)?;
    let info = children(info);
    // contentType signedData, [0] EXPLICIT SignedData.
    let (_, explicit, _) = info.get(1).filter(|e| e.0 == 0xA0)?;
    let (_, signed, _) = der(explicit)?;
    let parts = children(signed);
    let certs: Vec<(&[u8], &[u8], &[u8])> = parts
        .iter()
        .find(|p| p.0 == 0xA0)
        .map(|p| children(p.1))
        .unwrap_or_default()
        .into_iter()
        .filter(|c| c.0 == 0x30)
        .filter_map(|c| cert_names(c.1))
        .collect();
    let signer = parts
        .iter()
        .rev()
        .find(|p| p.0 == 0x31)
        .and_then(|set| children(set.1).into_iter().next())
        .and_then(|si| {
            let si = children(si.1);
            let ias = si.get(1).filter(|e| e.0 == 0x30)?;
            let ias = children(ias.1);
            Some((ias.first()?.2, ias.get(1)?.2))
        });
    let cert = match signer {
        Some((issuer, serial)) => certs
            .iter()
            .find(|c| c.0 == issuer && c.1 == serial)
            .or(certs.first()),
        None => certs.first(),
    }?;
    windows_name(cert.2)
}

/// A certificate's issuer, serial number (whole DER elements) and subject
/// (the name's content).
fn cert_names(cert: &[u8]) -> Option<(&[u8], &[u8], &[u8])> {
    let (_, tbs, _) = der(cert)?;
    let mut fields = children(tbs);
    if fields.first()?.0 == 0xA0 {
        fields.remove(0); // version
    }
    let serial = fields.first().filter(|f| f.0 == 0x02)?.2;
    let issuer = fields.get(2).filter(|f| f.0 == 0x30)?.2;
    let subject = fields.get(4).filter(|f| f.0 == 0x30)?.1;
    Some((issuer, serial, subject))
}

/// The attribute names Windows writes (`CertNameToStr`, X.500 form).
fn attribute_key(oid: &str) -> String {
    match oid {
        "2.5.4.3" => "CN",
        "2.5.4.7" => "L",
        "2.5.4.10" => "O",
        "2.5.4.11" => "OU",
        "1.2.840.113549.1.9.1" => "E",
        "2.5.4.6" => "C",
        "2.5.4.8" => "S",
        "2.5.4.9" => "STREET",
        "2.5.4.12" => "T",
        "2.5.4.42" => "G",
        "2.5.4.43" => "I",
        "2.5.4.4" => "SN",
        "0.9.2342.19200300.100.1.25" => "DC",
        "2.5.4.5" => "SERIALNUMBER",
        "2.5.4.13" => "Description",
        "2.5.4.17" => "PostalCode",
        "2.5.4.18" => "POBox",
        "2.5.4.20" => "Phone",
        "2.5.4.24" => "X21Address",
        "2.5.4.46" => "dnQualifier",
        _ => return format!("OID.{oid}"),
    }
    .to_string()
}

fn oid_text(b: &[u8]) -> Option<String> {
    let first = *b.first()?;
    let mut parts = vec![u64::from(first / 40).min(2), 0];
    parts[1] = u64::from(first) - parts[0] * 40;
    let mut v = 0u64;
    for &byte in &b[1..] {
        v = (v << 7) | u64::from(byte & 0x7F);
        if byte & 0x80 == 0 {
            parts.push(v);
            v = 0;
        }
    }
    Some(
        parts
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join("."),
    )
}

fn string_value(tag: u8, b: &[u8]) -> Option<String> {
    match tag {
        // UTF8String, PrintableString, IA5String, NumericString, VisibleString
        0x0C | 0x13 | 0x16 | 0x12 | 0x1A => String::from_utf8(b.to_vec()).ok(),
        // T61String: Latin-1, as Windows reads it.
        0x14 => Some(b.iter().map(|&c| c as char).collect()),
        // BMPString
        0x1E => {
            let units: Vec<u16> = b
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&c| u16::from_be_bytes(c))
                .collect();
            String::from_utf16(&units).ok()
        }
        // UniversalString
        0x1C => b
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&c| char::from_u32(u32::from_be_bytes(c)))
            .collect(),
        _ => None,
    }
}

/// A `Name` (its RDN sequence's content) as Windows writes a subject:
/// the last RDN first, `, ` between them, ` + ` inside one, values quoted
/// when they need it.
fn windows_name(name: &[u8]) -> Option<String> {
    let mut rdns = Vec::new();
    for (tag, set, _) in children(name) {
        if tag != 0x31 {
            return None;
        }
        let mut atvs = Vec::new();
        for (tag, atv, _) in children(set) {
            if tag != 0x30 {
                return None;
            }
            let atv = children(atv);
            let oid = atv.first().filter(|e| e.0 == 0x06)?;
            let value = atv.get(1)?;
            let key = attribute_key(&oid_text(oid.1)?);
            let value = string_value(value.0, value.1)?;
            atvs.push(format!("{key}={}", crate::msix::dn_value(&value)));
        }
        rdns.push(atvs.join(" + "));
    }
    rdns.reverse();
    Some(rdns.join(", "))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::temp_dir::TempDir;

    /// A DER element.
    fn el(tag: u8, content: &[u8]) -> Vec<u8> {
        let mut out = vec![tag];
        let n = content.len();
        if n < 0x80 {
            out.push(n as u8);
        } else if n < 0x100 {
            out.extend([0x81, n as u8]);
        } else {
            out.extend([0x82, (n >> 8) as u8, n as u8]);
        }
        out.extend(content);
        out
    }

    fn atv(oid: &[u8], tag: u8, value: &[u8]) -> Vec<u8> {
        el(0x31, &el(0x30, &[el(0x06, oid), el(tag, value)].concat()))
    }

    #[test]
    fn subjects_read_like_windows_writes_them() {
        // C=FR, S=Île-de-France, L=Paris, O=Tom, Jerry & Co, CN=Prism Test
        let name = [
            atv(&[0x55, 4, 6], 0x13, b"FR"),
            atv(&[0x55, 4, 8], 0x0C, "Île-de-France".as_bytes()),
            atv(&[0x55, 4, 7], 0x0C, b"Paris"),
            atv(&[0x55, 4, 10], 0x0C, b"Tom, Jerry & Co"),
            atv(&[0x55, 4, 3], 0x0C, b"Prism Test"),
            atv(
                &[0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x09, 0x01],
                0x16,
                b"dev@example.com",
            ),
            atv(&[0x2B, 6, 1, 4, 1, 0x82, 0x37, 0x3C, 2, 1, 3], 0x13, b"US"),
        ]
        .concat();
        assert_eq!(
            windows_name(&name).unwrap(),
            "OID.1.3.6.1.4.1.311.60.2.1.3=US, E=dev@example.com, CN=Prism Test, \
             O=\"Tom, Jerry & Co\", L=Paris, S=Île-de-France, C=FR"
        );
        // BMPString, and two values in one RDN.
        let bmp: Vec<u8> = "Ünï".encode_utf16().flat_map(u16::to_be_bytes).collect();
        let multi = el(
            0x31,
            &[
                el(0x30, &[el(0x06, &[0x55, 4, 11]), el(0x1E, &bmp)].concat()),
                el(0x30, &[el(0x06, &[0x55, 4, 3]), el(0x13, b"A")].concat()),
            ]
            .concat(),
        );
        assert_eq!(windows_name(&multi).unwrap(), "OU=Ünï + CN=A");
    }

    /// `PRISM_REQUIRE_SIGNING_TOOLS=1` (CI, where openssl and osslsigncode
    /// are installed): a signing test that cannot run fails instead of
    /// passing without testing anything.
    pub(crate) fn skip(why: &str) {
        let required = std::env::var("PRISM_REQUIRE_SIGNING_TOOLS").is_ok_and(|v| v == "1");
        assert!(!required, "PRISM_REQUIRE_SIGNING_TOOLS=1, but {why}");
        eprintln!("skipped: {why}");
    }

    /// The password is on no command line and in no child's environment:
    /// signtool gets a password-less copy, osslsigncode a file,
    /// PowerShell its standard input.
    #[test]
    fn the_password_stays_off_command_lines() {
        let cert = Certificate {
            pfx: "dev.pfx".into(),
            password: "s3cret-pass".into(),
            timestamp: Some("http://timestamp.example".into()),
        };
        let all = |cmd: &Command| {
            let mut words: Vec<String> = cmd
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            for (k, v) in cmd.get_envs() {
                words.push(k.to_string_lossy().into_owned());
                words.extend(v.map(|v| v.to_string_lossy().into_owned()));
            }
            words
        };
        let signtool = cert.signtool_command(
            Path::new("signtool.exe"),
            Path::new("tmp/key.pfx"),
            Path::new("app.exe"),
        );
        let words = all(&signtool);
        assert!(
            !words.iter().any(|w| w.contains("s3cret") || w == "/p"),
            "{words:?}"
        );
        assert_eq!(
            signtool.get_args().collect::<Vec<_>>(),
            [
                "sign",
                "/fd",
                "SHA256",
                "/f",
                "tmp/key.pfx",
                "/tr",
                "http://timestamp.example",
                "/td",
                "SHA256",
                "app.exe"
            ]
        );
        let ossl = cert.osslsigncode_command(
            Path::new("osslsigncode"),
            Path::new("tmp/pass"),
            Path::new("app.msix"),
            Path::new("tmp/signed.msix"),
        );
        let words = all(&ossl);
        assert!(!words.iter().any(|w| w.contains("s3cret")), "{words:?}");
        assert!(words.iter().any(|w| w == "-readpass"));
        let ps = unprotect_command(Path::new("dev.pfx"), Path::new("tmp/key.pfx"));
        let words = all(&ps);
        assert!(!words.iter().any(|w| w.contains("s3cret")), "{words:?}");
        // The script reads the password from its input, keeps the key in
        // memory and exports without a password.
        for part in ["OpenStandardInput", "EphemeralKeySet", "Export($type)"] {
            assert!(UNPROTECT_PS1.contains(part), "{part}");
        }
        assert!(!UNPROTECT_PS1.contains("Cert:"));
    }

    #[test]
    fn reads_whoamis_sid() {
        assert_eq!(
            sid_of("\"pc\\\\me\",\"S-1-5-21-1-2-3-1001\"\r\n").as_deref(),
            Some("S-1-5-21-1-2-3-1001")
        );
        assert_eq!(sid_of(""), None);
        assert_eq!(sid_of("\"pc\\\\me\",\"S-1-5 & x\""), None);
    }

    /// A tiny executable signed once with osslsigncode by a throwaway
    /// self-signed certificate (its private key was deleted): the signer
    /// is read back on every host, without any tool.
    const SIGNED_TINY_EXE: &[u8] = include_bytes!("testdata/signed-tiny.exe");

    #[test]
    fn reads_the_signer_of_a_signed_executable() {
        assert_eq!(
            signer_subject(SIGNED_TINY_EXE).unwrap(),
            "CN=Prism Fixture, O=\"Prism Fixture, Inc.\", S=Ile-de-France, C=FR"
        );
        // The same file unsigned (its first 1024 bytes, the certificate
        // table cleared), and a cut signature.
        let mut unsigned = SIGNED_TINY_EXE[..1024].to_vec();
        let dirs = 0x80 + 24 + 112 + 4 * 8;
        unsigned[dirs..dirs + 8].fill(0);
        assert_eq!(
            signer_subject(&unsigned).unwrap_err(),
            "the executable has no Authenticode signature"
        );
        let cut = &SIGNED_TINY_EXE[..SIGNED_TINY_EXE.len() - 100];
        assert!(signer_subject(cut).is_err());
    }

    /// A self-signed code-signing certificate and its `.pfx`, made with
    /// openssl in `dir` (never kept): `None` without openssl.
    pub(crate) fn test_pfx(dir: &Path, subject: &str, password: &str) -> Option<PathBuf> {
        let key = dir.join("key.pem");
        let cert = dir.join("cert.pem");
        let pfx = dir.join("test.pfx");
        let ok = Command::new("openssl")
            .args([
                "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2",
            ])
            .args(["-subj", subject])
            .args(["-addext", "extendedKeyUsage=codeSigning"])
            .args(["-addext", "basicConstraints=critical,CA:FALSE"])
            .arg("-keyout")
            .arg(&key)
            .arg("-out")
            .arg(&cert)
            .output()
            .is_ok_and(|o| o.status.success());
        if !ok {
            skip("no openssl to make a test certificate");
            return None;
        }
        let ok = Command::new("openssl")
            .args(["pkcs12", "-export", "-inkey"])
            .arg(&key)
            .arg("-in")
            .arg(&cert)
            .arg("-out")
            .arg(&pfx)
            .args(["-passout", &format!("pass:{password}")])
            .output()
            .is_ok_and(|o| o.status.success());
        let _ = std::fs::remove_file(&key);
        ok.then_some(pfx)
    }

    /// The smallest PE32+ file osslsigncode signs: headers, one section.
    pub(crate) fn tiny_exe() -> Vec<u8> {
        let mut pe = vec![0u8; 0x400];
        pe[..2].copy_from_slice(b"MZ");
        pe[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        let nt = 0x80;
        pe[nt..nt + 4].copy_from_slice(b"PE\0\0");
        let coff = nt + 4;
        pe[coff..coff + 2].copy_from_slice(&0x8664u16.to_le_bytes()); // x64
        pe[coff + 2..coff + 4].copy_from_slice(&1u16.to_le_bytes()); // sections
        pe[coff + 16..coff + 18].copy_from_slice(&240u16.to_le_bytes()); // optional header
        pe[coff + 18..coff + 20].copy_from_slice(&0x0022u16.to_le_bytes()); // executable, large address
        let opt = coff + 20;
        let put32 =
            |pe: &mut Vec<u8>, at: usize, v: u32| pe[at..at + 4].copy_from_slice(&v.to_le_bytes());
        pe[opt..opt + 2].copy_from_slice(&0x20bu16.to_le_bytes());
        put32(&mut pe, opt + 4, 0x200); // size of code
        put32(&mut pe, opt + 16, 0x1000); // entry point
        put32(&mut pe, opt + 20, 0x1000); // base of code
        pe[opt + 24..opt + 32].copy_from_slice(&0x1_4000_0000u64.to_le_bytes());
        put32(&mut pe, opt + 32, 0x1000); // section alignment
        put32(&mut pe, opt + 36, 0x200); // file alignment
        pe[opt + 40..opt + 42].copy_from_slice(&6u16.to_le_bytes()); // OS version
        pe[opt + 48..opt + 50].copy_from_slice(&6u16.to_le_bytes()); // subsystem version
        put32(&mut pe, opt + 56, 0x2000); // size of image
        put32(&mut pe, opt + 60, 0x200); // size of headers
        pe[opt + 68..opt + 70].copy_from_slice(&2u16.to_le_bytes()); // GUI
        put32(&mut pe, opt + 108, 16); // data directories
        let sec = opt + 240;
        pe[sec..sec + 5].copy_from_slice(b".text");
        put32(&mut pe, sec + 8, 0x10); // virtual size
        put32(&mut pe, sec + 12, 0x1000);
        put32(&mut pe, sec + 16, 0x200); // raw size
        put32(&mut pe, sec + 20, 0x200); // raw offset
        put32(&mut pe, sec + 36, 0x6000_0020); // code, execute, read
        pe[0x200] = 0xC3; // ret
        pe
    }

    /// Signs a tiny executable with a certificate made now, and reads its
    /// subject back (osslsigncode or signtool, and openssl, where they
    /// are).
    #[test]
    fn signs_and_reads_the_publisher_back() {
        let dir = TempDir::new("winsign");
        std::fs::create_dir_all(&dir).unwrap();
        let Some(pfx) = test_pfx(
            &dir,
            "/C=FR/ST=Ile-de-France/L=Paris/O=Prism Test, Inc./CN=Prism Test",
            "test-only",
        ) else {
            return;
        };
        let Ok(_) = Tool::find() else {
            skip("no signing tool");
            return;
        };
        let exe = dir.join("tiny.exe");
        std::fs::write(&exe, tiny_exe()).unwrap();
        assert_eq!(
            signer_subject(&tiny_exe()).unwrap_err(),
            "the executable has no Authenticode signature"
        );
        let cert = Certificate {
            pfx,
            password: "test-only".into(),
            timestamp: None,
        };
        cert.sign(&exe).unwrap();
        let signed = std::fs::read(&exe).unwrap();
        assert_eq!(
            signer_subject(&signed).unwrap(),
            "CN=Prism Test, O=\"Prism Test, Inc.\", L=Paris, S=Ile-de-France, C=FR"
        );
        // A wrong password fails with the tool's words, not silently.
        let wrong = Certificate {
            password: "wrong".into(),
            ..cert
        };
        assert!(wrong.sign(&exe).is_err());
        assert!(!format!("{wrong:?}").contains("wrong\""));
    }
}
