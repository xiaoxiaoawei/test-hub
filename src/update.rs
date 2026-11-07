//! The update channel contains no translation credentials and can only point to
//! this project's stable Windows x64 release assets.

use serde::{Deserialize, Serialize};
use std::{
    fmt,
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

mod sha256;
#[cfg(windows)]
mod winhttp;
#[cfg(windows)]
pub use winhttp::{check_for_updates, download_update};

pub const FEED_URL: &str =
    "https://github.com/xiaoxiaoawei/test-hub/releases/latest/download/update.json";
pub const MAX_EXECUTABLE_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
const REPOSITORY: &str = "https://github.com/xiaoxiaoawei/test-hub";

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Version(u16, u16, u16);

impl Version {
    fn parse(value: &str) -> Result<Self, String> {
        let mut parts = value.split('.');
        let mut part = || {
            let part = parts.next().ok_or_else(invalid_manifest)?;
            if part.is_empty()
                || part.len() > 5
                || (part.len() > 1 && part.starts_with('0'))
                || !part.bytes().all(|byte| byte.is_ascii_digit())
            {
                return Err(invalid_manifest());
            }
            part.parse::<u16>().map_err(|_| invalid_manifest())
        };
        let version = Self(part()?, part()?, part()?);
        if parts.next().is_some() {
            return Err(invalid_manifest());
        }
        Ok(version)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.0, self.1, self.2)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub version: String,
    pub download_url: String,
    pub notes_url: String,
    pub sha256: String,
    pub size: u64,
}

/// Metadata is revalidated at every I/O entry point, even when a caller has
/// deserialized it for the separate installer helper.
impl Release {
    fn validate(&self) -> Result<Version, String> {
        let version = Version::parse(&self.version)?;
        validate_digest_and_size(&self.sha256, self.size)?;
        let release_path = format!("{REPOSITORY}/releases/download/v{version}");
        let legacy_asset = format!("{release_path}/lingoleaf-{version}-windows-x64.exe");
        let stable_asset = format!("{release_path}/lingoleaf.exe");
        if (self.download_url != legacy_asset && self.download_url != stable_asset)
            || self.notes_url != format!("{REPOSITORY}/releases/tag/v{version}")
        {
            return Err(invalid_manifest());
        }
        Ok(version)
    }
}

pub type UpdateRelease = Release;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CheckResult {
    Current,
    Available(Release),
    NotPublished,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedDownload {
    pub path: PathBuf,
    pub version: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    version: String,
    download_url: String,
    notes_url: String,
    sha256: String,
    size: u64,
}

pub fn parse_manifest(bytes: &[u8], current_version: &str) -> Result<CheckResult, String> {
    if bytes.is_empty() || bytes.len() > MAX_MANIFEST_BYTES {
        return Err(invalid_manifest());
    }
    let manifest: Manifest = serde_json::from_slice(bytes).map_err(|_| invalid_manifest())?;
    if manifest.schema_version != 1 {
        return Err(invalid_manifest());
    }
    let release = Release {
        version: manifest.version,
        download_url: manifest.download_url,
        notes_url: manifest.notes_url,
        sha256: manifest.sha256,
        size: manifest.size,
    };
    let latest = release.validate()?;
    let current = Version::parse(current_version)?;
    if latest > current {
        Ok(CheckResult::Available(release))
    } else {
        // A stale published feed must never install a downgrade.
        Ok(CheckResult::Current)
    }
}

fn invalid_manifest() -> String {
    "更新信息无效，请稍后重试或从官方发布页下载。".into()
}

fn validate_digest_and_size(digest: &str, size: u64) -> Result<(), String> {
    if !(64..=MAX_EXECUTABLE_BYTES).contains(&size)
        || digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid_manifest());
    }
    Ok(())
}

/// Recheck size, SHA-256, PE architecture/type and, on Windows, the embedded
/// file/product version. The helper calls this immediately before replacement.
pub fn verify_download(
    path: &Path,
    expected_version: &str,
    expected_size: u64,
    expected_sha256: &str,
) -> Result<(), String> {
    open_verified_download(path, expected_version, expected_size, expected_sha256).map(drop)
}

/// The returned read handle denies write/delete sharing on Windows. Keep it
/// alive until the new process has acknowledged startup, then drop before any
/// rollback or cleanup that needs to rename this file.
pub fn open_verified_download(
    path: &Path,
    expected_version: &str,
    expected_size: u64,
    expected_sha256: &str,
) -> Result<File, String> {
    let version = Version::parse(expected_version)?;
    validate_digest_and_size(expected_sha256, expected_size)?;
    let metadata = fs::symlink_metadata(path).map_err(|_| "找不到已下载的更新文件。")?;
    if !metadata.file_type().is_file() || metadata.len() != expected_size {
        return Err("更新文件大小不符，请重新下载。".into());
    }
    #[cfg(windows)]
    let mut file = {
        use std::os::windows::fs::OpenOptionsExt;
        // Keep a read-only sharing handle alive through hash and VERSIONINFO
        // checks so this file cannot be modified or renamed during verification.
        fs::OpenOptions::new().read(true).share_mode(1).open(path)
    }
    .map_err(|_| "无法读取更新文件，请重新下载。")?;
    #[cfg(not(windows))]
    let mut file = File::open(path).map_err(|_| "无法读取更新文件，请重新下载。")?;
    let mut digest = sha256::Sha256::new();
    let mut buffer = [0_u8; 32 * 1024];
    let mut received = 0_u64;
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| "读取更新文件失败，请重新下载。")?;
        if count == 0 {
            break;
        }
        received += count as u64;
        if received > expected_size {
            return Err("更新文件大小已改变，请重新下载。".into());
        }
        digest.update(&buffer[..count]);
    }
    if received != expected_size {
        return Err("更新文件大小已改变，请重新下载。".into());
    }
    if sha256::hex(&digest.finish()) != expected_sha256 {
        return Err("更新文件校验失败，未执行更新，请重新下载。".into());
    }
    verify_pe(&mut file, expected_size)?;
    #[cfg(windows)]
    {
        winhttp::verify_embedded_version(path, version)?;
        winhttp::verify_path_identity(path, &file)?;
    }
    #[cfg(not(windows))]
    let _ = version;
    Ok(file)
}

fn verify_pe(file: &mut File, size: u64) -> Result<(), String> {
    let invalid = || "更新文件不是有效的 Windows x64 轻译程序。".to_owned();
    let mut dos = [0_u8; 64];
    file.seek(SeekFrom::Start(0)).map_err(|_| invalid())?;
    file.read_exact(&mut dos).map_err(|_| invalid())?;
    if &dos[..2] != b"MZ" {
        return Err(invalid());
    }
    let offset = u64::from(u32::from_le_bytes(dos[60..64].try_into().unwrap()));
    if offset < 64 || offset.checked_add(96).is_none_or(|end| end > size) {
        return Err(invalid());
    }
    let mut pe = [0_u8; 96];
    file.seek(SeekFrom::Start(offset)).map_err(|_| invalid())?;
    file.read_exact(&mut pe).map_err(|_| invalid())?;
    let machine = u16::from_le_bytes([pe[4], pe[5]]);
    let optional_size = u16::from_le_bytes([pe[20], pe[21]]);
    let characteristics = u16::from_le_bytes([pe[22], pe[23]]);
    let magic = u16::from_le_bytes([pe[24], pe[25]]);
    let subsystem = u16::from_le_bytes([pe[92], pe[93]]);
    if &pe[..4] != b"PE\0\0"
        || machine != 0x8664
        || optional_size < 72
        || offset + 24 + u64::from(optional_size) > size
        || characteristics & 0x0002 == 0
        || characteristics & 0x2000 != 0
        || magic != 0x020b
        || subsystem != 2
    {
        return Err(invalid());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(version: &str) -> serde_json::Value {
        serde_json::json!({
            "schema_version":1,"version":version,
            "download_url":format!("{REPOSITORY}/releases/download/v{version}/lingoleaf-{version}-windows-x64.exe"),
            "notes_url":format!("{REPOSITORY}/releases/tag/v{version}"),
            "sha256":"a".repeat(64),"size":625152,
        })
    }

    #[test]
    fn versions_compare_numbers_and_never_offer_downgrades() {
        for (latest, current, available) in [
            ("0.10.0", "0.9.9", true),
            ("1.0.0", "0.99.99", true),
            ("0.5.0", "0.5.0", false),
            ("0.4.2", "0.5.0", false),
        ] {
            let result =
                parse_manifest(&serde_json::to_vec(&manifest(latest)).unwrap(), current).unwrap();
            assert_eq!(matches!(result, CheckResult::Available(_)), available);
        }
        for invalid in [
            "v0.5.0",
            "0.05.0",
            "0.5",
            "0.5.0.1",
            "0.5.0-beta",
            "0.5.0+meta",
            "65536.0.0",
            " 0.5.0",
            "+0.5.0",
        ] {
            assert!(Version::parse(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn feed_rejects_wrong_origins_versions_hashes_and_unbounded_metadata() {
        let cases = [
            ("schema_version", serde_json::json!(2)),
            ("size", serde_json::json!(MAX_EXECUTABLE_BYTES + 1)),
            ("size", serde_json::json!(63)),
            ("sha256", serde_json::json!("A".repeat(64))),
            ("sha256", serde_json::json!("g".repeat(64))),
            (
                "download_url",
                serde_json::json!("https://evil.invalid/lingoleaf.exe"),
            ),
            (
                "download_url",
                serde_json::json!(format!(
                    "{REPOSITORY}/releases/download/v0.5.1/lingoleaf-0.5.1-windows-x64.exe"
                )),
            ),
            (
                "notes_url",
                serde_json::json!("https://github.com/other/repo/releases/tag/v0.5.0"),
            ),
            ("extra_command", serde_json::json!("not accepted")),
        ];
        for (key, value) in cases {
            let mut body = manifest("0.5.0");
            body[key] = value;
            assert!(
                parse_manifest(&serde_json::to_vec(&body).unwrap(), "0.4.2").is_err(),
                "{key}"
            );
        }
        assert!(parse_manifest(&vec![b' '; MAX_MANIFEST_BYTES + 1], "0.4.2").is_err());
        assert!(parse_manifest(br#"{"schema_version":1,"schema_version":1}"#, "0.4.2").is_err());
    }

    #[test]
    fn stable_and_legacy_asset_names_are_bound_to_the_exact_repository_and_version() {
        let mut body = manifest("0.7.0");
        let stable = format!("{REPOSITORY}/releases/download/v0.7.0/lingoleaf.exe");
        body["download_url"] = serde_json::json!(stable);
        assert!(matches!(
            parse_manifest(&serde_json::to_vec(&body).unwrap(), "0.6.0"),
            Ok(CheckResult::Available(_))
        ));
        for url in [
            stable.replace("v0.7.0/", "v0.7.1/"),
            format!("{stable}?download=1"),
            format!("{stable}#fragment"),
            stable.replace("lingoleaf.exe", "LingoLeaf.exe"),
            stable.replace("lingoleaf.exe", "../lingoleaf.exe"),
            stable.replace("lingoleaf.exe", "%6cingoleaf.exe"),
            stable.replace("github.com/", "github.com.evil.invalid/"),
            stable.replace("/test-hub/", "/other/"),
        ] {
            body["download_url"] = serde_json::json!(url);
            assert!(
                parse_manifest(&serde_json::to_vec(&body).unwrap(), "0.6.0").is_err(),
                "{url}"
            );
        }
        let legacy = manifest("0.7.0");
        assert!(matches!(
            parse_manifest(&serde_json::to_vec(&legacy).unwrap(), "0.6.0"),
            Ok(CheckResult::Available(_))
        ));
    }

    #[test]
    fn pe_checks_reject_console_dll_wrong_machine_and_truncated_headers() {
        let directory =
            std::env::temp_dir().join(format!("lingoleaf-pe-check-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("test.exe");
        let mut bytes = vec![0_u8; 512];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[60..64].copy_from_slice(&64_u32.to_le_bytes());
        bytes[64..68].copy_from_slice(b"PE\0\0");
        bytes[68..70].copy_from_slice(&0x8664_u16.to_le_bytes());
        bytes[84..86].copy_from_slice(&240_u16.to_le_bytes());
        bytes[86..88].copy_from_slice(&2_u16.to_le_bytes());
        bytes[88..90].copy_from_slice(&0x20b_u16.to_le_bytes());
        bytes[156..158].copy_from_slice(&2_u16.to_le_bytes());
        fs::write(&path, &bytes).unwrap();
        assert!(verify_pe(&mut File::open(&path).unwrap(), 512).is_ok());
        for (offset, replacement) in [(68, 0x14c_u16), (86, 0x2002), (156, 3), (88, 0x10b)] {
            let mut broken = bytes.clone();
            broken[offset..offset + 2].copy_from_slice(&replacement.to_le_bytes());
            fs::write(&path, broken).unwrap();
            assert!(verify_pe(&mut File::open(&path).unwrap(), 512).is_err());
        }
        bytes[60..64].copy_from_slice(&500_u32.to_le_bytes());
        fs::write(&path, bytes).unwrap();
        assert!(verify_pe(&mut File::open(&path).unwrap(), 512).is_err());
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn verification_checks_exact_size_and_hash_before_accepting_any_executable() {
        let directory =
            std::env::temp_dir().join(format!("lingoleaf-integrity-check-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("test.exe");
        fs::write(&path, [b'x'; 128]).unwrap();
        assert!(verify_download(&path, "0.5.1", 129, &"0".repeat(64))
            .unwrap_err()
            .contains("大小"));
        assert!(verify_download(&path, "0.5.1", 128, &"0".repeat(64))
            .unwrap_err()
            .contains("校验失败"));
        let mut hash = sha256::Sha256::new();
        hash.update(&[b'x'; 128]);
        assert!(
            verify_download(&path, "0.5.1", 128, &sha256::hex(&hash.finish()))
                .unwrap_err()
                .contains("Windows x64")
        );
        let _ = fs::remove_dir_all(directory);
    }
}
