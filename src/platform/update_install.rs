//! The already-running application starts a copy of itself to install an
//! independently verified update. The helper validates the parent process and
//! waits for its exit; it never enters the tray application or acquires its lock.

use crate::update::{open_verified_download, verify_download, VerifiedDownload};
use serde::{Deserialize, Serialize};
use std::{
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle, process::CommandExt},
    path::{Component, Path, PathBuf},
    process::Command,
    ptr,
    time::{Duration, SystemTime},
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
    },
    Security::{
        Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG},
        GetTokenInformation, TokenElevationType, TokenElevationTypeDefault, TokenElevationTypeFull,
        TokenElevationTypeLimited, TOKEN_ELEVATION_TYPE, TOKEN_QUERY,
    },
    Storage::FileSystem::{
        MoveFileExW, ReplaceFileW, FILE_SHARE_READ, MOVEFILE_REPLACE_EXISTING,
        MOVEFILE_WRITE_THROUGH,
    },
    System::Threading::{
        CreateEventW, GetCurrentProcess, GetCurrentProcessId, OpenEventW, OpenProcess,
        OpenProcessToken, QueryFullProcessImageNameW, SetEvent, WaitForMultipleObjects,
        WaitForSingleObject, CREATE_NO_WINDOW, EVENT_MODIFY_STATE,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    },
    UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK},
};

const APPLY_ARGUMENT: &str = "--lingoleaf-install-update";
const RESTART_ARGUMENT: &str = "--lingoleaf-update-restarted";
const MAX_TICKET_BYTES: u64 = 16 * 1024;
const PARENT_EXIT_MS: u32 = 30_000;
const STARTUP_MS: u32 = 30_000;
const HELPER_READY_MS: u32 = 5_000;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ticket {
    schema_version: u32,
    parent_id: u32,
    target: PathBuf,
    download: VerifiedDownload,
}

/// This blocks only while preparing and authenticating the helper. Call on the
/// updater worker, then exit the normal message loop only after it returns Ok.
/// The helper has opened the parent handle before acknowledging readiness.
pub fn launch(download: &VerifiedDownload) -> Result<(), String> {
    ensure_update_token()?;
    validate_new_version(&download.version)?;
    let directory = update_directory()?;
    fs::create_dir_all(&directory).map_err(|_| "无法创建更新目录，请手动下载新版。".to_owned())?;
    let directory = directory
        .canonicalize()
        .map_err(|_| "无法定位更新目录。".to_owned())?;
    let payload = payload_path(&directory, &download.path)?;
    verify_download(&payload, &download.version, download.size, &download.sha256)?;
    let target = std::env::current_exe()
        .and_then(fs::canonicalize)
        .map_err(|_| "无法定位当前程序，请手动下载新版。".to_owned())?;
    if target.starts_with(&directory) || !is_executable(&target) {
        return Err("请先将轻译放在普通目录，再使用自动更新；也可手动下载。".into());
    }
    if fs::metadata(&target)
        .map_err(|_| "无法读取当前程序。".to_owned())?
        .permissions()
        .readonly()
    {
        return Err("当前程序为只读文件，请手动下载新版到可写目录。".into());
    }
    let nonce = nonce()?;
    let parent = target.parent().ok_or("无法定位程序目录。")?;
    probe_directory(parent, &nonce)?;
    cleanup_stale_files(&directory);
    let helper = directory.join(helper_name(&nonce));
    let ticket_path = directory.join(ticket_name(&nonce));
    let ready = EventHandle::create(&event_name("HelperReady", &nonce))?;
    if copy_new_file(&target, &helper).is_err() {
        let _ = fs::remove_file(&helper);
        return Err("无法准备更新助手，请手动下载新版。".into());
    }
    let ticket = Ticket {
        schema_version: 1,
        parent_id: unsafe { GetCurrentProcessId() },
        target,
        download: VerifiedDownload {
            path: payload,
            ..download.clone()
        },
    };
    if let Err(error) = write_ticket(&ticket_path, &ticket) {
        let _ = fs::remove_file(&helper);
        return Err(error);
    }
    let mut child = match Command::new(&helper)
        .arg(APPLY_ARGUMENT)
        .arg(ticket_name(&nonce))
        .current_dir(&directory)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
    {
        Ok(child) => child,
        Err(_) => {
            let _ = fs::remove_file(&helper);
            let _ = fs::remove_file(&ticket_path);
            return Err("无法启动更新助手，当前程序未退出；请手动下载新版。".into());
        }
    };
    let handles = [ready.0, child.as_raw_handle() as HANDLE];
    let result = unsafe {
        WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, HELPER_READY_MS)
    };
    if result != WAIT_OBJECT_0 {
        let _ = child.kill();
        let _ = child.wait();
        let _ = fs::remove_file(&helper);
        let _ = fs::remove_file(&ticket_path);
        return Err("更新助手未准备就绪，当前程序将继续运行；请手动下载或稍后重试。".into());
    }
    Ok(())
}

/// Call before entering the application's ordinary single-instance path.
/// Returns true for the helper mode, including invalid helper invocations.
pub fn run_helper_if_requested() -> bool {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args.first().is_none_or(|value| value != APPLY_ARGUMENT) {
        return false;
    }
    let result = ensure_update_token().and_then(|_| match args.as_slice() {
        [_, name] => run_helper(name),
        _ => Err("更新助手参数无效，未更改当前程序。".into()),
    });
    if let Err(error) = result {
        unsafe {
            MessageBoxW(
                ptr::null_mut(),
                wide(&error).as_ptr(),
                wide("轻译 · 更新未完成").as_ptr(),
                MB_OK | MB_ICONERROR,
            );
        }
    }
    true
}

/// Call after normal UI initialization. A freshly installed process confirms its
/// own successful startup so the installer can retain or roll back the update.
pub fn notify_startup_ready() {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let [argument, token] = args.as_slice() else {
        return;
    };
    if argument != RESTART_ARGUMENT {
        return;
    }
    let Some(token) = token.to_str().filter(|value| valid_nonce(value)) else {
        return;
    };
    let event = unsafe {
        OpenEventW(
            EVENT_MODIFY_STATE,
            0,
            wide(&event_name("AppReady", token)).as_ptr(),
        )
    };
    if !event.is_null() {
        let event = EventHandle(event);
        unsafe {
            SetEvent(event.0);
        }
    }
}

fn run_helper(name: &OsStr) -> Result<(), String> {
    let token = ticket_nonce(name).ok_or("更新票据名称无效，未更改程序。")?;
    let directory = update_directory()?
        .canonicalize()
        .map_err(|_| "更新目录不存在。".to_owned())?;
    let helper = std::env::current_exe()
        .and_then(fs::canonicalize)
        .map_err(|_| "无法定位更新助手。".to_owned())?;
    if helper != directory.join(helper_name(token)) {
        return Err("更新助手位置不匹配，未更改程序。".into());
    }
    // Keep our original executable snapshot stable for every comparison and
    // rollback. A running image alone need not protect its pathname from rename.
    let _helper_identity_guard = read_locked(&helper)?;
    let ticket_path = directory.join(ticket_name(token));
    let ticket = read_ticket(&ticket_path)?;
    validate_new_version(&ticket.download.version)?;
    let parent = ProcessHandle::open(ticket.parent_id)?;
    // Resolve the target from the real process object before looking at any
    // ticket-controlled path. A forged ticket cannot cause UNC/file probing.
    let target = process_image(parent.0)?
        .canonicalize()
        .map_err(|_| "无法确认原程序路径。".to_owned())?;
    if ticket.target != target {
        return Err("更新票据的进程与原程序不匹配，未更改程序。".into());
    }
    let payload = payload_path(&directory, &ticket.download.path)?;
    if target.starts_with(&directory) || !is_executable(&target) || !payload.starts_with(&directory)
    {
        return Err("更新文件或目标位置不匹配，未更改程序。".into());
    }
    if !files_equal(&helper, &target)? {
        return Err("原程序与更新助手不匹配，未更改程序。".into());
    }
    verify_download(
        &payload,
        &ticket.download.version,
        ticket.download.size,
        &ticket.download.sha256,
    )?;
    let ready = unsafe {
        OpenEventW(
            EVENT_MODIFY_STATE,
            0,
            wide(&event_name("HelperReady", token)).as_ptr(),
        )
    };
    if ready.is_null() {
        return Err("原程序未授权本次更新，未更改程序。".into());
    }
    let ready = EventHandle(ready);
    if unsafe { SetEvent(ready.0) } == 0 {
        return Err("无法通知原程序准备退出。".into());
    }
    if unsafe { WaitForSingleObject(parent.0, PARENT_EXIT_MS) } != WAIT_OBJECT_0 {
        return Err("原程序未退出，本次更新已取消。".into());
    }
    // Download content and target can change while waiting. Revalidate both at
    // the final boundary, before modifying anything in the installation folder.
    if let Err(error) = verify_download(
        &payload,
        &ticket.download.version,
        ticket.download.size,
        &ticket.download.sha256,
    ) {
        restart_original(&helper, &target);
        return Err(error);
    }
    if !files_equal(&helper, &target)? {
        return Err("等待退出期间原程序已改变，已停止更新。".into());
    }
    let result = install_and_restart(&helper, &target, &payload, &ticket.download, token);
    let _ = fs::remove_file(ticket_path);
    result
}

fn install_and_restart(
    helper: &Path,
    target: &Path,
    payload: &Path,
    download: &VerifiedDownload,
    token: &str,
) -> Result<(), String> {
    ensure_update_token()?;
    let parent = target.parent().ok_or("无法定位程序目录。")?;
    let stage = parent.join(format!(".lingoleaf-update-{token}.new.exe"));
    let backup = parent.join(format!(".lingoleaf-update-{token}.bak.exe"));
    if stage.exists() || backup.exists() {
        return Err("更新临时文件冲突，已停止更新。".into());
    }
    let app_ready = match EventHandle::create(&event_name("AppReady", token)) {
        Ok(event) => event,
        Err(error) => {
            restart_original(helper, target);
            return Err(error);
        }
    };
    if let Err(error) = copy_new_file(payload, &stage)
        .and_then(|_| verify_download(&stage, &download.version, download.size, &download.sha256))
    {
        let _ = fs::remove_file(&stage);
        restart_original(helper, target);
        return Err(format!(
            "无法在原目录准备新版，原程序未被替换；请手动下载。{error}"
        ));
    }
    let replaced = unsafe {
        ReplaceFileW(
            wide_path(target).as_ptr(),
            wide_path(&stage).as_ptr(),
            wide_path(&backup).as_ptr(),
            0,
            ptr::null(),
            ptr::null(),
        )
    };
    if replaced == 0 {
        let original_error = unsafe { GetLastError() };
        let _ = fs::remove_file(&stage);
        if backup.exists() && files_equal(helper, &backup)? {
            restore_backup(target, &backup)?;
        }
        restart_original(helper, target);
        return Err(format!(
            "无法替换原程序（系统错误 {original_error}），请手动下载新版到可写目录。"
        ));
    }
    // ReplaceFile consumes names, so validation of the staging file is not the
    // execution boundary. Reopen the actual installed target, recheck all
    // integrity/version metadata and deny write/delete through startup.
    let execution_guard =
        match open_verified_download(target, &download.version, download.size, &download.sha256) {
            Ok(guard) => guard,
            Err(error) => {
                return rollback_after_failed_start(
                    helper,
                    target,
                    &backup,
                    &format!("新版未通过启动前校验：{error}"),
                )
            }
        };
    let started = Command::new(target)
        .arg(RESTART_ARGUMENT)
        .arg(token)
        .current_dir(parent)
        .spawn();
    let mut child = match started {
        Ok(child) => child,
        Err(_) => {
            drop(execution_guard);
            return rollback_after_failed_start(helper, target, &backup, "新版无法启动");
        }
    };
    let handles = [app_ready.0, child.as_raw_handle() as HANDLE];
    let result =
        unsafe { WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, STARTUP_MS) };
    if result == WAIT_OBJECT_0 {
        match child.try_wait() {
            Ok(None) => {
                let _ = fs::remove_file(&backup);
                drop(execution_guard);
                return Ok(());
            }
            Ok(Some(_)) => {
                drop(execution_guard);
                return rollback_after_failed_start(
                    helper,
                    target,
                    &backup,
                    "新版确认启动后立即退出",
                );
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                drop(execution_guard);
                return rollback_after_failed_start(
                    helper,
                    target,
                    &backup,
                    "无法确认新版启动状态",
                );
            }
        }
    }
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    drop(execution_guard);
    rollback_after_failed_start(
        helper,
        target,
        &backup,
        if result == WAIT_TIMEOUT {
            "新版未确认启动完成"
        } else {
            "新版启动时退出"
        },
    )
}

fn rollback_after_failed_start(
    helper: &Path,
    target: &Path,
    backup: &Path,
    reason: &str,
) -> Result<(), String> {
    if !files_equal(helper, backup)? {
        return Err(format!(
            "{reason}，旧版备份验证失败。请手动恢复程序，备份位置：{}",
            backup.display()
        ));
    }
    restore_backup(target, backup)?;
    restart_original(helper, target);
    Err(format!("{reason}，已恢复旧版。请稍后重试或手动下载。"))
}

fn restore_backup(target: &Path, backup: &Path) -> Result<(), String> {
    if unsafe {
        MoveFileExW(
            wide_path(backup).as_ptr(),
            wide_path(target).as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(format!(
            "无法恢复旧版，请手动从此备份恢复：{}",
            backup.display()
        ));
    }
    Ok(())
}

fn restart_original(helper: &Path, target: &Path) {
    // A rollback's destination also needs an execution boundary. Do not run a
    // path merely because moving the backup to that name succeeded.
    let Ok(_execution_guard) = read_locked(target) else {
        return;
    };
    if !files_equal(helper, target).unwrap_or(false) {
        return;
    }
    let mut command = Command::new(target);
    if let Some(parent) = target.parent() {
        command.current_dir(parent);
    }
    let _ = command.spawn();
}

fn read_locked(path: &Path) -> Result<File, String> {
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(path)
        .map_err(|_| "无法锁定程序文件，已停止更新。".into())
}

fn ensure_update_token() -> Result<(), String> {
    let mut token = ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err("无法确认程序运行权限，已停止自动更新；请手动下载新版。".into());
    }
    let token = TokenHandle(token);
    let mut kind: TOKEN_ELEVATION_TYPE = 0;
    let mut length = 0_u32;
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenElevationType,
            (&mut kind as *mut TOKEN_ELEVATION_TYPE).cast(),
            std::mem::size_of_val(&kind) as u32,
            &mut length,
        )
    } == 0
        || length as usize != std::mem::size_of_val(&kind)
    {
        return Err("无法确认程序运行权限，已停止自动更新；请手动下载新版。".into());
    }
    validate_elevation_type(kind)
}

fn validate_elevation_type(kind: TOKEN_ELEVATION_TYPE) -> Result<(), String> {
    if kind == TokenElevationTypeDefault || kind == TokenElevationTypeLimited {
        Ok(())
    } else if kind == TokenElevationTypeFull {
        Err("当前轻译以管理员权限运行。请普通启动后更新，或手动下载新版。".into())
    } else {
        Err("程序权限类型无法确认，已停止自动更新；请手动下载新版。".into())
    }
}

fn update_directory() -> Result<PathBuf, String> {
    let directory = std::env::var_os("LOCALAPPDATA")
        .filter(|value| !value.is_empty())
        .map(|value| PathBuf::from(value).join("LingoLeaf").join("updates"))
        .ok_or_else(|| "找不到本机更新目录，请手动下载新版。".to_owned())?;
    if !directory.is_absolute() {
        return Err("本机更新目录不是绝对路径，请手动下载新版。".into());
    }
    Ok(directory)
}

fn payload_path(directory: &Path, value: &Path) -> Result<PathBuf, String> {
    resolve_payload_path(directory, &update_directory()?, value)
}

fn resolve_payload_path(
    directory: &Path,
    native_directory: &Path,
    value: &Path,
) -> Result<PathBuf, String> {
    if !value.is_absolute()
        || !(value.starts_with(directory) || value.starts_with(native_directory))
        || value
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err("更新文件不在轻译的更新目录中，请重新下载。".into());
    }
    let value = value
        .canonicalize()
        .map_err(|_| "更新文件不存在，请重新下载。".to_owned())?;
    if !value.starts_with(directory) || !value.is_file() {
        return Err("更新文件的真实位置不匹配，未执行更新。".into());
    }
    Ok(value)
}

fn probe_directory(directory: &Path, token: &str) -> Result<(), String> {
    let probe = directory.join(format!(".lingoleaf-write-test-{token}"));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .map_err(|_| "程序目录不可写，请手动下载新版到可写目录；不会请求管理员权限。".to_owned())?;
    drop(file);
    fs::remove_file(probe).map_err(|_| "无法清理程序目录中的测试文件，请手动下载更新。".to_owned())
}

fn write_ticket(path: &Path, ticket: &Ticket) -> Result<(), String> {
    let bytes = serde_json::to_vec(ticket).map_err(|_| "无法准备更新票据。".to_owned())?;
    if bytes.len() as u64 > MAX_TICKET_BYTES {
        return Err("更新路径过长，请手动下载新版。".into());
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| "无法创建更新票据。".to_owned())?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| "无法保存更新票据。".to_owned())
}

fn read_ticket(path: &Path) -> Result<Ticket, String> {
    let file = File::open(path).map_err(|_| "更新票据不存在，未更改程序。".to_owned())?;
    if file
        .metadata()
        .map_err(|_| "无法读取更新票据。".to_owned())?
        .len()
        > MAX_TICKET_BYTES
    {
        return Err("更新票据过大，未更改程序。".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_TICKET_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "无法读取更新票据。".to_owned())?;
    if bytes.len() as u64 > MAX_TICKET_BYTES {
        return Err("更新票据过大。".into());
    }
    let ticket: Ticket =
        serde_json::from_slice(&bytes).map_err(|_| "更新票据无效，未更改程序。".to_owned())?;
    if ticket.schema_version != 1 || ticket.parent_id == 0 {
        return Err("更新票据版本或进程无效，未更改程序。".into());
    }
    Ok(ticket)
}

fn copy_new_file(source: &Path, destination: &Path) -> Result<(), String> {
    let mut source = File::open(source).map_err(|_| "无法读取更新文件。".to_owned())?;
    let mut target = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|_| "无法创建更新临时文件。".to_owned())?;
    let copied = std::io::copy(
        &mut std::io::Read::by_ref(&mut source).take(crate::update::MAX_EXECUTABLE_BYTES + 1),
        &mut target,
    )
    .map_err(|_| "无法写入更新临时文件。".to_owned())?;
    if copied > crate::update::MAX_EXECUTABLE_BYTES {
        return Err("更新文件过大，已停止写入。".into());
    }
    target
        .sync_all()
        .map_err(|_| "无法保存更新临时文件。".to_owned())
}

fn files_equal(left: &Path, right: &Path) -> Result<bool, String> {
    let mut left = File::open(left).map_err(|_| "无法验证原程序或备份。".to_owned())?;
    let mut right = File::open(right).map_err(|_| "无法验证原程序或备份。".to_owned())?;
    let length = left
        .metadata()
        .map_err(|_| "无法读取程序大小。".to_owned())?
        .len();
    if length > 32 * 1024 * 1024
        || length
            != right
                .metadata()
                .map_err(|_| "无法读取程序大小。".to_owned())?
                .len()
    {
        return Ok(false);
    }
    let mut a = [0_u8; 16 * 1024];
    let mut b = [0_u8; 16 * 1024];
    loop {
        let count_a = left
            .read(&mut a)
            .map_err(|_| "无法验证原程序内容。".to_owned())?;
        let count_b = right
            .read(&mut b)
            .map_err(|_| "无法验证原程序内容。".to_owned())?;
        if count_a != count_b || a[..count_a] != b[..count_b] {
            return Ok(false);
        }
        if count_a == 0 {
            return Ok(true);
        }
    }
}

fn validate_new_version(value: &str) -> Result<(), String> {
    fn parse(value: &str) -> Option<[u16; 3]> {
        let parts: Vec<&str> = value.split('.').collect();
        if parts.len() != 3 {
            return None;
        }
        let mut version = [0_u16; 3];
        for (number, part) in version.iter_mut().zip(parts) {
            if part.is_empty()
                || !part.bytes().all(|byte| byte.is_ascii_digit())
                || (part.len() > 1 && part.starts_with('0'))
            {
                return None;
            }
            *number = part.parse().ok()?;
        }
        Some(version)
    }
    let candidate = parse(value).ok_or("更新版本号无效。")?;
    let current = parse(env!("CARGO_PKG_VERSION")).ok_or("当前程序版本号无效。")?;
    if candidate <= current {
        return Err("更新版本不高于当前版本，未执行替换。".into());
    }
    Ok(())
}

fn is_executable(path: &Path) -> bool {
    path.extension()
        .is_some_and(|value| value.eq_ignore_ascii_case("exe"))
}
fn helper_name(token: &str) -> String {
    format!("lingoleaf-helper-{token}.exe")
}
fn ticket_name(token: &str) -> String {
    format!("install-{token}.json")
}
fn event_name(kind: &str, token: &str) -> String {
    format!("Local\\LingoLeaf.Update.{kind}.{token}")
}
fn valid_nonce(token: &str) -> bool {
    token.len() == 32
        && token
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}
fn ticket_nonce(name: &OsStr) -> Option<&str> {
    let name = name.to_str()?;
    name.strip_prefix("install-")
        .and_then(|value| value.strip_suffix(".json"))
        .filter(|value| valid_nonce(value))
}
fn nonce() -> Result<String, String> {
    let mut bytes = [0_u8; 16];
    if unsafe {
        BCryptGenRandom(
            ptr::null_mut(),
            bytes.as_mut_ptr(),
            bytes.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    } < 0
    {
        return Err("无法生成更新票据，请稍后重试。".into());
    }
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
fn cleanup_stale_files(directory: &Path) {
    let Ok(files) = fs::read_dir(directory) else {
        return;
    };
    for file in files.take(256).flatten() {
        let path = file.path();
        let name = file.file_name();
        let Some(name) = name.to_str() else { continue };
        let own_file = ticket_nonce(OsStr::new(name)).is_some()
            || name
                .strip_prefix("lingoleaf-helper-")
                .and_then(|value| value.strip_suffix(".exe"))
                .is_some_and(valid_nonce);
        if !own_file {
            continue;
        }
        let Ok(metadata) = file.metadata() else {
            continue;
        };
        if !metadata.is_file() || metadata.is_symlink() {
            continue;
        }
        if metadata
            .modified()
            .ok()
            .and_then(|time| SystemTime::now().duration_since(time).ok())
            .is_some_and(|age| age >= Duration::from_secs(24 * 60 * 60))
        {
            let _ = fs::remove_file(path);
        }
    }
}
fn process_image(process: HANDLE) -> Result<PathBuf, String> {
    let mut length = 32_768_u32;
    let mut path = vec![0_u16; length as usize];
    if unsafe { QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut length) } == 0 {
        return Err("无法确认原程序的真实路径，未更改程序。".into());
    }
    use std::os::windows::ffi::OsStringExt;
    Ok(PathBuf::from(OsString::from_wide(&path[..length as usize])))
}
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
fn wide_path(value: &Path) -> Vec<u16> {
    value.as_os_str().encode_wide().chain(Some(0)).collect()
}

struct ProcessHandle(HANDLE);
impl ProcessHandle {
    fn open(pid: u32) -> Result<Self, String> {
        let handle = unsafe {
            OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            )
        };
        if handle.is_null() {
            Err("无法等待原程序退出，未更改程序。".into())
        } else {
            Ok(Self(handle))
        }
    }
}
struct TokenHandle(HANDLE);
impl Drop for TokenHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
impl Drop for ProcessHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
struct EventHandle(HANDLE);
impl EventHandle {
    fn create(name: &str) -> Result<Self, String> {
        let handle = unsafe { CreateEventW(ptr::null(), 1, 0, wide(name).as_ptr()) };
        if handle.is_null() {
            Err("无法创建更新启动通知。".into())
        } else if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe {
                CloseHandle(handle);
            }
            Err("更新启动通知已存在，请重试。".into())
        } else {
            Ok(Self(handle))
        }
    }
}
impl Drop for EventHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticket_names_cannot_escape_update_directory_or_select_other_files() {
        let token = "0123456789abcdef0123456789abcdef";
        assert_eq!(ticket_nonce(OsStr::new(&ticket_name(token))), Some(token));
        for name in [
            "../install-0123456789abcdef0123456789abcdef.json",
            "install-../x.json",
            "install-0123456789ABCDEF0123456789ABCDEF.json",
            "install-0.json",
            "install-0123456789abcdef0123456789abcdef.json.exe",
        ] {
            assert!(ticket_nonce(OsStr::new(name)).is_none());
        }
    }

    #[test]
    fn install_rejects_current_older_and_non_stable_versions() {
        assert!(validate_new_version(env!("CARGO_PKG_VERSION")).is_err());
        assert!(validate_new_version("0.0.1").is_err());
        assert!(validate_new_version("65535.65535.65535").is_ok());
        for version in [
            "1.2",
            "1.2.3-beta",
            "v1.2.3",
            "01.2.3",
            "1.2.65536",
            "1.2.3.0",
        ] {
            assert!(validate_new_version(version).is_err());
        }
    }

    #[test]
    fn same_directory_replace_and_restore_preserve_original_bytes() {
        let directory =
            std::env::temp_dir().join(format!("lingoleaf-install-test-{}", nonce().unwrap()));
        fs::create_dir(&directory).unwrap();
        let target = directory.join("轻译.exe");
        let stage = directory.join("update.new.exe");
        let backup = directory.join("update.bak.exe");
        fs::write(&target, b"original executable bytes").unwrap();
        fs::write(&stage, b"updated executable bytes").unwrap();
        assert_ne!(
            unsafe {
                ReplaceFileW(
                    wide_path(&target).as_ptr(),
                    wide_path(&stage).as_ptr(),
                    wide_path(&backup).as_ptr(),
                    0,
                    ptr::null(),
                    ptr::null(),
                )
            },
            0
        );
        assert_eq!(fs::read(&target).unwrap(), b"updated executable bytes");
        assert_eq!(fs::read(&backup).unwrap(), b"original executable bytes");
        restore_backup(&target, &backup).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"original executable bytes");
        assert!(!backup.exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn payload_accepts_native_and_canonical_windows_paths_without_escaping_cache() {
        let directory =
            std::env::temp_dir().join(format!("lingoleaf-payload-path-{}", nonce().unwrap()));
        fs::create_dir(&directory).unwrap();
        let payload = directory.join("下载的轻译.exe");
        fs::write(&payload, b"payload fixture").unwrap();
        let canonical = directory.canonicalize().unwrap();
        let actual = payload.canonicalize().unwrap();
        assert_eq!(
            resolve_payload_path(&canonical, &directory, &payload).unwrap(),
            actual
        );
        assert_eq!(
            resolve_payload_path(&canonical, &directory, &actual).unwrap(),
            actual
        );
        assert!(resolve_payload_path(
            &canonical,
            &directory,
            &directory.join("..").join("other.exe")
        )
        .is_err());
        assert!(resolve_payload_path(
            &canonical,
            &directory,
            Path::new(r"\\untrusted-host\share\payload.exe")
        )
        .is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn executable_identity_lock_prevents_writing_or_renaming_until_dropped() {
        let directory =
            std::env::temp_dir().join(format!("lingoleaf-identity-lock-{}", nonce().unwrap()));
        fs::create_dir(&directory).unwrap();
        let payload = directory.join("protected.exe");
        fs::write(&payload, b"original executable bytes").unwrap();
        let guard = read_locked(&payload).unwrap();
        assert!(OpenOptions::new().write(true).open(&payload).is_err());
        assert!(fs::rename(&payload, directory.join("moved.exe")).is_err());
        assert_eq!(fs::read(&payload).unwrap(), b"original executable bytes");
        drop(guard);
        fs::rename(&payload, directory.join("moved.exe")).unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn auto_update_refuses_split_token_elevation_and_unknown_types() {
        assert!(validate_elevation_type(TokenElevationTypeDefault).is_ok());
        assert!(validate_elevation_type(TokenElevationTypeLimited).is_ok());
        assert!(validate_elevation_type(TokenElevationTypeFull).is_err());
        assert!(validate_elevation_type(0).is_err());
        assert!(validate_elevation_type(4).is_err());
    }
}
