//! Taking a photo (`take_photo`): the Windows Camera app's capture UI,
//! launched for a result (`microsoft.windows.camera.picker:`), the protocol
//! the Windows App SDK's `CameraCaptureUI` drives. The app hands the
//! Camera app an empty file of its own (`<data>\picked`) through a sharing
//! token (`SharedStorageAccessManager`); the Camera app writes the photo
//! into it and answers its token.
//!
//! Why not a class: `Windows.Media.Capture.CameraCaptureUI` is for UWP
//! apps, and the Windows App SDK's (1.7 and later) makes its file in
//! `ApplicationData::Current()`, which only a packaged app has. Capturing
//! frames ourselves (`MediaCapture`) would need a whole camera UI. The
//! Camera app is the one users know, asks for camera access itself, and
//! ships with Windows (`Microsoft.WindowsCamera`); without it, or without
//! a camera, `capabilities().camera` is false.

use std::path::{Path, PathBuf};

use prism_core::{DeviceError, DeviceValue, PermissionState, PickedFile};
use windows::ApplicationModel::DataTransfer::SharedStorageAccessManager;
use windows::Devices::Enumeration::{DeviceClass, DeviceInformation};
use windows::Foundation::Collections::ValueSet;
use windows::Foundation::{IPropertyValue, PropertyValue, Uri};
use windows::Storage::StorageFile;
use windows::System::{
    LaunchQuerySupportStatus, LaunchQuerySupportType, LaunchUriStatus, Launcher, LauncherOptions,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Shell::IInitializeWithWindow;
use windows::core::{HSTRING, IInspectable, Interface};

use crate::win::log;

/// The Camera app's package family.
pub const CAMERA_APP: &str = "Microsoft.WindowsCamera_8wekyb3d8bbwe";
/// Its capture UI, launched for results.
pub const PICKER: &str = "microsoft.windows.camera.picker:";

/// Cameras Windows can use (enabled video capture devices).
pub fn cameras() -> windows::core::Result<u32> {
    let all = DeviceInformation::FindAllAsyncDeviceClass(DeviceClass::VideoCapture)?.join()?;
    let mut n = 0;
    for d in all {
        if d.IsEnabled().unwrap_or(false) {
            n += 1;
        }
    }
    Ok(n)
}

/// Whether the Camera app's capture UI can be launched for a result.
pub fn picker() -> windows::core::Result<LaunchQuerySupportStatus> {
    Launcher::QueryUriSupportWithPackageFamilyNameAsync(
        &Uri::CreateUri(&HSTRING::from(PICKER))?,
        LaunchQuerySupportType::UriForResults,
        &HSTRING::from(CAMERA_APP),
    )?
    .join()
}

fn support_name(s: LaunchQuerySupportStatus) -> &'static str {
    match s {
        LaunchQuerySupportStatus::Available => "Available",
        LaunchQuerySupportStatus::AppNotInstalled => "AppNotInstalled",
        LaunchQuerySupportStatus::AppUnavailable => "AppUnavailable",
        LaunchQuerySupportStatus::NotSupported => "NotSupported",
        LaunchQuerySupportStatus::Unknown => "Unknown",
        _ => "?",
    }
}

/// What `PRISM_DEVICE_REPORT` says of the camera, without opening the
/// Camera app: the cameras, its capture UI, a sharing token for a photo
/// file in `dir` (made and removed), camera access, and why the Windows
/// App SDK's `CameraCaptureUI` cannot run here (`ApplicationData`).
pub fn report(dir: &Path) -> serde_json::Value {
    let token = match photo_file(dir) {
        Ok((path, token)) => {
            let removed = SharedStorageAccessManager::RemoveFile(&token).is_ok();
            let _ = std::fs::remove_file(&path);
            serde_json::json!({ "ok": true, "removed": removed })
        }
        Err(e) => serde_json::json!({ "error": format!("{e:?}") }),
    };
    let app_data = windows::Storage::ApplicationData::Current().map_or_else(
        |e| format!("error 0x{:08X}: {}", e.code().0 as u32, e.message()),
        |_| "ok".into(),
    );
    let webcam = windows::Security::Authorization::AppCapabilityAccess::AppCapability::Create(
        &HSTRING::from("webcam"),
    )
    .and_then(|c| c.CheckAccess());
    serde_json::json!({
        "cameras": cameras().map_or_else(|e| serde_json::json!(e.message()), |n| n.into()),
        "capture_ui": picker().map_or_else(|e| e.message(), |s| support_name(s).into()),
        "photo_file_token": token,
        "permission": format!("{:?}", permission(false)),
        "app_webcam_access": webcam.map_or_else(|e| e.message(), |a| crate::location::access_name(a).into()),
        "application_data_current": app_data,
    })
}

/// A camera and the Camera app's capture UI. Blocking: called off the UI
/// thread at startup.
pub fn present() -> bool {
    let cameras = cameras().unwrap_or_else(|e| {
        log!("camera: {}", e.message());
        0
    });
    if cameras == 0 {
        return false;
    }
    match picker() {
        Ok(LaunchQuerySupportStatus::Available) => true,
        Ok(status) => {
            log!(
                "camera: the Camera app's capture UI is {}",
                support_name(status)
            );
            false
        }
        Err(e) => {
            log!("camera: {}", e.message());
            false
        }
    }
}

/// Camera access: the device's and the user's switches and the Camera
/// app's own (Settings › Privacy & security › Camera). The Camera app
/// asks for itself when it opens, so a request only says whether it is
/// refused.
pub fn permission(request: bool) -> PermissionState {
    match crate::location::consent("webcam", CAMERA_APP) {
        PermissionState::NotDetermined if request => PermissionState::Granted,
        state => state,
    }
}

/// An empty file for the photo, and its sharing token.
fn photo_file(dir: &Path) -> Result<(PathBuf, HSTRING), DeviceError> {
    std::fs::create_dir_all(dir).map_err(|e| DeviceError::Failed(e.to_string()))?;
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let path = dir.join(format!("{unique}.jpg"));
    std::fs::File::create(&path).map_err(|e| DeviceError::Failed(e.to_string()))?;
    let token = StorageFile::GetFileFromPathAsync(&HSTRING::from(path.as_os_str()))
        .and_then(|op| op.join())
        .and_then(|file| SharedStorageAccessManager::AddFile(&file));
    match token {
        Ok(token) => Ok((path, token)),
        Err(e) => {
            let _ = std::fs::remove_file(&path);
            Err(DeviceError::Failed(format!("camera: {}", e.message())))
        }
    }
}

fn values(token: &HSTRING) -> windows::core::Result<ValueSet> {
    let v = ValueSet::new()?;
    let put = |k: &str, value: IInspectable| v.Insert(&HSTRING::from(k), &value).map(|_| ());
    put(
        "MediaType",
        PropertyValue::CreateString(&HSTRING::from("photo"))?,
    )?;
    put("AllowCropping", PropertyValue::CreateBoolean(false)?)?;
    // JPEG, the highest resolution, no crop.
    put("PhotoFormat", PropertyValue::CreateInt32(0)?)?;
    put("MaxResolution", PropertyValue::CreateInt32(0)?)?;
    for k in [
        "PhotoCropWidth",
        "PhotoCropHeight",
        "PhotoCropARWidth",
        "PhotoCropARHeight",
    ] {
        put(k, PropertyValue::CreateInt32(0)?)?;
    }
    put("PhotoFileToken", PropertyValue::CreateString(token)?)?;
    Ok(v)
}

fn launch(
    owner: HWND,
    token: &HSTRING,
) -> windows::core::Result<windows_future::IAsyncOperation<windows::System::LaunchUriResult>> {
    let options = LauncherOptions::new()?;
    unsafe { options.cast::<IInitializeWithWindow>()?.Initialize(owner)? };
    options.SetTreatAsUntrusted(false)?;
    options.SetDisplayApplicationPicker(false)?;
    options.SetTargetApplicationPackageFamilyName(&HSTRING::from(CAMERA_APP))?;
    Launcher::LaunchUriForResultsWithDataAsync(
        &Uri::CreateUri(&HSTRING::from(PICKER))?,
        &options,
        &values(token)?,
    )
}

/// The photo the Camera app answered with (`SelectedTokens`); `None` when
/// the user closed it.
fn answer(r: &windows::System::LaunchUriResult) -> Result<Option<HSTRING>, DeviceError> {
    match r.Status() {
        Ok(LaunchUriStatus::Success) => {}
        Ok(LaunchUriStatus::AppUnavailable | LaunchUriStatus::ProtocolUnavailable) => {
            return Err(DeviceError::Unavailable);
        }
        Ok(status) => {
            return Err(DeviceError::Failed(format!(
                "camera: launch status {}",
                status.0
            )));
        }
        Err(e) => return Err(DeviceError::Failed(format!("camera: {}", e.message()))),
    }
    let Ok(result) = r.Result() else {
        return Ok(None);
    };
    Ok(result
        .Lookup(&HSTRING::from("SelectedTokens"))
        .and_then(|v| v.cast::<IPropertyValue>()?.GetString())
        .ok()
        .filter(|t| !t.is_empty()))
}

/// The photo's file once the Camera app wrote it.
fn photo(ours: &Path, selected: &HSTRING) -> Result<PickedFile, DeviceError> {
    // The token names the file it was given: ours.
    let path = SharedStorageAccessManager::RedeemTokenForFileAsync(selected)
        .and_then(|op| op.join())
        .and_then(|f| f.Path())
        .map(|p| PathBuf::from(p.to_os_string()))
        .unwrap_or_else(|_| ours.to_path_buf());
    let _ = SharedStorageAccessManager::RemoveFile(selected);
    let size = std::fs::metadata(&path).map_or(0, |m| m.len());
    if size == 0 {
        return Err(DeviceError::Failed("camera: no photo was saved".into()));
    }
    Ok(PickedFile {
        name: path
            .file_name()
            .map_or_else(|| "photo.jpg".into(), |n| n.to_string_lossy().into_owned()),
        path: path.to_string_lossy().into_owned(),
        mime: "image/jpeg".into(),
        size,
    })
}

/// Opens the Camera app's capture UI for one photo, owned by `owner`, and
/// gives it to `done` on the UI thread (`Cancelled` when it is closed).
pub fn take_photo(
    owner: HWND,
    dir: PathBuf,
    done: impl FnOnce(Result<DeviceValue, DeviceError>) + Send + 'static,
) {
    let owner = crate::win::hwnd_value(owner);
    std::thread::spawn(move || {
        let (path, token) = match photo_file(&dir) {
            Ok(f) => f,
            Err(e) => return crate::shell::on_ui(move || done(Err(e))),
        };
        // The capture UI belongs to the window: launched from the UI thread.
        crate::shell::on_ui(move || {
            let finish =
                move |r: Result<PickedFile, DeviceError>, token: HSTRING, path: PathBuf| {
                    let _ = SharedStorageAccessManager::RemoveFile(&token);
                    if r.is_err() {
                        let _ = std::fs::remove_file(&path);
                    }
                    crate::shell::on_ui(move || done(r.map(|f| DeviceValue::Files(vec![f]))));
                };
            let op = match launch(crate::win::hwnd(owner), &token) {
                Ok(op) => op,
                Err(e) => {
                    return finish(
                        Err(DeviceError::Failed(format!("camera: {}", e.message()))),
                        token,
                        path,
                    );
                }
            };
            let t = token.clone();
            let p = path.clone();
            let r = op.when(move |r| {
                let answer = match r {
                    Ok(r) => answer(&r),
                    Err(e) => Err(DeviceError::Failed(format!("camera: {}", e.message()))),
                };
                // Redeeming waits: off Windows' callback thread.
                std::thread::spawn(move || {
                    let r = match answer {
                        Ok(Some(selected)) => photo(&p, &selected),
                        Ok(None) => Err(DeviceError::Cancelled),
                        Err(e) => Err(e),
                    };
                    finish(r, t, p);
                });
            });
            if let Err(e) = r {
                log!("camera: {e}");
            }
        });
    });
}
