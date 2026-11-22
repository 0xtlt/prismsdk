//! MSIX packages in plain Rust, for `prism build windows --msix`: the
//! package's `AppxManifest.xml`, and the package itself, laid out as
//! `makeappx pack` lays it out, so it is made the same on Windows, macOS
//! and Linux:
//!
//! - a Zip64 archive: each file's local header with a data descriptor
//!   (Zip64 sizes), the central directory with Zip64 sizes and offsets, the
//!   Zip64 end records;
//! - files cut into 64 KiB blocks: each block deflated on its own (a full
//!   flush ends it, so it can be read alone) or the file stored as it is
//!   (pictures, already compressed);
//! - `AppxBlockMap.xml`: each file's size, local header size and the
//!   SHA-256 of each block (and of the whole file when it has several),
//!   which Windows checks when it installs the package and reads its files;
//! - `[Content_Types].xml`, last: the content type of each extension.
//!
//! Part names are URI-encoded in the archive (`My%20File.txt`) and written
//! with backslashes in the block map (`assets\My File.txt`).
//!
//! Signing adds `AppxSignature.p7x` (`signtool` or `osslsigncode`, see
//! `winsign.rs`).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{self, Write};
use std::time::SystemTime;

use base64::Engine as _;
use flate2::{Compress, Compression, FlushCompress, Status};

/// The block map's block size.
const BLOCK: usize = 64 * 1024;

/// Files kept as they are: already compressed.
const STORED: [&str; 14] = [
    "png", "jpg", "jpeg", "gif", "webp", "avif", "heic", "zip", "gz", "br", "mp3", "mp4", "woff2",
    "msix",
];

/// Names Windows keeps for itself at the package's root.
const RESERVED: [&str; 5] = [
    "appxmanifest.xml",
    "appxblockmap.xml",
    "[content_types].xml",
    "appxsignature.p7x",
    "appxmetadata",
];

/// The content type of a file extension (lower case).
fn content_type(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "ico" => "image/vnd.microsoft.icon",
        "exe" | "dll" => "application/x-msdownload",
        "json" => "application/json",
        "xml" => "text/xml",
        "txt" => "text/plain",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" => "application/javascript",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    }
}

const MANIFEST_TYPE: &str = "application/vnd.ms-appx.manifest+xml";
const BLOCKMAP_TYPE: &str = "application/vnd.ms-appx.blockmap+xml";

/// Text for XML.
pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// A package path (`assets/fonts/a.ttf`) as its name in the archive: each
/// byte outside URI's unreserved characters and the separators percent-
/// encoded (OPC part names).
fn part_name(path: &str) -> String {
    let mut out = String::new();
    for b in path.bytes() {
        let keep = b.is_ascii_alphanumeric() || b"-._~!$&'()+,;=@/".contains(&b);
        if keep {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// Whether `path` may be a file of the package: relative, `/`-separated,
/// without empty, `.` or `..` parts, a reserved root name, or characters
/// Windows refuses in file names.
pub fn check_path(path: &str) -> Result<(), String> {
    let bad = |why: &str| Err(format!("{path:?} cannot go into an MSIX: {why}"));
    if path.is_empty() || path.starts_with('/') || path.contains('\\') {
        return bad("not a relative path");
    }
    let first = path
        .split('/')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if RESERVED.contains(&first.as_str()) {
        return bad("Windows reserves this name");
    }
    for part in path.split('/') {
        if part.is_empty() || part == "." || part == ".." || part.ends_with(['.', ' ']) {
            return bad("an empty, `.` or `..` part, or one ending with a dot or a space");
        }
        if part
            .chars()
            .any(|c| c.is_control() || r#"<>:"|?*"#.contains(c))
        {
            return bad("a character Windows forbids in file names");
        }
    }
    Ok(())
}

/// One block of a file in the block map.
struct Block {
    hash: [u8; 32],
    /// Its deflated size; `None` for a stored file.
    size: Option<usize>,
}

/// A file written to the archive.
struct Written {
    /// Its name in the archive.
    part: String,
    /// Its path in the block map (`\`-separated); `None` for the block map
    /// itself and `[Content_Types].xml`.
    path: Option<String>,
    method: u16,
    crc: u32,
    size: u64,
    stored: u64,
    offset: u64,
    lfh_size: usize,
    blocks: Vec<Block>,
    file_hash: Option<[u8; 32]>,
}

fn sha256(data: &[u8]) -> [u8; 32] {
    let d = ring::digest::digest(&ring::digest::SHA256, data);
    let mut out = [0u8; 32];
    out.copy_from_slice(d.as_ref());
    out
}

/// Runs `input` through the compressor with `flush`, appending to `out`.
fn deflate(c: &mut Compress, mut input: &[u8], out: &mut Vec<u8>, flush: FlushCompress) {
    loop {
        out.reserve(input.len() / 2 + 4096);
        let before = c.total_in();
        let status = c
            .compress_vec(input, out, flush)
            .expect("deflate never fails on memory");
        input = &input[(c.total_in() - before) as usize..];
        // Done: everything taken and the flush complete (room left over).
        let flushed = input.is_empty() && out.len() < out.capacity();
        if status == Status::StreamEnd || (flush != FlushCompress::Finish && flushed) {
            return;
        }
    }
}

/// The data as `makeappx` deflates it: each 64 KiB block ended by a full
/// flush (byte-aligned, no reference to the blocks before), then the
/// stream's end. Returns the data and each block's deflated size.
fn deflate_blocks(data: &[u8]) -> (Vec<u8>, Vec<usize>) {
    let mut c = Compress::new(Compression::default(), false);
    let mut out = Vec::new();
    let mut sizes = Vec::new();
    for block in data.chunks(BLOCK) {
        let before = out.len();
        deflate(&mut c, block, &mut out, FlushCompress::Full);
        sizes.push(out.len() - before);
    }
    deflate(&mut c, &[], &mut out, FlushCompress::Finish);
    (out, sizes)
}

/// Writes an MSIX package: the payload (package path → bytes, in the
/// order given), then `AppxManifest.xml`, `AppxBlockMap.xml` and
/// `[Content_Types].xml`.
pub struct Packer<W: Write> {
    out: W,
    offset: u64,
    files: Vec<Written>,
    /// Extension → content type, in the order first seen.
    defaults: BTreeMap<String, &'static str>,
    overrides: Vec<(String, &'static str)>,
    time: (u16, u16),
}

impl<W: Write> Packer<W> {
    pub fn new(out: W, modified: SystemTime) -> Self {
        Packer {
            out,
            offset: 0,
            files: Vec::new(),
            defaults: BTreeMap::new(),
            overrides: Vec::new(),
            time: crate::zip::dos_time(modified),
        }
    }

    /// Adds a payload file at `path` (`/`-separated).
    pub fn add(&mut self, path: &str, data: &[u8]) -> io::Result<()> {
        check_path(path).map_err(io::Error::other)?;
        let ext = path
            .rsplit('/')
            .next()
            .and_then(|f| f.rsplit_once('.'))
            .map(|(_, e)| e.to_ascii_lowercase());
        match &ext {
            Some(e) => {
                self.defaults.entry(e.clone()).or_insert(content_type(e));
            }
            None => self
                .overrides
                .push((format!("/{}", part_name(path)), content_type(""))),
        }
        let compress = !data.is_empty() && !ext.as_deref().is_some_and(|e| STORED.contains(&e));
        self.write(
            &part_name(path),
            Some(path.replace('/', "\\")),
            data,
            compress,
        )
    }

    fn write(
        &mut self,
        part: &str,
        path: Option<String>,
        data: &[u8],
        compress: bool,
    ) -> io::Result<()> {
        let mut crc = flate2::Crc::new();
        crc.update(data);
        let hashes = data.chunks(BLOCK).map(sha256);
        let (method, body, blocks) = if compress {
            let (body, sizes) = deflate_blocks(data);
            let blocks = hashes
                .zip(sizes)
                .map(|(hash, size)| Block {
                    hash,
                    size: Some(size),
                })
                .collect();
            (8u16, std::borrow::Cow::Owned(body), blocks)
        } else {
            let blocks = hashes.map(|hash| Block { hash, size: None }).collect();
            (0u16, std::borrow::Cow::Borrowed(data), blocks)
        };
        let name_len =
            u16::try_from(part.len()).map_err(|_| io::Error::other("a path over 64 KiB"))?;
        // Version needed 4.5 (Zip64), a data descriptor follows the data;
        // the CRC and sizes are in it.
        let mut local = Vec::with_capacity(30 + part.len());
        local.extend(0x0403_4b50u32.to_le_bytes());
        for v in [45u16, 0x0008, method, self.time.0, self.time.1] {
            local.extend(v.to_le_bytes());
        }
        local.extend([0u8; 12]); // CRC, sizes
        local.extend(name_len.to_le_bytes());
        local.extend(0u16.to_le_bytes()); // extra field
        local.extend(part.as_bytes());
        let mut descriptor = Vec::with_capacity(24);
        descriptor.extend(0x0807_4b50u32.to_le_bytes());
        descriptor.extend(crc.sum().to_le_bytes());
        descriptor.extend((body.len() as u64).to_le_bytes());
        descriptor.extend((data.len() as u64).to_le_bytes());
        self.out.write_all(&local)?;
        self.out.write_all(&body)?;
        self.out.write_all(&descriptor)?;
        let file_hash = (data.len() > BLOCK).then(|| sha256(data));
        self.files.push(Written {
            part: part.to_string(),
            path,
            method,
            crc: crc.sum(),
            size: data.len() as u64,
            stored: body.len() as u64,
            offset: self.offset,
            lfh_size: local.len(),
            blocks,
            file_hash,
        });
        self.offset += (local.len() + body.len() + descriptor.len()) as u64;
        Ok(())
    }

    /// The block map of the files written so far.
    fn block_map(&self) -> String {
        let mut x = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\r\n<BlockMap \
             xmlns=\"http://schemas.microsoft.com/appx/2010/blockmap\" \
             xmlns:b4=\"http://schemas.microsoft.com/appx/2021/blockmap\" \
             IgnorableNamespaces=\"b4\" HashMethod=\"http://www.w3.org/2001/04/xmlenc#sha256\">",
        );
        let b64 = |h: &[u8; 32]| base64::engine::general_purpose::STANDARD.encode(h);
        for f in &self.files {
            let Some(path) = &f.path else { continue };
            let _ = write!(
                x,
                "<File Name=\"{}\" Size=\"{}\" LfhSize=\"{}\">",
                xml_escape(path),
                f.size,
                f.lfh_size
            );
            for b in &f.blocks {
                match b.size {
                    Some(size) => {
                        let _ = write!(x, "<Block Hash=\"{}\" Size=\"{size}\"/>", b64(&b.hash));
                    }
                    None => {
                        let _ = write!(x, "<Block Hash=\"{}\"/>", b64(&b.hash));
                    }
                }
            }
            if let Some(h) = &f.file_hash {
                let _ = write!(x, "<b4:FileHash Hash=\"{}\"/>", b64(h));
            }
            x.push_str("</File>");
        }
        x.push_str("</BlockMap>");
        x
    }

    fn content_types(&self) -> String {
        let mut x = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Types \
             xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">",
        );
        for (ext, ty) in &self.defaults {
            let _ = write!(
                x,
                "<Default Extension=\"{}\" ContentType=\"{ty}\" />",
                xml_escape(ext)
            );
        }
        for (part, ty) in &self.overrides {
            let _ = write!(
                x,
                "<Override PartName=\"{}\" ContentType=\"{ty}\" />",
                xml_escape(part)
            );
        }
        x.push_str("</Types>");
        x
    }

    /// Writes the manifest, the block map, `[Content_Types].xml` and the
    /// central directory.
    pub fn finish(mut self, manifest: &str) -> io::Result<W> {
        // The manifest's type: `xml` files' default, unless the payload
        // has XML files of its own (then an override, as makeappx does).
        match self.defaults.get("xml") {
            None => {
                self.defaults.insert("xml".into(), MANIFEST_TYPE);
            }
            Some(&t) if t == MANIFEST_TYPE => {}
            Some(_) => self
                .overrides
                .push(("/AppxManifest.xml".into(), MANIFEST_TYPE)),
        }
        self.write(
            "AppxManifest.xml",
            Some("AppxManifest.xml".into()),
            manifest.as_bytes(),
            true,
        )?;
        let map = self.block_map();
        self.write("AppxBlockMap.xml", None, map.as_bytes(), true)?;
        self.overrides
            .push(("/AppxBlockMap.xml".into(), BLOCKMAP_TYPE));
        let types = self.content_types();
        self.write("[Content_Types].xml", None, types.as_bytes(), true)?;

        let mut central = Vec::new();
        for f in &self.files {
            central.extend(0x0201_4b50u32.to_le_bytes());
            for v in [45u16, 45, 0x0008, f.method, self.time.0, self.time.1] {
                central.extend(v.to_le_bytes());
            }
            central.extend(f.crc.to_le_bytes());
            central.extend(u32::MAX.to_le_bytes()); // sizes: in the Zip64 field
            central.extend(u32::MAX.to_le_bytes());
            central.extend((f.part.len() as u16).to_le_bytes());
            central.extend(28u16.to_le_bytes()); // extra field
            for v in [0u16, 0, 0] {
                central.extend(v.to_le_bytes()); // comment, disk, internal attributes
            }
            central.extend(0u32.to_le_bytes()); // external attributes
            central.extend(u32::MAX.to_le_bytes()); // offset: in the Zip64 field
            central.extend(f.part.as_bytes());
            central.extend(1u16.to_le_bytes()); // Zip64 extended information
            central.extend(24u16.to_le_bytes());
            central.extend(f.size.to_le_bytes());
            central.extend(f.stored.to_le_bytes());
            central.extend(f.offset.to_le_bytes());
        }
        let entries = self.files.len() as u64;
        let cd_offset = self.offset;
        let end64 = cd_offset + central.len() as u64;
        let mut end = central;
        end.extend(0x0606_4b50u32.to_le_bytes());
        end.extend(44u64.to_le_bytes());
        for v in [45u16, 45] {
            end.extend(v.to_le_bytes());
        }
        end.extend(0u32.to_le_bytes()); // this disk
        end.extend(0u32.to_le_bytes()); // the directory's disk
        end.extend(entries.to_le_bytes());
        end.extend(entries.to_le_bytes());
        end.extend((end64 - cd_offset).to_le_bytes());
        end.extend(cd_offset.to_le_bytes());
        end.extend(0x0706_4b50u32.to_le_bytes()); // Zip64 locator
        end.extend(0u32.to_le_bytes());
        end.extend(end64.to_le_bytes());
        end.extend(1u32.to_le_bytes());
        end.extend(0x0605_4b50u32.to_le_bytes());
        for _ in 0..4 {
            end.extend(u16::MAX.to_le_bytes());
        }
        end.extend(u32::MAX.to_le_bytes());
        end.extend(u32::MAX.to_le_bytes());
        end.extend(0u16.to_le_bytes()); // comment
        self.out.write_all(&end)?;
        Ok(self.out)
    }
}

// --- The manifest ----------------------------------------------------------------------

/// What `AppxManifest.xml` says.
#[derive(Debug, Default, Clone)]
pub struct Manifest {
    /// `Identity Name`: the bundle id, `_` written `-`.
    pub name: String,
    /// `Identity Publisher`: the signing certificate's subject.
    pub publisher: String,
    /// The four-part version.
    pub version: String,
    /// `x64`, `arm64` or `x86`.
    pub arch: String,
    pub display_name: String,
    pub publisher_display_name: String,
    pub description: String,
    /// The executable, at the package's root.
    pub executable: String,
    /// BCP 47 tags (`Resources`); English when empty.
    pub languages: Vec<String>,
    /// The COM class a click on a notification starts
    /// (`ToastActivatorCLSID`).
    pub toast_clsid: String,
    /// `DeviceCapability` names (`location`, `webcam`, `microphone`).
    pub device_capabilities: Vec<String>,
}

/// The logos the manifest names: (package path, size in pixels).
pub const LOGOS: [(&str, u32); 3] = [
    ("Logos/StoreLogo.png", 50),
    ("Logos/Square44x44Logo.png", 44),
    ("Logos/Square150x150Logo.png", 150),
];

/// The oldest Windows the package installs on (Windows 10 1809, the
/// Windows App SDK's), and the newest it was tested on.
const MIN_WINDOWS: &str = "10.0.17763.0";
const MAX_TESTED: &str = "10.0.26100.0";

impl Manifest {
    pub fn xml(&self) -> String {
        let e = xml_escape;
        let languages = if self.languages.is_empty() {
            vec!["en-us".to_string()]
        } else {
            self.languages.clone()
        };
        let mut resources = String::new();
        for l in &languages {
            let _ = writeln!(resources, "    <Resource Language=\"{}\" />", e(l));
        }
        let exe = e(&self.executable);
        let display = e(&self.display_name);
        let mut extensions = String::new();
        let clsid = e(&self.toast_clsid);
        let _ = write!(
            extensions,
            r#"
        <desktop:Extension Category="windows.toastNotificationActivation">
          <desktop:ToastNotificationActivation ToastActivatorCLSID="{clsid}" />
        </desktop:Extension>
        <com:Extension Category="windows.comServer">
          <com:ComServer>
            <com:ExeServer Executable="{exe}" DisplayName="{display}" Arguments="----AppNotificationActivated:">
              <com:Class Id="{clsid}" />
            </com:ExeServer>"#
        );
        extensions.push_str(
            r#"
          </com:ComServer>
        </com:Extension>"#,
        );
        let mut capabilities = String::from(r#"    <rescap:Capability Name="runFullTrust" />"#);
        for c in &self.device_capabilities {
            let _ = write!(capabilities, "\n    <DeviceCapability Name=\"{}\" />", e(c));
        }
        let [store, small, square] = LOGOS.map(|(path, _)| path.replace('/', "\\"));
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<Package
  xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
  xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10"
  xmlns:desktop="http://schemas.microsoft.com/appx/manifest/desktop/windows10"
  xmlns:com="http://schemas.microsoft.com/appx/manifest/com/windows10"
  xmlns:rescap="http://schemas.microsoft.com/appx/manifest/foundation/windows10/restrictedcapabilities"
  IgnorableNamespaces="uap desktop com rescap">
  <Identity Name="{name}" Publisher="{publisher}" Version="{version}" ProcessorArchitecture="{arch}" />
  <Properties>
    <DisplayName>{display}</DisplayName>
    <PublisherDisplayName>{publisher_display}</PublisherDisplayName>
    <Logo>{store}</Logo>
  </Properties>
  <Dependencies>
    <TargetDeviceFamily Name="Windows.Desktop" MinVersion="{MIN_WINDOWS}" MaxVersionTested="{MAX_TESTED}" />
    <PackageDependency Name="{framework}" MinVersion="{framework_version}" Publisher="{framework_publisher}" />
  </Dependencies>
  <Resources>
{resources}  </Resources>
  <Applications>
    <Application Id="App" Executable="{exe}" EntryPoint="Windows.FullTrustApplication">
      <uap:VisualElements DisplayName="{display}" Description="{description}" BackgroundColor="transparent" Square150x150Logo="{square}" Square44x44Logo="{small}" />
      <Extensions>{extensions}
      </Extensions>
    </Application>
  </Applications>
  <Capabilities>
{capabilities}
  </Capabilities>
</Package>
"#,
            name = e(&self.name),
            publisher = e(&self.publisher),
            version = e(&self.version),
            arch = e(&self.arch),
            publisher_display = e(&self.publisher_display_name),
            description = e(&self.description),
            framework = prism_build::winappsdk::FRAMEWORK_PACKAGE,
            framework_version = prism_build::winappsdk::FRAMEWORK_MIN_VERSION,
            framework_publisher = prism_build::winappsdk::FRAMEWORK_PUBLISHER,
        )
    }
}

/// The package name of a bundle id: 3 to 50 letters, digits, `.` and `-`
/// (`_` written `-`).
pub fn package_name(bundle_id: &str) -> Result<String, String> {
    let name = bundle_id.replace('_', "-");
    let ok = (3..=50).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    if ok {
        Ok(name)
    } else {
        Err(format!(
            "Prism.toml [app] bundle_id: {bundle_id:?} cannot name an MSIX (3 to 50 letters, digits, `.` and `-`)"
        ))
    }
}

/// A stable COM class id for `seed` (the package's name and publisher):
/// a name-based UUID (version 8, from SHA-256), the same on every build so
/// clicks on earlier notifications still reach the app.
pub fn class_id(seed: &str) -> String {
    let h = sha256(seed.as_bytes());
    let mut b = [0u8; 16];
    b.copy_from_slice(&h[..16]);
    b[6] = (b[6] & 0x0F) | 0x80;
    b[8] = (b[8] & 0x3F) | 0x80;
    format!(
        "{:02X}{:02X}{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        b[0],
        b[1],
        b[2],
        b[3],
        b[4],
        b[5],
        b[6],
        b[7],
        b[8],
        b[9],
        b[10],
        b[11],
        b[12],
        b[13],
        b[14],
        b[15]
    )
}

/// A package's `Publisher`: a distinguished name in Windows' form
/// (`CN=Example Ltd, O=Example Ltd, C=FR`), at most 8192 characters.
pub fn check_publisher(dn: &str) -> Result<(), String> {
    const KEYS: [&str; 20] = [
        "CN",
        "L",
        "O",
        "OU",
        "E",
        "C",
        "S",
        "STREET",
        "T",
        "G",
        "I",
        "SN",
        "DC",
        "SERIALNUMBER",
        "Description",
        "PostalCode",
        "POBox",
        "Phone",
        "X21Address",
        "dnQualifier",
    ];
    let err = || {
        format!(
            "{dn:?} is not a publisher name: a certificate subject like \"CN=Example Ltd, O=Example Ltd, C=FR\""
        )
    };
    if dn.is_empty() || dn.len() > 8192 {
        return Err(err());
    }
    for rdn in split_dn(dn) {
        let (key, value) = rdn.split_once('=').ok_or_else(err)?;
        let key = key.trim();
        let known = KEYS.contains(&key)
            || key.strip_prefix("OID.").is_some_and(|o| {
                o.split('.')
                    .all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            });
        if !known || value.trim().is_empty() {
            return Err(err());
        }
    }
    Ok(())
}

/// The `key=value` parts of a distinguished name, split at the commas
/// outside quotes.
fn split_dn(dn: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut start, mut quoted) = (0, false);
    for (i, c) in dn.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ',' if !quoted => {
                parts.push(&dn[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&dn[start..]);
    parts.into_iter().map(str::trim).collect()
}

/// The publisher an unsigned package gets: `CN=<name>` (quoted when it
/// needs to be).
pub fn default_publisher(name: &str) -> String {
    format!("CN={}", dn_value(name))
}

/// An attribute value as Windows writes it in a subject: in quotes (inner
/// quotes doubled) when it has a special character or edge spaces.
pub fn dn_value(v: &str) -> String {
    let special = v.chars().any(|c| ",+=\"<>#;\n".contains(c)) || v.trim() != v;
    if special {
        format!("\"{}\"", v.replace('"', "\"\""))
    } else {
        v.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn u16_at(b: &[u8], i: usize) -> u16 {
        u16::from_le_bytes([b[i], b[i + 1]])
    }
    fn u32_at(b: &[u8], i: usize) -> u32 {
        u32::from_le_bytes(b[i..i + 4].try_into().unwrap())
    }
    fn u64_at(b: &[u8], i: usize) -> u64 {
        u64::from_le_bytes(b[i..i + 8].try_into().unwrap())
    }

    /// (name, method, data offset, stored size, size, crc) from the Zip64
    /// central directory.
    fn entries(pkg: &[u8]) -> Vec<(String, u16, usize, usize, usize, u32)> {
        let end = pkg.len() - 22;
        assert_eq!(u32_at(pkg, end), 0x0605_4b50);
        assert_eq!(u16_at(pkg, end + 8), 0xFFFF);
        let locator = end - 20;
        assert_eq!(u32_at(pkg, locator), 0x0706_4b50);
        let end64 = u64_at(pkg, locator + 8) as usize;
        assert_eq!(u32_at(pkg, end64), 0x0606_4b50);
        let count = u64_at(pkg, end64 + 32) as usize;
        let mut at = u64_at(pkg, end64 + 48) as usize;
        let mut out = Vec::new();
        for _ in 0..count {
            assert_eq!(u32_at(pkg, at), 0x0201_4b50);
            assert_eq!(u16_at(pkg, at + 8), 0x0008);
            let method = u16_at(pkg, at + 10);
            let crc = u32_at(pkg, at + 16);
            assert_eq!(u32_at(pkg, at + 20), u32::MAX);
            let name_len = u16_at(pkg, at + 28) as usize;
            let extra_len = u16_at(pkg, at + 30) as usize;
            assert_eq!(extra_len, 28);
            let name = String::from_utf8(pkg[at + 46..at + 46 + name_len].to_vec()).unwrap();
            let x = at + 46 + name_len;
            assert_eq!((u16_at(pkg, x), u16_at(pkg, x + 2)), (1, 24));
            let size = u64_at(pkg, x + 4) as usize;
            let stored = u64_at(pkg, x + 12) as usize;
            let local = u64_at(pkg, x + 20) as usize;
            assert_eq!(u32_at(pkg, local), 0x0403_4b50);
            let data = local + 30 + u16_at(pkg, local + 26) as usize;
            // The data descriptor (Zip64) after the data.
            assert_eq!(u32_at(pkg, data + stored), 0x0807_4b50);
            assert_eq!(u32_at(pkg, data + stored + 4), crc);
            assert_eq!(u64_at(pkg, data + stored + 8) as usize, stored);
            assert_eq!(u64_at(pkg, data + stored + 16) as usize, size);
            out.push((name, method, data, stored, size, crc));
            at = x + extra_len;
        }
        out
    }

    fn read(pkg: &[u8], e: &(String, u16, usize, usize, usize, u32)) -> Vec<u8> {
        let raw = &pkg[e.2..e.2 + e.3];
        if e.1 == 8 {
            let mut out = Vec::new();
            flate2::read::DeflateDecoder::new(raw)
                .read_to_end(&mut out)
                .unwrap();
            out
        } else {
            raw.to_vec()
        }
    }

    #[test]
    fn packages_like_makeappx() {
        let big: Vec<u8> = (0..200_000u32)
            .map(|i| ((i * 7 + (i >> 9)) % 251) as u8)
            .collect();
        let png = b"\x89PNG\r\n\x1a\n not really".to_vec();
        let mut p = Packer::new(Vec::new(), SystemTime::now());
        p.add("app.exe", &big).unwrap();
        p.add("Logos/StoreLogo.png", &png).unwrap();
        p.add("assets/My File.txt", b"hello").unwrap();
        p.add("assets/empty.json", b"").unwrap();
        let pkg = p.finish("<Package/>").unwrap();
        let e = entries(&pkg);
        let names: Vec<&str> = e.iter().map(|e| e.0.as_str()).collect();
        assert_eq!(
            names,
            [
                "app.exe",
                "Logos/StoreLogo.png",
                "assets/My%20File.txt",
                "assets/empty.json",
                "AppxManifest.xml",
                "AppxBlockMap.xml",
                "[Content_Types].xml"
            ]
        );
        assert_eq!(read(&pkg, &e[0]), big);
        assert_eq!(e[0].1, 8);
        assert_eq!((e[1].1, read(&pkg, &e[1])), (0, png.clone()));
        assert_eq!(read(&pkg, &e[2]), b"hello");
        assert_eq!((e[3].1, e[3].4), (0, 0));
        assert_eq!(read(&pkg, &e[4]), b"<Package/>");

        // Each block inflates alone, to the 64 KiB its hash names.
        let map = String::from_utf8(read(&pkg, &e[5])).unwrap();
        assert!(map.starts_with(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\r\n<BlockMap"
        ));
        let file = map.split("<File Name=\"app.exe\"").nth(1).unwrap();
        let file = file.split("</File>").next().unwrap();
        assert!(file.starts_with(" Size=\"200000\" LfhSize=\"37\">"));
        let raw = &pkg[e[0].2..e[0].2 + e[0].3];
        let mut at = 0;
        let blocks: Vec<&str> = file.split("<Block ").skip(1).collect();
        assert_eq!(blocks.len(), 4);
        for (i, b) in blocks.iter().enumerate() {
            let hash = b
                .split("Hash=\"")
                .nth(1)
                .unwrap()
                .split('"')
                .next()
                .unwrap();
            let size: usize = b
                .split("Size=\"")
                .nth(1)
                .unwrap()
                .split('"')
                .next()
                .unwrap()
                .parse()
                .unwrap();
            let mut out = Vec::new();
            let mut d = flate2::Decompress::new(false);
            out.reserve(BLOCK + 16);
            d.decompress_vec(&raw[at..at + size], &mut out, flate2::FlushDecompress::Sync)
                .unwrap();
            let expected = &big[i * BLOCK..((i + 1) * BLOCK).min(big.len())];
            assert_eq!(out, expected, "block {i}");
            let b64 = base64::engine::general_purpose::STANDARD.encode(sha256(expected));
            assert_eq!(hash, b64);
            at += size;
        }
        // The stream's end after the last block.
        assert_eq!(&raw[at..], [0x03, 0x00]);
        assert!(file.contains("<b4:FileHash Hash=\""));
        assert!(map.contains(
            "<File Name=\"assets\\My File.txt\" Size=\"5\" LfhSize=\"50\"><Block Hash=\""
        ));
        assert!(
            map.contains("<File Name=\"assets\\empty.json\" Size=\"0\" LfhSize=\"47\"></File>")
        );
        assert!(map.contains("<File Name=\"AppxManifest.xml\""));
        assert!(!map.contains("AppxBlockMap.xml"));
        // Stored files' blocks have no size.
        let logo = map.split("Logos\\StoreLogo.png").nth(1).unwrap();
        let logo = logo.split("</File>").next().unwrap();
        let logo_blocks = logo.split_once('>').unwrap().1;
        assert!(logo_blocks.starts_with("<Block Hash=\""), "{logo}");
        assert!(!logo_blocks.contains("Size="), "{logo}");

        let types = String::from_utf8(read(&pkg, &e[6])).unwrap();
        for part in [
            "<Default Extension=\"exe\" ContentType=\"application/x-msdownload\" />",
            "<Default Extension=\"png\" ContentType=\"image/png\" />",
            "<Default Extension=\"xml\" ContentType=\"application/vnd.ms-appx.manifest+xml\" />",
            "<Override PartName=\"/AppxBlockMap.xml\" ContentType=\"application/vnd.ms-appx.blockmap+xml\" />",
        ] {
            assert!(types.contains(part), "{part} in {types}");
        }
    }

    #[test]
    fn xml_payloads_keep_their_type() {
        let mut p = Packer::new(Vec::new(), SystemTime::now());
        p.add("assets/data.xml", b"<a/>").unwrap();
        p.add("LICENSE", b"MIT").unwrap();
        let pkg = p.finish("<Package/>").unwrap();
        let e = entries(&pkg);
        let types = String::from_utf8(read(&pkg, e.last().unwrap())).unwrap();
        assert!(types.contains("<Default Extension=\"xml\" ContentType=\"text/xml\" />"));
        assert!(types.contains(
            "<Override PartName=\"/AppxManifest.xml\" ContentType=\"application/vnd.ms-appx.manifest+xml\" />"
        ));
        assert!(types.contains(
            "<Override PartName=\"/LICENSE\" ContentType=\"application/octet-stream\" />"
        ));
    }

    #[test]
    fn refuses_paths_windows_refuses() {
        for bad in [
            "AppxManifest.xml",
            "appxmetadata/x",
            "a/../b",
            "/abs",
            "a\\b",
            "a/b.",
            "a:b",
            "",
        ] {
            assert!(check_path(bad).is_err(), "{bad}");
        }
        assert!(check_path("assets/AppxManifest.xml").is_ok());
        assert_eq!(part_name("a b/é[1].txt"), "a%20b/%C3%A9%5B1%5D.txt");
    }

    #[test]
    fn names_and_ids() {
        assert_eq!(
            package_name("com.example.my_app").unwrap(),
            "com.example.my-app"
        );
        assert!(package_name("a.b").is_ok());
        assert!(package_name("ab").is_err());
        assert!(package_name(&format!("com.{}", "x".repeat(50))).is_err());
        let id = class_id("com.example.app|CN=Example");
        assert_eq!(id, class_id("com.example.app|CN=Example"));
        assert_ne!(id, class_id("com.example.app|CN=Other"));
        assert_eq!(&id[14..15], "8");
    }

    #[test]
    fn publishers() {
        for ok in [
            "CN=Prism Test",
            "CN=Example Ltd, O=Example Ltd, L=Paris, S=Ile-de-France, C=FR",
            "CN=\"Tom, Jerry\", O=Acme",
            "CN=Contoso, OID.2.25.311729368913984317654407730594956997722=1",
        ] {
            assert!(check_publisher(ok).is_ok(), "{ok}");
        }
        for bad in ["", "Example", "XX=1", "CN=", "CN=a, junk"] {
            assert!(check_publisher(bad).is_err(), "{bad}");
        }
        assert_eq!(default_publisher("Acme"), "CN=Acme");
        assert_eq!(default_publisher("Tom, Jerry"), "CN=\"Tom, Jerry\"");
        assert_eq!(dn_value("a \"b\", c"), "\"a \"\"b\"\", c\"");
    }

    #[test]
    fn manifests() {
        let m = Manifest {
            name: "com.example.counter".into(),
            publisher: "CN=Example & Co".into(),
            version: "1.2.0.7".into(),
            arch: "x64".into(),
            display_name: "Counter".into(),
            publisher_display_name: "Example & Co".into(),
            description: "Counts things".into(),
            executable: "counter.exe".into(),
            languages: vec!["en".into(), "fr".into()],
            toast_clsid: class_id("x"),
            device_capabilities: vec!["location".into()],
        };
        let x = m.xml();
        for part in [
            r#"<Identity Name="com.example.counter" Publisher="CN=Example &amp; Co" Version="1.2.0.7" ProcessorArchitecture="x64" />"#,
            r#"<PackageDependency Name="Microsoft.WindowsAppRuntime.2" MinVersion="2.7.0.0" Publisher="CN=Microsoft Corporation, O=Microsoft Corporation, L=Redmond, S=Washington, C=US" />"#,
            r#"<Application Id="App" Executable="counter.exe" EntryPoint="Windows.FullTrustApplication">"#,
            r#"Square150x150Logo="Logos\Square150x150Logo.png" Square44x44Logo="Logos\Square44x44Logo.png""#,
            "<Logo>Logos\\StoreLogo.png</Logo>",
            r#"<Resource Language="en" />"#,
            r#"<Resource Language="fr" />"#,
            r#"Arguments="----AppNotificationActivated:""#,
            r#"<rescap:Capability Name="runFullTrust" />"#,
            r#"<DeviceCapability Name="location" />"#,
        ] {
            assert!(x.contains(part), "{part}\n{x}");
        }
        assert!(x.contains(&format!("ToastActivatorCLSID=\"{}\"", class_id("x"))));
        assert!(x.contains(&format!("<com:Class Id=\"{}\" />", class_id("x"))));
        // `rescap` before `DeviceCapability`, as the schema orders them.
        assert!(x.find("rescap:Capability").unwrap() < x.find("DeviceCapability").unwrap());
    }
}
