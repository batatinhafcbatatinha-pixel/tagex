//! In-memory representation of the library (files, tags, sources).

use egui::Color32;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    Image,
    Video,
    Audio,
    Document,
    Archive,
    Code,
    Other,
}

impl Kind {
    pub const ALL: [Kind; 7] = [
        Kind::Image,
        Kind::Video,
        Kind::Audio,
        Kind::Document,
        Kind::Archive,
        Kind::Code,
        Kind::Other,
    ];

    pub fn from_ext(ext: &str) -> Kind {
        match ext {
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "heic" | "heif"
            | "avif" | "svg" | "ico" | "raw" | "cr2" | "nef" | "arw" | "dng" | "psd" => Kind::Image,
            "mp4" | "mkv" | "avi" | "mov" | "wmv" | "webm" | "flv" | "m4v" | "mpg" | "mpeg"
            | "ts" | "3gp" => Kind::Video,
            "mp3" | "flac" | "wav" | "ogg" | "m4a" | "aac" | "wma" | "opus" | "aiff" => Kind::Audio,
            "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "odt" | "ods" | "odp"
            | "txt" | "md" | "rtf" | "epub" | "csv" => Kind::Document,
            "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "zst" | "iso" => Kind::Archive,
            "rs" | "py" | "js" | "tsx" | "jsx" | "c" | "h" | "cpp" | "hpp" | "cs"
            | "java" | "kt" | "go" | "rb" | "php" | "html" | "css" | "json" | "toml" | "yaml"
            | "yml" | "xml" | "sh" | "ps1" | "bat" | "sql" | "lua" => Kind::Code,
            _ => Kind::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Image => crate::i18n::tr("Images", "Imagens"),
            Kind::Video => crate::i18n::tr("Videos", "Vídeos"),
            Kind::Audio => crate::i18n::tr("Audio", "Áudio"),
            Kind::Document => crate::i18n::tr("Documents", "Documentos"),
            Kind::Archive => crate::i18n::tr("Archives", "Compactados"),
            Kind::Code => crate::i18n::tr("Code", "Código"),
            Kind::Other => crate::i18n::tr("Other", "Outros"),
        }
    }

    pub fn color(self) -> Color32 {
        match self {
            Kind::Image => Color32::from_rgb(0x4c, 0xc9, 0xa4),
            Kind::Video => Color32::from_rgb(0xf0, 0x6c, 0x7a),
            Kind::Audio => Color32::from_rgb(0xf5, 0xb1, 0x4c),
            Kind::Document => Color32::from_rgb(0x5b, 0x9c, 0xf5),
            Kind::Archive => Color32::from_rgb(0xb0, 0x8c, 0x6a),
            Kind::Code => Color32::from_rgb(0xa7, 0x8b, 0xfa),
            Kind::Other => Color32::from_rgb(0x8a, 0x90, 0x9c),
        }
    }

    /// Whether Windows can usually produce a real thumbnail (not just an icon).
    pub fn has_thumbnail(self) -> bool {
        matches!(self, Kind::Image | Kind::Video | Kind::Document)
    }
}

#[derive(Clone, Debug)]
pub struct FileEntry {
    pub id: i64,
    #[allow(dead_code)]
    pub source: i64,
    pub path: String,
    pub name: String,
    pub name_lc: String,
    pub ext: String,
    pub kind: Kind,
    pub size: u64,
    pub mtime: i64,
    /// Sorted tag ids.
    pub tags: Vec<i64>,
}

impl FileEntry {
    pub fn has_tag(&self, tag: i64) -> bool {
        self.tags.binary_search(&tag).is_ok()
    }
    pub fn add_tag(&mut self, tag: i64) -> bool {
        match self.tags.binary_search(&tag) {
            Ok(_) => false,
            Err(i) => {
                self.tags.insert(i, tag);
                true
            }
        }
    }
    pub fn remove_tag(&mut self, tag: i64) -> bool {
        match self.tags.binary_search(&tag) {
            Ok(i) => {
                self.tags.remove(i);
                true
            }
            Err(_) => false,
        }
    }
    pub fn folder(&self) -> &str {
        match self.path.rfind(['\\', '/']) {
            Some(i) => &self.path[..i],
            None => "",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Tag {
    pub id: i64,
    pub name: String,
    pub color: Color32,
}

#[derive(Clone, Debug)]
pub struct Source {
    pub id: i64,
    pub path: String,
}

impl Source {
    pub fn is_network(&self) -> bool {
        self.path.starts_with("\\\\") || self.path.starts_with("//")
    }
}

#[derive(Default)]
pub struct Snapshot {
    pub files: Vec<FileEntry>,
    pub tags: Vec<Tag>,
    pub sources: Vec<Source>,
    pub generation: u64,
}

pub fn color_to_u32(c: Color32) -> u32 {
    ((c.r() as u32) << 16) | ((c.g() as u32) << 8) | c.b() as u32
}

pub fn u32_to_color(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

pub const TAG_PALETTE: [Color32; 12] = [
    Color32::from_rgb(0x7c, 0x6c, 0xff),
    Color32::from_rgb(0x4c, 0xc9, 0xa4),
    Color32::from_rgb(0xf0, 0x6c, 0x7a),
    Color32::from_rgb(0xf5, 0xb1, 0x4c),
    Color32::from_rgb(0x5b, 0x9c, 0xf5),
    Color32::from_rgb(0xe8, 0x79, 0xd8),
    Color32::from_rgb(0x9a, 0xd9, 0x5b),
    Color32::from_rgb(0x4f, 0xd1, 0xe0),
    Color32::from_rgb(0xff, 0x8a, 0x4c),
    Color32::from_rgb(0xc4, 0xa7, 0xff),
    Color32::from_rgb(0xe0, 0xd2, 0x5a),
    Color32::from_rgb(0xa0, 0xa8, 0xb8),
];

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// Formats a unix timestamp in local time as `YYYY-MM-DD HH:MM`.
pub fn format_time(ts: i64) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_opt(ts, 0) {
        chrono::LocalResult::Single(t) if ts > 0 => t.format("%Y-%m-%d %H:%M").to_string(),
        _ => String::new(),
    }
}
