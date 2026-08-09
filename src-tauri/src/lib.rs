use tauri_plugin_dialog::DialogExt;
use serde::Serialize;
use regex::Regex;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::LazyLock;
use tauri::Emitter;

// --- Static Regex (compiled once) ---

static RJ_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)RJ\d+").unwrap());
static FANZA_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)d_\d+|[a-z]+[0-9]{3,}").unwrap());
static HTML_TAG_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").unwrap());

// DLSite HTML scraping regexes
static DLSITE_TITLE_CIRCLE_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<title>.*?\[(.*?)\]\s*\|\s*DLsite.*?</title>").unwrap());
static DLSITE_BRAND_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<span itemprop="brand" class="maker_name">\s*<a[^>]*>(.*?)</a>"#).unwrap());
static DLSITE_MAKER_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<span class="maker_name">\s*<a[^>]*>(.*?)</a>"#).unwrap());
static DLSITE_AUTHOR_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<th>作者</th>\s*<td>(.*?)</td>").unwrap());
static DLSITE_CV_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<th>声優</th>\s*<td>(.*?)</td>").unwrap());
static DLSITE_GENRE_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<th>ジャンル</th>\s*<td>(.*?)</td>").unwrap());
static DLSITE_SERIES_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<th>シリーズ名</th>\s*<td>(.*?)</td>").unwrap());

// Fanza HTML scraping regexes
static FANZA_TITLE_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<title>(.*?)\((.*?)\)｜FANZA同人</title>").unwrap());
static FANZA_TITLE_FALLBACK_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<title>(.*?)</title>").unwrap());
static FANZA_AUTHOR_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<dt>原画</dt>\s*<dd>(.*?)</dd>|<dt>著者</dt>\s*<dd>(.*?)</dd>|<dt>作者</dt>\s*<dd>(.*?)</dd>").unwrap());
static FANZA_DATE_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<dt>配信開始日</dt>\s*<dd>(.*?)</dd>").unwrap());
static FANZA_GENRE_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<dt>ジャンル</dt>\s*<dd>(.*?)</dd>").unwrap());
static FANZA_CV_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<dt>声優</dt>\s*<dd>(.*?)</dd>").unwrap());
static FANZA_SERIES_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<dt>シリーズ名</dt>\s*<dd>(.*?)</dd>").unwrap());

// --- Helper functions ---

fn strip_html_tags(s: &str) -> String {
    HTML_TAG_REGEX.replace_all(s, "").replace("\n", " ").trim().to_string()
}

// --- Structs ---

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
    zip_filename: String,
    file: String,
    current: usize,
    total: usize,
}

// --- Commands ---

#[tauri::command]
async fn select_directory(app_handle: tauri::AppHandle) -> Result<Option<String>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app_handle.dialog().file().pick_folder(move |folder| {
        let _ = tx.send(folder);
    });
    
    let result = rx.await.map_err(|e| e.to_string())?;
    if let Some(path) = result {
        Ok(Some(path.to_string()))
    } else {
        Ok(None)
    }
}

#[tauri::command]
async fn process_zip(app: tauri::AppHandle, file_path_str: String, output_base: String, template: String, duplicate_action: String, delete_original: bool) -> Result<ProcessResult, String> {
    let client = reqwest::Client::new();

    let base_path = Path::new(&output_base);
    if !base_path.exists() {
        fs::create_dir_all(base_path).map_err(|e| e.to_string())?;
    }
    let path = Path::new(&file_path_str);
    if !path.exists() {
        return Err("File does not exist".into());
    }
    
    let file_name = match path.file_name().and_then(|n| n.to_str()) {
        Some(name) => name,
        None => return Err("Invalid file name".into())
    };

    let mut matched_id = None;
    let mut circle = String::from("UnknownCircle");
    let mut author = String::from("UnknownAuthor");
    let mut title = String::from(file_name);
    let mut date = String::from("UnknownDate");
    let mut cv = String::from("");
    let mut genre = String::from("");
    let mut series = String::from("");
        
    // Try to find RJ code (DLSite)
    if let Some(caps) = RJ_REGEX.captures(file_name) {
        let rj_code = caps[0].to_uppercase();
        matched_id = Some(rj_code.clone());
        
        // Fetch metadata from DLSite API
        let api_url = format!("https://www.dlsite.com/maniax/product/info/ajax?product_id={}", rj_code);
        if let Ok(response) = client.get(&api_url).send().await {
            if let Ok(json) = response.json::<serde_json::Value>().await {
                let product_info = if let Some(arr) = json.get(&rj_code).and_then(|v| v.as_array()) {
                    arr.first()
                } else {
                    json.get(&rj_code)
                };
                
                if let Some(info) = product_info {
                    if let Some(maker) = info.get("maker_name").and_then(|m| m.as_str()) {
                        circle = maker.to_string();
                    }
                    if let Some(work) = info.get("work_name").and_then(|w| w.as_str()) {
                        title = work.to_string();
                    }
                    if let Some(reg_date) = info.get("regist_date").and_then(|d| d.as_str()) {
                        date = reg_date.split(' ').next().unwrap_or("UnknownDate").to_string();
                    }
                }
            }
        }
        
        // Fetch HTML from DLSite to get extra details
        let html_url = format!("https://www.dlsite.com/maniax/work/=/product_id/{}.html", rj_code);
        if let Ok(response) = client.get(&html_url).header("Cookie", "adultchecked=1").send().await {
            if let Ok(html) = response.text().await {
                if circle == "UnknownCircle" {
                    if let Some(circle_cap) = DLSITE_TITLE_CIRCLE_REGEX.captures(&html) {
                        circle = circle_cap[1].trim().to_string();
                    } else if let Some(circle_cap) = DLSITE_BRAND_REGEX.captures(&html) {
                        circle = circle_cap[1].trim().to_string();
                    } else if let Some(circle_cap) = DLSITE_MAKER_REGEX.captures(&html) {
                        circle = circle_cap[1].trim().to_string();
                    }
                }
                if let Some(author_cap) = DLSITE_AUTHOR_REGEX.captures(&html) {
                    author = strip_html_tags(&author_cap[1]);
                }
                if let Some(cv_cap) = DLSITE_CV_REGEX.captures(&html) {
                    cv = strip_html_tags(&cv_cap[1]);
                }
                if let Some(genre_cap) = DLSITE_GENRE_REGEX.captures(&html) {
                    genre = strip_html_tags(&genre_cap[1]);
                }
                if let Some(series_cap) = DLSITE_SERIES_REGEX.captures(&html) {
                    series = strip_html_tags(&series_cap[1]);
                }
            }
        }
    } 
    // Try Fanza if not DLSite
    else if let Some(caps) = FANZA_REGEX.captures(file_name) {
        let fanza_id = caps[0].to_lowercase();
        matched_id = Some(fanza_id.clone());
        
        // Fetch HTML from Fanza
        let url = format!("https://www.dmm.co.jp/dc/doujin/-/detail/=/cid={}/", fanza_id);
        if let Ok(response) = client.get(&url).header("Cookie", "age_check_done=1").send().await {
            if let Ok(html) = response.text().await {
                if let Some(caps) = FANZA_TITLE_REGEX.captures(&html) {
                    title = caps[1].trim().to_string();
                    circle = caps[2].trim().to_string();
                } else if let Some(title_cap) = FANZA_TITLE_FALLBACK_REGEX.captures(&html) {
                    let full_title = title_cap[1].to_string();
                    title = full_title.replace("｜FANZA同人", "").trim().to_string();
                }
                if let Some(author_cap) = FANZA_AUTHOR_REGEX.captures(&html) {
                    let extracted = author_cap.get(1).or(author_cap.get(2)).or(author_cap.get(3)).map_or("", |m| m.as_str());
                    author = strip_html_tags(extracted);
                }
                if let Some(date_cap) = FANZA_DATE_REGEX.captures(&html) {
                    date = date_cap[1].split(' ').next().unwrap_or("UnknownDate").replace("/", "-");
                }
                if let Some(genre_cap) = FANZA_GENRE_REGEX.captures(&html) {
                    genre = strip_html_tags(&genre_cap[1]);
                }
                if let Some(cv_cap) = FANZA_CV_REGEX.captures(&html) {
                    cv = strip_html_tags(&cv_cap[1]);
                }
                if let Some(series_cap) = FANZA_SERIES_REGEX.captures(&html) {
                    series = strip_html_tags(&series_cap[1]);
                }
            }
        }
    }
    
    let mut final_output_path = None;
    let mut final_title = None;

    if let Some(id) = matched_id {
        // Clean illegal characters for windows path
        let sanitize = |s: &str| s.replace(&['\\', '/', ':', '*', '?', '"', '<', '>', '|'][..], "_");
        let circle = sanitize(&circle);
        
        // Fallback author to circle if author is not explicitly defined
        let author = if author == "UnknownAuthor" { circle.clone() } else { sanitize(&author) };
        
        let title = sanitize(&title);
        let date = sanitize(&date);
        let cv = sanitize(&cv.replace("  ", " "));
        let genre = sanitize(&genre.replace("  ", " "));
        let series = sanitize(&series);
        
        // Build target path
        let mut target_folder_name = template.clone();
        target_folder_name = target_folder_name.replace("$AUTHOR", &author);
        target_folder_name = target_folder_name.replace("$CIRCLE", &circle);
        target_folder_name = target_folder_name.replace("$TITLE", &title);
        target_folder_name = target_folder_name.replace("$ID", &id);
        target_folder_name = target_folder_name.replace("$DATE", &date);
        target_folder_name = target_folder_name.replace("$CV", &cv);
        target_folder_name = target_folder_name.replace("$GENRE", &genre);
        target_folder_name = target_folder_name.replace("$SERIES", &series);
        
        let mut target_dir = base_path.join(&target_folder_name);
        
        // Handle duplicate
        if target_dir.exists() {
            if duplicate_action == "skip" {
                return Ok(ProcessResult {
                    success: true,
                    message: format!("スキップしました: {}", file_name),
                    output_path: None,
                    title: Some(title.clone()),
                    skipped: true,
                });
            } else {
                let mut counter = 2;
                loop {
                    let new_name = format!("{} ({})", target_folder_name, counter);
                    target_dir = base_path.join(&new_name);
                    if !target_dir.exists() {
                        break;
                    }
                    counter += 1;
                }
            }
        }
        
        fs::create_dir_all(&target_dir).map_err(|e| e.to_string())?;
        
        // Extract ZIP
        let file = fs::File::open(path).map_err(|e| e.to_string())?;
        let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
        
        let total_files = archive.len();
        for i in 0..total_files {
            let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
            
            // Shift-JIS decoding logic
            let raw_name = file.name_raw();
            let decoded_name = if let Ok(s) = std::str::from_utf8(raw_name) {
                s.to_string()
            } else {
                let (cow, _, _) = encoding_rs::SHIFT_JIS.decode(raw_name);
                cow.into_owned()
            };
            
            let lower_name = decoded_name.to_lowercase();
            if lower_name.contains("__macosx") || 
               lower_name.ends_with(".ds_store") || 
               lower_name.ends_with("thumbs.db") || 
               lower_name.ends_with("desktop.ini") {
                continue;
            }

            let _ = app.emit("extract-progress", ProgressPayload {
                zip_filename: file_name.to_string(),
                file: decoded_name.clone(),
                current: i + 1,
                total: total_files,
            });

            let outpath = target_dir.join(&decoded_name);

            if file.is_dir() {
                fs::create_dir_all(&outpath).map_err(|e| e.to_string())?;
            } else {
                if let Some(p) = outpath.parent() {
                    if !p.exists() {
                        fs::create_dir_all(p).map_err(|e| e.to_string())?;
                    }
                }
                let mut outfile = fs::File::create(&outpath).map_err(|e| e.to_string())?;
                io::copy(&mut file, &mut outfile).map_err(|e| e.to_string())?;
            }
        }
        
        final_output_path = Some(target_dir.to_string_lossy().into_owned());
        final_title = Some(title.clone());
    }
        
    // Delete original file if requested
    if delete_original {
        if let Err(e) = trash::delete(&file_path_str) {
            return Ok(ProcessResult {
                success: true,
                message: format!("Processed {} successfully, but failed to move original to recycle bin: {}", file_name, e),
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
}

#[tauri::command]
fn open_folder(path: String) {
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("explorer").arg(path.replace("/", "\\")).spawn();
    
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(path).spawn();
    
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![select_directory, process_zip, open_folder])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
