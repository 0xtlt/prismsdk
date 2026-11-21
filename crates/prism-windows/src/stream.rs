//! A remote video read with the app's request headers (`Video::header`,
//! #207), as AVFoundation's `AVURLAssetHTTPHeaderFieldsKey` and Android's
//! `setVideoURI(uri, headers)` send them: Media Foundation's own HTTP
//! source takes no headers, so the player reads a WinRT
//! `IRandomAccessStream` of ours (`MediaSource::CreateFromStream`) whose
//! reads are HTTP range requests with the headers (prism-http: rustls,
//! redirects that drop secret headers on another origin). The file is
//! fetched in blocks of 1 MB, the last few kept; the player seeks
//! anywhere. HLS and DASH (`.m3u8`, `.mpd`) go through
//! `AdaptiveMediaSource` with an `HttpClient` carrying the headers
//! (`media.rs`).
//!
//! The headers hold tokens: they are never logged.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use windows::Foundation::IClosable_Impl;
use windows::Storage::Streams::{
    IBuffer, IInputStream, IInputStream_Impl, IOutputStream, IOutputStream_Impl,
    IRandomAccessStream, IRandomAccessStream_Impl, InputStreamOptions,
};
use windows::Win32::Foundation::E_NOTIMPL;
use windows::Win32::System::WinRT::IBufferByteAccess;
use windows_core::{AgileReference, Error, HRESULT, Interface, Result, implement};
use windows_future::{IAsyncOperation, IAsyncOperationWithProgress};

/// Bytes fetched per request.
const BLOCK: u64 = 1 << 20;
/// Blocks kept.
const KEPT: usize = 6;

/// The remote file: where it is, its headers, its size and the blocks
/// fetched.
pub struct Remote {
    url: String,
    headers: Vec<(String, String)>,
    size: u64,
    blocks: Mutex<VecDeque<(u64, Arc<Vec<u8>>)>>,
}

/// `bytes=a-b` of a file of `size` bytes, block `i`.
fn block_range(i: u64, size: u64) -> (u64, u64) {
    let start = i * BLOCK;
    (start, (start + BLOCK).min(size).saturating_sub(1))
}

/// The total size in a `Content-Range: bytes a-b/total` header.
fn total_of(content_range: &str) -> Option<u64> {
    content_range.rsplit_once('/')?.1.trim().parse().ok()
}

impl Remote {
    fn get(&self, range: (u64, u64)) -> std::result::Result<prism_http::Response, String> {
        let mut req = prism_http::Request::get(&self.url)
            .header("Range", format!("bytes={}-{}", range.0, range.1))
            // Ranges of the file itself, not of a compressed body.
            .header("Accept-Encoding", "identity")
            .timeout(std::time::Duration::from_secs(30));
        for (k, v) in &self.headers {
            req = req.secret_header(k.clone(), v.clone());
        }
        let res = req.send_blocking().map_err(|e| e.0)?;
        if res.status != 206 {
            return Err(format!("HTTP {} for a range", res.status));
        }
        Ok(res)
    }

    /// Opens `url` with `headers` (blocking): its size and content type.
    /// `Err` when the server does not answer ranges.
    pub fn open(
        url: &str,
        headers: &[(&str, &str)],
    ) -> std::result::Result<(Arc<Remote>, String), String> {
        let mut remote = Remote {
            url: url.to_string(),
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            size: 0,
            blocks: Mutex::new(VecDeque::new()),
        };
        // The first block tells the size (and is kept).
        let res = remote.get((0, BLOCK - 1))?;
        let size = res
            .header("content-range")
            .and_then(total_of)
            .ok_or("no Content-Range in the answer")?;
        let kind = res
            .header("content-type")
            .map(|t| t.split(';').next().unwrap_or(t).trim().to_string())
            .filter(|t| !t.is_empty() && t != "application/octet-stream")
            .unwrap_or_else(|| prism_core::SaveFile::path(std::path::Path::new(url)).mime_type());
        remote.size = size;
        remote
            .blocks
            .get_mut()
            .map_err(|e| e.to_string())?
            .push_back((0, Arc::new(res.body)));
        Ok((Arc::new(remote), kind))
    }

    fn block(&self, i: u64) -> std::result::Result<Arc<Vec<u8>>, String> {
        if let Ok(b) = self.blocks.lock()
            && let Some((_, data)) = b.iter().find(|(n, _)| *n == i)
        {
            return Ok(data.clone());
        }
        let res = self.get(block_range(i, self.size))?;
        let data = Arc::new(res.body);
        if let Ok(mut b) = self.blocks.lock() {
            b.push_back((i, data.clone()));
            while b.len() > KEPT {
                b.pop_front();
            }
        }
        Ok(data)
    }

    /// Reads at most `out.len()` bytes at `pos`: how many.
    fn read(&self, pos: u64, out: &mut [u8]) -> std::result::Result<usize, String> {
        let mut done = 0;
        while done < out.len() && pos + (done as u64) < self.size {
            let at = pos + done as u64;
            let block = self.block(at / BLOCK)?;
            let offset = (at % BLOCK) as usize;
            if offset >= block.len() {
                break;
            }
            let n = (block.len() - offset).min(out.len() - done);
            out[done..done + n].copy_from_slice(&block[offset..offset + n]);
            done += n;
        }
        Ok(done)
    }
}

/// The player's view of a remote file: where it reads next.
#[implement(
    IRandomAccessStream,
    IInputStream,
    IOutputStream,
    windows::Foundation::IClosable
)]
pub struct Stream {
    remote: Arc<Remote>,
    position: Arc<AtomicU64>,
}

impl Stream {
    pub fn open(remote: Arc<Remote>) -> IRandomAccessStream {
        Stream {
            remote,
            position: Arc::new(AtomicU64::new(0)),
        }
        .into()
    }

    fn at(&self, pos: u64) -> Stream {
        Stream {
            remote: self.remote.clone(),
            position: Arc::new(AtomicU64::new(pos)),
        }
    }
}

fn unsupported<T>() -> Result<T> {
    Err(Error::from(E_NOTIMPL))
}

impl IRandomAccessStream_Impl for Stream_Impl {
    fn Size(&self) -> Result<u64> {
        Ok(self.remote.size)
    }
    fn SetSize(&self, _: u64) -> Result<()> {
        unsupported()
    }
    fn GetInputStreamAt(&self, position: u64) -> Result<IInputStream> {
        Ok(self.at(position).into())
    }
    fn GetOutputStreamAt(&self, _: u64) -> Result<IOutputStream> {
        unsupported()
    }
    fn Position(&self) -> Result<u64> {
        Ok(self.position.load(Ordering::SeqCst))
    }
    fn Seek(&self, position: u64) -> Result<()> {
        self.position.store(position, Ordering::SeqCst);
        Ok(())
    }
    fn CloneStream(&self) -> Result<IRandomAccessStream> {
        Ok(self.at(0).into())
    }
    fn CanRead(&self) -> Result<bool> {
        Ok(true)
    }
    fn CanWrite(&self) -> Result<bool> {
        Ok(false)
    }
}

impl IInputStream_Impl for Stream_Impl {
    fn ReadAsync(
        &self,
        buffer: windows_core::Ref<'_, IBuffer>,
        count: u32,
        _: InputStreamOptions,
    ) -> Result<IAsyncOperationWithProgress<IBuffer, u32>> {
        let buffer: IBuffer = buffer.ok()?.clone();
        let agile = AgileReference::new(&buffer)?;
        let (remote, position) = (self.remote.clone(), self.position.clone());
        Ok(IAsyncOperationWithProgress::spawn(move || {
            let buffer = agile.resolve()?;
            let capacity = buffer.Capacity()?.min(count) as usize;
            let pos = position.load(Ordering::SeqCst);
            let access: IBufferByteAccess = buffer.cast()?;
            let n = unsafe {
                let out = std::slice::from_raw_parts_mut(access.Buffer()?, capacity);
                remote.read(pos, out).map_err(|e| {
                    crate::win::log!("video: {e}");
                    Error::from(HRESULT(0x8007_0026_u32 as i32))
                })?
            };
            buffer.SetLength(n as u32)?;
            position.store(pos + n as u64, Ordering::SeqCst);
            Ok(buffer)
        }))
    }
}

impl IOutputStream_Impl for Stream_Impl {
    fn WriteAsync(
        &self,
        _: windows_core::Ref<'_, IBuffer>,
    ) -> Result<IAsyncOperationWithProgress<u32, u32>> {
        unsupported()
    }
    fn FlushAsync(&self) -> Result<IAsyncOperation<bool>> {
        unsupported()
    }
}

impl IClosable_Impl for Stream_Impl {
    fn Close(&self) -> Result<()> {
        Ok(())
    }
}

/// Whether `url` is an adaptive stream (HLS, DASH).
pub fn adaptive(url: &str) -> bool {
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase();
    path.ends_with(".m3u8") || path.ends_with(".mpd")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_cover_the_file_in_blocks() {
        assert_eq!(block_range(0, 3 * BLOCK + 5), (0, BLOCK - 1));
        assert_eq!(block_range(3, 3 * BLOCK + 5), (3 * BLOCK, 3 * BLOCK + 4));
        assert_eq!(total_of("bytes 0-1023/4096"), Some(4096));
        assert_eq!(total_of("bytes */4096"), Some(4096));
        assert_eq!(total_of("bytes 0-1023/*"), None);
        assert!(adaptive("https://a.b/live/index.m3u8?token=1"));
        assert!(adaptive("https://a.b/x.MPD"));
        assert!(!adaptive("https://a.b/x.mp4"));
    }
}
