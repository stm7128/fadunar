pub mod core;

use core::{
    build_target_relative_path, decode_html_entities, extract_work_id, reserve_target_dir,
    resolve_zip_entry_path, sanitize_path_segment, strip_html_tags, WorkId, WorkMetadata,
};
use regex::Regex;
use serde::Serialize;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use tauri::Emitter;
use tauri_plugin_dialog::DialogExt;

// DLSite HTML scraping regexes
static DLSITE_TITLE_CIRCLE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<title>.*?\[(.*?)\]\s*\|\s*DLsite.*?</title>").unwrap());
static DLSITE_BRAND_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<span itemprop="brand" class="maker_name">\s*<a[^>]*>(.*?)</a>"#).unwrap()
});
static DLSITE_MAKER_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?s)<span class="maker_name">\s*<a[^>]*>(.*?)</a>"#).unwrap());
static DLSITE_AUTHOR_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<th>作者</th>\s*<td>(.*?)</td>").unwrap());
static DLSITE_CV_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<th>声優</th>\s*<td>(.*?)</td>").unwrap());
static DLSITE_GENRE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<th>ジャンル</th>\s*<td>(.*?)</td>").unwrap());
static DLSITE_SERIES_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<th>シリーズ名</th>\s*<td>(.*?)</td>").unwrap());

// Fanza HTML scraping regexes
static FANZA_TITLE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<title>(.*?)\((.*?)\)｜FANZA同人</title>").unwrap());
static FANZA_TITLE_FALLBACK_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<title>(.*?)</title>").unwrap());
static FANZA_AUTHOR_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)<dt>原画</dt>\s*<dd>(.*?)</dd>|<dt>著者</dt>\s*<dd>(.*?)</dd>|<dt>作者</dt>\s*<dd>(.*?)</dd>").unwrap()
});
static FANZA_DATE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<dt>配信開始日</dt>\s*<dd>(.*?)</dd>").unwrap());
static FANZA_GENRE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<dt>ジャンル</dt>\s*<dd>(.*?)</dd>").unwrap());
static FANZA_CV_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<dt>声優</dt>\s*<dd>(.*?)</dd>").unwrap());
static FANZA_SERIES_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<dt>シリーズ名</dt>\s*<dd>(.*?)</dd>").unwrap());

// --- IPC Payloads ---

#[derive(Serialize)]
struct ProcessResult {
    success: bool,
    message: String,
    output_path: Option<String>,
    title: Option<String>,
    skipped: bool,
}

#[derive(Clone, Serialize)]
struct ProgressPayload {
    task_id: String,
    zip_filename: String,
    file: String,
    current: usize,
    total: usize,
}

#[derive(Serialize)]
struct DropClassification {
    valid: Vec<String>,
    ignored_non_zip: usize,
    ignored_unknown_id: usize,
}

// --- ZIP Extraction ---

fn extract_zip_to_dir(
    app: &tauri::AppHandle,
    task_id: &str,
    zip_path: &Path,
    file_name: &str,
    target_dir: &Path,
) -> Result<(), String> {
    let file = fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;

    let total_files = archive.len();
    let mut last_emit = Instant::now();
    for i in 0..total_files {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;

        // Shift-JIS decoding fallback for Japanese ZIP archives
        let raw_name = entry.name_raw();
        let decoded_name = if let Ok(s) = std::str::from_utf8(raw_name) {
            s.to_string()
        } else {
            let (cow, _, _) = encoding_rs::SHIFT_JIS.decode(raw_name);
            cow.into_owned()
        };

        let lower_name = decoded_name.to_lowercase();
        if lower_name.contains("__macosx")
            || lower_name.ends_with(".ds_store")
            || lower_name.ends_with("thumbs.db")
            || lower_name.ends_with("desktop.ini")
        {
            continue;
        }

        let Some(outpath) = resolve_zip_entry_path(target_dir, &decoded_name)? else {
            continue;
        };

        let is_boundary = i == 0 || i + 1 == total_files;
        if is_boundary || last_emit.elapsed() >= Duration::from_millis(80) {
            let _ = app.emit(
                "extract-progress",
                ProgressPayload {
                    task_id: task_id.to_string(),
                    zip_filename: file_name.to_string(),
                    file: decoded_name,
                    current: i + 1,
                    total: total_files,
                },
            );
            last_emit = Instant::now();
        }

        if entry.is_dir() {
            fs::create_dir_all(&outpath).map_err(|e| e.to_string())?;
        } else {
            if let Some(p) = outpath.parent() {
                if !p.exists() {
                    fs::create_dir_all(p).map_err(|e| e.to_string())?;
                }
            }
            let mut outfile = fs::File::create(&outpath).map_err(|e| e.to_string())?;
            io::copy(&mut entry, &mut outfile).map_err(|e| e.to_string())?;
        }
    }

    Ok(())
}

// --- Metadata Fetchers ---

async fn fetch_dlsite_metadata(
    client: &reqwest::Client,
    rj_code: &str,
    default_title: &str,
) -> WorkMetadata {
    let mut meta = WorkMetadata::new(default_title);

    let api_url = format!(
        "https://www.dlsite.com/maniax/product/info/ajax?product_id={}",
        rj_code
    );
    if let Ok(response) = client.get(&api_url).send().await {
        if response.status().is_success() {
            if let Ok(json) = response.json::<serde_json::Value>().await {
                let product_info =
                    if let Some(arr) = json.get(rj_code).and_then(|v| v.as_array()) {
                        arr.first()
                    } else {
                        json.get(rj_code)
                    };

                if let Some(info) = product_info {
                    if let Some(maker) = info.get("maker_name").and_then(|m| m.as_str()) {
                        meta.circle = decode_html_entities(maker).trim().to_string();
                    }
                    if let Some(work) = info.get("work_name").and_then(|w| w.as_str()) {
                        meta.title = decode_html_entities(work).trim().to_string();
                    }
                    if let Some(reg_date) = info.get("regist_date").and_then(|d| d.as_str()) {
                        meta.date = reg_date.split(' ').next().unwrap_or("UnknownDate").to_string();
                    }
                }
            }
        }
    }

    let html_url = format!(
        "https://www.dlsite.com/maniax/work/=/product_id/{}.html",
        rj_code
    );
    if let Ok(response) = client
        .get(&html_url)
        .header("Cookie", "adultchecked=1")
        .send()
        .await
    {
        if response.status().is_success() {
            if let Ok(html) = response.text().await {
                if meta.circle == "UnknownCircle" {
                    if let Some(cap) = DLSITE_TITLE_CIRCLE_REGEX.captures(&html) {
                        meta.circle = strip_html_tags(&cap[1]);
                    } else if let Some(cap) = DLSITE_BRAND_REGEX.captures(&html) {
                        meta.circle = strip_html_tags(&cap[1]);
                    } else if let Some(cap) = DLSITE_MAKER_REGEX.captures(&html) {
                        meta.circle = strip_html_tags(&cap[1]);
                    }
                }
                if let Some(cap) = DLSITE_AUTHOR_REGEX.captures(&html) {
                    let parsed = strip_html_tags(&cap[1]);
                    if !parsed.is_empty() {
                        meta.author = parsed;
                    }
                }
                if let Some(cap) = DLSITE_CV_REGEX.captures(&html) {
                    meta.cv = strip_html_tags(&cap[1]);
                }
                if let Some(cap) = DLSITE_GENRE_REGEX.captures(&html) {
                    meta.genre = strip_html_tags(&cap[1]);
                }
                if let Some(cap) = DLSITE_SERIES_REGEX.captures(&html) {
                    meta.series = strip_html_tags(&cap[1]);
                }
            }
        }
    }

    meta
}

async fn fetch_fanza_metadata(
    client: &reqwest::Client,
    fanza_id: &str,
    default_title: &str,
) -> WorkMetadata {
    let mut meta = WorkMetadata::new(default_title);

    let url = format!("https://www.dmm.co.jp/dc/doujin/-/detail/=/cid={}/", fanza_id);
    if let Ok(response) = client
        .get(&url)
        .header("Cookie", "age_check_done=1")
        .send()
        .await
    {
        if response.status().is_success() {
            if let Ok(html) = response.text().await {
                if let Some(caps) = FANZA_TITLE_REGEX.captures(&html) {
                    meta.title = strip_html_tags(&caps[1]);
                    meta.circle = strip_html_tags(&caps[2]);
                } else if let Some(cap) = FANZA_TITLE_FALLBACK_REGEX.captures(&html) {
                    let full_title = strip_html_tags(&cap[1]);
                    let cleaned = full_title.replace("｜FANZA同人", "").trim().to_string();
                    if !cleaned.is_empty() {
                        meta.title = cleaned;
                    }
                }
                if let Some(cap) = FANZA_AUTHOR_REGEX.captures(&html) {
                    let extracted = cap
                        .get(1)
                        .or(cap.get(2))
                        .or(cap.get(3))
                        .map_or("", |m| m.as_str());
                    let parsed = strip_html_tags(extracted);
                    if !parsed.is_empty() {
                        meta.author = parsed;
                    }
                }
                if let Some(cap) = FANZA_DATE_REGEX.captures(&html) {
                    let parsed = strip_html_tags(&cap[1]);
                    if let Some(date_part) = parsed.split(' ').next() {
                        if !date_part.is_empty() {
                            meta.date = date_part.replace('/', "-");
                        }
                    }
                }
                if let Some(cap) = FANZA_GENRE_REGEX.captures(&html) {
                    meta.genre = strip_html_tags(&cap[1]);
                }
                if let Some(cap) = FANZA_CV_REGEX.captures(&html) {
                    meta.cv = strip_html_tags(&cap[1]);
                }
                if let Some(cap) = FANZA_SERIES_REGEX.captures(&html) {
                    meta.series = strip_html_tags(&cap[1]);
                }
            }
        }
    }

    meta
}

// --- Commands ---

#[tauri::command]
async fn select_directory(app_handle: tauri::AppHandle) -> Result<Option<String>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app_handle.dialog().file().pick_folder(move |folder| {
        let _ = tx.send(folder);
    });

    let result = rx.await.map_err(|e| e.to_string())?;
    Ok(result.map(|path| path.to_string()))
}

#[tauri::command]
fn classify_dropped_files(paths: Vec<String>) -> DropClassification {
    let mut valid = Vec::new();
    let mut ignored_non_zip = 0usize;
    let mut ignored_unknown_id = 0usize;

    for path_str in paths {
        if !path_str.to_lowercase().ends_with(".zip") {
            ignored_non_zip += 1;
            continue;
        }

        let file_name = Path::new(&path_str)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        if extract_work_id(file_name).is_some() {
            valid.push(path_str);
        } else {
            ignored_unknown_id += 1;
        }
    }

    DropClassification {
        valid,
        ignored_non_zip,
        ignored_unknown_id,
    }
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn process_zip(
    app: tauri::AppHandle,
    client: tauri::State<'_, reqwest::Client>,
    task_id: String,
    file_path_str: String,
    output_base: String,
    template: String,
    duplicate_action: String,
    delete_original: bool,
) -> Result<ProcessResult, String> {
    if output_base.trim().is_empty() {
        return Err("デフォルト保存先フォルダが設定されていません".into());
    }

    let base_path = PathBuf::from(&output_base);
    let zip_path = PathBuf::from(&file_path_str);
    if !zip_path.exists() {
        return Err("File does not exist".into());
    }

    let file_name = zip_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| String::from("Invalid file name"))?
        .to_string();

    let work_id = extract_work_id(&file_name)
        .ok_or_else(|| format!("非対応のZIPファイルです (作品IDを検出できませんでした): {}", file_name))?;

    let meta = match &work_id {
        WorkId::Dlsite(rj_code) => fetch_dlsite_metadata(client.inner(), rj_code, &file_name).await,
        WorkId::Fanza(fanza_id) => fetch_fanza_metadata(client.inner(), fanza_id, &file_name).await,
    };

    let rel_path = build_target_relative_path(&template, work_id.as_str(), &meta)?;
    let sanitized_title = sanitize_path_segment(&meta.title);
    let display_title = if sanitized_title.is_empty() {
        work_id.as_str().to_string()
    } else {
        sanitized_title
    };

    tokio::task::spawn_blocking(move || -> Result<ProcessResult, String> {
        if !base_path.exists() {
            fs::create_dir_all(&base_path).map_err(|e| e.to_string())?;
        }

        let Some(target_dir) = reserve_target_dir(&base_path, &rel_path, &duplicate_action)? else {
            return Ok(ProcessResult {
                success: true,
                message: format!("スキップしました: {}", file_name),
                output_path: None,
                title: Some(display_title),
                skipped: true,
            });
        };

        if let Err(err) = extract_zip_to_dir(&app, &task_id, &zip_path, &file_name, &target_dir) {
            let _ = fs::remove_dir_all(&target_dir);
            return Err(err);
        }

        let final_output_path = Some(target_dir.to_string_lossy().into_owned());
        let final_title = Some(display_title);

        if delete_original {
            if let Err(e) = trash::delete(&file_path_str) {
                return Ok(ProcessResult {
                    success: true,
                    message: format!(
                        "Processed {} successfully, but failed to move original to recycle bin: {}",
                        file_name, e
                    ),
                    output_path: final_output_path,
                    title: final_title,
                    skipped: false,
                });
            }
        }

        Ok(ProcessResult {
            success: true,
            message: format!("Processed {} successfully.", file_name),
            output_path: final_output_path,
            title: final_title,
            skipped: false,
        })
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?
}

#[tauri::command]
fn open_folder(path: String) -> Result<(), String> {
    let dir_path = Path::new(&path);
    if !dir_path.is_dir() {
        return Err("指定されたパスは有効なフォルダではありません".into());
    }

    #[cfg(target_os = "windows")]
    std::process::Command::new("explorer")
        .arg(path.replace('/', "\\"))
        .spawn()
        .map_err(|e| e.to_string())?;

    #[cfg(target_os = "macos")]
    std::process::Command::new("open")
        .arg(&path)
        .spawn()
        .map_err(|e| e.to_string())?;

    #[cfg(target_os = "linux")]
    std::process::Command::new("xdg-open")
        .arg(&path)
        .spawn()
        .map_err(|e| e.to_string())?;

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    tauri::Builder::default()
        .manage(http_client)
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            select_directory,
            classify_dropped_files,
            process_zip,
            open_folder
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
