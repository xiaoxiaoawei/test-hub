//! Isolated Windows installer/helper regression. It never contacts an update
//! feed, uses an API key, or replaces the supplied application/fixture files.
//!
//! Build the two PE64 GUI fixtures (a MinGW cross toolchain works on Linux):
//! ```text
//! x86_64-w64-mingw32-windres examples/support/update_candidate.rc -O coff -o /tmp/update-candidate.o
//! x86_64-w64-mingw32-gcc -municode -mwindows -DACK_STARTUP=1 examples/support/update_candidate.c /tmp/update-candidate.o -lshell32 -o /tmp/update-good.exe
//! x86_64-w64-mingw32-gcc -municode -mwindows -DACK_STARTUP=0 examples/support/update_candidate.c /tmp/update-candidate.o -lshell32 -o /tmp/update-bad.exe
//! cargo build --example update_smoke --target x86_64-pc-windows-gnu
//! ```
//! Run `update_smoke.exe path/to/update-good.exe path/to/update-bad.exe` on
//! Windows, or identify a Wine run explicitly as compatibility testing.
//! The parent is a copy of this example with the new installer protocol. This
//! does not claim that the original 0.4.2 application had an updater.

#[cfg(not(windows))]
fn main() {
    eprintln!("update_smoke requires Windows or a Windows-compatible runtime.");
    std::process::exit(1);
}

#[cfg(windows)]
fn main() {
    if lingoleaf::platform::update_install::run_helper_if_requested() {
        return;
    }
    if let Err(error) = windows::run() {
        eprintln!("Update smoke check failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod windows {
    use lingoleaf::{platform::update_install, update};
    use serde::{Deserialize, Serialize};
    use std::{
        fs::{self, File},
        io::Read,
        path::{Path, PathBuf},
        process::{Child, Command},
        ptr, thread,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE, HWND, WAIT_OBJECT_0},
        Security::Cryptography::CryptHashCertificate2,
        System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, WaitForSingleObject,
            PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
        },
        UI::WindowsAndMessaging::{
            GetClassNameW, GetDlgItem, GetTopWindow, GetWindow, GetWindowTextW,
            GetWindowThreadProcessId, SendMessageTimeoutW, BM_CLICK, GW_HWNDNEXT, SMTO_ABORTIFHUNG,
            SMTO_BLOCK,
        },
    };

    const VERSION: &str = "0.10.3";
    const PARENT_FILE: &str = "update-smoke-parent.json";
    const PARENT_ARGUMENT: &str = "--update-smoke-parent";

    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ParentFixture {
        original_sha256: String,
        download: update::VerifiedDownload,
    }

    pub(super) fn run() -> Result<(), String> {
        let args: Vec<_> = std::env::args_os().skip(1).collect();
        let current = std::env::current_exe()
            .and_then(fs::canonicalize)
            .map_err(|error| format!("Cannot locate this example: {error}"))?;
        let parent_file = current
            .parent()
            .ok_or("The example has no parent directory.")?
            .join(PARENT_FILE);
        if args.as_slice() == [PARENT_ARGUMENT] {
            return run_parent(&current, &parent_file);
        }
        if args.is_empty() && parent_file.is_file() {
            // Only a copied fixture with its matching digest can become the
            // no-argument rollback target. The ordinary harness never does so.
            let fixture = read_parent(&current, &parent_file)?;
            if file_digest(&current)? != fixture.original_sha256 {
                return Err("The rollback parent does not match its fixture.".into());
            }
            return fs::write(
                local_data()?.join("rollback-restarted.txt"),
                b"old parent restarted\n",
            )
            .map_err(|error| format!("Cannot record the rollback restart: {error}"));
        }
        let [good, bad] = args.as_slice() else {
            return Err(
                "Pass the good and bad 0.10.3 Windows GUI fixture EXEs as two arguments.".into(),
            );
        };
        let good = PathBuf::from(good)
            .canonicalize()
            .map_err(|error| format!("Cannot locate the good fixture: {error}"))?;
        let bad = PathBuf::from(bad)
            .canonicalize()
            .map_err(|error| format!("Cannot locate the bad fixture: {error}"))?;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "lingoleaf-update-smoke-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&root).map_err(|error| format!("Cannot create fixture scope: {error}"))?;
        let owned = OwnedDirectory(root);
        run_case(&owned.0.join("success"), &current, &good, true)?;
        run_case(&owned.0.join("rollback"), &current, &bad, false)?;
        println!("Update helper passed: verified replacement and startup acknowledgement; failed startup restored the exact old EXE and restarted it.");
        println!("Only temporary parent copies and fixed public fixtures were used; no network request was sent.");
        Ok(())
    }

    fn run_parent(current: &Path, parent_file: &Path) -> Result<(), String> {
        let fixture = read_parent(current, parent_file)?;
        if file_digest(current)? != fixture.original_sha256 {
            return Err("The fixture parent changed before the update.".into());
        }
        update_install::launch(&fixture.download)?;
        fs::write(
            local_data()?.join("helper-ready.txt"),
            b"helper opened the parent\n",
        )
        .map_err(|error| format!("Cannot record helper readiness: {error}"))?;
        // Returning exits the parent. The authenticated helper owns the wait and
        // may only replace this disposable copy after this process has exited.
        Ok(())
    }

    fn read_parent(current: &Path, file: &Path) -> Result<ParentFixture, String> {
        let expected_data = current
            .parent()
            .and_then(Path::parent)
            .ok_or("Invalid fixture directory.")?
            .join("data")
            .canonicalize()
            .map_err(|_| "The fixture data directory is missing.")?;
        if local_data()?
            .canonicalize()
            .map_err(|error| error.to_string())?
            != expected_data
        {
            return Err("The parent fixture is outside its isolated data scope.".into());
        }
        let bytes =
            fs::read(file).map_err(|error| format!("Cannot read parent fixture: {error}"))?;
        if bytes.len() > 16 * 1024 {
            return Err("The parent fixture is unexpectedly large.".into());
        }
        serde_json::from_slice(&bytes).map_err(|error| format!("Invalid parent fixture: {error}"))
    }

    fn local_data() -> Result<PathBuf, String> {
        std::env::var_os("LOCALAPPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| "The fixture has no isolated LOCALAPPDATA.".into())
    }

    fn run_case(
        root: &Path,
        harness: &Path,
        candidate: &Path,
        succeeds: bool,
    ) -> Result<(), String> {
        let install = root.join("app");
        let data = root.join("data");
        let updates = data.join("LingoLeaf").join("updates");
        fs::create_dir_all(&install).map_err(|error| error.to_string())?;
        fs::create_dir_all(&updates).map_err(|error| error.to_string())?;
        let target = install.join("lingoleaf.exe");
        fs::copy(harness, &target)
            .map_err(|error| format!("Cannot copy the owned parent: {error}"))?;
        let payload = updates.join("candidate.exe");
        fs::copy(candidate, &payload)
            .map_err(|error| format!("Cannot stage the fixture: {error}"))?;
        let original = file_digest(&target)?;
        let candidate_digest = file_digest(&payload)?;
        let download = update::VerifiedDownload {
            path: payload.clone(),
            version: VERSION.into(),
            sha256: candidate_digest.clone(),
            size: fs::metadata(&payload)
                .map_err(|error| error.to_string())?
                .len(),
        };
        // This is the production verification API, including real VERSIONINFO.
        // Fixture preparation does not suppress or replace any verifier.
        update::verify_download(&payload, VERSION, download.size, &candidate_digest)?;
        let fixture = ParentFixture {
            original_sha256: original.clone(),
            download,
        };
        fs::write(
            install.join(PARENT_FILE),
            serde_json::to_vec(&fixture).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        let child = Command::new(&target)
            .arg(PARENT_ARGUMENT)
            .env("LOCALAPPDATA", &data)
            .current_dir(&install)
            .spawn()
            .map_err(|error| format!("Cannot start the owned parent: {error}"))?;
        let mut owned = OwnedChild(child);
        wait_until(
            "the fixture parent to hand off and exit",
            Duration::from_secs(10),
            || match owned.0.try_wait().map_err(|error| error.to_string())? {
                Some(status) if status.success() => Ok(true),
                Some(status) => Err(format!("The fixture parent exited with {status}.")),
                None => Ok(false),
            },
        )?;
        if !data.join("helper-ready.txt").is_file() {
            return Err("The parent exited without authenticating a helper.".into());
        }
        if succeeds {
            wait_until(
                "the candidate startup acknowledgement",
                Duration::from_secs(10),
                || {
                    Ok(data.join("candidate-ready.txt").is_file()
                        && no_backups(&install)?
                        && file_digest(&target)? == candidate_digest)
                },
            )?;
            if data.join("rollback-restarted.txt").exists() {
                return Err("The successful candidate unexpectedly rolled back.".into());
            }
            wait_candidate_exit(&data, &target)?;
            println!("Verified 0.10.3 fixture replacement, startup ack and backup cleanup passed.");
        } else {
            wait_until(
                "the restored parent restart",
                Duration::from_secs(10),
                || {
                    Ok(data.join("candidate-failed.txt").is_file()
                        && data.join("rollback-restarted.txt").is_file()
                        && no_backups(&install)?
                        && file_digest(&target)? == original)
                },
            )?;
            dismiss_owned_helper_error(&updates)?;
            println!("Exit-43 candidate rollback restored the exact old EXE and restarted it.");
        }
        Ok(())
    }

    fn no_backups(directory: &Path) -> Result<bool, String> {
        for file in fs::read_dir(directory).map_err(|error| error.to_string())? {
            let file = file.map_err(|error| error.to_string())?;
            if file
                .file_name()
                .to_string_lossy()
                .starts_with(".lingoleaf-update-")
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn wait_candidate_exit(data: &Path, target: &Path) -> Result<(), String> {
        let pid: u32 = fs::read_to_string(data.join("candidate.pid"))
            .map_err(|error| error.to_string())?
            .trim()
            .parse()
            .map_err(|_| "Invalid owned candidate PID.")?;
        let Some(process) = Process::open(pid) else {
            // It may already have exited naturally; never fall back to killing
            // a discovered PID or to changing another process.
            return Ok(());
        };
        if process
            .image()?
            .canonicalize()
            .map_err(|error| error.to_string())?
            != target.canonicalize().map_err(|error| error.to_string())?
        {
            return Err("The fixture PID no longer belongs to the owned candidate.".into());
        }
        if unsafe { WaitForSingleObject(process.0, 8_000) } != WAIT_OBJECT_0 {
            return Err("The resident fixture did not exit naturally.".into());
        }
        Ok(())
    }

    fn dismiss_owned_helper_error(updates: &Path) -> Result<(), String> {
        let mut helpers = Vec::new();
        for file in fs::read_dir(updates).map_err(|error| error.to_string())? {
            let file = file.map_err(|error| error.to_string())?;
            let name = file.file_name();
            if name.to_string_lossy().starts_with("lingoleaf-helper-") {
                helpers.push(
                    file.path()
                        .canonicalize()
                        .map_err(|error| error.to_string())?,
                );
            }
        }
        if helpers.len() != 1 {
            return Err("The isolated case did not create exactly one helper.".into());
        }
        wait_until(
            "the owned rollback notification",
            Duration::from_secs(5),
            || {
                let mut window = unsafe { GetTopWindow(ptr::null_mut()) };
                for _ in 0..4096 {
                    if window.is_null() {
                        break;
                    }
                    let mut class = [0_u16; 64];
                    let mut title = [0_u16; 128];
                    unsafe {
                        GetClassNameW(window, class.as_mut_ptr(), class.len() as i32);
                        GetWindowTextW(window, title.as_mut_ptr(), title.len() as i32);
                    }
                    if text(&class) == "#32770" && text(&title) == "轻译 · 更新未完成" {
                        let mut pid = 0;
                        unsafe { GetWindowThreadProcessId(window, &mut pid) };
                        if let Some(process) = Process::open(pid) {
                            if process
                                .image()?
                                .canonicalize()
                                .map_err(|error| error.to_string())?
                                == helpers[0]
                            {
                                click_ok(window)?;
                                if unsafe { WaitForSingleObject(process.0, 3_000) } != WAIT_OBJECT_0
                                {
                                    return Err(
                                        "The owned helper did not exit after its dialog closed."
                                            .into(),
                                    );
                                }
                                return Ok(true);
                            }
                        }
                    }
                    window = unsafe { GetWindow(window, GW_HWNDNEXT) };
                }
                Ok(false)
            },
        )
    }

    fn click_ok(dialog: HWND) -> Result<(), String> {
        let button = unsafe { GetDlgItem(dialog, 1) };
        let mut result = 0;
        if button.is_null()
            || unsafe {
                SendMessageTimeoutW(
                    button,
                    BM_CLICK,
                    0,
                    0,
                    SMTO_ABORTIFHUNG | SMTO_BLOCK,
                    3_000,
                    &mut result,
                )
            } == 0
        {
            return Err("Cannot close the owned helper notification.".into());
        }
        Ok(())
    }

    fn file_digest(path: &Path) -> Result<String, String> {
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|error| format!("Cannot hash an owned fixture: {error}"))?
            .take(update::MAX_EXECUTABLE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        if bytes.len() as u64 > update::MAX_EXECUTABLE_BYTES {
            return Err("The fixture exceeds the updater's executable size limit; build the example with --release.".into());
        }
        let mut digest = [0_u8; 32];
        let mut length = digest.len() as u32;
        if unsafe {
            CryptHashCertificate2(
                wide("SHA256").as_ptr(),
                0,
                ptr::null(),
                bytes.as_ptr(),
                bytes.len() as u32,
                digest.as_mut_ptr(),
                &mut length,
            )
        } == 0
            || length != digest.len() as u32
        {
            return Err("Windows could not compute the fixture SHA-256.".into());
        }
        Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
    }

    fn text(value: &[u16]) -> String {
        String::from_utf16_lossy(
            &value[..value
                .iter()
                .position(|value| *value == 0)
                .unwrap_or(value.len())],
        )
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }

    fn wait_until(
        label: &str,
        timeout: Duration,
        mut condition: impl FnMut() -> Result<bool, String>,
    ) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        loop {
            if condition()? {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!("Timed out waiting for {label}."));
            }
            thread::sleep(Duration::from_millis(25));
        }
    }

    struct Process(HANDLE);
    impl Process {
        fn open(pid: u32) -> Option<Self> {
            let handle = unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                    0,
                    pid,
                )
            };
            (!handle.is_null()).then_some(Self(handle))
        }

        fn image(&self) -> Result<PathBuf, String> {
            use std::os::windows::ffi::OsStringExt;
            let mut buffer = vec![0_u16; 32_768];
            let mut count = buffer.len() as u32;
            if unsafe { QueryFullProcessImageNameW(self.0, 0, buffer.as_mut_ptr(), &mut count) }
                == 0
            {
                return Err("Cannot identify an owned fixture process.".into());
            }
            Ok(PathBuf::from(std::ffi::OsString::from_wide(
                &buffer[..count as usize],
            )))
        }
    }
    impl Drop for Process {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    struct OwnedChild(Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            if self.0.try_wait().ok().flatten().is_none() {
                // This handle was returned by spawning our own disposable copy.
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }

    struct OwnedDirectory(PathBuf);
    impl Drop for OwnedDirectory {
        fn drop(&mut self) {
            if fs::remove_dir_all(&self.0).is_err() {
                eprintln!(
                    "Retained isolated update fixture directory: {}",
                    self.0.display()
                );
            }
        }
    }
}
