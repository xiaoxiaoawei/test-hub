//! User settings. API credentials are never serialized in plaintext.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

const SETTINGS_VERSION: u32 = 1;
const MAX_CONFIG_BYTES: u64 = 128 * 1024;

/// Runtime icon appearance. The executable's embedded file icon is unchanged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IconStyle {
    #[default]
    Leaf,
    Translate,
    Letter,
}

/// Shape of the floating selection icon, independent of its glyph.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IconShape {
    Rounded,
    Square,
    Circle,
    #[default]
    Leaf,
}

/// Action taken after selecting text while automatic selection is enabled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionMode {
    #[default]
    Click,
    Direct,
}

/// Persisted application palette, independent of the operating system theme.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
}

/// Color family, independent of day/night mode and the selection icon color.
/// Missing fields in older configurations retain their warm palette.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemePreset {
    Ocean,
    Aurora,
    Nebula,
    #[default]
    Amber,
}

pub const THEME_PRESETS: &[ThemePreset] = &[
    ThemePreset::Ocean,
    ThemePreset::Aurora,
    ThemePreset::Nebula,
    ThemePreset::Amber,
];

impl ThemePreset {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Ocean => "深海蓝",
            Self::Aurora => "极光青",
            Self::Nebula => "星云紫",
            Self::Amber => "暖砂金",
        }
    }

    pub const fn subtitle(self) -> &'static str {
        match self {
            Self::Ocean => "清晰 · 沉静",
            Self::Aurora => "轻盈 · 清新",
            Self::Nebula => "灵感 · 探索",
            Self::Amber => "温暖 · 专注",
        }
    }
}

/// Supported translation preferences. Automatic detection is source-only.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    Auto,
    #[default]
    English,
    SimplifiedChinese,
    TraditionalChinese,
    Japanese,
    Korean,
    French,
    German,
    Spanish,
    Russian,
    Portuguese,
    Italian,
}

impl Language {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Auto => "自动检测",
            Self::English => "英语",
            Self::SimplifiedChinese => "简体中文",
            Self::TraditionalChinese => "繁体中文",
            Self::Japanese => "日语",
            Self::Korean => "韩语",
            Self::French => "法语",
            Self::German => "德语",
            Self::Spanish => "西班牙语",
            Self::Russian => "俄语",
            Self::Portuguese => "葡萄牙语",
            Self::Italian => "意大利语",
        }
    }

    /// A fixed, non-user-editable name for request instructions. Callers should
    /// handle Auto separately and ask the service to detect the source language.
    pub const fn prompt_name(self) -> &'static str {
        match self {
            Self::Auto => "auto-detect",
            Self::English => "English",
            Self::SimplifiedChinese => "Simplified Chinese",
            Self::TraditionalChinese => "Traditional Chinese",
            Self::Japanese => "Japanese",
            Self::Korean => "Korean",
            Self::French => "French",
            Self::German => "German",
            Self::Spanish => "Spanish",
            Self::Russian => "Russian",
            Self::Portuguese => "Portuguese",
            Self::Italian => "Italian",
        }
    }
}

pub const SOURCE_LANGUAGES: &[Language] = &[
    Language::Auto,
    Language::English,
    Language::SimplifiedChinese,
    Language::TraditionalChinese,
    Language::Japanese,
    Language::Korean,
    Language::French,
    Language::German,
    Language::Spanish,
    Language::Russian,
    Language::Portuguese,
    Language::Italian,
];

pub const TARGET_LANGUAGES: &[Language] = &[
    Language::English,
    Language::SimplifiedChinese,
    Language::TraditionalChinese,
    Language::Japanese,
    Language::Korean,
    Language::French,
    Language::German,
    Language::Spanish,
    Language::Russian,
    Language::Portuguese,
    Language::Italian,
];

/// A manual-translation shortcut, stored independently of the fixed selection
/// shortcut Ctrl+Alt+T. Modifiers use Win32's portable ALT/CONTROL/SHIFT/WIN bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hotkey {
    pub modifiers: u32,
    pub virtual_key: u32,
}

impl Default for Hotkey {
    fn default() -> Self {
        Self {
            modifiers: 0x0001 | 0x0002,
            virtual_key: 0x20,
        }
    }
}

impl Hotkey {
    pub fn parse(value: &str) -> Result<Self, String> {
        let mut modifiers = 0;
        let mut virtual_key = None;
        for part in value.split('+').map(str::trim) {
            if part.is_empty() {
                return Err("快捷键格式无效，例如 Ctrl+Alt+Space。".into());
            }
            let upper = part.to_ascii_uppercase();
            let modifier = match upper.as_str() {
                "CTRL" | "CONTROL" => Some(0x0002),
                "ALT" => Some(0x0001),
                "SHIFT" => Some(0x0004),
                "WIN" | "WINDOWS" | "META" => Some(0x0008),
                _ => None,
            };
            if let Some(modifier) = modifier {
                if modifiers & modifier != 0 {
                    return Err("快捷键不能重复修饰键。".into());
                }
                modifiers |= modifier;
                continue;
            }
            if virtual_key.is_some() {
                return Err("快捷键只能包含一个主键。".into());
            }
            let key = if matches!(upper.as_str(), "SPACE" | "SPACEBAR" | "空格") {
                0x20
            } else if upper.len() == 1 && upper.as_bytes()[0].is_ascii_alphanumeric() {
                u32::from(upper.as_bytes()[0])
            } else if let Some(function) = upper.strip_prefix('F') {
                let number = function
                    .parse::<u32>()
                    .map_err(|_| "快捷键主键无效。".to_owned())?;
                if !(1..=12).contains(&number) {
                    return Err("功能键仅支持 F1–F12。".into());
                }
                0x70 + number - 1
            } else {
                return Err("主键支持字母、数字、Space 或 F1–F12。".into());
            };
            virtual_key = Some(key);
        }
        let hotkey = Self {
            modifiers,
            virtual_key: virtual_key.ok_or_else(|| "请填写快捷键主键。".to_owned())?,
        };
        hotkey.validate()?;
        Ok(hotkey)
    }

    pub fn validate(self) -> Result<(), String> {
        if self.modifiers & !0x000f != 0 || self.modifiers & (0x0001 | 0x0002 | 0x0008) == 0 {
            return Err("快捷键至少需要 Ctrl、Alt 或 Win 修饰键。".into());
        }
        if !(self.virtual_key == 0x20
            || (u32::from(b'A')..=u32::from(b'Z')).contains(&self.virtual_key)
            || (u32::from(b'0')..=u32::from(b'9')).contains(&self.virtual_key)
            || (0x70..=0x7b).contains(&self.virtual_key))
        {
            return Err("主键支持字母、数字、Space 或 F1–F12。".into());
        }
        if self.modifiers == 0x0003 && self.virtual_key == u32::from(b'T') {
            return Err("Ctrl+Alt+T 已用于翻译选区，请选择其他快捷键。".into());
        }
        if (self.modifiers & 0x0008 != 0 && self.virtual_key == u32::from(b'L'))
            || (self.modifiers == 0x0001 && self.virtual_key == 0x73)
        {
            return Err("此快捷键由 Windows 保留，请选择其他组合。".into());
        }
        Ok(())
    }

    pub fn label(self) -> String {
        let mut parts = Vec::new();
        for (flag, name) in [
            (0x0002, "Ctrl"),
            (0x0001, "Alt"),
            (0x0004, "Shift"),
            (0x0008, "Win"),
        ] {
            if self.modifiers & flag != 0 {
                parts.push(name.to_owned());
            }
        }
        parts.push(match self.virtual_key {
            0x20 => "Space".into(),
            0x70..=0x7b => format!("F{}", self.virtual_key - 0x70 + 1),
            key => char::from_u32(key).map_or_else(|| "?".to_owned(), |key| key.to_string()),
        });
        parts.join(" + ")
    }
}

/// A separate online speech service. Translation credentials are deliberately
/// absent from this value unless the user independently enters the same key.
#[derive(Clone)]
pub struct SpeechSettings {
    pub base_url: String,
    pub model: String,
    pub voice: String,
    pub api_key: String,
    pub timeout_seconds: u32,
}

impl Default for SpeechSettings {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            model: default_speech_model(),
            voice: default_speech_voice(),
            api_key: String::new(),
            timeout_seconds: default_timeout(),
        }
    }
}

impl fmt::Debug for SpeechSettings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SpeechSettings")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("voice", &self.voice)
            .field(
                "api_key",
                &if self.api_key.is_empty() {
                    "(empty)"
                } else {
                    "[REDACTED]"
                },
            )
            .field("timeout_seconds", &self.timeout_seconds)
            .finish()
    }
}

impl SpeechSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.base_url.trim().is_empty() {
            return Err("请先配置朗读接口。".into());
        }
        parse_speech_endpoint(&self.base_url)?;
        validate_speech_identifier(&self.model, "朗读模型", true)?;
        validate_speech_identifier(&self.voice, "朗读音色", true)?;
        validate_speech_key(&self.api_key)?;
        if !(1..=120).contains(&self.timeout_seconds) {
            return Err("朗读超时必须为 1–120 秒。".into());
        }
        Ok(())
    }
}

fn validate_speech_identifier(value: &str, name: &str, required: bool) -> Result<(), String> {
    if value.chars().count() > 256
        || value.chars().any(char::is_control)
        || (required && value.trim().is_empty())
    {
        return Err(format!("请填写有效的{name}（最多 256 个字符）。"));
    }
    Ok(())
}

fn validate_speech_key(value: &str) -> Result<(), String> {
    if value.len() > 8192 || value.chars().any(char::is_control) {
        return Err("朗读 API Key 格式无效。".into());
    }
    Ok(())
}

#[derive(Clone)]
pub struct Settings {
    pub base_url: String,
    pub model: String,
    pub timeout_seconds: u32,
    pub api_key: String,
    pub speech_base_url: String,
    pub speech_model: String,
    pub speech_voice: String,
    pub speech_api_key: String,
    pub manual_hotkey: Hotkey,
    /// Overall switch for automatic text-selection translation.
    pub selection_button: bool,
    pub selection_mode: SelectionMode,
    /// Explicit opt-in to temporarily copy a selection when UIA cannot read it.
    pub compatibility_capture: bool,
    pub theme_mode: ThemeMode,
    pub theme_preset: ThemePreset,
    pub source_language: Language,
    pub target_language: Language,
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
            speech_base_url: String::new(),
            speech_model: default_speech_model(),
            speech_voice: default_speech_voice(),
            speech_api_key: String::new(),
            manual_hotkey: Hotkey::default(),
            selection_button: true,
            selection_mode: SelectionMode::default(),
            compatibility_capture: false,
            theme_mode: ThemeMode::default(),
            theme_preset: ThemePreset::default(),
            source_language: default_source_language(),
            target_language: default_target_language(),
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
            .field("speech_base_url", &self.speech_base_url)
            .field("speech_model", &self.speech_model)
            .field("speech_voice", &self.speech_voice)
            .field(
                "speech_api_key",
                &if self.speech_api_key.is_empty() {
                    "(empty)"
                } else {
                    "[REDACTED]"
                },
            )
            .field("manual_hotkey", &self.manual_hotkey)
            .field("selection_button", &self.selection_button)
            .field("selection_mode", &self.selection_mode)
            .field("compatibility_capture", &self.compatibility_capture)
            .field("theme_mode", &self.theme_mode)
            .field("theme_preset", &self.theme_preset)
            .field("source_language", &self.source_language)
            .field("target_language", &self.target_language)
            .field("icon_style", &self.icon_style)
            .field("icon_shape", &self.icon_shape)
            .field("icon_size", &self.icon_size)
            .field("icon_color", &format_args!("#{:06X}", self.icon_color))
            .finish()
    }
}

impl Settings {
    pub fn speech_settings(&self) -> SpeechSettings {
        SpeechSettings {
            base_url: self.speech_base_url.clone(),
            model: self.speech_model.clone(),
            voice: self.speech_voice.clone(),
            api_key: self.speech_api_key.clone(),
            timeout_seconds: self.timeout_seconds,
        }
    }

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
        self.manual_hotkey.validate()?;
        if !self.speech_base_url.trim().is_empty() {
            parse_speech_endpoint(&self.speech_base_url)?;
        }
        validate_speech_identifier(&self.speech_model, "朗读模型", false)?;
        validate_speech_identifier(&self.speech_voice, "朗读音色", false)?;
        validate_speech_key(&self.speech_api_key)?;
        if self.target_language == Language::Auto {
            return Err("目标语言不能选择自动检测。".into());
        }
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
fn default_source_language() -> Language {
    Language::English
}
fn default_target_language() -> Language {
    Language::SimplifiedChinese
}
fn default_icon_color() -> u32 {
    0x19_A9_7C
}
fn default_speech_model() -> String {
    "tts-1".into()
}
fn default_speech_voice() -> String {
    "alloy".into()
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
    compatibility_capture: bool,
    #[serde(default)]
    theme_mode: ThemeMode,
    #[serde(default)]
    theme_preset: ThemePreset,
    #[serde(default = "default_source_language")]
    source_language: Language,
    #[serde(default = "default_target_language")]
    target_language: Language,
    #[serde(default)]
    icon_style: IconStyle,
    #[serde(default)]
    icon_shape: IconShape,
    #[serde(default = "default_icon_size")]
    icon_size: u32,
    #[serde(default = "default_icon_color")]
    icon_color: u32,
    #[serde(default)]
    icon_defaults_version: u32,
    #[serde(default)]
    api_key_encrypted: Vec<u8>,
    #[serde(default)]
    speech_base_url: String,
    #[serde(default = "default_speech_model")]
    speech_model: String,
    #[serde(default = "default_speech_voice")]
    speech_voice: String,
    #[serde(default)]
    speech_api_key_encrypted: Vec<u8>,
    #[serde(default)]
    manual_hotkey: Hotkey,
}

impl StoredSettings {
    fn from_settings(
        settings: &Settings,
        encrypted_key: Vec<u8>,
        encrypted_speech_key: Vec<u8>,
    ) -> Self {
        Self {
            version: SETTINGS_VERSION,
            base_url: settings.base_url.trim().trim_end_matches('/').to_owned(),
            model: settings.model.trim().to_owned(),
            timeout_seconds: settings.timeout_seconds,
            selection_button: settings.selection_button,
            selection_mode: settings.selection_mode,
            compatibility_capture: settings.compatibility_capture,
            theme_mode: settings.theme_mode,
            theme_preset: settings.theme_preset,
            source_language: settings.source_language,
            target_language: settings.target_language,
            icon_style: settings.icon_style,
            icon_shape: settings.icon_shape,
            icon_size: settings.icon_size,
            icon_color: settings.icon_color,
            icon_defaults_version: 1,
            api_key_encrypted: encrypted_key,
            speech_base_url: settings
                .speech_base_url
                .trim()
                .trim_end_matches('/')
                .to_owned(),
            speech_model: settings.speech_model.trim().to_owned(),
            speech_voice: settings.speech_voice.trim().to_owned(),
            speech_api_key_encrypted: encrypted_speech_key,
            manual_hotkey: settings.manual_hotkey,
        }
    }

    fn into_settings(self, api_key: String, speech_api_key: String) -> Result<Settings, String> {
        if self.version != SETTINGS_VERSION {
            return Err("设置文件版本不受支持，请使用匹配的应用版本。".into());
        }
        // Earlier clients had no marker distinguishing their factory icon.
        // Only the complete old tuple is migrated; every customized tuple and
        // every value explicitly saved by this version remains untouched.
        let migrate_factory_icon = self.icon_defaults_version == 0
            && self.icon_style == IconStyle::Translate
            && self.icon_shape == IconShape::Rounded
            && self.icon_color == 0xD6_A5_75
            && self.icon_size == 32;
        let settings = Settings {
            base_url: self.base_url,
            model: self.model,
            timeout_seconds: self.timeout_seconds,
            selection_button: self.selection_button,
            selection_mode: self.selection_mode,
            compatibility_capture: self.compatibility_capture,
            theme_mode: self.theme_mode,
            theme_preset: self.theme_preset,
            source_language: self.source_language,
            target_language: self.target_language,
            icon_style: if migrate_factory_icon {
                IconStyle::Leaf
            } else {
                self.icon_style
            },
            icon_shape: if migrate_factory_icon {
                IconShape::Leaf
            } else {
                self.icon_shape
            },
            icon_size: self.icon_size,
            icon_color: if migrate_factory_icon {
                default_icon_color()
            } else {
                self.icon_color
            },
            api_key,
            speech_base_url: self.speech_base_url,
            speech_model: self.speech_model,
            speech_voice: self.speech_voice,
            speech_api_key,
            manual_hotkey: self.manual_hotkey,
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
    let speech_api_key = if stored.speech_api_key_encrypted.is_empty() {
        String::new()
    } else {
        unprotect_key(&stored.speech_api_key_encrypted)
            .map_err(|_| "无法解密朗读 API Key，请重新填写。".to_owned())?
    };
    stored.into_settings(api_key, speech_api_key)
}

pub fn save(settings: &Settings) -> Result<(), String> {
    settings.validate()?;
    let encrypted_key = if settings.api_key.is_empty() {
        Vec::new()
    } else {
        protect_key(settings.api_key.as_bytes())?
    };
    let encrypted_speech_key = if settings.speech_api_key.is_empty() {
        Vec::new()
    } else {
        protect_key(settings.speech_api_key.as_bytes())
            .map_err(|_| "无法加密朗读 API Key，设置未保存。".to_owned())?
    };
    let stored = StoredSettings::from_settings(settings, encrypted_key, encrypted_speech_key);
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
    parse_service_endpoint(base_url, "/chat/completions")
}

pub(crate) fn parse_speech_endpoint(base_url: &str) -> Result<Endpoint, String> {
    if base_url
        .trim()
        .trim_end_matches('/')
        .ends_with("/chat/completions")
    {
        return Err("朗读接口不能使用聊天地址，请填写语音服务 Base URL。".into());
    }
    parse_service_endpoint(base_url, "/audio/speech")
}

fn parse_service_endpoint(base_url: &str, route: &str) -> Result<Endpoint, String> {
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
        return Err("远程服务必须使用 HTTPS。HTTP 仅允许 localhost 或回环 IP。".into());
    }
    let base_path = base_path.trim_end_matches('/');
    let path = if base_path.ends_with(route) {
        base_path.to_owned()
    } else {
        format!("{base_path}{route}")
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
    fn manual_shortcuts_roundtrip_and_reject_typing_or_reserved_combinations() {
        let default = Hotkey::default();
        assert_eq!(default.label(), "Ctrl + Alt + Space");
        assert_eq!(Hotkey::parse("ctrl + alt + 空格").unwrap(), default);
        for (text, modifiers, key) in [
            ("Ctrl+Shift+J", 0x0006, u32::from(b'J')),
            ("Alt+9", 0x0001, u32::from(b'9')),
            ("Win+Shift+F8", 0x000c, 0x77),
            ("Ctrl+Alt+F12", 0x0003, 0x7b),
        ] {
            let hotkey = Hotkey::parse(text).unwrap();
            assert_eq!(hotkey.modifiers, modifiers);
            assert_eq!(hotkey.virtual_key, key);
            assert_eq!(Hotkey::parse(&hotkey.label()).unwrap(), hotkey);
            assert_eq!(
                serde_json::from_value::<Hotkey>(serde_json::to_value(hotkey).unwrap()).unwrap(),
                hotkey
            );
        }
        for text in [
            "",
            "Space",
            "Shift+A",
            "Ctrl+",
            "Ctrl+Ctrl+Q",
            "Ctrl+A+B",
            "Ctrl+Alt+T",
            "Win+L",
            "Alt+F4",
            "Ctrl+F13",
            "Ctrl+Tab",
        ] {
            assert!(Hotkey::parse(text).is_err(), "accepted shortcut: {text}");
        }
        assert!(Hotkey {
            modifiers: 0x4003,
            virtual_key: 0x20
        }
        .validate()
        .is_err());
    }

    #[test]
    fn speech_configuration_is_opt_in_with_an_independent_key_and_audio_route() {
        let old: StoredSettings = serde_json::from_str(
            r#"{"version":1,"base_url":"https://api.deepseek.com/v1","model":"deepseek-chat"}"#,
        )
        .unwrap();
        let mut settings = old
            .into_settings("translation-test-key".into(), String::new())
            .unwrap();
        assert!(settings.speech_base_url.is_empty());
        assert!(settings.speech_api_key.is_empty());
        assert_eq!(settings.manual_hotkey, Hotkey::default());
        assert!(settings.validate().is_ok());
        assert!(settings.speech_settings().validate().is_err());
        settings.speech_base_url = "https://speech.example/v1/audio/speech/".into();
        settings.speech_model = "kokoro".into();
        settings.speech_voice = "zf_xiaobei".into();
        settings.speech_api_key = "speech-test-only-key".into();
        let speech = settings.speech_settings();
        speech.validate().unwrap();
        assert_eq!(speech.api_key, "speech-test-only-key");
        assert_eq!(settings.api_key, "translation-test-key");
        assert_eq!(
            parse_speech_endpoint(&speech.base_url).unwrap().path,
            "/v1/audio/speech"
        );
        assert_eq!(
            parse_speech_endpoint("http://127.0.0.1:8880/v1")
                .unwrap()
                .path,
            "/v1/audio/speech"
        );
        assert_eq!(
            parse_endpoint(&settings.base_url).unwrap().path,
            "/v1/chat/completions"
        );
        for invalid in [
            "http://speech.example/v1",
            "https://speech.example/v1?key=secret",
            "https://user:secret@speech.example/v1",
            "https://speech.example/v1/chat/completions",
            "https://speech.example/v1#secret",
        ] {
            assert!(
                parse_speech_endpoint(invalid).is_err(),
                "accepted speech address: {invalid}"
            );
        }
        for change in [0, 1, 2] {
            let mut invalid = speech.clone();
            match change {
                0 => invalid.api_key = "key\r\nHeader:bad".into(),
                1 => invalid.model.clear(),
                _ => invalid.voice = "voice\0bad".into(),
            }
            assert!(invalid.validate().is_err());
        }
    }

    #[test]
    fn translation_and_speech_secrets_remain_separate_and_redacted_in_storage() {
        let settings = Settings {
            api_key: "translation-only-test-secret".into(),
            speech_base_url: "https://speech.example/v1".into(),
            speech_api_key: "speech-only-test-secret".into(),
            manual_hotkey: Hotkey::parse("Ctrl+Shift+F7").unwrap(),
            ..Settings::default()
        };
        let stored = StoredSettings::from_settings(&settings, vec![18, 42, 71], vec![9, 8, 7]);
        let json = serde_json::to_string(&stored).unwrap();
        let debug = format!("{settings:?} {:?}", settings.speech_settings());
        for secret in [&settings.api_key, &settings.speech_api_key] {
            assert!(!json.contains(secret));
            assert!(!debug.contains(secret));
        }
        let stored: StoredSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(stored.api_key_encrypted, [18, 42, 71]);
        assert_eq!(stored.speech_api_key_encrypted, [9, 8, 7]);
        let restored = stored
            .into_settings(settings.api_key.clone(), settings.speech_api_key.clone())
            .unwrap();
        assert_eq!(restored.api_key, settings.api_key);
        assert_eq!(restored.speech_api_key, settings.speech_api_key);
        assert_eq!(restored.manual_hotkey, settings.manual_hotkey);
        assert_eq!(restored.speech_base_url, settings.speech_base_url);
    }

    #[test]
    fn legacy_factory_icon_migrates_once_without_replacing_custom_appearance() {
        let factory = serde_json::json!({"version":1,"base_url":"https://api.deepseek.com/v1","model":"deepseek-chat","icon_style":"translate","icon_shape":"rounded","icon_size":32,"icon_color":0xD6_A5_75});
        let stored: StoredSettings = serde_json::from_value(factory.clone()).unwrap();
        assert_eq!(stored.icon_defaults_version, 0);
        let migrated = stored.into_settings(String::new(), String::new()).unwrap();
        assert_eq!(migrated.icon_style, IconStyle::Leaf);
        assert_eq!(migrated.icon_shape, IconShape::Leaf);
        assert_eq!(migrated.icon_color, 0x19_A9_7C);
        let stamped = StoredSettings::from_settings(&migrated, vec![], vec![]);
        assert_eq!(stamped.icon_defaults_version, 1);
        // A user explicitly choosing the old combination must retain it later.
        let mut explicit = factory.clone();
        explicit["icon_defaults_version"] = 1.into();
        let saved: StoredSettings = serde_json::from_value(explicit).unwrap();
        let preserved = saved.into_settings(String::new(), String::new()).unwrap();
        assert_eq!(preserved.icon_style, IconStyle::Translate);
        assert_eq!(preserved.icon_shape, IconShape::Rounded);
        assert_eq!(preserved.icon_color, 0xD6_A5_75);
        for (field, value) in [
            ("icon_style", serde_json::json!("letter")),
            ("icon_shape", serde_json::json!("circle")),
            ("icon_size", serde_json::json!(48)),
            ("icon_color", serde_json::json!(0x40_48_80)),
        ] {
            let mut custom = factory.clone();
            custom[field] = value.clone();
            let stored: StoredSettings = serde_json::from_value(custom.clone()).unwrap();
            let restored = stored.into_settings(String::new(), String::new()).unwrap();
            let saved =
                serde_json::to_value(StoredSettings::from_settings(&restored, vec![], vec![]))
                    .unwrap();
            assert_eq!(saved[field], value, "custom {field} changed");
            for original in ["icon_style", "icon_shape", "icon_size", "icon_color"] {
                assert_eq!(saved[original], custom[original]);
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn independent_dpapi_speech_and_translation_keys_roundtrip() {
        let settings = Settings {
            api_key: "translation-secret-仅供测试".into(),
            speech_api_key: "speech-secret-仅供测试".into(),
            speech_base_url: "https://speech.example/v1".into(),
            ..Settings::default()
        };
        let a = protect_key(settings.api_key.as_bytes()).unwrap();
        let b = protect_key(settings.speech_api_key.as_bytes()).unwrap();
        assert_ne!(a, b);
        let raw = serde_json::to_vec(&StoredSettings::from_settings(&settings, a, b)).unwrap();
        let stored: StoredSettings = serde_json::from_slice(&raw).unwrap();
        let a = unprotect_key(&stored.api_key_encrypted).unwrap();
        let b = unprotect_key(&stored.speech_api_key_encrypted).unwrap();
        let restored = stored.into_settings(a, b).unwrap();
        assert_eq!(restored.api_key, settings.api_key);
        assert_eq!(restored.speech_api_key, settings.speech_api_key);
    }

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
        let stored = StoredSettings::from_settings(&settings, vec![18, 42, 71], Vec::new());
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
        let decoded = legacy.into_settings(String::new(), String::new()).unwrap();
        assert!(decoded.selection_button);
        assert_eq!(decoded.timeout_seconds, 20);
        assert_eq!(decoded.selection_mode, SelectionMode::Click);
        assert!(!decoded.compatibility_capture);
        assert_eq!(decoded.theme_mode, ThemeMode::Dark);
        assert_eq!(decoded.source_language, Language::English);
        assert_eq!(decoded.target_language, Language::SimplifiedChinese);
        assert_eq!(decoded.icon_style, IconStyle::Leaf);
        assert_eq!(decoded.icon_shape, IconShape::Leaf);
        assert_eq!(decoded.icon_size, 32);
        assert_eq!(decoded.icon_color, 0x19_A9_7C);
        let settings = Settings {
            selection_button: false,
            timeout_seconds: 60,
            ..Settings::default()
        };
        let encoded = serde_json::to_vec(&StoredSettings::from_settings(
            &settings,
            vec![],
            Vec::new(),
        ))
        .unwrap();
        let stored: StoredSettings = serde_json::from_slice(&encoded).unwrap();
        let result = stored.into_settings(String::new(), String::new()).unwrap();
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
        let settings = stored.into_settings(String::new(), String::new()).unwrap();
        assert_eq!(settings.base_url, "https://api.deepseek.com/v1");
        assert_eq!(settings.model, "deepseek-chat");
        assert_eq!(settings.timeout_seconds, 30);
        assert!(!settings.selection_button);
        assert_eq!(settings.selection_mode, SelectionMode::Click);
        assert!(!settings.compatibility_capture);
        assert_eq!(settings.icon_style, IconStyle::Leaf);
        assert_eq!(settings.icon_shape, IconShape::Leaf);
        assert_eq!(settings.icon_size, 32);
        assert_eq!(settings.icon_color, 0x19_A9_7C);
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
            let settings = stored
                .into_settings("test-only-key".into(), String::new())
                .unwrap();
            assert_eq!(settings.base_url, "https://example.com/v1");
            assert_eq!(settings.model, "custom-model");
            assert_eq!(settings.timeout_seconds, 60);
            assert_eq!(settings.selection_button, selection_button);
            assert_eq!(settings.selection_mode, SelectionMode::Click);
            assert!(!settings.compatibility_capture);
            assert_eq!(settings.icon_style, IconStyle::Leaf);
            assert_eq!(settings.icon_color, 0x12_34_56);
            assert_eq!(settings.icon_shape, IconShape::Leaf);
            assert_eq!(settings.icon_size, 32);
            assert_eq!(settings.api_key, "test-only-key");
        }
    }

    #[test]
    fn old_direct_mode_configuration_never_enables_clipboard_capture() {
        // Direct translation was an existing choice before compatibility copying.
        let stored: StoredSettings = serde_json::from_str(
            r#"{"version":1,"base_url":"https://example.com/v1","model":"test","selection_button":true,"selection_mode":"direct","icon_style":"letter","icon_shape":"circle","icon_size":48,"icon_color":1193046,"api_key_encrypted":[18,42,71]}"#,
        )
        .unwrap();
        let encrypted_key = stored.api_key_encrypted.clone();
        let settings = stored
            .into_settings("test-only-key".into(), String::new())
            .unwrap();
        assert!(settings.selection_button);
        assert_eq!(settings.selection_mode, SelectionMode::Direct);
        assert!(!settings.compatibility_capture);
        assert_eq!(settings.icon_style, IconStyle::Letter);
        assert_eq!(settings.icon_shape, IconShape::Circle);
        assert_eq!(settings.icon_size, 48);
        assert_eq!(settings.icon_color, 0x12_34_56);
        let saved = StoredSettings::from_settings(&settings, encrypted_key, Vec::new());
        assert!(!saved.compatibility_capture);
        assert_eq!(saved.api_key_encrypted, [18, 42, 71]);
    }

    #[test]
    fn explicit_clipboard_capture_choice_survives_settings_roundtrip() {
        assert!(!Settings::default().compatibility_capture);
        assert!(!Settings::deepseek().compatibility_capture);
        for mode in [SelectionMode::Click, SelectionMode::Direct] {
            let settings = Settings {
                compatibility_capture: true,
                selection_mode: mode,
                ..Settings::default()
            };
            settings.validate().unwrap();
            let encoded =
                serde_json::to_value(StoredSettings::from_settings(&settings, vec![], Vec::new()))
                    .unwrap();
            assert_eq!(encoded["compatibility_capture"], true);
            assert_eq!(encoded["version"], 1);
            let stored: StoredSettings = serde_json::from_value(encoded).unwrap();
            let restored = stored.into_settings(String::new(), String::new()).unwrap();
            assert!(restored.compatibility_capture);
            assert_eq!(restored.selection_mode, mode);
        }
    }

    #[test]
    fn old_direct_configuration_keeps_warm_dark_theme_and_existing_settings() {
        let legacy = serde_json::json!({
            "version": 1,
            "base_url": "https://example.com/v1",
            "model": "custom-model",
            "timeout_seconds": 30,
            "selection_mode": "direct",
            "compatibility_capture": true,
            "icon_style": "letter",
            "icon_shape": "circle",
            "icon_size": 48,
            "icon_color": 0x12_34_56,
            "api_key_encrypted": [18, 42, 71]
        });
        let stored: StoredSettings = serde_json::from_value(legacy).unwrap();
        assert_eq!(stored.api_key_encrypted, [18, 42, 71]);
        let restored = stored
            .into_settings("test-only-key".into(), String::new())
            .unwrap();
        assert_eq!(restored.theme_mode, ThemeMode::Dark);
        assert_eq!(restored.base_url, "https://example.com/v1");
        assert_eq!(restored.model, "custom-model");
        assert_eq!(restored.selection_mode, SelectionMode::Direct);
        assert!(restored.compatibility_capture);
        assert_eq!(restored.icon_shape, IconShape::Circle);
        assert_eq!(restored.icon_color, 0x12_34_56);
        assert_eq!(restored.api_key, "test-only-key");
    }

    #[test]
    fn legacy_palette_migration_preserves_dark_and_beige_user_preferences() {
        for (mode, expected_mode) in [("dark", ThemeMode::Dark), ("light", ThemeMode::Light)] {
            let document = serde_json::json!({
                "version": 1,
                "base_url": "https://api.deepseek.com/v1",
                "model": "custom-model",
                "timeout_seconds": 37,
                "theme_mode": mode,
                "source_language": "auto",
                "target_language": "japanese",
                "selection_mode": "direct",
                "compatibility_capture": true,
                "icon_style": "letter",
                "icon_shape": "circle",
                "icon_size": 48,
                "icon_color": 0x40_48_80,
                "api_key_encrypted": [18, 42, 71]
            });
            let stored: StoredSettings = serde_json::from_value(document).unwrap();
            let encrypted = stored.api_key_encrypted.clone();
            let restored = stored
                .into_settings("test-only-key".into(), String::new())
                .unwrap();
            assert_eq!(restored.theme_preset, ThemePreset::Amber);
            assert_eq!(restored.theme_mode, expected_mode);
            assert_eq!(restored.base_url, "https://api.deepseek.com/v1");
            assert_eq!(restored.model, "custom-model");
            assert_eq!(restored.timeout_seconds, 37);
            assert_eq!(restored.source_language, Language::Auto);
            assert_eq!(restored.target_language, Language::Japanese);
            assert_eq!(restored.selection_mode, SelectionMode::Direct);
            assert!(restored.compatibility_capture);
            assert_eq!(restored.icon_style, IconStyle::Letter);
            assert_eq!(restored.icon_shape, IconShape::Circle);
            assert_eq!(restored.icon_size, 48);
            assert_eq!(restored.icon_color, 0x40_48_80);
            assert_eq!(restored.api_key, "test-only-key");
            let saved = StoredSettings::from_settings(&restored, encrypted, Vec::new());
            assert_eq!(saved.version, 1);
            assert_eq!(saved.api_key_encrypted, [18, 42, 71]);
        }
        assert_eq!(ThemePreset::default(), ThemePreset::Amber);
        assert_eq!(Settings::default().theme_preset, ThemePreset::Amber);
        assert_eq!(
            crate::visual::Palette::for_theme(ThemeMode::Light, ThemePreset::Amber).background,
            0xF4EAD8
        );
    }

    #[test]
    fn color_families_roundtrip_independently_from_theme_mode_and_api_settings() {
        for (&preset, encoded_name) in THEME_PRESETS
            .iter()
            .zip(["ocean", "aurora", "nebula", "amber"])
        {
            for mode in [ThemeMode::Dark, ThemeMode::Light] {
                let original = Settings {
                    theme_mode: mode,
                    theme_preset: preset,
                    base_url: "https://example.com/custom/v1".into(),
                    model: "custom-model".into(),
                    timeout_seconds: 37,
                    api_key: "test-only-key".into(),
                    source_language: Language::Auto,
                    target_language: Language::Japanese,
                    selection_mode: SelectionMode::Direct,
                    compatibility_capture: true,
                    icon_style: IconStyle::Leaf,
                    icon_shape: IconShape::Circle,
                    icon_size: 48,
                    icon_color: 0x40_48_80,
                    ..Settings::default()
                };
                let encoded = serde_json::to_value(StoredSettings::from_settings(
                    &original,
                    vec![18, 42, 71],
                    Vec::new(),
                ))
                .unwrap();
                assert_eq!(encoded["theme_preset"], encoded_name);
                assert_eq!(encoded["version"], 1);
                assert!(encoded.get("api_key").is_none());
                let stored: StoredSettings = serde_json::from_value(encoded).unwrap();
                assert_eq!(stored.api_key_encrypted, [18, 42, 71]);
                let restored = stored
                    .into_settings(original.api_key.clone(), original.speech_api_key.clone())
                    .unwrap();
                assert_eq!(restored.theme_preset, preset);
                assert_eq!(restored.theme_mode, mode);
                assert_eq!(restored.base_url, original.base_url);
                assert_eq!(restored.model, original.model);
                assert_eq!(restored.timeout_seconds, original.timeout_seconds);
                assert_eq!(restored.api_key, original.api_key);
                assert_eq!(restored.source_language, original.source_language);
                assert_eq!(restored.target_language, original.target_language);
                assert_eq!(restored.selection_button, original.selection_button);
                assert_eq!(restored.selection_mode, original.selection_mode);
                assert_eq!(
                    restored.compatibility_capture,
                    original.compatibility_capture
                );
                assert_eq!(restored.icon_style, original.icon_style);
                assert_eq!(restored.icon_shape, original.icon_shape);
                assert_eq!(restored.icon_size, original.icon_size);
                assert_eq!(restored.icon_color, original.icon_color);
            }
        }
        assert!(
            serde_json::from_str::<StoredSettings>(r#"{"theme_preset":"unknown-family"}"#).is_err()
        );
    }

    #[test]
    fn changing_visual_templates_never_invalidates_translation_cache() {
        let original = Settings::default();
        let mut cache = crate::cache::TranslationCache::new();
        cache.insert(&original, "A quiet workspace.", "安静的工作区。");
        for &preset in THEME_PRESETS {
            for mode in [ThemeMode::Dark, ThemeMode::Light] {
                let changed = Settings {
                    theme_mode: mode,
                    theme_preset: preset,
                    ..original.clone()
                };
                assert_eq!(
                    cache.get(&changed, "A quiet workspace."),
                    Some("安静的工作区。".into()),
                    "visual-only change discarded the cache: {preset:?} / {mode:?}"
                );
            }
        }
    }

    #[test]
    fn both_theme_choices_roundtrip_without_changing_translation_or_appearance() {
        for (mode, encoded_mode) in [(ThemeMode::Dark, "dark"), (ThemeMode::Light, "light")] {
            let settings = Settings {
                theme_mode: mode,
                model: "custom-model".into(),
                selection_mode: SelectionMode::Direct,
                compatibility_capture: true,
                icon_shape: IconShape::Circle,
                icon_size: 48,
                ..Settings::default()
            };
            let encoded = serde_json::to_value(StoredSettings::from_settings(
                &settings,
                vec![18, 42, 71],
                Vec::new(),
            ))
            .unwrap();
            assert_eq!(encoded["theme_mode"], encoded_mode);
            assert_eq!(encoded["version"], SETTINGS_VERSION);
            let stored: StoredSettings = serde_json::from_value(encoded).unwrap();
            assert_eq!(stored.api_key_encrypted, [18, 42, 71]);
            let restored = stored
                .into_settings("test-only-key".into(), String::new())
                .unwrap();
            assert_eq!(restored.theme_mode, mode);
            assert_eq!(restored.model, settings.model);
            assert_eq!(restored.selection_mode, settings.selection_mode);
            assert_eq!(
                restored.compatibility_capture,
                settings.compatibility_capture
            );
            assert_eq!(restored.icon_shape, settings.icon_shape);
            assert_eq!(restored.icon_size, settings.icon_size);
        }
    }

    #[test]
    fn old_version_zero_five_configuration_keeps_english_to_simplified_chinese() {
        let document = serde_json::json!({
            "version": 1,
            "base_url": "https://api.deepseek.com/v1",
            "model": "deepseek-chat",
            "timeout_seconds": 30,
            "selection_mode": "direct",
            "compatibility_capture": true,
            "theme_mode": "light",
            "icon_style": "leaf",
            "icon_shape": "circle",
            "icon_size": 48,
            "icon_color": 0x40_48_80,
            "api_key_encrypted": [18, 42, 71]
        });
        let stored: StoredSettings = serde_json::from_value(document).unwrap();
        let encrypted = stored.api_key_encrypted.clone();
        let restored = stored
            .into_settings("test-only-key".into(), String::new())
            .unwrap();
        assert_eq!(restored.source_language, Language::English);
        assert_eq!(restored.target_language, Language::SimplifiedChinese);
        assert_eq!(restored.base_url, "https://api.deepseek.com/v1");
        assert_eq!(restored.model, "deepseek-chat");
        assert_eq!(restored.timeout_seconds, 30);
        assert_eq!(restored.selection_mode, SelectionMode::Direct);
        assert!(restored.compatibility_capture);
        assert_eq!(restored.theme_mode, ThemeMode::Light);
        assert_eq!(restored.icon_style, IconStyle::Leaf);
        assert_eq!(restored.icon_shape, IconShape::Circle);
        assert_eq!(restored.icon_size, 48);
        assert_eq!(restored.icon_color, 0x40_48_80);
        let saved = StoredSettings::from_settings(&restored, encrypted, Vec::new());
        assert_eq!(saved.api_key_encrypted, [18, 42, 71]);
        assert_eq!(saved.version, 1);
    }

    #[test]
    fn all_language_pairs_roundtrip_with_stable_values_and_existing_preferences() {
        assert_eq!(Settings::default().source_language, Language::English);
        assert_eq!(
            Settings::default().target_language,
            Language::SimplifiedChinese
        );
        for &source in SOURCE_LANGUAGES {
            for &target in TARGET_LANGUAGES {
                let settings = Settings {
                    source_language: source,
                    target_language: target,
                    theme_mode: ThemeMode::Light,
                    selection_mode: SelectionMode::Direct,
                    compatibility_capture: true,
                    icon_shape: IconShape::Circle,
                    icon_size: 48,
                    ..Settings::default()
                };
                settings.validate().unwrap();
                let encoded = serde_json::to_value(StoredSettings::from_settings(
                    &settings,
                    vec![18, 42, 71],
                    Vec::new(),
                ))
                .unwrap();
                assert_eq!(
                    encoded["source_language"],
                    serde_json::to_value(source).unwrap()
                );
                assert_eq!(
                    encoded["target_language"],
                    serde_json::to_value(target).unwrap()
                );
                let stored: StoredSettings = serde_json::from_value(encoded).unwrap();
                assert_eq!(stored.api_key_encrypted, [18, 42, 71]);
                let restored = stored
                    .into_settings("test-only-key".into(), String::new())
                    .unwrap();
                assert_eq!(restored.source_language, source);
                assert_eq!(restored.target_language, target);
                assert_eq!(restored.theme_mode, ThemeMode::Light);
                assert_eq!(restored.selection_mode, SelectionMode::Direct);
                assert!(restored.compatibility_capture);
                assert_eq!(restored.icon_shape, IconShape::Circle);
                assert_eq!(restored.icon_size, 48);
                assert!(!source.label().is_empty());
                assert!(!target.prompt_name().is_empty());
            }
        }
        assert_eq!(serde_json::to_value(Language::English).unwrap(), "english");
        assert_eq!(
            serde_json::to_value(Language::SimplifiedChinese).unwrap(),
            "simplified_chinese"
        );
        assert_eq!(
            serde_json::to_value(Language::TraditionalChinese).unwrap(),
            "traditional_chinese"
        );
        assert_eq!(serde_json::to_value(Language::Auto).unwrap(), "auto");
    }

    #[test]
    fn automatic_language_is_source_only_and_unknown_preferences_are_rejected() {
        let automatic_source = Settings {
            source_language: Language::Auto,
            ..Settings::default()
        };
        automatic_source.validate().unwrap();
        assert!(!TARGET_LANGUAGES.contains(&Language::Auto));
        let invalid_target = Settings {
            target_language: Language::Auto,
            ..Settings::default()
        };
        assert!(invalid_target.validate().is_err());
        let stored = StoredSettings::from_settings(&invalid_target, vec![], Vec::new());
        assert!(stored.into_settings(String::new(), String::new()).is_err());
        assert!(serde_json::from_str::<StoredSettings>(
            r#"{"source_language":"unknown-language"}"#
        )
        .is_err());
        assert!(serde_json::from_str::<StoredSettings>(
            r#"{"target_language":"unknown-language"}"#
        )
        .is_err());
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
                (IconShape::Leaf, "leaf"),
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
                        let encoded = serde_json::to_value(StoredSettings::from_settings(
                            &settings,
                            vec![],
                            Vec::new(),
                        ))
                        .unwrap();
                        assert_eq!(encoded["version"], 1);
                        assert_eq!(encoded["icon_style"], name);
                        assert_eq!(encoded["icon_shape"], shape_name);
                        assert_eq!(encoded["icon_size"], 48);
                        assert_eq!(encoded["icon_color"], color);
                        assert_eq!(encoded["selection_mode"], mode_name);
                        let stored: StoredSettings = serde_json::from_value(encoded).unwrap();
                        let restored = stored.into_settings(String::new(), String::new()).unwrap();
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
            let stored = StoredSettings::from_settings(&settings, vec![], Vec::new());
            assert!(stored.into_settings(String::new(), String::new()).is_err());
        }
        for size in [16, 32, 64] {
            let settings = Settings {
                icon_size: size,
                ..Settings::default()
            };
            settings.validate().unwrap();
            let encoded = serde_json::to_vec(&StoredSettings::from_settings(
                &settings,
                vec![],
                Vec::new(),
            ))
            .unwrap();
            let stored: StoredSettings = serde_json::from_slice(&encoded).unwrap();
            assert_eq!(
                stored
                    .into_settings(String::new(), String::new())
                    .unwrap()
                    .icon_size,
                size
            );
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
            let stored = StoredSettings::from_settings(&settings, vec![], Vec::new());
            assert!(stored.into_settings(String::new(), String::new()).is_err());
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
        assert!(stored.into_settings(String::new(), String::new()).is_err());
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
