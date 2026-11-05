//! User settings. API credentials are never serialized in plaintext.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

const SETTINGS_VERSION: u32 = 1;
const MAX_CONFIG_BYTES: u64 = 64 * 1024;

/// Runtime icon appearance. The executable's embedded file icon is unchanged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IconStyle {
    Leaf,
    #[default]
    Translate,
    Letter,
}

/// Shape of the floating selection icon, independent of its glyph.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IconShape {
    #[default]
    Rounded,
    Square,
    Circle,
}

/// Action taken after selecting text while automatic selection is enabled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionMode {
    #[default]
    Click,
    Direct,
}

#[derive(Clone)]
pub struct Settings {
    pub base_url: String,
    pub model: String,
    pub timeout_seconds: u32,
    pub api_key: String,
    /// Overall switch for automatic text-selection translation.
    pub selection_button: bool,
    pub selection_mode: SelectionMode,
    pub icon_style: IconStyle,
    pub icon_shape: IconShape,
    /// Floating icon size in logical pixels, before per-monitor DPI scaling.
    pub icon_size: u32,
    /// RGB color in the portable 0xRRGGBB representation, not Win32 COLORREF.
    pub icon_color: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".into(),
            model: "gpt-4o-mini".into(),
            timeout_seconds: 20,
            api_key: String::new(),
            selection_button: true,
            selection_mode: SelectionMode::default(),
            icon_style: IconStyle::default(),
            icon_shape: IconShape::default(),
            icon_size: default_icon_size(),
            icon_color: default_icon_color(),
        }
    }
}

// Settings can be inspected during development without exposing a credential.
impl fmt::Debug for Settings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Settings")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("timeout_seconds", &self.timeout_seconds)
            .field(
                "api_key",
                &if self.api_key.is_empty() {
                    "(empty)"
                } else {
                    "[REDACTED]"
                },
            )
            .field("selection_button", &self.selection_button)
            .field("selection_mode", &self.selection_mode)
            .field("icon_style", &self.icon_style)
            .field("icon_shape", &self.icon_shape)
            .field("icon_size", &self.icon_size)
            .field("icon_color", &format_args!("#{:06X}", self.icon_color))
            .finish()
    }
}

impl Settings {
    /// A key-free preset for the official DeepSeek OpenAI-compatible API.
    /// Selecting a preset must not carry a credential over from another service.
    pub fn deepseek() -> Self {
        Self {
            base_url: "https://api.deepseek.com/v1".into(),
            model: "deepseek-chat".into(),
            timeout_seconds: 30,
            ..Self::default()
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        parse_endpoint(&self.base_url)?;
        if self.model.trim().is_empty() || self.model.chars().count() > 256 {
            return Err("请填写有效的模型名称（最多 256 个字符）。".into());
        }
        if self.model.chars().any(char::is_control) {
            return Err("模型名称不能包含控制字符。".into());
        }
        if !(1..=120).contains(&self.timeout_seconds) {
            return Err("网络超时必须为 1–120 秒。".into());
        }
        if self.api_key.len() > 8192 || self.api_key.chars().any(char::is_control) {
            return Err("API Key 格式无效，不能包含换行或控制字符。".into());
        }
        if self.icon_color > 0xFF_FF_FF {
            return Err("图标颜色必须是有效的 RGB 颜色（#000000–#FFFFFF）。".into());
        }
        if !(16..=64).contains(&self.icon_size) {
            return Err("图标大小必须为 16–64 像素。".into());
        }
        Ok(())
    }
}

fn default_version() -> u32 {
    SETTINGS_VERSION
}
fn default_base_url() -> String {
    Settings::default().base_url
}
fn default_model() -> String {
    Settings::default().model
}
fn default_timeout() -> u32 {
    Settings::default().timeout_seconds
}
fn default_selection_button() -> bool {
    true
}
fn default_icon_color() -> u32 {
    0xD6_A5_75
}
fn default_icon_size() -> u32 {
    32
}

#[derive(Serialize, Deserialize)]
struct StoredSettings {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default = "default_base_url")]
    base_url: String,
    #[serde(default = "default_model")]
    model: String,
    #[serde(default = "default_timeout")]
    timeout_seconds: u32,
    #[serde(default = "default_selection_button")]
    selection_button: bool,
    #[serde(default)]
    selection_mode: SelectionMode,
    #[serde(default)]
    icon_style: IconStyle,
    #[serde(default)]
    icon_shape: IconShape,
    #[serde(default = "default_icon_size")]
    icon_size: u32,
    #[serde(default = "default_icon_color")]
    icon_color: u32,
    #[serde(default)]
    api_key_encrypted: Vec<u8>,
}

impl StoredSettings {
    fn from_settings(settings: &Settings, encrypted_key: Vec<u8>) -> Self {
        Self {
            version: SETTINGS_VERSION,
            base_url: settings.base_url.trim().trim_end_matches('/').to_owned(),
            model: settings.model.trim().to_owned(),
            timeout_seconds: settings.timeout_seconds,
            selection_button: settings.selection_button,
            selection_mode: settings.selection_mode,
            icon_style: settings.icon_style,
            icon_shape: settings.icon_shape,
            icon_size: settings.icon_size,
            icon_color: settings.icon_color,
            api_key_encrypted: encrypted_key,
        }
    }

    fn into_settings(self, api_key: String) -> Result<Settings, String> {
        if self.version != SETTINGS_VERSION {
            return Err("设置文件版本不受支持，请使用匹配的应用版本。".into());
        }
        let settings = Settings {
            base_url: self.base_url,
            model: self.model,
            timeout_seconds: self.timeout_seconds,
            selection_button: self.selection_button,
            selection_mode: self.selection_mode,
            icon_style: self.icon_style,
            icon_shape: self.icon_shape,
            icon_size: self.icon_size,
            icon_color: self.icon_color,
            api_key,
        };
        settings.validate()?;
        Ok(settings)
    }
}

/// Whether this user has already saved a configuration (independent of API key presence).
pub fn has_saved_settings() -> bool {
    settings_path().is_ok_and(|path| path.is_file())
}

pub fn load() -> Result<Settings, String> {
    let path = settings_path()?;
    let metadata = match fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Settings::default())
        }
        Err(_) => return Err("无法读取设置文件，请检查用户目录权限。".into()),
    };
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err("设置文件过大，请备份后重新配置。".into());
    }
    let contents = fs::read(&path).map_err(|_| "无法读取设置文件。".to_owned())?;
    let stored: StoredSettings = serde_json::from_slice(&contents)
        .map_err(|_| "设置文件格式损坏，请备份后重新配置。".to_owned())?;
    if stored.version != SETTINGS_VERSION {
        return Err("设置文件版本不受支持，请使用匹配的应用版本。".into());
    }
    let api_key = if stored.api_key_encrypted.is_empty() {
        String::new()
    } else {
        unprotect_key(&stored.api_key_encrypted)?
    };
    stored.into_settings(api_key)
}

pub fn save(settings: &Settings) -> Result<(), String> {
    settings.validate()?;
    let encrypted_key = if settings.api_key.is_empty() {
        Vec::new()
    } else {
        protect_key(settings.api_key.as_bytes())?
    };
    let stored = StoredSettings::from_settings(settings, encrypted_key);
    let bytes = serde_json::to_vec_pretty(&stored).map_err(|_| "无法生成设置文件。".to_owned())?;
    let path = settings_path()?;
    write_atomically(&path, &bytes)
}

fn settings_path() -> Result<PathBuf, String> {
    let local_app_data = std::env::var_os("LOCALAPPDATA")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "找不到 Windows 用户配置目录 LOCALAPPDATA。".to_owned())?;
    Ok(PathBuf::from(local_app_data)
        .join("LingoLeaf")
        .join("settings.json"))
}

fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "设置文件路径无效。".to_owned())?;
    fs::create_dir_all(parent).map_err(|_| "无法创建用户设置目录。".to_owned())?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "系统时间无效，无法保存设置。".to_owned())?
        .as_nanos();
    let temporary = parent.join(format!(".settings-{}-{nonce}.tmp", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| "无法创建临时设置文件。".to_owned())?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| "写入设置失败，请检查磁盘空间和目录权限。".to_owned())?;
        drop(file);
        replace_file(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: Both UTF-16 paths are NUL-terminated and valid during the call.
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        Err("无法替换设置文件，请检查目录权限。".into())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    fs::rename(source, destination).map_err(|_| "无法替换设置文件。".into())
}

#[cfg(windows)]
fn protect_key(bytes: &[u8]) -> Result<Vec<u8>, String> {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    let length = u32::try_from(bytes.len()).map_err(|_| "API Key 过长。".to_owned())?;
    let input = CRYPT_INTEGER_BLOB {
        cbData: length,
        pbData: bytes.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: null_mut(),
    };
    // SAFETY: DPAPI borrows the input; output is copied and freed with LocalFree.
    let succeeded = unsafe {
        CryptProtectData(
            &input,
            null(),
            null(),
            null(),
            null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if succeeded == 0 {
        return Err("Windows 无法加密 API Key，设置未保存。".into());
    }
    if output.pbData.is_null() {
        return Err("Windows 返回了无效的加密结果，设置未保存。".into());
    }
    let encrypted =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData.cast());
    }
    Ok(encrypted)
}

#[cfg(windows)]
fn unprotect_key(bytes: &[u8]) -> Result<String, String> {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    let length =
        u32::try_from(bytes.len()).map_err(|_| "保存的 API Key 无效，请重新填写。".to_owned())?;
    let input = CRYPT_INTEGER_BLOB {
        cbData: length,
        pbData: bytes.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: null_mut(),
    };
    // SAFETY: DPAPI owns output until it is copied, zeroed, and freed below.
    let succeeded = unsafe {
        CryptUnprotectData(
            &input,
            null_mut(),
            null(),
            null(),
            null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if succeeded == 0 || output.pbData.is_null() {
        return Err("无法解密 API Key。密钥绑定 Windows 用户，请在设置中重新填写。".into());
    }
    let decrypted =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        std::ptr::write_bytes(output.pbData, 0, output.cbData as usize);
        LocalFree(output.pbData.cast());
    }
    String::from_utf8(decrypted).map_err(|_| "保存的 API Key 无效，请重新填写。".into())
}

#[cfg(not(windows))]
fn protect_key(_bytes: &[u8]) -> Result<Vec<u8>, String> {
    Err("API Key 的加密保存仅支持 Windows。".into())
}

#[cfg(not(windows))]
fn unprotect_key(_bytes: &[u8]) -> Result<String, String> {
    Err("API Key 的解密仅支持 Windows。".into())
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Endpoint {
    pub host: String,
    pub port: u16,
    pub path: String,
    pub secure: bool,
}

pub(crate) fn parse_endpoint(base_url: &str) -> Result<Endpoint, String> {
    let url = base_url.trim();
    if url.is_empty()
        || url.len() > 2048
        || !url.is_ascii()
        || url.chars().any(|c| c.is_control() || c.is_whitespace())
        || url.contains(['?', '#', '\\', '@'])
    {
        return Err("服务地址无效：请填写不含账号、查询参数或片段的 HTTP(S) Base URL。".into());
    }
    let (scheme, remainder) = url
        .split_once("://")
        .ok_or_else(|| "服务地址需要以 https:// 开头；本地服务可使用 http://。".to_owned())?;
    let secure = if scheme.eq_ignore_ascii_case("https") {
        true
    } else if scheme.eq_ignore_ascii_case("http") {
        false
    } else {
        return Err("仅支持 HTTPS 服务；HTTP 仅限本机服务。".into());
    };
    let (authority, base_path) = match remainder.split_once('/') {
        Some((authority, path)) => (authority, format!("/{path}")),
        None => (remainder, String::new()),
    };
    if authority.is_empty() {
        return Err("服务地址缺少主机名。".into());
    }
    let default_port = if secure { 443 } else { 80 };
    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let (host, tail) = bracketed
            .split_once(']')
            .ok_or_else(|| "IPv6 地址需要使用方括号。".to_owned())?;
        if host.parse::<std::net::Ipv6Addr>().is_err() {
            return Err("IPv6 服务地址无效。".into());
        }
        let port = if tail.is_empty() {
            default_port
        } else {
            parse_port(
                tail.strip_prefix(':')
                    .ok_or_else(|| "服务地址端口无效。".to_owned())?,
            )?
        };
        (host.to_owned(), port)
    } else {
        let (host, port) = match authority.split_once(':') {
            Some((host, port)) => (host, parse_port(port)?),
            None => (authority, default_port),
        };
        if !valid_hostname(host) {
            return Err("服务地址中的主机名无效。".into());
        }
        (host.to_ascii_lowercase(), port)
    };
    let is_loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if !secure && !is_loopback {
        return Err("远程翻译服务必须使用 HTTPS。HTTP 仅允许 localhost 或回环 IP。".into());
    }
    let base_path = base_path.trim_end_matches('/');
    let path = if base_path.ends_with("/chat/completions") {
        base_path.to_owned()
    } else {
        format!("{base_path}/chat/completions")
    };
    Ok(Endpoint {
        host,
        port,
        path,
        secure,
    })
}

fn parse_port(port: &str) -> Result<u16, String> {
    if !port.is_empty() && port.bytes().all(|c| c.is_ascii_digit()) {
        if let Ok(port @ 1..=u16::MAX) = port.parse::<u16>() {
            return Ok(port);
        }
    }
    Err("服务地址端口必须为 1–65535。".into())
}

fn valid_hostname(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_select_to_translate_is_enabled() {
        let settings = Settings::default();
        assert!(settings.selection_button);
        assert!(settings.api_key.is_empty());
        settings.validate().unwrap();
    }

    #[test]
    fn deepseek_preset_forms_the_translation_route_without_a_credential() {
        let settings = Settings::deepseek();
        settings.validate().unwrap();
        let endpoint = parse_endpoint(&settings.base_url).unwrap();
        assert_eq!(endpoint.host, "api.deepseek.com");
        assert_eq!(endpoint.path, "/v1/chat/completions");
        assert!(endpoint.secure);
        assert_eq!(settings.model, "deepseek-chat");
        assert!(settings.api_key.is_empty());
        assert!(settings.selection_button);
        assert_eq!(settings.timeout_seconds, 30);
        // Adding a provider preset does not migrate or replace existing defaults.
        assert_eq!(Settings::default().base_url, "https://api.openai.com/v1");
    }

    #[test]
    fn endpoints_preserve_base_paths_without_duplicating_the_route() {
        for base_url in [
            "https://example.com/v1",
            "https://example.com/v1/",
            "https://example.com/v1/chat/completions/",
        ] {
            assert_eq!(
                parse_endpoint(base_url).unwrap(),
                Endpoint {
                    host: "example.com".into(),
                    port: 443,
                    path: "/v1/chat/completions".into(),
                    secure: true,
                }
            );
        }
        assert_eq!(
            parse_endpoint("https://example.com").unwrap().path,
            "/chat/completions"
        );
    }

    #[test]
    fn local_servers_support_http_ipv4_ipv6_and_ports() {
        for url in [
            "http://localhost:11434/v1",
            "http://127.0.0.1:11434/v1",
            "http://[::1]:11434/v1",
        ] {
            let endpoint = parse_endpoint(url).unwrap();
            assert_eq!(endpoint.port, 11434);
            assert!(!endpoint.secure);
        }
        assert_eq!(
            parse_endpoint("https://[2001:db8::1]:8443/api")
                .unwrap()
                .host,
            "2001:db8::1"
        );
    }

    #[test]
    fn invalid_or_insecure_endpoints_are_rejected() {
        for url in [
            "",
            "example.com/v1",
            "ftp://example.com/v1",
            "http://example.com/v1",
            "https://user:pass@example.com/v1",
            "https://example.com/v1?key=secret",
            "https://example.com/v1#fragment",
            "https://example.com\\other",
            "https://example.com:0/v1",
            "https://example.com:65536/v1",
            "https://example.com:abc/v1",
            "https://example.com:/v1",
            "https://[not-ipv6]/v1",
            "https://example.com\r\nx:1",
            "https:///v1",
            "http://localhost.example.com/v1",
            "https://-example.com/v1",
        ] {
            assert!(parse_endpoint(url).is_err(), "accepted invalid URL: {url}");
        }
    }

    #[test]
    fn credentials_are_redacted_and_only_ciphertext_is_serialized() {
        let settings = Settings {
            api_key: "secret-token-never-save".into(),
            ..Settings::default()
        };
        assert!(!format!("{settings:?}").contains(&settings.api_key));
        let stored = StoredSettings::from_settings(&settings, vec![18, 42, 71]);
        let json = serde_json::to_string(&stored).unwrap();
        assert!(!json.contains(&settings.api_key));
        assert!(!json.contains("\"api_key\":"));
        assert!(json.contains("\"api_key_encrypted\":[18,42,71]"));
    }

    #[test]
    fn old_settings_get_safe_defaults_and_new_settings_roundtrip() {
        let legacy: StoredSettings =
            serde_json::from_str(r#"{"base_url":"https://example.com/v1","model":"test"}"#)
                .unwrap();
        let decoded = legacy.into_settings(String::new()).unwrap();
        assert!(decoded.selection_button);
        assert_eq!(decoded.timeout_seconds, 20);
        assert_eq!(decoded.selection_mode, SelectionMode::Click);
        assert_eq!(decoded.icon_style, IconStyle::Translate);
        assert_eq!(decoded.icon_shape, IconShape::Rounded);
        assert_eq!(decoded.icon_size, 32);
        assert_eq!(decoded.icon_color, 0xD6_A5_75);
        let settings = Settings {
            selection_button: false,
            timeout_seconds: 60,
            ..Settings::default()
        };
        let encoded =
            serde_json::to_vec(&StoredSettings::from_settings(&settings, vec![])).unwrap();
        let stored: StoredSettings = serde_json::from_slice(&encoded).unwrap();
        let result = stored.into_settings(String::new()).unwrap();
        assert!(!result.selection_button);
        assert_eq!(result.timeout_seconds, 60);
    }

    #[test]
    fn version_one_configuration_preserves_provider_and_gets_default_icon() {
        // This is the persisted shape used by 0.2.0; icon fields did not exist.
        let stored: StoredSettings = serde_json::from_str(
            r#"{"version":1,"base_url":"https://api.deepseek.com/v1","model":"deepseek-chat","timeout_seconds":30,"selection_button":false,"api_key_encrypted":[]}"#,
        )
        .unwrap();
        let settings = stored.into_settings(String::new()).unwrap();
        assert_eq!(settings.base_url, "https://api.deepseek.com/v1");
        assert_eq!(settings.model, "deepseek-chat");
        assert_eq!(settings.timeout_seconds, 30);
        assert!(!settings.selection_button);
        assert_eq!(settings.selection_mode, SelectionMode::Click);
        assert_eq!(settings.icon_style, IconStyle::Translate);
        assert_eq!(settings.icon_shape, IconShape::Rounded);
        assert_eq!(settings.icon_size, 32);
        assert_eq!(settings.icon_color, 0xD6_A5_75);
    }

    #[test]
    fn version_zero_three_settings_keep_appearance_and_default_to_click_mode() {
        // 0.3.0 stores icon style/color but no shape, size, or selection mode.
        for selection_button in [false, true] {
            let document = serde_json::json!({
                "version": 1,
                "base_url": "https://example.com/v1",
                "model": "custom-model",
                "timeout_seconds": 60,
                "selection_button": selection_button,
                "icon_style": "leaf",
                "icon_color": 0x12_34_56,
                "api_key_encrypted": [18, 42, 71],
            });
            let stored: StoredSettings = serde_json::from_value(document).unwrap();
            assert_eq!(stored.api_key_encrypted, [18, 42, 71]);
            let settings = stored.into_settings("test-only-key".into()).unwrap();
            assert_eq!(settings.base_url, "https://example.com/v1");
            assert_eq!(settings.model, "custom-model");
            assert_eq!(settings.timeout_seconds, 60);
            assert_eq!(settings.selection_button, selection_button);
            assert_eq!(settings.selection_mode, SelectionMode::Click);
            assert_eq!(settings.icon_style, IconStyle::Leaf);
            assert_eq!(settings.icon_color, 0x12_34_56);
            assert_eq!(settings.icon_shape, IconShape::Rounded);
            assert_eq!(settings.icon_size, 32);
            assert_eq!(settings.api_key, "test-only-key");
        }
    }

    #[test]
    fn every_icon_and_selection_mode_combination_survives_settings_roundtrip() {
        for (style, name, color) in [
            (IconStyle::Leaf, "leaf", 0x00_78_D4),
            (IconStyle::Translate, "translate", 0xFF_00_A6),
            (IconStyle::Letter, "letter", 0x00_00_00),
        ] {
            for (shape, shape_name) in [
                (IconShape::Rounded, "rounded"),
                (IconShape::Square, "square"),
                (IconShape::Circle, "circle"),
            ] {
                for (mode, mode_name) in [
                    (SelectionMode::Click, "click"),
                    (SelectionMode::Direct, "direct"),
                ] {
                    for selection_button in [false, true] {
                        let settings = Settings {
                            selection_button,
                            selection_mode: mode,
                            icon_style: style,
                            icon_shape: shape,
                            icon_size: 48,
                            icon_color: color,
                            ..Settings::default()
                        };
                        let encoded =
                            serde_json::to_value(StoredSettings::from_settings(&settings, vec![]))
                                .unwrap();
                        assert_eq!(encoded["version"], 1);
                        assert_eq!(encoded["icon_style"], name);
                        assert_eq!(encoded["icon_shape"], shape_name);
                        assert_eq!(encoded["icon_size"], 48);
                        assert_eq!(encoded["icon_color"], color);
                        assert_eq!(encoded["selection_mode"], mode_name);
                        let stored: StoredSettings = serde_json::from_value(encoded).unwrap();
                        let restored = stored.into_settings(String::new()).unwrap();
                        assert_eq!(restored.selection_button, selection_button);
                        assert_eq!(restored.selection_mode, mode);
                        assert_eq!(restored.icon_style, style);
                        assert_eq!(restored.icon_shape, shape);
                        assert_eq!(restored.icon_size, 48);
                        assert_eq!(restored.icon_color, color);
                    }
                }
            }
        }
    }

    #[test]
    fn icon_size_limits_are_checked_for_runtime_and_stored_settings() {
        for size in [0, 15, 65, u32::MAX] {
            let settings = Settings {
                icon_size: size,
                ..Settings::default()
            };
            assert!(settings.validate().is_err(), "accepted icon size {size}");
            let stored = StoredSettings::from_settings(&settings, vec![]);
            assert!(stored.into_settings(String::new()).is_err());
        }
        for size in [16, 32, 64] {
            let settings = Settings {
                icon_size: size,
                ..Settings::default()
            };
            settings.validate().unwrap();
            let encoded =
                serde_json::to_vec(&StoredSettings::from_settings(&settings, vec![])).unwrap();
            let stored: StoredSettings = serde_json::from_slice(&encoded).unwrap();
            assert_eq!(stored.into_settings(String::new()).unwrap().icon_size, size);
        }
    }

    #[test]
    fn invalid_rgb_colors_are_rejected_without_restricting_full_rgb_range() {
        for color in [0x01_00_00_00, u32::MAX] {
            let settings = Settings {
                icon_color: color,
                ..Settings::default()
            };
            assert!(settings.validate().is_err());
            let stored = StoredSettings::from_settings(&settings, vec![]);
            assert!(stored.into_settings(String::new()).is_err());
        }
        for color in [0x00_00_00, 0xFF_FF_FF] {
            assert!(Settings {
                icon_color: color,
                ..Settings::default()
            }
            .validate()
            .is_ok());
        }
    }

    #[test]
    fn invalid_model_timeout_and_header_injection_are_rejected() {
        for seconds in [0, 121, u32::MAX] {
            assert!(Settings {
                timeout_seconds: seconds,
                ..Settings::default()
            }
            .validate()
            .is_err());
        }
        assert!(Settings {
            api_key: "token\r\nx:evil".into(),
            ..Settings::default()
        }
        .validate()
        .is_err());
        assert!(Settings {
            model: " ".into(),
            ..Settings::default()
        }
        .validate()
        .is_err());
        let stored: StoredSettings = serde_json::from_str(r#"{"version":999}"#).unwrap();
        assert!(stored.into_settings(String::new()).is_err());
    }

    #[test]
    fn atomic_replace_preserves_latest_contents_and_leaves_no_temp_files() {
        let directory = std::env::temp_dir().join(format!(
            "lingoleaf-config-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("settings.json");
        write_atomically(&path, b"first").unwrap();
        write_atomically(&path, b"second").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_roundtrip_preserves_unicode_secret() {
        let key = "test-secret-仅供测试-current-user-only";
        let encrypted = protect_key(key.as_bytes()).unwrap();
        assert_ne!(encrypted, key.as_bytes());
        assert_eq!(unprotect_key(&encrypted).unwrap(), key);
    }
}
