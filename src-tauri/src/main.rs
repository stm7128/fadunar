// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    fadunar_lib::run()
}

#[cfg(test)]
mod tests {
    use fadunar_lib::core::{
        build_target_relative_path, extract_work_id, reserve_target_dir, resolve_zip_entry_path,
        sanitize_path_segment, strip_html_tags, WorkId, WorkMetadata,
    };
    use std::fs;
    use std::path::{Path, PathBuf};

    #[test]
    fn extracts_work_ids_accurately() {
        assert_eq!(
            extract_work_id("[RJ123456] 作品.zip"),
            Some(WorkId::Dlsite("RJ123456".into()))
        );
        assert_eq!(
            extract_work_id("d_543210_archive.zip"),
            Some(WorkId::Fanza("d_543210".into()))
        );
        assert_eq!(
            extract_work_id("abcd001.zip"),
            Some(WorkId::Fanza("abcd001".into()))
        );
        assert_eq!(extract_work_id("backup2026.zip"), None);
        assert_eq!(extract_work_id("photo001.zip"), None);
        assert_eq!(extract_work_id("plain_file.zip"), None);
    }

    #[test]
    fn decodes_html_entities_and_strips_tags() {
        let raw = "<a>Author &amp; Co.</a>\n<span>&#039;Test&#x21;</span>";
        assert_eq!(strip_html_tags(raw), "Author & Co. 'Test!");
    }

    #[test]
    fn sanitizes_windows_path_segments() {
        assert_eq!(sanitize_path_segment("CON"), "_CON");
        assert_eq!(sanitize_path_segment("nul.txt"), "_nul.txt");
        assert_eq!(
            sanitize_path_segment("a:b/c\\d*e?f\"g<h>i|j.. "),
            "a_b_c_d_e_f_g_h_i_j"
        );
        assert_eq!(sanitize_path_segment(".."), "_");
    }

    #[test]
    fn template_skips_empty_segments_and_blocks_traversal() {
        let meta = WorkMetadata {
            circle: "Circle".into(),
            author: "$TITLE".into(),
            title: "MyWork".into(),
            date: "2026-01-01".into(),
            cv: "".into(),
            genre: "".into(),
            series: "".into(),
        };

        let rel = build_target_relative_path("$SERIES/$TITLE", "RJ123", &meta).unwrap();
        assert_eq!(rel, PathBuf::from("MyWork"));

        let rel_no_reexpand =
            build_target_relative_path("$AUTHOR/$TITLE", "RJ123", &meta).unwrap();
        assert_eq!(rel_no_reexpand, PathBuf::from("$TITLE").join("MyWork"));

        assert!(build_target_relative_path("../$TITLE", "RJ123", &meta).is_err());
        assert!(build_target_relative_path("C:/$TITLE", "RJ123", &meta).is_err());
    }

    #[test]
    fn blocks_zip_slip_entries() {
        let target = Path::new("C:\\output\\work");
        assert!(resolve_zip_entry_path(target, "../evil.txt").is_err());
        assert!(resolve_zip_entry_path(target, "a/../../evil.txt").is_err());
        assert!(resolve_zip_entry_path(target, "/etc/passwd").is_err());
        assert!(resolve_zip_entry_path(target, "C:\\evil.txt").is_err());
        assert!(resolve_zip_entry_path(target, "sub\\..\\..\\evil.txt").is_err());

        assert_eq!(
            resolve_zip_entry_path(target, "./audio/track01.wav").unwrap(),
            Some(target.join("audio").join("track01.wav"))
        );
        assert_eq!(resolve_zip_entry_path(target, ".").unwrap(), None);
    }

    #[test]
    fn reserves_unique_directories_under_skip_and_rename() {
        let temp_root = std::env::temp_dir().join(format!(
            "fadunar_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&temp_root).unwrap();

        let rel = Path::new("Circle").join("Work");
        let first = reserve_target_dir(&temp_root, &rel, "rename").unwrap();
        assert_eq!(first, Some(temp_root.join("Circle").join("Work")));

        let skipped = reserve_target_dir(&temp_root, &rel, "skip").unwrap();
        assert_eq!(skipped, None);

        let second = reserve_target_dir(&temp_root, &rel, "rename").unwrap();
        assert_eq!(second, Some(temp_root.join("Circle").join("Work (2)")));

        let _ = fs::remove_dir_all(&temp_root);
    }
}
