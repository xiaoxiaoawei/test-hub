use super::{
    parse_manifest, sha256, CheckResult, Release, VerifiedDownload, Version, FEED_URL,
    MAX_MANIFEST_BYTES,
};
use std::{
    ffi::c_void,
    fs::{self, File, OpenOptions},
    io::Write,
    mem::size_of,
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle},
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{
    Foundation::GetLastError,
    Networking::WinHttp::*,
    Storage::FileSystem::{
        GetFileInformationByHandle, GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
        BY_HANDLE_FILE_INFORMATION, VS_FIXEDFILEINFO,
    },
};

const MAX_URL_BYTES: usize = 8192;
const MAX_REDIRECTS: usize = 5;
static DOWNLOAD_NONCE: AtomicU64 = AtomicU64::new(0);

struct HttpHandle(*mut c_void);
impl HttpHandle {
    fn checked(handle: *mut c_void) -> Result<Self, String> {
        if handle.is_null() {
            Err(network_error())
        } else {
            Ok(Self(handle))
        }
    }
}
impl Drop for HttpHandle {
    fn drop(&mut self) {
        unsafe { WinHttpCloseHandle(self.0) };
    }
}

#[derive(Clone, Debug)]
struct Endpoint {
    host: String,
    path: String,
    port: u16,
    secure: bool,
}

impl Endpoint {
    fn trusted(url: &str) -> Result<Self, String> {
        let invalid = || "更新服务器跳转到了不受支持的地址，未下载文件。".to_owned();
        if url.len() > MAX_URL_BYTES
            || !url.is_ascii()
            || url
                .bytes()
                .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
            || url.contains(['\\', '#'])
        {
            return Err(invalid());
        }
        let remainder = url.strip_prefix("https://").ok_or_else(invalid)?;
        let (host, path) = remainder.split_once('/').ok_or_else(invalid)?;
        let path = format!("/{path}");
        let permitted = match host {
            "github.com" => trusted_github_path(&path),
            "release-assets.githubusercontent.com" | "objects.githubusercontent.com" => {
                path.starts_with("/github-production-release-asset/")
                    || path.starts_with("/github-production-release-asset-2e65be/")
            }
            "github-cloud.s3.amazonaws.com" => path.starts_with("/releases/"),
            _ => false,
        };
        if !permitted || host.contains(['@', ':']) {
            return Err(invalid());
        }
        Ok(Self {
            host: host.into(),
            path,
            port: 443,
            secure: true,
        })
    }
}

fn trusted_github_path(path: &str) -> bool {
    if path == "/xiaoxiaoawei/test-hub/releases/latest/download/update.json" {
        return true;
    }
    let Some(remainder) = path.strip_prefix("/xiaoxiaoawei/test-hub/releases/download/v") else {
        return false;
    };
    let Some((version, name)) = remainder.split_once('/') else {
        return false;
    };
    let Ok(version) = Version::parse(version) else {
        return false;
    };
    name == "update.json"
        || name == "lingoleaf.exe"
        || name == format!("lingoleaf-{version}-windows-x64.exe")
}

enum Response {
    Found,
    NotFound,
}

pub fn check_for_updates() -> Result<CheckResult, String> {
    let mut bytes = Vec::new();
    let response = fetch(
        FEED_URL,
        MAX_MANIFEST_BYTES as u64,
        None,
        Duration::from_secs(20),
        |chunk| {
            bytes.extend_from_slice(chunk);
            Ok(())
        },
    )?;
    match response {
        Response::NotFound => Ok(CheckResult::NotPublished),
        Response::Found => parse_manifest(&bytes, env!("CARGO_PKG_VERSION")),
    }
}

pub fn download_update(
    release: &Release,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<VerifiedDownload, String> {
    release.validate()?;
    // A caller cannot turn a deserialized old manifest into a downgrade.
    if Version::parse(&release.version)? <= Version::parse(env!("CARGO_PKG_VERSION"))? {
        return Err("该版本不比当前版本新，未执行下载。".into());
    }
    let directory = staging_directory()?;
    let mut partial = PartialDownload {
        path: directory.join("download.part"),
        directory: directory.clone(),
        keep: false,
    };
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&partial.path)
        .map_err(|_| "无法创建更新下载文件，请检查用户目录权限。")?;
    let mut digest = sha256::Sha256::new();
    let mut received = 0_u64;
    let mut notified = Instant::now();
    on_progress(0, release.size);
    let response = fetch(
        &release.download_url,
        release.size,
        Some(release.size),
        Duration::from_secs(120),
        |chunk| {
            file.write_all(chunk)
                .map_err(|_| "无法写入更新文件，请检查磁盘空间。".to_owned())?;
            digest.update(chunk);
            received += chunk.len() as u64;
            if notified.elapsed() >= Duration::from_millis(100) {
                on_progress(received, release.size);
                notified = Instant::now();
            }
            Ok(())
        },
    )?;
    if matches!(response, Response::NotFound) {
        return Err("更新文件尚未发布，请稍后重试。".into());
    }
    if received != release.size || sha256::hex(&digest.finish()) != release.sha256 {
        return Err("更新文件大小或校验和不符，已取消下载。".into());
    }
    file.sync_all()
        .map_err(|_| "无法保存更新文件，请检查磁盘空间。")?;
    drop(file);
    super::verify_download(
        &partial.path,
        &release.version,
        release.size,
        &release.sha256,
    )?;
    let path = directory.join(format!("lingoleaf-{}-windows-x64.exe", release.version));
    fs::rename(&partial.path, &path).map_err(|_| "无法完成更新文件保存，请重新下载。")?;
    partial.keep = true;
    on_progress(received, release.size);
    Ok(VerifiedDownload {
        path,
        version: release.version.clone(),
        sha256: release.sha256.clone(),
        size: release.size,
    })
}

fn staging_directory() -> Result<PathBuf, String> {
    let base = std::env::var_os("LOCALAPPDATA")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or("找不到当前用户的更新目录。")?;
    if !base.is_absolute() {
        return Err("用户更新目录无效。".into());
    }
    let base = base.join("LingoLeaf").join("updates");
    fs::create_dir_all(&base).map_err(|_| "无法创建更新目录。")?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "系统时间无效，无法创建更新目录。")?
        .as_nanos();
    for _ in 0..10 {
        let nonce = DOWNLOAD_NONCE.fetch_add(1, Ordering::Relaxed);
        let directory = base.join(format!(
            "download-{}-{timestamp}-{nonce}",
            std::process::id()
        ));
        match fs::create_dir(&directory) {
            Ok(()) => return Ok(directory),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err("无法创建更新下载目录。".into()),
        }
    }
    Err("无法创建独立的更新下载目录。".into())
}
struct PartialDownload {
    path: PathBuf,
    directory: PathBuf,
    keep: bool,
}
impl Drop for PartialDownload {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_file(&self.path);
            let _ = fs::remove_dir(&self.directory);
        }
    }
}

fn fetch(
    initial_url: &str,
    limit: u64,
    expected_size: Option<u64>,
    timeout: Duration,
    mut sink: impl FnMut(&[u8]) -> Result<(), String>,
) -> Result<Response, String> {
    let mut endpoint = Endpoint::trusted(initial_url)?;
    let session = HttpHandle::checked(unsafe {
        WinHttpOpen(
            wide(concat!("LingoLeaf-Update/", env!("CARGO_PKG_VERSION"))).as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            null(),
            null(),
            0,
        )
    })?;
    let deadline = Instant::now() + timeout;
    for redirects in 0..=MAX_REDIRECTS {
        let (connection, request, status) = request(&session, &endpoint, deadline)?;
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            if redirects == MAX_REDIRECTS {
                return Err("更新服务器跳转次数过多，请稍后重试。".into());
            }
            let location = header(&request, WINHTTP_QUERY_LOCATION)?
                .ok_or("更新服务器未提供有效的下载地址。")?;
            let url = if location.starts_with('/') && !location.starts_with("//") {
                format!("https://{}{}", endpoint.host, location)
            } else {
                location
            };
            endpoint = Endpoint::trusted(&url)?;
            drop(request);
            drop(connection);
            continue;
        }
        if status == 404 {
            return Ok(Response::NotFound);
        }
        if status != 200 {
            return Err(format!("更新服务器暂时不可用（HTTP {status}）。"));
        }
        read_body(&request, deadline, limit, expected_size, &mut sink)?;
        return Ok(Response::Found);
    }
    unreachable!("bounded redirect loop returns on its last iteration")
}

fn request(
    session: &HttpHandle,
    endpoint: &Endpoint,
    deadline: Instant,
) -> Result<(HttpHandle, HttpHandle, u32), String> {
    timeouts(session, deadline)?;
    let connection = HttpHandle::checked(unsafe {
        WinHttpConnect(session.0, wide(&endpoint.host).as_ptr(), endpoint.port, 0)
    })?;
    let request = HttpHandle::checked(unsafe {
        WinHttpOpenRequest(
            connection.0,
            wide("GET").as_ptr(),
            wide(&endpoint.path).as_ptr(),
            null(),
            null(),
            null(),
            if endpoint.secure {
                WINHTTP_FLAG_SECURE
            } else {
                0
            },
        )
    })?;
    let redirect = WINHTTP_OPTION_REDIRECT_POLICY_NEVER;
    let disable = WINHTTP_DISABLE_COOKIES;
    for (option, value) in [
        (WINHTTP_OPTION_REDIRECT_POLICY, redirect),
        (WINHTTP_OPTION_DISABLE_FEATURE, disable),
    ] {
        if unsafe {
            WinHttpSetOption(
                request.0,
                option,
                (&value as *const u32).cast(),
                size_of::<u32>() as u32,
            )
        } == 0
        {
            return Err(network_error());
        }
    }
    timeouts(&request, deadline)?;
    let headers =
        wide("Accept: application/json, application/octet-stream\r\nCache-Control: no-cache\r\n");
    if unsafe {
        WinHttpSendRequest(
            request.0,
            headers.as_ptr(),
            (headers.len() - 1) as u32,
            null(),
            0,
            0,
            0,
        )
    } == 0
    {
        return Err(network_error());
    }
    timeouts(&request, deadline)?;
    if unsafe { WinHttpReceiveResponse(request.0, null_mut()) } == 0 {
        return Err(network_error());
    }
    let mut status = 0_u32;
    let mut length = size_of::<u32>() as u32;
    if unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            null(),
            (&mut status as *mut u32).cast(),
            &mut length,
            null_mut(),
        )
    } == 0
    {
        return Err(network_error());
    }
    Ok((connection, request, status))
}

fn read_body(
    request: &HttpHandle,
    deadline: Instant,
    limit: u64,
    expected: Option<u64>,
    sink: &mut impl FnMut(&[u8]) -> Result<(), String>,
) -> Result<(), String> {
    let declared = header(request, WINHTTP_QUERY_CONTENT_LENGTH)?
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| "更新服务器返回了无效的文件大小。".to_owned())
        })
        .transpose()?;
    if declared
        .is_some_and(|size| size > limit || expected.is_some_and(|expected| size != expected))
    {
        return Err("更新文件大小超出限制或与发布信息不符。".into());
    }
    let mut received = 0_u64;
    let mut bytes = [0_u8; 32 * 1024];
    loop {
        timeouts(request, deadline)?;
        let mut available = 0_u32;
        if unsafe { WinHttpQueryDataAvailable(request.0, &mut available) } == 0 {
            return Err(network_error());
        }
        if available == 0 {
            break;
        }
        timeouts(request, deadline)?;
        let mut count = 0_u32;
        if unsafe {
            WinHttpReadData(
                request.0,
                bytes.as_mut_ptr().cast(),
                available.min(bytes.len() as u32),
                &mut count,
            )
        } == 0
        {
            return Err(network_error());
        }
        if count == 0 {
            break;
        }
        received = received
            .checked_add(u64::from(count))
            .ok_or("更新文件大小无效。")?;
        if received > limit {
            return Err("更新文件超出大小限制，已停止下载。".into());
        }
        sink(&bytes[..count as usize])?;
    }
    if expected.is_some_and(|expected| received != expected)
        || declared.is_some_and(|declared| received != declared)
    {
        return Err("更新下载不完整，请重新下载。".into());
    }
    Ok(())
}

fn header(request: &HttpHandle, query: u32) -> Result<Option<String>, String> {
    let mut bytes = 0_u32;
    let result = unsafe {
        WinHttpQueryHeaders(request.0, query, null(), null_mut(), &mut bytes, null_mut())
    };
    if result == 0 {
        let error = unsafe { GetLastError() };
        if error == ERROR_WINHTTP_HEADER_NOT_FOUND {
            return Ok(None);
        }
        if error != 122 {
            return Err(network_error());
        }
    }
    if bytes == 0 || bytes as usize > (MAX_URL_BYTES + 1) * 2 || bytes % 2 != 0 {
        return Err("更新服务器返回了过长或无效的响应头。".into());
    }
    let mut buffer = vec![0_u16; bytes as usize / 2];
    if unsafe {
        WinHttpQueryHeaders(
            request.0,
            query,
            null(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
            null_mut(),
        )
    } == 0
    {
        return Err(network_error());
    }
    let end = buffer
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(buffer.len());
    let value = String::from_utf16(&buffer[..end]).map_err(|_| "更新服务器响应头编码无效。")?;
    Ok(Some(value))
}

fn timeouts(handle: &HttpHandle, deadline: Instant) -> Result<(), String> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .filter(|value| !value.is_zero())
        .ok_or("更新请求超时，请稍后重试。")?;
    let milliseconds = remaining.as_millis().clamp(1, 15_000) as i32;
    if unsafe {
        WinHttpSetTimeouts(
            handle.0,
            milliseconds,
            milliseconds,
            milliseconds,
            milliseconds,
        )
    } == 0
    {
        Err(network_error())
    } else {
        Ok(())
    }
}
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
fn network_error() -> String {
    match unsafe { GetLastError() } {
        ERROR_WINHTTP_TIMEOUT => "更新检查或下载超时，请稍后重试。",
        ERROR_WINHTTP_SECURE_FAILURE => "更新服务器证书验证失败，请检查系统时间与网络。",
        ERROR_WINHTTP_NAME_NOT_RESOLVED | ERROR_WINHTTP_CANNOT_CONNECT => {
            "无法连接更新服务器，请检查网络和系统代理。"
        }
        _ => "更新网络请求失败，请稍后重试。",
    }
    .into()
}

pub(super) fn verify_embedded_version(path: &Path, expected: Version) -> Result<(), String> {
    let invalid = || "更新文件的内嵌版本与发布信息不符，未执行更新。".to_owned();
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    if path[..path.len() - 1].contains(&0) {
        return Err(invalid());
    }
    let mut ignored = 0_u32;
    let size = unsafe { GetFileVersionInfoSizeW(path.as_ptr(), &mut ignored) };
    if size == 0 || size > 1024 * 1024 {
        return Err(invalid());
    }
    let mut bytes = vec![0_u8; size as usize];
    if unsafe { GetFileVersionInfoW(path.as_ptr(), 0, size, bytes.as_mut_ptr().cast()) } == 0 {
        return Err(invalid());
    }
    let mut fixed = null_mut();
    let mut length = 0_u32;
    if unsafe {
        VerQueryValueW(
            bytes.as_ptr().cast(),
            wide("\\").as_ptr(),
            &mut fixed,
            &mut length,
        )
    } == 0
        || fixed.is_null()
        || length < size_of::<VS_FIXEDFILEINFO>() as u32
    {
        return Err(invalid());
    }
    let fixed = unsafe { std::ptr::read_unaligned(fixed.cast::<VS_FIXEDFILEINFO>()) };
    let ms = (u32::from(expected.0) << 16) | u32::from(expected.1);
    let ls = u32::from(expected.2) << 16;
    if fixed.dwSignature != 0xfeef04bd
        || fixed.dwFileType != 1
        || fixed.dwFileVersionMS != ms
        || fixed.dwFileVersionLS != ls
        || fixed.dwProductVersionMS != ms
        || fixed.dwProductVersionLS != ls
    {
        return Err(invalid());
    }
    Ok(())
}

pub(super) fn verify_path_identity(path: &Path, verified: &File) -> Result<(), String> {
    let invalid = || "更新文件在校验期间发生变化，未执行更新。".to_owned();
    let path_file = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(path)
        .map_err(|_| invalid())?;
    fn identity(file: &File) -> Option<(u32, u32, u32)> {
        let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
            None
        } else {
            Some((
                information.dwVolumeSerialNumber,
                information.nFileIndexHigh,
                information.nFileIndexLow,
            ))
        }
    }
    let original = identity(verified).ok_or_else(invalid)?;
    let current = identity(&path_file).ok_or_else(invalid)?;
    if original != current {
        return Err(invalid());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    #[test]
    fn redirects_are_https_exact_project_or_release_cdn_only() {
        for accepted in [FEED_URL,"https://github.com/xiaoxiaoawei/test-hub/releases/download/v0.5.0/update.json","https://github.com/xiaoxiaoawei/test-hub/releases/download/v0.5.0/lingoleaf-0.5.0-windows-x64.exe","https://release-assets.githubusercontent.com/github-production-release-asset/123/id?sig=a%2Bb"] {assert!(Endpoint::trusted(accepted).is_ok(),"{accepted}");}
        for rejected in ["http://github.com/xiaoxiaoawei/test-hub/releases/latest/download/update.json","https://github.com@evil.invalid/file","https://github.com.evil.invalid/file","https://github.com/other/repo/releases/download/v0.5.0/update.json","https://github.com/xiaoxiaoawei/test-hub/releases/download/v0.5.0/lingoleaf-0.6.0-windows-x64.exe","https://github.com/xiaoxiaoawei/test-hub/releases/download/v0.5.0-beta/update.json","https://release-assets.githubusercontent.com/anything.exe","https://release-assets.githubusercontent.com.evil.invalid/github-production-release-asset/test","https://github.com/xiaoxiaoawei/test-hub/releases/latest/download/update.json#fragment","https://github.com/xiaoxiaoawei/test-hub/releases/latest/download/update.json\r\nInjected: yes"] {assert!(Endpoint::trusted(rejected).is_err(),"{rejected}");}
    }

    #[test]
    fn stable_asset_redirect_path_remains_exactly_scoped() {
        let stable =
            "https://github.com/xiaoxiaoawei/test-hub/releases/download/v0.7.0/lingoleaf.exe";
        assert!(Endpoint::trusted(stable).is_ok());
        for invalid in [
            stable.replace("v0.7.0/", "v0.7.0-beta/"),
            stable.replace("/test-hub/", "/other/"),
            stable.replace("lingoleaf.exe", "LingoLeaf.exe"),
            stable.replace("lingoleaf.exe", "../lingoleaf.exe"),
            stable.replace("lingoleaf.exe", "%6cingoleaf.exe"),
            format!("{stable}?download=1"),
            format!("{stable}#fragment"),
        ] {
            assert!(Endpoint::trusted(&invalid).is_err(), "{invalid}");
        }
    }

    fn fixture(response: Vec<u8>) -> (u16, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut buffer = Vec::new();
            let mut chunk = [0_u8; 1024];
            while !buffer.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let count = stream.read(&mut chunk).unwrap();
                if count == 0 {
                    return;
                }
                buffer.extend_from_slice(&chunk[..count]);
            }
            let request = String::from_utf8(buffer).unwrap().to_ascii_lowercase();
            assert!(request.starts_with("get /test "));
            assert!(!request.contains("authorization:"));
            assert!(!request.contains("cookie:"));
            let _ = stream.write_all(&response);
        });
        (port, worker)
    }
    fn local_response(
        response: Vec<u8>,
        limit: u64,
        expected: Option<u64>,
    ) -> Result<Vec<u8>, String> {
        let (port, worker) = fixture(response);
        let result = (|| {
            let session = HttpHandle::checked(unsafe {
                WinHttpOpen(
                    wide("LingoLeaf-Update-Test").as_ptr(),
                    WINHTTP_ACCESS_TYPE_NO_PROXY,
                    null(),
                    null(),
                    0,
                )
            })?;
            let endpoint = Endpoint {
                host: "127.0.0.1".into(),
                path: "/test".into(),
                port,
                secure: false,
            };
            let (_connection, request, status) =
                request(&session, &endpoint, Instant::now() + Duration::from_secs(5))?;
            assert_eq!(status, 200);
            let mut bytes = Vec::new();
            read_body(
                &request,
                Instant::now() + Duration::from_secs(5),
                limit,
                expected,
                &mut |chunk| {
                    bytes.extend_from_slice(chunk);
                    Ok(())
                },
            )?;
            Ok(bytes)
        })();
        worker.join().unwrap();
        result
    }
    #[test]
    fn actual_http_get_is_keyless_and_rejects_oversized_or_truncated_bodies() {
        let response =
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello".to_vec();
        assert_eq!(
            local_response(response.clone(), 5, Some(5)).unwrap(),
            b"hello"
        );
        assert!(local_response(response, 4, None).is_err());
        assert!(local_response(
            b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nhello".to_vec(),
            4,
            None
        )
        .is_err());
        assert!(local_response(
            b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nhello".to_vec(),
            8,
            Some(8)
        )
        .is_err());
    }

    #[test]
    fn execution_guard_rejects_changed_identity_and_blocks_writes_and_renames() {
        let directory =
            std::env::temp_dir().join(format!("lingoleaf-update-guard-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("verified.exe");
        let other = directory.join("other.exe");
        fs::write(&path, b"first owned fixture").unwrap();
        fs::write(&other, b"second owned fixture").unwrap();
        let guard = OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&path)
            .unwrap();
        assert!(verify_path_identity(&path, &guard).is_ok());
        assert!(verify_path_identity(&other, &guard).is_err());
        assert!(OpenOptions::new().write(true).open(&path).is_err());
        assert!(fs::rename(&path, directory.join("changed.exe")).is_err());
        drop(guard);
        fs::remove_dir_all(directory).unwrap();
    }
}
