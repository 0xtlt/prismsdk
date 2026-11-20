//! A small zip writer (deflate, UTF-8 names), for `prism build windows`:
//! Explorer, `Expand-Archive`, `tar` and `unzip` open it.

use std::io::{self, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::Compression;
use flate2::write::DeflateEncoder;

pub struct ZipWriter<W: Write> {
    out: W,
    offset: u64,
    central: Vec<u8>,
    entries: u64,
}

fn too_big() -> io::Error {
    io::Error::other("the archive is over 4 GB or 65535 files (no Zip64)")
}

impl<W: Write> ZipWriter<W> {
    pub fn new(out: W) -> Self {
        ZipWriter {
            out,
            offset: 0,
            central: Vec::new(),
            entries: 0,
        }
    }

    /// Adds a file at `name` (`/`-separated), deflated unless that makes
    /// it bigger.
    pub fn add(&mut self, name: &str, data: &[u8], modified: SystemTime) -> io::Result<()> {
        let mut crc = flate2::Crc::new();
        crc.update(data);
        let crc = crc.sum();
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data)?;
        let deflated = encoder.finish()?;
        let (method, body): (u16, &[u8]) = if deflated.len() < data.len() {
            (8, &deflated)
        } else {
            (0, data)
        };
        let (time, date) = dos_time(modified);
        let small = |n: usize| u32::try_from(n).map_err(|_| too_big());
        let (size, stored) = (small(data.len())?, small(body.len())?);
        let offset = u32::try_from(self.offset).map_err(|_| too_big())?;
        let name_len = u16::try_from(name.len()).map_err(|_| too_big())?;

        // Version needed 2.0, flags: UTF-8 names.
        let common = |b: &mut Vec<u8>| {
            for v in [20u16, 0x0800, method, time, date] {
                b.extend(v.to_le_bytes());
            }
            for v in [crc, stored, size] {
                b.extend(v.to_le_bytes());
            }
            b.extend(name_len.to_le_bytes());
            b.extend(0u16.to_le_bytes()); // extra field
        };
        let mut local = 0x0403_4b50u32.to_le_bytes().to_vec();
        common(&mut local);
        local.extend(name.as_bytes());
        self.out.write_all(&local)?;
        self.out.write_all(body)?;
        self.offset += (local.len() + body.len()) as u64;

        let c = &mut self.central;
        c.extend(0x0201_4b50u32.to_le_bytes());
        c.extend(20u16.to_le_bytes()); // made by: MS-DOS, 2.0
        common(c);
        c.extend(0u16.to_le_bytes()); // comment
        c.extend(0u16.to_le_bytes()); // disk
        c.extend(0u16.to_le_bytes()); // internal attributes
        c.extend(0x20u32.to_le_bytes()); // FILE_ATTRIBUTE_ARCHIVE
        c.extend(offset.to_le_bytes());
        c.extend(name.as_bytes());
        self.entries += 1;
        Ok(())
    }

    /// Writes the central directory.
    pub fn finish(mut self) -> io::Result<W> {
        let entries = u16::try_from(self.entries).map_err(|_| too_big())?;
        let size = u32::try_from(self.central.len()).map_err(|_| too_big())?;
        let offset = u32::try_from(self.offset).map_err(|_| too_big())?;
        self.out.write_all(&self.central)?;
        let mut end = 0x0605_4b50u32.to_le_bytes().to_vec();
        for v in [0u16, 0, entries, entries] {
            end.extend(v.to_le_bytes());
        }
        end.extend(size.to_le_bytes());
        end.extend(offset.to_le_bytes());
        end.extend(0u16.to_le_bytes()); // comment
        self.out.write_all(&end)?;
        Ok(self.out)
    }
}

/// MS-DOS time and date (UTC, two-second steps, from 1980).
fn dos_time(t: SystemTime) -> (u16, u16) {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .max(315_532_800); // 1980-01-01
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Days to civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let year = (year - 1980).clamp(0, 127) as u16;
    let time = (((rem / 3600) << 11) | ((rem % 3600 / 60) << 5) | ((rem % 60) / 2)) as u16;
    let date = (year << 9) | ((month as u16) << 5) | day as u16;
    (time, date)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::time::Duration;

    #[test]
    fn dos_dates() {
        // 2026-11-20 16:18:04 UTC.
        let t = UNIX_EPOCH + Duration::from_secs(1_795_191_484);
        let (time, date) = dos_time(t);
        assert_eq!((date >> 9) + 1980, 2026);
        assert_eq!((date >> 5) & 0xF, 11);
        assert_eq!(date & 0x1F, 20);
        assert_eq!(
            (time >> 11, (time >> 5) & 0x3F, (time & 0x1F) * 2),
            (16, 18, 4)
        );
        assert_eq!(dos_time(UNIX_EPOCH), (0, (1 << 5) | 1));
    }

    /// Reads the archive back with its central directory.
    #[test]
    fn archives_read_back() {
        let big = "Prism ".repeat(1000);
        let mut zip = ZipWriter::new(Vec::new());
        let now = SystemTime::now();
        zip.add("App/app.exe", big.as_bytes(), now).unwrap();
        zip.add("App/é.txt", b"x", now).unwrap();
        let bytes = zip.finish().unwrap();

        let end = bytes.len() - 22;
        assert_eq!(&bytes[end..end + 4], &0x0605_4b50u32.to_le_bytes());
        let u16_at = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
        let u32_at = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
        assert_eq!(u16_at(end + 10), 2);
        let mut at = u32_at(end + 16) as usize;
        let mut files = Vec::new();
        for _ in 0..2 {
            assert_eq!(u32_at(at), 0x0201_4b50);
            let method = u16_at(at + 10);
            let stored = u32_at(at + 20) as usize;
            let size = u32_at(at + 24) as usize;
            let name_len = u16_at(at + 28) as usize;
            let local = u32_at(at + 42) as usize;
            let name = String::from_utf8(bytes[at + 46..at + 46 + name_len].to_vec()).unwrap();
            let data_at = local + 30 + u16_at(local + 26) as usize;
            let raw = &bytes[data_at..data_at + stored];
            let data = if method == 8 {
                let mut out = Vec::new();
                flate2::read::DeflateDecoder::new(raw)
                    .read_to_end(&mut out)
                    .unwrap();
                out
            } else {
                raw.to_vec()
            };
            assert_eq!(data.len(), size);
            files.push((name, method, data));
            at += 46 + name_len;
        }
        assert_eq!(files[0].0, "App/app.exe");
        assert_eq!(files[0].1, 8);
        assert_eq!(files[0].2, big.as_bytes());
        assert_eq!((files[1].0.as_str(), files[1].1), ("App/é.txt", 0));
    }
}
