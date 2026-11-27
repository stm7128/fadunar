use regex::Regex;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{LazyLock, Mutex};

static RJ_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:^|[^a-z0-9])(RJ\d+)(?:$|[^a-z0-9])").unwrap());
static FANZA_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])(d_\d+|[a-z]{1,5}[0-9]{3,5})(?:$|[^a-z0-9])").unwrap()
});
static HTML_TAG_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").unwrap());

static DIR_CREATE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

const NON_FANZA_PREFIXES: &[&str] = &[
    "track", "part", "disc", "disk", "vol", "ver", "img", "pic", "photo", "file", "copy", "audio",
    "voice", "bgm", "test", "demo", "trial", "extra", "bonus", "data", "page", "scene",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkId {
    Dlsite(String),
    Fanza(String),
}

impl WorkId {
    pub fn as_str(&self) -> &str {
        match self {
            WorkId::Dlsite(id) | WorkId::Fanza(id) => id,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorkMetadata {
    pub circle: String,
    pub author: String,
    pub title: String,
    pub date: String,
    pub cv: String,
    pub genre: String,
    pub series: String,
}

impl WorkMetadata {
    pub fn new(default_title: &str) -> Self {
        Self {
            circle: String::from("UnknownCircle"),
            author: String::from("UnknownAuthor"),
            title: String::from(default_title),
            date: String::from("UnknownDate"),
            cv: String::new(),
            genre: String::new(),
            series: String::new(),
        }
    }
}

pub fn extract_work_id(file_name: &str) -> Option<WorkId> {
    if let Some(caps) = RJ_REGEX.captures(file_name) {
        return Some(WorkId::Dlsite(caps[1].to_uppercase()));
    }

    if let Some(caps) = FANZA_REGEX.captures(file_name) {
        let candidate = caps[1].to_lowercase();
        if !candidate.starts_with("d_") {
            let alpha_prefix: String = candidate
                .chars()
                .take_while(|c| c.is_ascii_alphabetic())
                .collect();
            if NON_FANZA_PREFIXES.contains(&alpha_prefix.as_str()) {
                return None;
            }
        }
        return Some(WorkId::Fanza(candidate));
    }

    None
}

pub fn decode_html_entities(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(amp_idx) = rest.find('&') {
        out.push_str(&rest[..amp_idx]);
        let after_amp = &rest[amp_idx..];

        if let Some(semi_idx) = after_amp.find(';') {
            if semi_idx <= 12 {
                let entity = &after_amp[1..semi_idx];
                let decoded = match entity {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    "nbsp" => Some(' '),
                    _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                        u32::from_str_radix(&entity[2..], 16)
                            .ok()
                            .and_then(char::from_u32)
                    }
                    _ if entity.starts_with('#') => {
                        entity[1..].parse::<u32>().ok().and_then(char::from_u32)
                    }
                    _ => None,
                };

                if let Some(ch) = decoded {
                    out.push(ch);
                    rest = &after_amp[semi_idx + 1..];
                    continue;
                }
            }
        }

        out.push('&');
        rest = &after_amp[1..];
    }

    out.push_str(rest);
    out
}

pub fn strip_html_tags(s: &str) -> String {
    let without_tags = HTML_TAG_REGEX.replace_all(s, "");
    let decoded = decode_html_entities(&without_tags);
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_windows_reserved_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

pub fn sanitize_path_segment(s: &str) -> String {
    let mut cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_control()
                || matches!(
                    c,
                    '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
                )
            {
                '_'
            } else {
                c
            }
        })
        .collect();

    cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");

    if cleaned == "." || cleaned == ".." {
        return String::from("_");
    }

    loop {
        let trimmed = cleaned.trim().trim_end_matches('.');
        if trimmed.len() == cleaned.len() {
            cleaned = trimmed.to_string();
            break;
        }
        cleaned = trimmed.to_string();
    }

    if cleaned.is_empty() {
        return String::new();
    }

    if cleaned.chars().count() > 120 {
        cleaned = cleaned.chars().take(120).collect::<String>();
        cleaned = cleaned.trim().trim_end_matches('.').trim().to_string();
    }

    if cleaned.is_empty() {
        return String::new();
    }

    if is_windows_reserved_name(&cleaned) {
        cleaned.insert(0, '_');
    }

    cleaned
}

fn expand_template_segment(raw_segment: &str, id: &str, meta: &WorkMetadata) -> String {
    let author_val = if meta.author == "UnknownAuthor" {
        &meta.circle
    } else {
        &meta.author
    };

    let tokens: [(&str, &str); 8] = [
        ("$AUTHOR", author_val),
        ("$CIRCLE", &meta.circle),
        ("$TITLE", &meta.title),
        ("$SERIES", &meta.series),
        ("$GENRE", &meta.genre),
        ("$DATE", &meta.date),
        ("$CV", &meta.cv),
        ("$ID", id),
    ];

    let mut out = String::with_capacity(raw_segment.len());
    let mut rest = raw_segment;

    while let Some(dollar_idx) = rest.find('$') {
        out.push_str(&rest[..dollar_idx]);
        let after_dollar = &rest[dollar_idx..];

        if let Some((token, value)) = tokens
            .iter()
            .find(|(token, _)| after_dollar.starts_with(token))
        {
            out.push_str(value);
            rest = &after_dollar[token.len()..];
        } else {
            out.push('$');
            rest = &after_dollar[1..];
        }
    }

    out.push_str(rest);
    out
}

pub fn build_target_relative_path(
    template: &str,
    id: &str,
    meta: &WorkMetadata,
) -> Result<PathBuf, String> {
    let mut rel_path = PathBuf::new();

    for raw_seg in template.split(['/', '\\']) {
        let trimmed_raw = raw_seg.trim();
        if trimmed_raw == ".." || trimmed_raw.contains(':') {
            return Err("命名規則に無効なパス指定 ('..' や ':') は使用できません".into());
        }
        if trimmed_raw.is_empty() || trimmed_raw == "." {
            continue;
        }

        let expanded = expand_template_segment(trimmed_raw, id, meta);
        let sanitized = sanitize_path_segment(&expanded);
        if !sanitized.is_empty() {
            rel_path.push(sanitized);
        }
    }

    if rel_path.as_os_str().is_empty() {
        let fallback = sanitize_path_segment(id);
        rel_path.push(if fallback.is_empty() {
            String::from("Work")
        } else {
            fallback
        });
    }

    Ok(rel_path)
}

pub fn resolve_zip_entry_path(
    target_dir: &Path,
    entry_name: &str,
) -> Result<Option<PathBuf>, String> {
    if entry_name.contains('\0') {
        return Err("ZIPエントリ名に不正なNULL文字が含まれています".into());
    }

    for raw_seg in entry_name.split(['/', '\\']) {
        if raw_seg == ".." || raw_seg.contains(':') {
            return Err(format!(
                "不正なパスを含むZIPエントリを検出しました: {}",
                entry_name
            ));
        }
    }

    let mut rel = PathBuf::new();
    for component in Path::new(entry_name).components() {
        match component {
            Component::Normal(part) => rel.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "不正なパスを含むZIPエントリを検出しました: {}",
                    entry_name
                ));
            }
        }
    }

    if rel.as_os_str().is_empty() {
        return Ok(None);
    }

    let full_path = target_dir.join(&rel);
    if !full_path.starts_with(target_dir) {
        return Err(format!(
            "出力先ディレクトリ外への書き込みを検出しました: {}",
            entry_name
        ));
    }

    Ok(Some(full_path))
}

pub fn reserve_target_dir(
    base_path: &Path,
    rel_path: &Path,
    duplicate_action: &str,
) -> Result<Option<PathBuf>, String> {
    let _guard = DIR_CREATE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let mut target_dir = base_path.join(rel_path);

    if target_dir.exists() {
        if duplicate_action == "skip" {
            return Ok(None);
        }

        let parent = target_dir.parent().unwrap_or(base_path).to_path_buf();
        let leaf = target_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Work")
            .to_string();

        let mut counter = 2usize;
        loop {
            let candidate = parent.join(format!("{} ({})", leaf, counter));
            if !candidate.exists() {
                target_dir = candidate;
                break;
            }
            counter += 1;
        }
    }

    fs::create_dir_all(&target_dir).map_err(|e| e.to_string())?;
    Ok(Some(target_dir))
}
