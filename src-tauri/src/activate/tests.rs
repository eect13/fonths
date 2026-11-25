//! Extracted activate module unit & architectural contract tests.

#[allow(unused_imports)]
use super::*;

#[cfg(test)]
pub(super) fn all_activate_prod_sources() -> String {
    [
        include_str!("mod.rs"),
        include_str!("winfont.rs"),
        include_str!("session.rs"),
        include_str!("cdn_sync.rs"),
        include_str!("disk_scan.rs"),
        include_str!("downloads.rs"),
    ]
    .join("\n")
    .replace('\r', "")
}

#[cfg(test)]
pub(super) fn all_activate_sources() -> String {
    [all_activate_prod_sources(), include_str!("tests.rs").replace('\r', "")].join("\n")
}

#[cfg(test)]
mod complete_marker_tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_family_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "fm-complete-{}-{}-{}",
            label,
            std::process::id(),
            nanos
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn verify_clears_legacy_bare_complete() {
        let dir = temp_family_dir("bare");
        fs::write(dir.join(".complete"), b"1").unwrap();
        verify_complete_marker(&dir);
        assert!(
            !family_complete_marker(&dir).is_file(),
            "legacy bare .complete \"1\" must be cleared"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_clears_complete_when_expected_missing() {
        let dir = temp_family_dir("empty");
        fs::write(dir.join(".complete"), b"").unwrap();
        verify_complete_marker(&dir);
        assert!(!family_complete_marker(&dir).is_file());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_keeps_expected_aware_stamp_when_faces_match() {
        let dir = temp_family_dir("ok");
        fs::write(dir.join(".expected"), b"1").unwrap();
        fs::write(dir.join(".complete"), b"1").unwrap();
        // Minimal intact TTF: magic + pad to >= 256 bytes.
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(256, 0);
        fs::write(dir.join("Regular.ttf"), &fake).unwrap();
        verify_complete_marker(&dir);
        assert!(
            family_complete_marker(&dir).is_file(),
            "honest .expected=1 stamp must remain"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_keeps_multiface_complete_body_without_sidecar() {
        let dir = temp_family_dir("multi");
        fs::write(dir.join(".complete"), b"2").unwrap();
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(256, 0);
        fs::write(dir.join("Regular.ttf"), &fake).unwrap();
        fs::write(dir.join("Bold.ttf"), &fake).unwrap();
        verify_complete_marker(&dir);
        assert!(
            family_complete_marker(&dir).is_file(),
            "expected-aware .complete body >1 must remain when faces match"
        );
        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    fn verify_clears_google_latin_pack_without_planned() {
        // Libre Baskerville latin lie: .complete=.expected=4, only *-latin-*, no .google-planned.
        let parent = temp_family_dir("libre-lie-parent");
        let dir = parent.join("Libre Baskerville");
        fs::create_dir_all(&dir).unwrap();
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(256, 0);
        for name in [
            "libre-baskerville-latin-400-normal.ttf",
            "libre-baskerville-latin-400-italic.ttf",
            "libre-baskerville-latin-700-normal.ttf",
            "libre-baskerville-latin-700-italic.ttf",
        ] {
            fs::write(dir.join(name), &fake).unwrap();
        }
        fs::write(dir.join(".expected"), b"4").unwrap();
        fs::write(dir.join(".complete"), b"4").unwrap();
        fs::write(dir.join(".fontsource-version"), b"5.0.0").unwrap();
        assert!(is_official_google_family("Libre Baskerville"));
        assert!(google_catalog_face_floor("Libre Baskerville").unwrap_or(0) >= 8);
        assert_eq!(count_intact_faces(&dir), 4);
        // Old verify would keep this (4 intact >= .expected=4). New rule must clear.
        verify_complete_marker(&dir);
        assert!(
            !family_complete_marker(&dir).is_file(),
            "Fontsource latin pack without .google-planned must not stay complete"
        );
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn verify_keeps_honest_google_planned_at_catalog_floor() {
        let parent = temp_family_dir("libre-ok-parent");
        let dir = parent.join("Libre Baskerville");
        fs::create_dir_all(&dir).unwrap();
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(256, 0);
        let floor = google_catalog_face_floor("Libre Baskerville").expect("catalog floor");
        let weights = [400, 500, 600, 700];
        let mut keys = Vec::new();
        for w in weights {
            for style in ["normal", "italic"] {
                let name = format!("libre-baskerville-{w}-{style}.ttf");
                fs::write(dir.join(&name), &fake).unwrap();
                keys.push(name);
            }
        }
        assert_eq!(keys.len(), floor);
        fs::write(dir.join(".google-planned"), keys.join("\n").as_bytes()).unwrap();
        fs::write(dir.join(".expected"), floor.to_string().as_bytes()).unwrap();
        fs::write(dir.join(".complete"), floor.to_string().as_bytes()).unwrap();
        verify_complete_marker(&dir);
        assert!(
            family_complete_marker(&dir).is_file(),
            "honest Google keys + .google-planned at catalog floor must remain"
        );
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn verify_keeps_google_planned_below_catalog_floor() {
        // Sofia Sans: catalog floor (18) vs a thinner CSS-shaped plan. Must NOT clear as latin lie.
        let parent = temp_family_dir("sofia-planned-below-floor");
        let dir = parent.join("Sofia Sans");
        fs::create_dir_all(&dir).unwrap();
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(256, 0);
        let floor = google_catalog_face_floor("Sofia Sans").expect("catalog floor");
        // Catalog is 9 weights × italic = 18 (1/1000 edges dropped upstream).
        assert!(floor >= 18, "Sofia Sans floor should cover 100..900 × italic, got {floor}");
        // CSS-shaped plan thinner than catalog floor (omit 100/900).
        let weights = [200, 300, 400, 500, 600, 700, 800];
        let mut keys = Vec::new();
        for w in weights {
            for style in ["normal", "italic"] {
                let name = format!("sofia-sans-{w}-{style}.ttf");
                fs::write(dir.join(&name), &fake).unwrap();
                keys.push(name);
            }
        }
        assert!(keys.len() < floor, "planned must be below catalog floor ({}/{})", keys.len(), floor);
        fs::write(dir.join(".google-planned"), keys.join("\n").as_bytes()).unwrap();
        fs::write(dir.join(".expected"), keys.len().to_string().as_bytes()).unwrap();
        fs::write(dir.join(".complete"), keys.len().to_string().as_bytes()).unwrap();
        assert!(
            !official_google_complete_is_lie(&dir),
            "planned key list below floor must not be treated as a lie"
        );
        verify_complete_marker(&dir);
        assert!(
            family_complete_marker(&dir).is_file(),
            "honest .google-planned below catalog floor must remain complete"
        );
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn catalog_floor_roboto_and_cormorant() {
        assert_eq!(google_catalog_face_floor("Roboto"), Some(18));
        assert_eq!(google_catalog_face_floor("Cormorant"), Some(10));
        assert_eq!(google_catalog_face_floor("libre-baskerville"), Some(8));
    }

    #[test]
    fn fs_only_google_vf_folder_maps_42dot_and_skips_material() {
        assert_eq!(fs_only_google_vf_folder("42dot Sans"), Some("42dotsans".into()));
        assert_eq!(fs_only_google_vf_folder("Finlandica Text"), Some("finlandicatext".into()));
        assert_eq!(fs_only_google_vf_folder("Finlandica Headline"), Some("finlandicaheadline".into()));
        // Plain "Finlandica" has no fixed folder; it maps only when the shipped
        // fontsource-other catalog marks it variable (it does as of 1.0.207).
        assert_eq!(
            fs_only_google_vf_folder("Finlandica"),
            fontsource_other_is_variable("Finlandica").then(|| "finlandica".to_string())
        );
        assert_eq!(
            fs_only_google_vf_folder("Big Shoulders Display"),
            Some("bigshouldersdisplay".into())
        );
        assert!(fs_only_google_vf_folder("Material Symbols Outlined").is_none());
        assert!(family_ensures_google_vf("42dot Sans"));
        assert!(!family_ensures_google_vf("Material Symbols Outlined"));
    }

    #[test]
    fn family_ensures_drives_backfill_gate_not_google_catalog_alone() {
        assert!(family_ensures_google_vf("42dot Sans"));
        assert!(!google_catalog_is_variable("42dot Sans"));
        assert!(fs_only_google_vf_folder("42dot Sans").is_some());
        assert!(family_expects_dual_variable("Finlandica Text"));
        assert!(family_expects_dual_variable("Finlandica Headline"));
        assert!(!family_expects_dual_variable("Finlandica"));
        assert!(!family_expects_dual_variable("42dot Sans"));
    }

    #[test]
    fn is_variable_face_filename_accepts_dest_and_google_originals() {
        assert!(is_variable_face_filename("nunito-variable-wght.ttf"));
        assert!(is_variable_face_filename("Nunito-VariableFont_wght.ttf"));
        assert!(is_variable_face_filename("Nunito[wght].ttf"));
        assert!(is_variable_face_filename("Nunito-Italic[wght].ttf"));
        assert!(!is_variable_face_filename("nunito-400-normal.ttf"));
        assert!(!is_variable_face_filename("roboto-700-italic.ttf"));
    }

    #[test]
    fn catalog_variable_axis_specs_use_weight_span() {
        assert!(google_catalog_is_variable("Roboto"));
        assert!(google_catalog_is_variable("Libre Baskerville"));
        assert!(google_catalog_is_variable("Chiron Sung HK"));
        let roboto = variable_axis_specs("Roboto");
        assert!(
            roboto.iter().any(|a| a.contains("100..900")),
            "Roboto variable axes must span 100..900: {roboto:?}"
        );
        let libre = variable_axis_specs("Libre Baskerville");
        assert!(
            libre.iter().any(|a| a.contains("400..700")),
            "Libre Baskerville variable axes must span catalog weights: {libre:?}"
        );
        assert!(listing_is_400_swept(&[
            ("normal".into(), "400".into(), "https://x/a.ttf".into())
        ]));
        assert!(!listing_is_400_swept(&[
            ("normal".into(), "400".into(), "https://x/a.ttf".into()),
            ("normal".into(), "700".into(), "https://x/b.ttf".into()),
        ]));
    }

    #[test]
    fn purge_unplanned_strips_latin_keeps_google_keys() {
        let parent = temp_family_dir("purge-latin");
        let dir = parent.join("Libre Baskerville");
        fs::create_dir_all(&dir).unwrap();
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(256, 0);
        fs::write(dir.join("libre-baskerville-400-normal.ttf"), &fake).unwrap();
        fs::write(dir.join("libre-baskerville-latin-400-normal.ttf"), &fake).unwrap();
        fs::write(dir.join("libre-baskerville-latin-700-italic.ttf"), &fake).unwrap();
        let keys = vec!["libre-baskerville-400-normal.ttf".into()];
        purge_unplanned_font_files(&dir, &keys);
        assert!(dir.join("libre-baskerville-400-normal.ttf").is_file());
        assert!(!dir.join("libre-baskerville-latin-400-normal.ttf").is_file());
        assert!(!dir.join("libre-baskerville-latin-700-italic.ttf").is_file());
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn purge_unplanned_strips_all_latin_and_duplicate_unplanned() {
        // Google re-download must remove EVERY *-latin-* plus prior static duplicates
        // not in the new planned key list — not merely a subset of latin pads.
        let parent = temp_family_dir("purge-dupes");
        let dir = parent.join("Inter");
        fs::create_dir_all(&dir).unwrap();
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(256, 0);
        let keep = "inter-100-900-normal.ttf";
        fs::write(dir.join(keep), &fake).unwrap();
        // Prior static google-shaped faces (unplanned after variable listing).
        fs::write(dir.join("inter-400-normal.ttf"), &fake).unwrap();
        fs::write(dir.join("inter-700-italic.ttf"), &fake).unwrap();
        // Fontsource latin pads (all must go).
        fs::write(dir.join("inter-latin-400-normal.ttf"), &fake).unwrap();
        fs::write(dir.join("inter-latin-700-normal.ttf"), &fake).unwrap();
        fs::write(dir.join("inter-latin-400-italic.ttf"), &fake).unwrap();
        // Non-latin subset pack + odd duplicate name.
        fs::write(dir.join("inter-cyrillic-400-normal.ttf"), &fake).unwrap();
        fs::write(dir.join("Inter-Regular.ttf"), &fake).unwrap();
        // Compat sidecar must remain.
        fs::write(dir.join("inter-compat-outline.ttf"), &fake).unwrap();
        let keys = vec![keep.into(), "inter-latin-400-normal.ttf".into()]; // latin in planned must still die
        purge_unplanned_font_files(&dir, &keys);
        assert!(dir.join(keep).is_file(), "planned google key must stay");
        assert!(dir.join("inter-compat-outline.ttf").is_file(), "compat sidecar must stay");
        for gone in [
            "inter-400-normal.ttf",
            "inter-700-italic.ttf",
            "inter-latin-400-normal.ttf",
            "inter-latin-700-normal.ttf",
            "inter-latin-400-italic.ttf",
            "inter-cyrillic-400-normal.ttf",
            "Inter-Regular.ttf",
        ] {
            assert!(
                !dir.join(gone).is_file(),
                "{gone} must be purged on Google re-download"
            );
        }
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn purge_latin_named_files_strips_legacy_packs() {
        let parent = temp_family_dir("purge-latin-names");
        let dir = parent.join("Roboto");
        fs::create_dir_all(&dir).unwrap();
        let fake = vec![0u8; 512];
        fs::write(dir.join("roboto-400-normal.ttf"), &fake).unwrap();
        fs::write(dir.join("roboto-latin-400-normal.ttf"), &fake).unwrap();
        fs::write(dir.join("roboto-latin-700-italic.ttf"), &fake).unwrap();
        purge_latin_named_files(&dir);
        assert!(dir.join("roboto-400-normal.ttf").is_file());
        assert!(!dir.join("roboto-latin-400-normal.ttf").is_file());
        assert!(!dir.join("roboto-latin-700-italic.ttf").is_file());
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn purge_latin_named_files_keeps_slug_embedded_latin() {
        // P0b: family slug embeds "latin" — Google faces must not be purged.
        let parent = temp_family_dir("purge-slug-latin");
        let dir = parent.join("M PLUS Code Latin");
        fs::create_dir_all(&dir).unwrap();
        let fake = vec![0u8; 512];
        fs::write(dir.join("m-plus-code-latin-400-normal.ttf"), &fake).unwrap();
        fs::write(dir.join("m-plus-code-latin-700-italic.ttf"), &fake).unwrap();
        // True Fontsource subset pad (double latin) must still go.
        fs::write(dir.join("m-plus-code-latin-latin-400-normal.ttf"), &fake).unwrap();
        purge_latin_named_files(&dir);
        assert!(
            dir.join("m-plus-code-latin-400-normal.ttf").is_file(),
            "slug-embedded latin must NOT be treated as subset"
        );
        assert!(dir.join("m-plus-code-latin-700-italic.ttf").is_file());
        assert!(
            !dir.join("m-plus-code-latin-latin-400-normal.ttf").is_file(),
            "real {{slug}}-latin-* subset pack must still purge"
        );
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn purge_unplanned_keeps_slug_embedded_latin_google_keys() {
        let parent = temp_family_dir("purge-anek");
        let dir = parent.join("Anek Latin");
        fs::create_dir_all(&dir).unwrap();
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(256, 0);
        let keep = "anek-latin-400-normal.ttf";
        fs::write(dir.join(keep), &fake).unwrap();
        fs::write(dir.join("anek-latin-latin-400-normal.ttf"), &fake).unwrap();
        purge_unplanned_font_files(&dir, &[keep.into()]);
        assert!(
            dir.join(keep).is_file(),
            "planned Google face for anek-latin must stay"
        );
        assert!(!dir.join("anek-latin-latin-400-normal.ttf").is_file());
        let _ = fs::remove_dir_all(&parent);
    }
}

#[cfg(test)]
mod install_path_tests {

    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_family_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "fm-install-{}-{}-{}",
            label,
            std::process::id(),
            nanos
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn variable_planned_filenames_not_purged() {
        let parent = temp_family_dir("purge-var");
        let dir = parent.join("Nunito");
        fs::create_dir_all(&dir).unwrap();
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(256, 0);
        let var_roman = "nunito-variable-wght.ttf";
        let var_italic = "nunito-variable-wght-italic.ttf";
        let inst = "nunito-200-normal.ttf";
        fs::write(dir.join(var_roman), &fake).unwrap();
        fs::write(dir.join(var_italic), &fake).unwrap();
        fs::write(dir.join(inst), &fake).unwrap();
        fs::write(dir.join("nunito-latin-400-normal.ttf"), &fake).unwrap();
        let keys = vec![
            inst.into(),
            var_roman.into(),
            var_italic.into(),
        ];
        purge_unplanned_font_files(&dir, &keys);
        assert!(dir.join(var_roman).is_file(), "variable roman must stay");
        assert!(dir.join(var_italic).is_file(), "variable italic must stay");
        assert!(dir.join(inst).is_file(), "instance must stay");
        assert!(!dir.join("nunito-latin-400-normal.ttf").is_file());
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn unplanned_variable_filenames_are_purged() {
        let parent = temp_family_dir("purge-unplanned-var");
        let dir = parent.join("Nunito");
        fs::create_dir_all(&dir).unwrap();
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(256, 0);
        let inst = "nunito-400-normal.ttf";
        let stale_var = "nunito-variable-wght.ttf";
        fs::write(dir.join(inst), &fake).unwrap();
        fs::write(dir.join(stale_var), &fake).unwrap();
        purge_unplanned_font_files(&dir, &[inst.into()]);
        assert!(dir.join(inst).is_file());
        assert!(
            !dir.join(stale_var).is_file(),
            "unplanned *-variable-* must not linger forever"
        );
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn parse_google_instance_face_name_splits_weight_style() {
        assert_eq!(
            parse_google_instance_face_name("nunito", "nunito-200-normal.ttf"),
            Some(("200".into(), "normal".into()))
        );
        assert_eq!(
            parse_google_instance_face_name("nunito", "nunito-200-1000-italic.ttf"),
            Some(("200-1000".into(), "italic".into()))
        );
        assert_eq!(
            parse_google_instance_face_name("nunito", "nunito-variable-wght.ttf"),
            None
        );
        assert_eq!(
            parse_google_instance_face_name("nunito", "nunito-latin-400-normal.ttf"),
            None
        );
        assert_eq!(
            parse_google_instance_face_name(
                "m-plus-code-latin",
                "m-plus-code-latin-400-normal.ttf"
            ),
            Some(("400".into(), "normal".into()))
        );
        assert_eq!(
            parse_google_instance_face_name(
                "m-plus-code-latin",
                "m-plus-code-latin-latin-400-normal.ttf"
            ),
            None
        );
    }

    /// Minimal sfnt with Win name 1/2/4/6/16/17 for heal tests.
    fn minimal_named_font(family: &str, style: &str) -> Vec<u8> {
        fn utf16_be(s: &str) -> Vec<u8> {
            s.encode_utf16().flat_map(u16::to_be_bytes).collect()
        }
        fn checksum(data: &[u8]) -> u32 {
            let mut sum = 0u32;
            let mut i = 0;
            while i + 4 <= data.len() {
                sum = sum.wrapping_add(u32::from_be_bytes([
                    data[i], data[i + 1], data[i + 2], data[i + 3],
                ]));
                i += 4;
            }
            sum
        }
        let full = if style.eq_ignore_ascii_case("Regular") {
            family.to_string()
        } else {
            format!("{family} {style}")
        };
        let ps: String = format!(
            "{}-{}",
            family.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>(),
            style.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>()
        );
        let ids = [
            (1u16, family),
            (2, style),
            (4, full.as_str()),
            (6, ps.as_str()),
            (16, family),
            (17, style),
        ];
        let mut strings = Vec::new();
        let mut recs = Vec::new();
        for (id, text) in ids {
            let data = utf16_be(text);
            let off = strings.len() as u16;
            recs.extend_from_slice(&3u16.to_be_bytes());
            recs.extend_from_slice(&1u16.to_be_bytes());
            recs.extend_from_slice(&0x0409u16.to_be_bytes());
            recs.extend_from_slice(&id.to_be_bytes());
            recs.extend_from_slice(&(data.len() as u16).to_be_bytes());
            recs.extend_from_slice(&off.to_be_bytes());
            strings.extend_from_slice(&data);
        }
        let string_offset = (6 + ids.len() * 12) as u16;
        let mut name = Vec::new();
        name.extend_from_slice(&0u16.to_be_bytes());
        name.extend_from_slice(&(ids.len() as u16).to_be_bytes());
        name.extend_from_slice(&string_offset.to_be_bytes());
        name.extend_from_slice(&recs);
        name.extend_from_slice(&strings);
        while name.len() % 4 != 0 {
            name.push(0);
        }
        let mut head = vec![0u8; 54];
        head[0..4].copy_from_slice(&0x00010000u32.to_be_bytes());
        let mut font = Vec::new();
        font.extend_from_slice(&0x00010000u32.to_be_bytes());
        font.extend_from_slice(&2u16.to_be_bytes());
        font.extend_from_slice(&32u16.to_be_bytes());
        font.extend_from_slice(&1u16.to_be_bytes());
        font.extend_from_slice(&0u16.to_be_bytes());
        let head_off = 12 + 2 * 16;
        font.extend_from_slice(b"head");
        font.extend_from_slice(&0u32.to_be_bytes());
        font.extend_from_slice(&(head_off as u32).to_be_bytes());
        font.extend_from_slice(&(head.len() as u32).to_be_bytes());
        let name_off = head_off + head.len();
        font.extend_from_slice(b"name");
        font.extend_from_slice(&0u32.to_be_bytes());
        font.extend_from_slice(&(name_off as u32).to_be_bytes());
        font.extend_from_slice(&(name.len() as u32).to_be_bytes());
        font.extend_from_slice(&head);
        font.extend_from_slice(&name);
        let head_cs = checksum(&font[head_off..head_off + head.len()]);
        font[12 + 4..12 + 8].copy_from_slice(&head_cs.to_be_bytes());
        let name_cs = checksum(&font[name_off..name_off + name.len()]);
        font[12 + 16 + 4..12 + 16 + 8].copy_from_slice(&name_cs.to_be_bytes());
        // Pad to satisfy ttf_intact length floor (256).
        if font.len() < 256 {
            font.resize(256, 0);
        }
        font
    }

    #[test]
    fn heal_rewrites_mashed_instance_names_in_place() {
        let dir = temp_family_dir("heal-names");
        let mashed = minimal_named_font("Nunito ExtraLight", "Regular");
        assert_eq!(
            crate::namepatch::read_name_id(&mashed, 1).as_deref(),
            Some("Nunito ExtraLight")
        );
        let path = dir.join("nunito-200-normal.ttf");
        fs::write(&path, &mashed).unwrap();
        // Already-correct var sibling stays a no-op (id1 already catalog family).
        let var_path = dir.join("nunito-variable-wght.ttf");
        let var_bytes = minimal_named_font("Nunito", "Regular");
        fs::write(&var_path, &var_bytes).unwrap();

        let n = heal_google_instance_names_in_dir(&dir, "Nunito");
        assert_eq!(n.healed, 1, "exactly one instance face should heal");
        assert_eq!(n.locked, 0);
        let healed = fs::read(&path).unwrap();
        assert_eq!(crate::namepatch::read_name_id(&healed, 1).as_deref(), Some("Nunito"));
        assert_eq!(
            crate::namepatch::read_name_id(&healed, 2).as_deref(),
            Some("ExtraLight")
        );
        let var_after = fs::read(&var_path).unwrap();
        assert_eq!(
            crate::namepatch::read_name_id(&var_after, 1).as_deref(),
            Some("Nunito"),
            "already-clean variable TTF must remain untouched"
        );
        // Second pass is a no-op.
        assert_eq!(heal_google_instance_names_in_dir(&dir, "Nunito").healed, 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn heal_rewrites_mashed_variable_names_in_place() {
        let dir = temp_family_dir("heal-var-names");
        // Skye: Nunito var id1 mashed with ExtraLight; static ExtraLight style stays ExtraLight.
        let var_mashed = minimal_named_font("Nunito ExtraLight", "Regular");
        let var_path = dir.join("nunito-variable-wght.ttf");
        fs::write(&var_path, &var_mashed).unwrap();
        let var_italic = minimal_named_font("Nunito ExtraLight", "Italic");
        let var_italic_path = dir.join("nunito-variable-wght-italic.ttf");
        fs::write(&var_italic_path, &var_italic).unwrap();
        let static_mashed = minimal_named_font("Nunito ExtraLight", "Regular");
        let static_path = dir.join("nunito-200-normal.ttf");
        fs::write(&static_path, &static_mashed).unwrap();

        let n = heal_google_instance_names_in_dir(&dir, "Nunito");
        assert_eq!(n.healed, 3, "var roman + var italic + ExtraLight instance should heal");
        assert_eq!(n.locked, 0);

        let var_after = fs::read(&var_path).unwrap();
        assert_eq!(
            crate::namepatch::read_name_id(&var_after, 1).as_deref(),
            Some("Nunito")
        );
        assert_eq!(
            crate::namepatch::read_name_id(&var_after, 2).as_deref(),
            Some("Regular"),
            "var roman style must be Regular, not ExtraLight"
        );
        assert_eq!(
            crate::namepatch::read_name_id(&var_after, 16).as_deref(),
            Some("Nunito")
        );

        let var_it_after = fs::read(&var_italic_path).unwrap();
        assert_eq!(
            crate::namepatch::read_name_id(&var_it_after, 1).as_deref(),
            Some("Nunito")
        );
        assert_eq!(
            crate::namepatch::read_name_id(&var_it_after, 2).as_deref(),
            Some("Italic")
        );

        let static_after = fs::read(&static_path).unwrap();
        assert_eq!(
            crate::namepatch::read_name_id(&static_after, 1).as_deref(),
            Some("Nunito")
        );
        assert_eq!(
            crate::namepatch::read_name_id(&static_after, 2).as_deref(),
            Some("ExtraLight"),
            "instance ExtraLight style must stay ExtraLight"
        );

        assert_eq!(heal_google_instance_names_in_dir(&dir, "Nunito").healed, 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn heal_stats_add_coalesces_totals_and_skip_vars_counts_statics_only() {
        let mut a = HealStats {
            healed: 2,
            locked: 1,
            write_failed: 0,
        };
        a.add(HealStats {
            healed: 3,
            locked: 4,
            write_failed: 1,
        });
        assert_eq!(a.healed, 5);
        assert_eq!(a.locked, 5);
        assert_eq!(a.write_failed, 1);

        let dir = temp_family_dir("heal-skip-vars");
        let var_mashed = minimal_named_font("Nunito ExtraLight", "Regular");
        fs::write(dir.join("nunito-variable-wght.ttf"), &var_mashed).unwrap();
        let static_mashed = minimal_named_font("Nunito ExtraLight", "Regular");
        fs::write(dir.join("nunito-200-normal.ttf"), &static_mashed).unwrap();

        let statics_only = heal_google_names_in_dir(&dir, "Nunito", false);
        assert_eq!(
            statics_only.healed, 1,
            "include_vars=false must heal static instance only (ensure already owns vars)"
        );
        // Var still mashed
        let var_after = fs::read(dir.join("nunito-variable-wght.ttf")).unwrap();
        assert_eq!(
            crate::namepatch::read_name_id(&var_after, 1).as_deref(),
            Some("Nunito ExtraLight")
        );
        let all = heal_google_names_in_dir(&dir, "Nunito", true);
        assert_eq!(all.healed, 1, "second pass heals the remaining var");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Locked/in-use rewrite must increment locked (or write_failed), not look like
    /// success with healed=0 and no error. Soft-fail keeps bytes intact.
    #[test]
    fn heal_locked_write_increments_locked_not_silent_success() {
        let dir = temp_family_dir("heal-locked");
        let mashed = minimal_named_font("Nunito ExtraLight", "Regular");
        let path = dir.join("nunito-variable-wght.ttf");
        fs::write(&path, &mashed).unwrap();

        // Unix: directory not writable → delete/replace fails with PermissionDenied,
        // which `is_lock_err` maps to the same "files locked" path as Win sharing.
        // Windows ignores read-only on directories, so hold a real sharing lock
        // instead (read allowed, no write/delete share — like Word or fontdrvhost).
        #[cfg(not(windows))]
        let mut perms = fs::metadata(&dir).unwrap().permissions();
        #[cfg(not(windows))]
        {
            perms.set_readonly(true);
            fs::set_permissions(&dir, perms.clone()).unwrap();
        }
        #[cfg(windows)]
        let lock = {
            use std::os::windows::fs::OpenOptionsExt;
            const FILE_SHARE_READ: u32 = 0x1;
            fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ)
                .open(&path)
                .unwrap()
        };

        let stats = heal_google_instance_names_in_dir(&dir, "Nunito");

        #[cfg(not(windows))]
        {
            perms.set_readonly(false);
            fs::set_permissions(&dir, perms).unwrap();
        }
        #[cfg(windows)]
        drop(lock);

        assert_eq!(
            stats.healed, 0,
            "locked rewrite must not count as healed"
        );
        assert!(
            stats.locked >= 1,
            "expected locked>=1 (got locked={}, write_failed={}) — must not look like silent success",
            stats.locked,
            stats.write_failed
        );
        let after = fs::read(&path).unwrap();
        assert_eq!(
            crate::namepatch::read_name_id(&after, 1).as_deref(),
            Some("Nunito ExtraLight"),
            "soft-fail must leave mashed bytes untouched"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn weight_range_tokens_detected_for_variable_css() {
        assert!(weight_token_is_range("200-1000"));
        assert!(weight_token_is_range("100-900"));
        assert!(!weight_token_is_range("400"));
        assert!(!weight_token_is_range("700"));
        assert!(listing_has_only_range_weights(&[
            ("normal".into(), "200-1000".into(), "https://x/a.ttf".into()),
            ("italic".into(), "200-1000".into(), "https://x/b.ttf".into()),
        ]));
        assert!(!listing_has_only_range_weights(&[
            ("normal".into(), "200".into(), "https://x/a.ttf".into()),
            ("normal".into(), "300".into(), "https://x/b.ttf".into()),
        ]));
    }

        #[test]
    fn merge_variable_into_planned_vars_only_when_vf_present() {
        let statics = vec![
            "nunito-200-normal.ttf".into(),
            "nunito-400-normal.ttf".into(),
        ];
        let vars = vec![
            "nunito-variable-wght.ttf".into(),
            "nunito-variable-wght-italic.ttf".into(),
        ];
        // 1.0.206u VF-primary: vars present → planned vars only (no static backup).
        let keys = merge_variable_into_planned_keys(&statics, &vars);
        assert_eq!(
            keys,
            vec![
                "nunito-variable-wght.ttf",
                "nunito-variable-wght-italic.ttf",
            ]
        );
        assert!(!keys.iter().any(|k| k.contains("-normal")));
        // No vars → keep statics.
        let keys_static = merge_variable_into_planned_keys(&statics, &[]);
        assert_eq!(keys_static, statics);
        // Dedup vars.
        let mixed = vec![
            "nunito-variable-wght.ttf".into(),
            "nunito-400-normal.ttf".into(),
        ];
        let keys2 = merge_variable_into_planned_keys(&mixed, &vars);
        assert_eq!(keys2.len(), 2);
        assert_eq!(keys2.iter().filter(|k| k.contains("-variable-")).count(), 2);
        assert!(
            !keys2.iter().any(|k| k.contains("-variable-") && keys2.iter().filter(|x| *x == k).count() > 1),
            "no duplicate var keys"
        );
    }

    #[test]
    fn dir_vf_primary_ready_respects_dual_vf_and_denylist() {
        let parent = temp_family_dir("vf-primary-gate");
        let dir = parent.join("Inter");
        fs::create_dir_all(&dir).unwrap();
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(256, 0);
        assert!(!dir_vf_primary_ready(&dir, "Inter"));
        fs::write(dir.join("inter-variable-wght.ttf"), &fake).unwrap();
        assert!(dir_vf_primary_ready(&dir, "Inter"));
        // Dual-VF: roman alone is not enough.
        let hei = parent.join("Chiron Hei HK");
        fs::create_dir_all(&hei).unwrap();
        fs::write(hei.join("chiron-hei-hk-variable-wght.ttf"), &fake).unwrap();
        assert!(!dir_vf_primary_ready(&hei, "Chiron Hei HK"));
        fs::write(hei.join("chiron-hei-hk-variable-wght-italic.ttf"), &fake).unwrap();
        assert!(dir_vf_primary_ready(&hei, "Chiron Hei HK"));
        // No-public-VF denylist never VF-primary even with a fake var file.
        let gs = parent.join("Google Sans");
        fs::create_dir_all(&gs).unwrap();
        fs::write(gs.join("google-sans-variable-wght.ttf"), &fake).unwrap();
        assert!(family_has_no_public_vf("Google Sans"));
        assert!(!dir_vf_primary_ready(&gs, "Google Sans"));
        // Poppins catalog static-only — no intact VF → not VF-primary.
        let pop = parent.join("Poppins");
        fs::create_dir_all(&pop).unwrap();
        fs::write(pop.join("poppins-400-normal.ttf"), &fake).unwrap();
        assert!(!dir_vf_primary_ready(&pop, "Poppins"));
        // Finlandica Text (catalog dual-VF) — roman alone not enough.
        let ft = parent.join("Finlandica Text");
        fs::create_dir_all(&ft).unwrap();
        fs::write(ft.join("finlandica-text-variable-wght.ttf"), &fake).unwrap();
        assert!(family_expects_dual_variable("Finlandica Text"));
        assert!(!dir_vf_primary_ready(&ft, "Finlandica Text"));
        fs::write(ft.join("finlandica-text-variable-wght-italic.ttf"), &fake).unwrap();
        assert!(dir_vf_primary_ready(&ft, "Finlandica Text"));
        assert!(!family_expects_dual_variable("Finlandica"));
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn purge_after_successful_vf_add_ordering_keep_statics_until_commit() {
        // Skye P1: merge keep_statics + no purge on adopt; vars-only merge after.
        let statics = vec!["inter-400-normal.ttf".into()];
        let vars = vec!["inter-variable-wght.ttf".into()];
        let kept = merge_variable_into_planned_keys_keep_statics(&statics, &vars);
        assert!(kept.iter().any(|k| k.contains("-variable-")));
        assert!(kept.iter().any(|k| k == "inter-400-normal.ttf"));
        let only = merge_variable_into_planned_keys(&statics, &vars);
        assert_eq!(only, vars);
        assert!(!only.iter().any(|k| k.contains("-normal")));
    }

    #[test]
    fn purge_redundant_statics_keeps_vf_deletes_statics() {
        let parent = temp_family_dir("purge-statics-vf");
        let dir = parent.join("Inter");
        fs::create_dir_all(&dir).unwrap();
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(256, 0);
        let varf = "inter-variable-wght.ttf";
        let inst = "inter-400-normal.ttf";
        fs::write(dir.join(varf), &fake).unwrap();
        fs::write(dir.join(inst), &fake).unwrap();
        write_google_planned(&dir, &[varf.into(), inst.into()]);
        mark_family_complete(&dir, 2);
        assert!(dir_vf_primary_ready(&dir, "Inter"));
        let (del, lock) = purge_redundant_statics_in_dir(&dir, "Inter");
        assert_eq!(lock, 0);
        assert!(del >= 1, "should delete static: del={del}");
        assert!(dir.join(varf).is_file(), "VF must remain");
        assert!(!dir.join(inst).is_file(), "static must be gone");
        let keys = read_google_planned_keys(&dir).expect("planned");
        assert_eq!(keys, vec![varf.to_string()]);
        assert!(dir_is_complete(&dir));
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn docs_vf_sync_loop_stops_before_further_purges_when_cancelled() {
        // 1.0.206aa amend: exercise real `run_docs_vf_sync_purge_loop` (not a sim copy).
        use std::path::PathBuf;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        let dirs: Vec<PathBuf> = (0..10).map(|i| PathBuf::from(format!("dir-{i}"))).collect();
        let cancel = AtomicBool::new(false);
        let purge_calls = AtomicUsize::new(0);

        let (cancelled, _fam, statics, _lock, done) = run_docs_vf_sync_purge_loop(
            &dirs,
            &cancel,
            |_dir, _name| {},
            |_dir, _name| {
                let n = purge_calls.fetch_add(1, Ordering::SeqCst);
                if n == 2 {
                    // After third purge starts counting from 0: cancel before next
                    cancel.store(true, Ordering::SeqCst);
                }
                (1usize, 0usize)
            },
        );
        assert!(cancelled, "cancel after 3rd purge must stop further dirs");
        assert_eq!(purge_calls.load(Ordering::SeqCst), 3, "dirs 0..=2 purged only");
        assert_eq!(done, 3);
        assert_eq!(statics, 3);

        let cancel2 = AtomicBool::new(true);
        let purge2 = AtomicUsize::new(0);
        let dirs2: Vec<PathBuf> = (0..5).map(|i| PathBuf::from(format!("x-{i}"))).collect();
        let (cancelled2, _, statics2, _, done2) = run_docs_vf_sync_purge_loop(
            &dirs2,
            &cancel2,
            |_, _| {},
            |_, _| {
                purge2.fetch_add(1, Ordering::SeqCst);
                (1, 0)
            },
        );
        assert!(cancelled2);
        assert_eq!(purge2.load(Ordering::SeqCst), 0, "pre-set cancel skips all purges");
        assert_eq!(statics2, 0);
        assert_eq!(done2, 0);

        // Cancel set in prepare (before purge) at index 3 — purges 0..=2 only.
        let cancel3 = AtomicBool::new(false);
        let purge3 = AtomicUsize::new(0);
        let idx = AtomicUsize::new(0);
        let dirs3: Vec<PathBuf> = (0..8).map(|i| PathBuf::from(format!("p-{i}"))).collect();
        let (cancelled3, _, statics3, _, done3) = run_docs_vf_sync_purge_loop(
            &dirs3,
            &cancel3,
            |_, _| {
                let i = idx.fetch_add(1, Ordering::SeqCst);
                if i == 3 {
                    cancel3.store(true, Ordering::SeqCst);
                }
            },
            |_, _| {
                purge3.fetch_add(1, Ordering::SeqCst);
                (1, 0)
            },
        );
        assert!(cancelled3);
        assert_eq!(purge3.load(Ordering::SeqCst), 3);
        assert_eq!(statics3, 3);
        assert_eq!(done3, 3);
    }

    #[test]
    fn docs_vf_sync_execute_mid_purge_returns_cancelled_true() {
        // 1.0.206ab: real execute path (not only loop helper) — cancel after N purges.
        use std::path::PathBuf;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        let dirs: Vec<PathBuf> = (0..18).map(|i| PathBuf::from(format!("fam-{i}"))).collect();
        let cancel = AtomicBool::new(false);
        let purge_calls = AtomicUsize::new(0);
        let result = docs_vf_sync_execute(
            dirs,
            &cancel,
            |_, _| {},
            |_, _| {
                let n = purge_calls.fetch_add(1, Ordering::SeqCst);
                if n == 4 {
                    cancel.store(true, Ordering::SeqCst);
                }
                (1usize, 0usize)
            },
        );
        assert!(result.cancelled, "must return cancelled:true");
        assert_eq!(purge_calls.load(Ordering::SeqCst), 5, "purges stop after cancel");
        assert_eq!(result.statics_deleted, 5);
        assert_eq!(result.families_seen, 18);
        assert!(result.statics_deleted < 18, "must not purge remaining dirs");
    }

    #[test]
    fn docs_vf_sync_execute_scan_cancel_skips_all_purges() {
        // Cancel set during/after scan (before purge loop) — cancelled:true, zero purges.
        use std::path::PathBuf;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        let dirs: Vec<PathBuf> = (0..12).map(|i| PathBuf::from(format!("s-{i}"))).collect();
        let cancel = AtomicBool::new(true);
        let purge_calls = AtomicUsize::new(0);
        let result = docs_vf_sync_execute(
            dirs,
            &cancel,
            |_, _| {},
            |_, _| {
                purge_calls.fetch_add(1, Ordering::SeqCst);
                (1, 0)
            },
        );
        assert!(result.cancelled);
        assert_eq!(purge_calls.load(Ordering::SeqCst), 0);
        assert_eq!(result.statics_deleted, 0);
        assert_eq!(result.families_purged, 0);
        assert_eq!(result.families_seen, 12);
    }

    #[test]
    fn docs_vf_session_live_flag_defaults_false() {
        // 1.0.206ag: Escape hatch gated on docs_vf_session_live (not bare Stopping…).
        assert!(!docs_vf_session_is_live());
    }

    #[test]
    fn docs_vf_emit_throttle_keeps_webview_responsive_contract() {
        // 1.0.206ae Gate D: ≥800ms docs emit; every-N dirs; Activate stays 350ms.
        assert!(docs_vf_emit_progress_throttle_ms() >= 800);
        assert!(docs_vf_emit_progress_throttle_ms() >= emit_progress_throttle_ms());
        assert!(docs_vf_emit_every_n_dirs() >= 16);
    }

    #[test]
    fn docs_vf_sync_execute_cancel_flag_after_last_purge_still_cancelled() {
        // 1.0.206ad: cancel set as final purge returns — must still be cancelled:true (not success).
        use std::path::PathBuf;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        let dirs: Vec<PathBuf> = (0..3).map(|i| PathBuf::from(format!("end-{i}"))).collect();
        let cancel = AtomicBool::new(false);
        let purge_calls = AtomicUsize::new(0);
        let result = docs_vf_sync_execute(
            dirs,
            &cancel,
            |_, _| {},
            |_, _| {
                let n = purge_calls.fetch_add(1, Ordering::SeqCst);
                if n + 1 == 3 {
                    cancel.store(true, Ordering::SeqCst);
                }
                (1usize, 0usize)
            },
        );
        assert!(result.cancelled, "flag after last purge must yield cancelled:true");
        assert_eq!(purge_calls.load(Ordering::SeqCst), 3);
        assert_eq!(result.statics_deleted, 3);
    }

    #[test]
    fn adopt_variable_into_plan_on_statics_only_complete_folder() {
        let parent = temp_family_dir("adopt-var");
        let dir = parent.join("Nunito");
        fs::create_dir_all(&dir).unwrap();
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(256, 0);
        let inst = "nunito-400-normal.ttf";
        let var_roman = "nunito-variable-wght.ttf";
        fs::write(dir.join(inst), &fake).unwrap();
        fs::write(dir.join(var_roman), &fake).unwrap();
        // Legacy complete: statics planned only (the bug — 16 static, 0 variable).
        write_google_planned(&dir, &[inst.into()]);
        mark_family_complete(&dir, 1);
        assert!(dir_has_intact_variable(&dir));
        adopt_variable_files_into_plan(&dir, &[var_roman.into()]);
        let keys = read_google_planned_keys(&dir).expect("planned keys");
        assert!(
            keys.iter().any(|k| k == var_roman),
            "planned must include variable after adopt: {keys:?}"
        );
        // Skye P1: adopt must NOT purge — statics stay until VarsOnly Add>0.
        assert!(
            keys.iter().any(|k| k == inst),
            "pre-Add planned keeps static for fallback: {keys:?}"
        );
        assert!(
            dir.join(inst).is_file(),
            "static must remain on disk until successful VF Add"
        );
        assert!(dir_is_complete(&dir), "complete when all planned intact");
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn complete_folder_without_var_is_detected_as_missing_catalog_variable() {
        let parent = temp_family_dir("missing-var-detect");
        let dir = parent.join("Nunito");
        fs::create_dir_all(&dir).unwrap();
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(256, 0);
        fs::write(dir.join("nunito-400-normal.ttf"), &fake).unwrap();
        assert!(google_catalog_is_variable("Nunito"));
        assert!(!dir_has_intact_variable(&dir));
        // After a var file appears, detector flips.
        fs::write(dir.join("nunito-variable-wght.ttf"), &fake).unwrap();
        assert!(dir_has_intact_variable(&dir));
        let _ = fs::remove_dir_all(&parent);
    }


    #[test]
    fn register_zero_detail_never_claims_complete_without_marker() {
        let parent = temp_family_dir("honest-toast-complete");
        let dir = parent.join("Clear Sans");
        fs::create_dir_all(&dir).unwrap();
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(256, 0);
        for w in [100u16, 300, 400, 500, 700] {
            fs::write(dir.join(format!("clear-sans-{w}-normal.ttf")), &fake).unwrap();
        }
        write_expected_faces(&dir, 5);
        // NO .complete marker
        assert!(!dir_is_complete(&dir));
        let h = family_disk_honesty_in(&dir, "Clear Sans");
        assert_eq!(h.intact, 5);
        assert!(!h.has_complete);
        let msg = format!(
            "Clear Sans — files on disk {intact}/{expected}, .complete={complete}, GDI live 0 ({cause})",
            intact = h.intact,
            expected = h.expected.unwrap_or(0),
            complete = if h.has_complete { "yes" } else { "no" },
            cause = RegisterFailKind::AddReturnedZero.label(),
        );
        assert!(msg.contains(".complete=no"), "{msg}");
        assert!(!msg.contains("on disk (.complete)"), "{msg}");
        assert!(msg.contains("AddFontResourceExW returned 0"), "{msg}");
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn note_register_zero_clears_sticky_complete_marker() {
        let parent = temp_family_dir("sticky-complete-clear");
        let dir = parent.join("Nunito");
        fs::create_dir_all(&dir).unwrap();
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(256, 0);
        fs::write(dir.join("nunito-400-normal.ttf"), &fake).unwrap();
        mark_family_complete(&dir, 1);
        assert!(dir_is_complete(&dir));
        clear_complete_marker(&dir);
        assert!(!dir_is_complete(&dir), "sticky .complete must clear after GDI 0");
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn disk_family_missing_variable_marks_incomplete_even_with_complete() {
        let parent = temp_family_dir("missing-vf-incomplete");
        let dir = parent.join("Chiron Hei HK");
        fs::create_dir_all(&dir).unwrap();
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        // Full-size static so undersized heuristic does not clear .complete first.
        fake.resize(200 * 1024, 0);
        fs::write(dir.join("chiron-hei-hk-400-normal.ttf"), &fake).unwrap();
        write_google_planned(&dir, &["chiron-hei-hk-400-normal.ttf".into()]);
        mark_family_complete(&dir, 1);
        assert!(dir_is_complete(&dir));
        assert!(catalog_variable_expects_public_vf("Chiron Hei HK"));
        assert!(!dir_has_intact_variable(&dir));
        let h = family_disk_honesty_in(&dir, "Chiron Hei HK");
        assert!(h.missing_variable);
        let incomplete = (h.intact > 0 && !h.has_complete) || h.missing_variable;
        assert!(incomplete, "catalog-variable without VF must show Incomplete/Repair");
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn variable_facet_honesty_requires_disk_vf_filename() {
        let parent = temp_family_dir("facet-disk-vf");
        let dir = parent.join("42dot Sans");
        fs::create_dir_all(&dir).unwrap();
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(256, 0);
        for w in [100u16, 200, 300, 400, 500, 700] {
            fs::write(dir.join(format!("42dot-sans-{w}-normal.ttf")), &fake).unwrap();
        }
        assert!(!dir_has_intact_variable(&dir), "statics must not count as Variable");
        fs::write(dir.join("42dot-sans-variable-wght.ttf"), &fake).unwrap();
        assert!(dir_has_intact_variable(&dir));
        let _ = fs::remove_dir_all(&parent);
    }


    #[test]
    fn undersized_google_face_size_heuristic_matches_gidugu_remnant() {
        let parent = temp_family_dir("gidugu-undersized");
        let dir = parent.join("Gidugu");
        fs::create_dir_all(&dir).unwrap();
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(38404, 0); // live Documents remnant
        let path = dir.join("gidugu-400-normal.ttf");
        fs::write(&path, &fake).unwrap();
        assert!(is_undersized_google_static_face(&path));
        assert!(is_tiny_latin_subset_face(&path));
        // Full upstream ~461KB must not look undersized
        fake.resize(460988, 0);
        let full = dir.join("gidugu-400-full.ttf");
        fs::write(&full, &fake).unwrap();
        assert!(!is_undersized_google_static_face(&full));
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn verify_clears_complete_when_official_undersized_static_present() {
        let parent = temp_family_dir("undersized-complete-clear");
        // Allowlisted Gidugu remnant — not all-Google size band.
        let dir = parent.join("Gidugu");
        fs::create_dir_all(&dir).unwrap();
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(38404, 0);
        fs::write(dir.join("gidugu-400-normal.ttf"), &fake).unwrap();
        mark_family_complete(&dir, 1);
        assert!(dir_is_complete(&dir));
        assert!(is_official_google_family("Gidugu"));
        assert!(dir_has_undersized_google_static(&dir, "Gidugu"));
        verify_complete_marker(&dir);
        assert!(!dir_is_complete(&dir), "undersized vs Google must clear sticky .complete");
        // Collateral: ordinary small Google statics must NOT clear .complete on size alone.
        // Honest .google-planned so catalog-floor lie does not confound the size heuristic.
        let nunito = parent.join("Nunito");
        fs::create_dir_all(&nunito).unwrap();
        fs::write(nunito.join("nunito-400-normal.ttf"), &fake).unwrap();
        write_google_planned(&nunito, &["nunito-400-normal.ttf".into()]);
        mark_family_complete(&nunito, 1);
        assert!(is_official_google_family("Nunito"));
        assert!(!dir_has_undersized_google_static(&nunito, "Nunito"));
        verify_complete_marker(&nunito);
        assert!(dir_is_complete(&nunito), "non-allowlisted Google must not clear on size alone");
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn register_fail_kind_labels_split_stage_add_unload() {
        assert!(RegisterFailKind::StageCopyFailed.label().contains("stage-copy"));
        assert!(RegisterFailKind::AddReturnedZero.label().contains("AddFontResourceExW"));
        assert!(RegisterFailKind::Unloading.label().contains("unloading"));
        assert_eq!(
            RegisterFailKind::Refused.worsen(RegisterFailKind::StageCopyFailed),
            RegisterFailKind::StageCopyFailed
        );
    }


    #[test]
    fn google_fonts_variable_cdn_urls_jsdelivr_then_github_raw() {
        let urls = google_fonts_variable_cdn_urls("ofl", "chirongoroundtc", "ChironGoRoundTC[wght].ttf");
        assert!(
            urls[0].starts_with("https://cdn.jsdelivr.net/gh/google/fonts@main/ofl/chirongoroundtc/"),
            "jsDelivr first: {}",
            urls[0]
        );
        assert!(
            urls[0].contains("ChironGoRoundTC%5Bwght%5D.ttf"),
            "brackets encoded: {}",
            urls[0]
        );
        assert_eq!(
            urls[1],
            "https://raw.githubusercontent.com/google/fonts/main/ofl/chirongoroundtc/ChironGoRoundTC%5Bwght%5D.ttf"
        );
        const { assert!(MAX_TTF_FETCH_BYTES >= 64 * 1024 * 1024, "CJK VFs need >52MB headroom"); }
    }

    #[test]
    fn google_fonts_metadata_cdn_urls_jsdelivr_then_github_raw() {
        let urls = google_fonts_metadata_cdn_urls("ofl", "chironheihk");
        assert_eq!(
            urls[0],
            "https://cdn.jsdelivr.net/gh/google/fonts@main/ofl/chironheihk/METADATA.pb"
        );
        assert_eq!(
            urls[1],
            "https://raw.githubusercontent.com/google/fonts/main/ofl/chironheihk/METADATA.pb"
        );
    }

    #[test]
    fn dual_vf_families_are_hei_and_sung_only() {
        assert!(family_expects_dual_variable("Chiron Hei HK"));
        assert!(family_expects_dual_variable("Chiron Sung HK"));
        assert!(!family_expects_dual_variable("Chiron GoRound TC"));
        assert!(!family_expects_dual_variable("Nunito"));
        assert!(!family_expects_dual_variable("Noto Serif KR"));
    }

    #[test]
    fn dual_vf_roman_only_on_disk_detected_as_missing_italic() {
        let parent = temp_family_dir("dual-vf-roman-only");
        let dir = parent.join("Chiron Hei HK");
        fs::create_dir_all(&dir).unwrap();
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(256, 0);
        fs::write(dir.join("chiron-hei-hk-variable-wght.ttf"), &fake).unwrap();
        assert!(dir_has_intact_variable(&dir));
        assert!(!dir_has_intact_variable_italic(&dir));
        assert!(family_expects_dual_variable("Chiron Hei HK"));
        fs::write(dir.join("chiron-hei-hk-variable-wght-italic.ttf"), &fake).unwrap();
        assert!(dir_has_intact_variable_italic(&dir));
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn dual_vf_roman_only_is_in_variable_backfill_need() {
        let parent = temp_family_dir("dual-vf-need");
        let dir = parent.join("Chiron Hei HK");
        fs::create_dir_all(&dir).unwrap();
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(256, 0);
        fs::write(dir.join("chiron-hei-hk-variable-wght.ttf"), &fake).unwrap();
        assert!(
            dir_needs_variable_backfill(&dir, "Chiron Hei HK"),
            "roman-only dual-VF must still need backfill for italic"
        );
        fs::write(dir.join("chiron-hei-hk-variable-wght-italic.ttf"), &fake).unwrap();
        assert!(
            !dir_needs_variable_backfill(&dir, "Chiron Hei HK"),
            "roman+italic dual-VF is satisfied"
        );
        // Non-dual with any var is satisfied.
        let nunito = parent.join("Nunito");
        fs::create_dir_all(&nunito).unwrap();
        fs::write(nunito.join("nunito-variable-wght.ttf"), &fake).unwrap();
        assert!(!dir_needs_variable_backfill(&nunito, "Nunito"));
        // No var at all still needs backfill.
        let empty = parent.join("Noto Serif KR");
        fs::create_dir_all(&empty).unwrap();
        fs::write(empty.join("noto-serif-kr-400-normal.ttf"), &fake).unwrap();
        assert!(dir_needs_variable_backfill(&empty, "Noto Serif KR"));
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn tiny_latin_subset_face_heuristic() {
        let parent = temp_family_dir("tiny-cjk");
        let dir = parent.join("faces");
        fs::create_dir_all(&dir).unwrap();
        let tiny = dir.join("chiron-hei-hk-400-normal.ttf");
        let mut bytes = b"\x00\x01\x00\x00".to_vec();
        bytes.resize(40 * 1024, 0); // ~40KB latin shred
        fs::write(&tiny, &bytes).unwrap();
        assert!(is_tiny_latin_subset_face(&tiny));
        assert!(face_should_replace_as_tiny_cjk("Chiron Hei HK", &tiny));
        assert!(face_should_replace_as_tiny_cjk("Chiron GoRound TC", &tiny));
        assert!(face_should_replace_as_tiny_cjk("Noto Serif KR", &tiny));
        assert!(face_should_replace_as_tiny_cjk("LXGW WenKai", &tiny));
        // 1.0.168: Gidugu allowlist — not every official Google face in the size band.
        assert!(face_should_replace_undersized_google("Gidugu", &tiny));
        assert!(face_should_replace_as_tiny_cjk("Gidugu", &tiny));
        assert!(!face_should_replace_undersized_google("Nunito", &tiny));
        assert!(!face_should_replace_as_tiny_cjk("Nunito", &tiny));
        let full = dir.join("full.ttf");
        bytes.resize(200 * 1024, 0);
        fs::write(&full, &bytes).unwrap();
        assert!(!is_tiny_latin_subset_face(&full));
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn session_paths_refuse_documents() {
        let docs = PathBuf::from(r"C:\Users\Eric\Documents\Font Manager\A\a.ttf");
        let stage = PathBuf::from(r"C:\Users\Eric\AppData\Local\Font Manager\gdi-maps\aa.ttf");
        assert!(crate::session_stage::must_not_register_as_gdi_path(&docs));
        let filtered = crate::session_stage::filter_session_paths_refuse_documents(&[
            docs.clone(),
            stage.clone(),
        ]);
        assert_eq!(filtered, vec![stage]);
    }


    #[test]
    fn no_public_vf_denylist_is_exact_seven() {
        const EXPECTED: &[&str] = &[
            "Google Sans",
            "Edu NSW ACT Cursive",
            "Edu NSW ACT Hand Pre",
            "Edu QLD Hand",
            "Edu SA Hand",
            "Edu VIC WA NT Hand",
            "Edu VIC WA NT Hand Pre",
        ];
        assert_eq!(EXPECTED.len(), 7);
        for name in EXPECTED {
            assert!(family_has_no_public_vf(name), "{name}");
            assert!(!catalog_variable_expects_public_vf(name), "{name}");
        }
        // Spot-check: nothing else invents a denylist skip.
        assert!(!family_has_no_public_vf("Nunito"));
        assert!(!family_has_no_public_vf("Chiron Hei HK"));
    }

    #[test]
    fn no_public_vf_denylist_skips_invented_variable_expectation() {
        assert!(google_catalog_is_variable("Google Sans"));
        assert!(family_has_no_public_vf("Google Sans"));
        assert!(!catalog_variable_expects_public_vf("Google Sans"));
        assert!(family_has_no_public_vf("Edu NSW ACT Cursive"));
        assert!(family_has_no_public_vf("Edu VIC WA NT Hand Pre"));
        // P1 gap families DO expect a public VF.
        assert!(catalog_variable_expects_public_vf("Chiron GoRound TC"));
        assert!(catalog_variable_expects_public_vf("Chiron Hei HK"));
        assert!(catalog_variable_expects_public_vf("Chiron Sung HK"));
        assert!(catalog_variable_expects_public_vf("Noto Serif KR"));
        assert!(catalog_variable_expects_public_vf("Noto Serif SC"));
        assert!(catalog_variable_expects_public_vf("Nunito"));
    }

    #[test]
    fn complete_statics_kept_until_post_add_purge() {
        // Eric: .complete must not block VF backfill while VF missing.
        // 1.0.206u Skye: adopt keeps statics; purge only after VarsOnly Add>0.
        let parent = temp_family_dir("statics-only-complete");
        let dir = parent.join("Nunito");
        fs::create_dir_all(&dir).unwrap();
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(256, 0);
        let inst = "nunito-400-normal.ttf";
        let varf = "nunito-variable-wght.ttf";
        fs::write(dir.join(inst), &fake).unwrap();
        let static_bytes_before = fs::read(dir.join(inst)).unwrap();
        write_google_planned(&dir, &[inst.into()]);
        mark_family_complete(&dir, 1);
        assert!(dir_is_complete(&dir));
        assert!(!dir_has_intact_variable(&dir));
        assert!(
            !official_google_complete_is_lie(&dir),
            "missing VF must not clear .complete (would risk static wipe on Repair)"
        );
        verify_complete_marker(&dir);
        assert!(
            family_complete_marker(&dir).is_file(),
            ".complete stays; ensure/backfill still fetches vars while ready"
        );
        // Simulate VF landing (ensure path) then stamp honesty via adopt.
        fs::write(dir.join(varf), &fake).unwrap();
        adopt_variable_files_into_plan(&dir, &[varf.into()]);
        let keys = read_google_planned_keys(&dir).expect("planned");
        assert!(keys.iter().any(|k| k == varf), "planned gains var: {keys:?}");
        // Skye P1: adopt keeps statics until VarsOnly Add>0.
        assert!(keys.iter().any(|k| k == inst), "planned keeps static pre-Add: {keys:?}");
        assert_eq!(
            fs::read(dir.join(inst)).unwrap(),
            static_bytes_before,
            "static face bytes untouched by adopt"
        );
        assert!(dir.is_dir());
        assert!(dir.join(inst).is_file());
        assert!(dir.join(varf).is_file(), "VF remains");
        assert!(dir_is_complete(&dir), "stamp honest once vars intact");
        // Simulate post-Add commit: vars-only plan + purge.
        write_google_planned(&dir, &[varf.into()]);
        mark_family_complete(&dir, 1);
        let (del, lock) = purge_redundant_statics_in_dir(&dir, "Nunito");
        assert_eq!(lock, 0);
        assert!(del >= 1, "purge-after-successful-VF-Add deletes static");
        assert!(!dir.join(inst).is_file(), "static gone after post-Add purge");
        assert!(dir.join(varf).is_file(), "VF remains after purge");
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn verify_keeps_complete_for_no_public_vf_statics_only() {
        let parent = temp_family_dir("no-public-vf-complete");
        let dir = parent.join("Google Sans");
        fs::create_dir_all(&dir).unwrap();
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(256, 0);
        let inst = "google-sans-400-normal.ttf";
        fs::write(dir.join(inst), &fake).unwrap();
        write_google_planned(&dir, &[inst.into()]);
        mark_family_complete(&dir, 1);
        assert!(google_catalog_is_variable("Google Sans"));
        assert!(family_has_no_public_vf("Google Sans"));
        assert!(
            !official_google_complete_is_lie(&dir),
            "do not invent VF requirement for Google Sans"
        );
        verify_complete_marker(&dir);
        assert!(
            family_complete_marker(&dir).is_file(),
            "no-public-VF statics-only complete must remain"
        );
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn verify_keeps_complete_when_catalog_variable_has_intact_vf() {
        let parent = temp_family_dir("complete-with-vf");
        let dir = parent.join("Nunito");
        fs::create_dir_all(&dir).unwrap();
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(256, 0);
        let inst = "nunito-400-normal.ttf";
        let varf = "nunito-variable-wght.ttf";
        fs::write(dir.join(inst), &fake).unwrap();
        fs::write(dir.join(varf), &fake).unwrap();
        write_google_planned(&dir, &[varf.into(), inst.into()]);
        mark_family_complete(&dir, 2);
        assert!(dir_has_intact_variable(&dir));
        assert!(!official_google_complete_is_lie(&dir));
        verify_complete_marker(&dir);
        assert!(family_complete_marker(&dir).is_file());
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn sort_faces_var_first_orders_variable_ahead_of_statics() {
        let mut files = vec![
            PathBuf::from("nunito-400-normal.ttf"),
            PathBuf::from("nunito-variable-wght.ttf"),
            PathBuf::from("nunito-200-normal.ttf"),
            PathBuf::from("nunito-variable-wght-italic.ttf"),
        ];
        sort_faces_var_first(&mut files);
        let names: Vec<_> = files
            .iter()
            .filter_map(|p| p.file_name().and_then(|s| s.to_str()))
            .collect();
        assert!(
            names[0].contains("-variable-") && names[1].contains("-variable-"),
            "vars first: {names:?}"
        );
        assert!(
            !names[2].contains("-variable-") && !names[3].contains("-variable-"),
            "statics after: {names:?}"
        );
    }

    #[test]
    fn variable_face_filename_is_sanitize_friendly() {
        assert_eq!(
            variable_face_filename("nunito", "wght", false),
            "nunito-variable-wght.ttf"
        );
        assert_eq!(
            variable_face_filename("nunito", "wght", true),
            "nunito-variable-wght-italic.ttf"
        );
        assert_eq!(
            variable_face_filename("roboto", "wdth,wght", false),
            "roboto-variable-wdth-wght.ttf"
        );
        assert!(!variable_face_filename("nunito", "wght", false).contains("latin"));
    }

    #[test]
    fn latin_filename_never_emitted_by_google_or_fontsource_helpers() {
        assert!(!filename_has_latin_subset(
            &google_face_filename("nunito", "200", "normal"),
            "nunito"
        ));
        assert!(!filename_has_latin_subset(
            &fontsource_face_filename("nunito", "latin", 200, "normal"),
            "nunito"
        ));
        assert!(!filename_has_latin_subset(
            &variable_face_filename("nunito", "wght", false),
            "nunito"
        ));
    }

    #[test]
    fn migrate_download_source_stamp_google_planned() {
        let dir = temp_family_dir("mig-google");
        write_google_planned(
            &dir,
            &["nunito-400-normal.ttf".into(), "nunito-700-normal.ttf".into()],
        );
        assert!(read_download_source(&dir).is_none());
        migrate_download_source_stamp(&dir);
        assert_eq!(read_download_source(&dir), Some(FetchIntent::Google));
        // Idempotent — do not overwrite an existing stamp.
        write_download_source(&dir, FetchIntent::Fontsource);
        migrate_download_source_stamp(&dir);
        assert_eq!(read_download_source(&dir), Some(FetchIntent::Fontsource));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrate_download_source_stamp_fontsource_planned() {
        let dir = temp_family_dir("mig-fs");
        write_fontsource_planned(&dir, &["clear-sans-400-normal.ttf".into()]);
        migrate_download_source_stamp(&dir);
        assert_eq!(read_download_source(&dir), Some(FetchIntent::Fontsource));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrate_download_source_stamp_latin_dominates() {
        // temp_family_dir prefixes the label — nest a real family leaf so
        // dir_slug_hint == "rubik" and `-latin-` subset detection works.
        let root = temp_family_dir("mig-latin-root");
        let dir = root.join("Rubik");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("rubik-latin-400-normal.ttf"), b"x").unwrap();
        fs::write(dir.join("rubik-latin-700-normal.ttf"), b"x").unwrap();
        fs::write(dir.join("rubik-latin-900-normal.ttf"), b"x").unwrap();
        // One non-latin google-shaped name — latin still dominates.
        fs::write(dir.join("rubik-400-normal.ttf"), b"x").unwrap();
        migrate_download_source_stamp(&dir);
        assert_eq!(read_download_source(&dir), Some(FetchIntent::Fontsource));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn migrate_download_source_stamp_leaves_ambiguous_unset() {
        let dir = temp_family_dir("mig-amb");
        fs::write(dir.join("rubik-400-normal.ttf"), b"x").unwrap();
        fs::write(dir.join("rubik-700-normal.ttf"), b"x").unwrap();
        migrate_download_source_stamp(&dir);
        assert!(
            read_download_source(&dir).is_none(),
            "ambiguous folder must stay unset — never guess google"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn face_allowed_for_register_google_stamp_skips_fontsource() {
        let dir = temp_family_dir("src-stamp-google");
        fs::write(dir.join(".download-source"), b"google").unwrap();
        write_google_planned(
            &dir,
            &["inter-400-normal.ttf".into(), "inter-700-normal.ttf".into()],
        );
        write_fontsource_planned(&dir, &["inter-latin-400-normal.ttf".into()]);
        let g = dir.join("inter-400-normal.ttf");
        let fs = dir.join("inter-latin-400-normal.ttf");
        fs::write(&g, b"x").unwrap();
        fs::write(&fs, b"x").unwrap();
        assert!(face_allowed_for_register(&dir, &g, "Inter"));
        assert!(!face_allowed_for_register(&dir, &fs, "Inter"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn face_allowed_for_register_fontsource_stamp_skips_google_planned() {
        let dir = temp_family_dir("src-stamp-fs");
        fs::write(dir.join(".download-source"), b"fontsource").unwrap();
        write_google_planned(&dir, &["clear-sans-400-normal.ttf".into()]);
        write_fontsource_planned(
            &dir,
            &["clear-sans-400-normal.ttf".into(), "clear-sans-700-normal.ttf".into()],
        );
        let keep = dir.join("clear-sans-400-normal.ttf");
        let other = dir.join("clear-sans-900-normal.ttf");
        fs::write(&keep, b"x").unwrap();
        fs::write(&other, b"x").unwrap();
        assert!(face_allowed_for_register(&dir, &keep, "Clear Sans"));
        assert!(!face_allowed_for_register(&dir, &other, "Clear Sans"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_fetch_intent_labels() {
        assert_eq!(parse_fetch_intent("google"), Some(FetchIntent::Google));
        assert_eq!(parse_fetch_intent("fontsource"), Some(FetchIntent::Fontsource));
        assert_eq!(parse_fetch_intent("other"), Some(FetchIntent::Fontsource));
        assert_eq!(parse_fetch_intent("local"), Some(FetchIntent::Local));
        assert_eq!(parse_fetch_intent("nope"), None);
    }

    #[test]
    fn infer_fetch_intent_official_vs_exclusive() {
        assert_eq!(infer_fetch_intent("Nunito"), FetchIntent::Google);
        assert_eq!(infer_fetch_intent("Clear Sans"), FetchIntent::Fontsource);
    }


    #[test]
    fn filename_has_latin_subset_slug_aware() {
        // Family slug embeds "latin" — Google face is NOT a Fontsource subset pack.
        assert!(
            !filename_has_latin_subset(
                "m-plus-code-latin-400-normal.ttf",
                "m-plus-code-latin"
            ),
            "m-plus-code-latin-400-normal must NOT be latin-subset"
        );
        assert!(!filename_has_latin_subset(
            "anek-latin-700-italic.ttf",
            "anek-latin"
        ));
        // True Fontsource subset after a normal slug.
        assert!(
            filename_has_latin_subset("roboto-latin-400-normal.ttf", "roboto"),
            "roboto-latin-400-normal must BE latin-subset"
        );
        // Edge: real subset token after a slug that itself ends in latin.
        assert!(
            filename_has_latin_subset(
                "m-plus-code-latin-latin-400-normal.ttf",
                "m-plus-code-latin"
            ),
            "m-plus-code-latin-latin-400-normal must BE latin-subset"
        );
        assert!(filename_has_latin_subset(
            "roboto-latin-ext-400-normal.ttf",
            "roboto"
        ));
        assert!(filename_has_latin_subset(
            "noto-sans-jp-japanese-latin-400-normal.ttf",
            "noto-sans-jp"
        ));
    }

    #[test]
    fn fontsource_face_filename_never_embeds_latin() {
        assert_eq!(
            fontsource_face_filename("libre-baskerville", "latin", 400, "normal"),
            "libre-baskerville-400-normal.ttf"
        );
        assert_eq!(
            fontsource_face_filename("noto-sans-jp", "japanese-latin", 400, "normal"),
            "noto-sans-jp-400-normal.ttf"
        );
        assert!(
            !filename_has_latin_subset(
                &fontsource_face_filename("roboto", "latin-ext", 700, "italic"),
                "roboto"
            ),
            "latin-ext must not put latin in the filename"
        );
        // Non-latin script subset may keep its token:
        let cjk = fontsource_face_filename("chiron-sung-hk", "chinese-hongkong", 400, "normal");
        assert_eq!(cjk, "chiron-sung-hk-chinese-hongkong-400-normal.ttf");
        assert!(!filename_has_latin_subset(&cjk, "chiron-sung-hk"));
    }

    #[test]
    fn pick_weights_keeps_all_advertised() {
        let w = pick_fontsource_weights(&[100, 400, 500, 700]);
        assert_eq!(w, vec![100, 400, 500, 700]);
    }

    #[test]
    fn fontsource_styles_italic_only_does_not_invent_normal() {
        // Syne Italic: API styles=["italic"] only — must not schedule 400-normal.
        let styles = fontsource_styles_from_meta(&["italic".into()]);
        assert_eq!(styles, vec![true]);
        let expected = 1usize; // 1 subset × 1 weight × 1 style
        assert_eq!(
            expected,
            1usize.saturating_mul(1).saturating_mul(styles.len())
        );
    }

    #[test]
    fn fontsource_styles_normal_and_italic() {
        let styles = fontsource_styles_from_meta(&["normal".into(), "italic".into()]);
        assert_eq!(styles, vec![false, true]);
    }

    #[test]
    fn fontsource_styles_normal_only() {
        let styles = fontsource_styles_from_meta(&["normal".into()]);
        assert_eq!(styles, vec![false]);
    }

    #[test]
    fn fontsource_abort_skips_italic_only_and_dual_style() {
        // Italic-only: never abort on a normal miss (normal is not scheduled).
        assert!(!fontsource_abort_on_normal_404(&[true], true, 400, false));
        // Dual-style: keep going after 400-normal 404 so italic can download.
        assert!(!fontsource_abort_on_normal_404(&[false, true], true, 400, false));
        // Normal-only missing package: fast abort.
        assert!(fontsource_abort_on_normal_404(&[false], true, 400, false));
        // Non-first subset / non-400 / italic miss: never abort.
        assert!(!fontsource_abort_on_normal_404(&[false], false, 400, false));
        assert!(!fontsource_abort_on_normal_404(&[false], true, 700, false));
        assert!(!fontsource_abort_on_normal_404(&[false, true], true, 400, true));
    }

    #[test]
    fn ttf_urls_prefers_latest_before_pinned() {
        let urls = ttf_urls("syne-italic", "2.76", 400, true, "latin", 0);
        assert!(
            urls[0].contains("@latest/"),
            "first URL must be @latest, got {}",
            urls[0]
        );
        assert!(
            urls.iter().any(|u| u.contains("@2.76/")),
            "pinned version should still be tried after @latest"
        );
        let latest_only = ttf_urls("syne-italic", "", 400, true, "latin", 0);
        assert!(latest_only[0].contains("@latest/"));
        assert_eq!(
            latest_only.iter().filter(|u| u.contains("cdn.jsdelivr.net/fontsource")).count(),
            1,
            "empty pin must not duplicate @latest"
        );
        let sauce = ttf_urls("open-sauce-sans", "5.3.0", 400, false, "latin", 0);
        assert!(sauce[0].contains("@latest/"));
        assert!(
            sauce.iter().any(|u| u.contains("open-sauce-sans@5.3.0/")),
            "npm pin 5.3.0 must be tried after @latest: {sauce:?}"
        );
        assert!(
            !sauce.iter().any(|u| u.contains("@1.477/")),
            "foundry version must not be used as a jsDelivr pin"
        );
        assert!(
            sauce.iter().any(|u| u.contains("marcologous/Open-Sauce-Fonts")
                && u.contains("OpenSauceSans-Regular.ttf")),
            "Open Sauce must fall back to GitHub TTFs after jsDelivr non-TTF bodies: {sauce:?}"
        );
        let sauce_i = ttf_urls("open-sauce-sans", "5.3.0", 900, true, "latin", 0);
        assert!(
            sauce_i.iter().any(|u| u.contains("OpenSauceSans-BlackItalic.ttf")),
            "900 italic upstream: {sauce_i:?}"
        );
        let cs = ttf_urls("clear-sans", "5.3.0", 400, false, "latin", 0);
        assert!(
            cs.iter().all(|u| u.contains("intel/clear-sans")),
            "clear-sans gated to Intel: {cs:?}"
        );
        assert!(!ttf_magic(&[0x80, 0x01, 0x33, 0x11]));
        assert!(ttf_magic(b"\x00\x01\x00\x00"));
    }

    #[test]
    fn clear_sans_urls_use_intel_not_fontsource_cdn() {
        let urls = ttf_urls("clear-sans", "5.3.0", 400, false, "latin", 0);
        assert!(
            !urls.is_empty(),
            "clear-sans must have Intel URLs"
        );
        assert!(
            urls.iter().all(|u| u.contains("intel/clear-sans") && u.contains("ClearSans-Regular.ttf")),
            "exclusive Intel Regular TTFs, no Fontsource CDN: {urls:?}"
        );
        assert!(
            !urls.iter().any(|u| u.contains("fontsource") || u.contains("@fontsource")),
            "must not treat Fontsource clear-sans as GDI source: {urls:?}"
        );
        let bold = ttf_urls("clear-sans", "", 700, false, "latin", 0);
        assert!(
            bold.iter().any(|u| u.contains("ClearSans-Bold.ttf")),
            "{bold:?}"
        );
        let italic = ttf_urls("clear-sans", "", 400, true, "latin", 0);
        assert!(
            italic.iter().any(|u| u.contains("ClearSans-Italic.ttf")),
            "{italic:?}"
        );
        assert!(clear_sans_intel_ttf_urls(100, true, "").is_empty(), "no ThinItalic upstream");
        assert!(clear_sans_intel_ttf_urls(300, true, "").is_empty(), "no LightItalic upstream");
    }

    #[test]
    fn clear_sans_planned_is_intel_eight_not_fontsource_matrix() {
        // Fontsource clear-sans meta: weights [100,300,400,500,700] × italic = 10.
        // Intel pin ships exactly 8 TTFs — planned/expected must match Intel only.
        assert_eq!(clear_sans_intel_planned_count(), 8);
        assert_eq!(CLEAR_SANS_INTEL_FACES.len(), 8);
        for (w, italic) in CLEAR_SANS_INTEL_FACES {
            assert!(
                clear_sans_style_token(*w, *italic).is_some(),
                "Intel face missing style token: {w} italic={italic}"
            );
        }
        // Faces Fontsource matrix would demand but Intel does not ship:
        assert!(clear_sans_style_token(100, true).is_none());
        assert!(clear_sans_style_token(300, true).is_none());
        let keys = clear_sans_intel_face_keys();
        assert_eq!(keys.len(), 8);
        assert!(keys.contains(&"clear-sans-100-normal.ttf".into()));
        assert!(keys.contains(&"clear-sans-300-normal.ttf".into()));
        assert!(keys.contains(&"clear-sans-400-normal.ttf".into()));
        assert!(keys.contains(&"clear-sans-500-normal.ttf".into()));
        assert!(keys.contains(&"clear-sans-700-normal.ttf".into()));
        assert!(keys.contains(&"clear-sans-400-italic.ttf".into()));
        assert!(keys.contains(&"clear-sans-500-italic.ttf".into()));
        assert!(keys.contains(&"clear-sans-700-italic.ttf".into()));
        assert!(!keys.iter().any(|k| k.contains("100-italic") || k.contains("300-italic")));
        // Catalog/FS matrix size must not be used as the plan.
        let fs_matrix = 5usize * 2; // weights × italic from fontsource-other
        assert_ne!(clear_sans_intel_planned_count(), fs_matrix);
    }

    #[test]
    fn clear_sans_rewrites_stale_expected_ten_to_intel_eight() {
        let parent = temp_family_dir("clear-sans-expected-8");
        let dir = parent.join("Clear Sans");
        fs::create_dir_all(&dir).unwrap();
        // Sticky Fontsource plan:
        write_expected_faces(&dir, 10);
        let _ = fs::write(dir.join(".complete"), b"10");
        // 8 Intel-sized keys on disk (outside the Fontsource shred band).
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(305 * 1024, 0);
        for (w, italic) in CLEAR_SANS_INTEL_FACES {
            let style = if *italic { "italic" } else { "normal" };
            let name = google_face_filename("clear-sans", &w.to_string(), style);
            fs::write(dir.join(name), &fake).unwrap();
        }
        // Leftover ThinItalic demand must not keep expected at 10.
        fs::write(dir.join("clear-sans-100-italic.ttf"), &fake).unwrap();

        heal_clear_sans_expected_plan(&dir, "Clear Sans");
        assert_eq!(read_expected_faces(&dir), Some(8));
        assert!(dir_is_complete(&dir), "8/8 Intel plan satisfied → .complete");
        let h = family_disk_honesty_in(&dir, "Clear Sans");
        assert_eq!(h.expected, Some(8));
        assert!(h.has_complete);
        assert!(h.intact >= 8);
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn os2_fstype_detects_restricted_embedding() {
        // Minimal SFNT with one OS/2 table, fsType=4 at offset 8 of table.
        let mut bytes = vec![0u8; 12 + 16 + 16];
        bytes[0..4].copy_from_slice(b"\x00\x01\x00\x00");
        bytes[4] = 0;
        bytes[5] = 1; // one table
        bytes[12..16].copy_from_slice(b"OS/2");
        let toff: u32 = 28;
        bytes[20..24].copy_from_slice(&toff.to_be_bytes());
        bytes[24..28].copy_from_slice(&16u32.to_be_bytes());
        bytes.resize(28 + 16, 0);
        bytes[28 + 8] = 0;
        bytes[28 + 9] = 4; // fsType = 4
        assert_eq!(os2_fstype(&bytes), Some(4));
        assert!(os2_fstype(&bytes).map(|fs| fs & 0x0006 != 0).unwrap_or(false));
        assert_eq!(os2_fstype(b"wOFF"), None);
    }

    #[test]
    fn clear_sans_odd_size_needs_intel_heal() {
        let parent = temp_family_dir("clear-sans-heal");
        let dir = parent.join("Clear Sans");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("clear-sans-400-normal.ttf");
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(67 * 1024, 0);
        fs::write(&path, &fake).unwrap();
        assert!(clear_sans_face_needs_intel_heal(&path));
        fake.resize(305 * 1024, 0);
        fs::write(&path, &fake).unwrap();
        // Large SFNT without restricted OS/2 — heal not required by size band.
        assert!(!clear_sans_face_needs_intel_heal(&path));
        assert!(is_clear_sans_family("Clear Sans"));
        assert!(family_known_gdi_session_incapable("Gidugu"));
        assert!(!family_known_gdi_session_incapable("Nunito"));
        assert!(known_gdi_incapable_entry("gidugu").is_some());
        assert_eq!(fontsource_gdi_offer_slug("Gidugu"), "gidugu");
        assert_eq!(fontsource_gdi_offer_subsets("Gidugu"), &["telugu", "latin"][..]);
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn known_gdi_incapable_allowlist_is_table_not_single_hardcode() {
        // 1.0.186: shared table — Gidugu first; append rows for future bad fonts.
        assert!(!KNOWN_GDI_SESSION_INCAPABLE.is_empty());
        assert!(
            KNOWN_GDI_SESSION_INCAPABLE
                .iter()
                .any(|e| e.family.eq_ignore_ascii_case("Gidugu")),
            "Gidugu must remain first-class allowlist entry"
        );
        assert!(family_known_gdi_session_incapable("GIDUGU"));
        assert!(!family_known_gdi_session_incapable("Roboto"));
        let urls = fontsource_gdi_offer_ttf_urls("Gidugu");
        assert!(urls.iter().any(|u| u.contains("gidugu") && u.contains("telugu")));
        assert!(urls
            .iter()
            .any(|u| u.contains("@fontsource/gidugu") || u.contains("fontsource/fonts/gidugu")));
        let dest = fontsource_face_filename(&fontsource_gdi_offer_slug("Gidugu"), "latin", 400, "normal");
        assert_eq!(dest, "gidugu-400-normal.ttf");
    }

    #[test]
    fn soft_settled_requires_provenance_not_bare_complete() {
        // P1 206e: bare .complete (hard-emoji tip upgrade) ≠ Settled; provenance after Add=0 does.
        assert!(!soft_scan_settled_from_provenance(false, true, true, false));
        assert!(soft_scan_settled_from_provenance(true, true, true, false));
        assert!(!soft_scan_settled_from_provenance(true, false, true, false));
        assert!(!soft_scan_settled_from_provenance(true, true, true, true));
        // Soft full-face without provenance: not Incomplete / not Repair.
        assert!(!soft_scan_incomplete(false, true, true, true, false, false, false));
        // Soft Settled with provenance: not Incomplete.
        assert!(!soft_scan_incomplete(true, true, true, true, true, false, false));
        // Soft undersized: Incomplete.
        assert!(soft_scan_incomplete(false, true, false, true, false, false, true));
        // Non-soft missing .complete: Incomplete.
        assert!(soft_scan_incomplete(false, false, true, true, false, false, false));

        let parent = temp_family_dir("soft-prov-206e");
        let dir = parent.join("Noto Color Emoji");
        fs::create_dir_all(&dir).unwrap();
        let mut full = b"\x00\x01\x00\x00".to_vec();
        full.resize(300 * 1024, 0);
        fs::write(dir.join("NotoColorEmoji.ttf"), &full).unwrap();
        // Bare .complete without provenance — wipe + exclude repair.
        mark_family_complete(&dir, 1);
        assert!(dir_is_complete(&dir));
        assert!(!dir_has_settled_add_zero_provenance(&dir));
        wipe_soft_complete_lacking_provenance(&dir, "Noto Color Emoji");
        assert!(!dir_is_complete(&dir), "upgrade wipe bare soft .complete");
        assert!(soft_full_face_exclude_repair(&dir, "Noto Color Emoji"));
        // After real Add=0 stamp: provenance + Settled.
        stamp_soft_settle_dir_after_add_zero(&dir, "Noto Color Emoji");
        assert!(dir_is_complete(&dir));
        assert!(dir_has_settled_add_zero_provenance(&dir));
        assert!(!soft_full_face_exclude_repair(&dir, "Noto Color Emoji"));
        assert!(soft_scan_settled_from_provenance(true, true, true, false));
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn soft_power_retry_clears_session_gdi_refuse_so_add_can_run() {
        // P1 206f Skye HOLD: after soft Add=0 refuse, Power Retry must clear refuse
        // so family_early_skip_soft_session_refused no longer returns AddReturnedZero without Add.
        let fam = "Noto Color Emoji";
        clear_session_gdi_refused(fam);
        assert!(!family_session_gdi_refused(fam));
        assert!(family_soft_try_add_then_settle(fam));

        note_session_gdi_refused(fam);
        assert!(family_session_gdi_refused(fam));
        assert!(soft_early_skip_from_session_refuse(true, true, true));
        assert!(!soft_retry_allows_add_after_clear(true, false));

        // Soft Retry path (UI / clear_session_gdi_refused_family).
        clear_session_gdi_refused(fam);
        assert!(!family_session_gdi_refused(fam), "Retry must clear session refuse");
        assert!(soft_retry_allows_add_after_clear(true, true));
        assert!(
            !soft_early_skip_from_session_refuse(true, family_session_gdi_refused(fam), true),
            "after clear, soft early-skip must not block Add"
        );

        // Refuse can be re-noted after another Add=0 (Activate All skip still works).
        note_session_gdi_refused(fam);
        assert!(family_session_gdi_refused(fam));
        clear_session_gdi_refused(fam);

        // Non-settle family: note is a no-op.
        note_session_gdi_refused("Nunito");
        assert!(!family_session_gdi_refused("Nunito"));
    }

    #[test]
    fn emoji_allowlist_and_upstream_urls_for_settled_honesty() {
        // 1.0.206c/d: emoji soft try-Add-first — NOT hard allowlist (Gidugu-only).
        // Soft table SOFT_GDI_TRY_ADD_FIRST is the single Rust source (TS mirror).
        assert!(!SOFT_GDI_TRY_ADD_FIRST.is_empty());
        assert_eq!(SOFT_GDI_TRY_ADD_FIRST.len(), 2);
        assert!(SOFT_GDI_TRY_ADD_FIRST.iter().any(|e| e.family == "Noto Color Emoji"));
        assert!(SOFT_GDI_TRY_ADD_FIRST.iter().any(|e| e.family == "Noto Emoji"));
        assert!(!family_known_gdi_session_incapable("Noto Color Emoji"));
        assert!(!family_known_gdi_session_incapable("Noto Emoji"));
        assert!(family_soft_try_add_then_settle("Noto Color Emoji"));
        assert!(family_may_settle_add_zero("Noto Color Emoji"));
        assert!(family_may_settle_add_zero("Noto Emoji"));
        assert!(family_known_gdi_session_incapable("Gidugu"));
        assert!(is_emoji_session_family("Noto Color Emoji"));
        assert!(is_emoji_session_family("noto emoji"));
        assert!(!is_emoji_session_family("Nunito"));
        let urls = ttf_urls("noto-color-emoji", "", 400, false, "emoji", 0);
        assert!(
            urls.iter().any(|u| u.contains("NotoColorEmoji.ttf")),
            "noto-color-emoji must use full upstream color TTF"
        );
        assert!(
            urls.iter().all(|u| !u.contains("fontsource-variable")),
            "never WOFF2 variable pack for emoji install"
        );
    }

    #[test]
    fn remnant_purge_unblocks_early_skip_for_known_incapable() {
        // 1.0.188: full Google face + undersized FS offer dest + latin-named orphan.
        // Purge drops remnants; undersized must not block early-skip predicate.
        let parent = temp_family_dir("gidugu-remnant-purge-188");
        let dir = parent.join("Gidugu");
        fs::create_dir_all(&dir).unwrap();
        let mut full = b"\x00\x01\x00\x00".to_vec();
        full.resize(460988, 0);
        fs::write(dir.join("Gidugu-Regular.ttf"), &full).unwrap();
        let mut tiny = b"\x00\x01\x00\x00".to_vec();
        tiny.resize(38404, 0);
        // Legacy try_fontsource dest (subset "latin" → google-shaped name).
        fs::write(dir.join("gidugu-400-normal.ttf"), &tiny).unwrap();
        fs::write(dir.join("gidugu-latin-400-normal.ttf"), &tiny).unwrap();
        fs::write(dir.join("gidugu-telugu-400-normal.ttf"), &tiny).unwrap();
        assert!(dir_has_undersized_google_static(&dir, "Gidugu"));
        assert!(!early_skip_after_refuse_or_settled(
            true, true, true, true, true
        ));
        let n = purge_known_incapable_fontsource_remnants(&dir, "Gidugu");
        assert!(n >= 3, "purged {n}");
        assert!(!dir.join("gidugu-400-normal.ttf").exists());
        assert!(!dir.join("gidugu-latin-400-normal.ttf").exists());
        assert!(!dir.join("gidugu-telugu-400-normal.ttf").exists());
        assert!(dir.join("Gidugu-Regular.ttf").exists(), "keep full Google face");
        assert!(!dir_has_undersized_google_static(&dir, "Gidugu"));
        assert!(
            early_skip_after_refuse_or_settled(true, true, true, true, false),
            "after purge, early-skip must succeed"
        );
        // Offer command must not fetch/write Fontsource TTFs (source lock below in JS).
        assert!(fontsource_offer_activated_only_if_add(1));
        assert!(!fontsource_offer_activated_only_if_add(0));
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn family_skip_add_requires_this_process_loaded_and_count_match() {
        // Maps / last-session never skip walk. New file (count++) forces walk.
        assert!(
            !family_may_skip_add_this_process(false, 8, 8, true),
            "not in loaded() this process must walk+Add"
        );
        assert!(
            !family_may_skip_add_this_process(true, 9, 8, true),
            "new on-disk face must not skip walk"
        );
        assert!(
            !family_may_skip_add_this_process(true, 8, 8, false),
            "resized / unmatched maps must restage+Add"
        );
        assert!(
            !family_may_skip_add_this_process(true, 0, 0, true),
            "empty family must not skip"
        );
        assert!(
            family_may_skip_add_this_process(true, 8, 8, true),
            "same-session already-Add'd + count match + maps ⇒ skip walk"
        );
    }

    #[test]
    fn live_settled_library_honesty_gidugu_add_zero() {
        // Honest ceiling while Gidugu Add=0: Live 2099 · Settled 1 · Library 2100.
        assert_eq!(catalog_expected_gdi_live(2100, 0), 2100);
        assert_eq!(catalog_expected_gdi_live(2100, 1), 2099);
        assert_eq!(
            format_live_settled_library(2099, 1, 2100),
            "Live 2099 · Settled 1 · Library 2100"
        );
        assert!(family_known_gdi_session_incapable("Gidugu"));
        assert!(!family_may_claim_session_activated(true, 0));
        assert!(
            family_may_claim_session_activated(true, 1),
            "Add>0 may Activate — never from settle alone"
        );
        assert!(settled_implies_not_activated(true, 0));
        assert!(!settled_implies_not_activated(true, 1));
        assert!(!job_toast_is_fail(0));
        assert!(job_toast_is_fail(1));
        assert_eq!(job_downloaded_count(2100, 2099, 0, 1), 0);
        assert!(retry_must_attempt_register_before_settle(true));
        assert!(!retry_must_attempt_register_before_settle(false));
        assert!(retry_gidugu_settle_without_refetch(0, true, true, false));
        assert!(
            !retry_gidugu_settle_without_refetch(1, true, true, false),
            "Add>0 is live — do not settle"
        );
        assert!(
            !retry_gidugu_settle_without_refetch(0, true, true, true),
            "undersized remnant must Repair/fetch, not settle"
        );
        // Early-skip: refused OR already settled — no Activate All churn.
        assert!(early_skip_after_refuse_or_settled(true, true, false, true, false));
        assert!(early_skip_after_refuse_or_settled(true, false, true, true, false));
        assert!(!early_skip_after_refuse_or_settled(true, false, false, true, false));
        assert!(!early_skip_after_refuse_or_settled(true, true, true, true, true));
        assert!(!early_skip_after_refuse_or_settled(false, true, true, true, false));
        assert!(fontsource_offer_activated_only_if_add(1));
        assert!(!fontsource_offer_activated_only_if_add(0));
        let urls = gidugu_fontsource_ttf_urls();
        assert!(urls.iter().any(|u| u.contains("gidugu") && u.contains("telugu")));
        assert!(urls.iter().any(|u| u.contains("@fontsource/gidugu") || u.contains("fontsource/fonts/gidugu")));
    }

    #[test]
    fn progress_bar_never_shows_done_over_total() {
        // 1.0.181: skipped double-count showed 4044/2253.
        assert_eq!(progress_bar_done_total(4044, 2253), (4044, 4044));
        assert_eq!(progress_bar_done_total(2253, 2253), (2253, 2253));
        assert_eq!(progress_bar_done_total(0, 0), (0, 1));
        assert_eq!(progress_bar_done_total(10, 100), (10, 100));
    }

    #[test]
    fn face_loaded_this_session_may_skip_add() {
        assert!(!face_may_skip_add(false), "maps alone must not skip Add");
        assert!(face_may_skip_add(true), "this-session loaded() may skip re-Add");
    }

    #[test]
    fn size_matched_map_skips_copy_not_add() {
        // Size-matched gdi-map ⇒ skip re-copy only; never authorizes skip-Add.
        let parent = temp_family_dir("live-map-skip");
        let src = parent.join("src.ttf");
        let dest = parent.join("dest.ttf");
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(4096, 0);
        fs::write(&src, &fake).unwrap();
        fs::write(&dest, &fake).unwrap();
        let src_len = fs::metadata(&src).unwrap().len();
        let matched = fs::metadata(&dest)
            .map(|m| m.len() == src_len && src_len >= 256)
            .unwrap_or(false);
        assert!(matched, "size-matched map must allow skip-copy");
        // Map alone must NOT satisfy skip-Add (loaded() empty ⇒ must Add).
        assert!(
            !face_may_skip_add(false),
            "map-only must not skip AddFontResourceEx"
        );
        assert!(
            face_may_skip_add(true),
            "in-process loaded (real Add) may skip re-Add"
        );
        fs::write(&dest, &fake[..512]).unwrap();
        let mismatched = fs::metadata(&dest).map(|m| m.len() == src_len).unwrap_or(false);
        assert!(!mismatched);
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn skip_live_toast_requires_session_active_not_maps_alone() {
        // Sticky CJK toast fix: session-active + maps ⇒ no failed_names.
        // Maps alone (post-Quit leftover) must NOT claim live / suppress fail.
        assert!(
            !family_toast_exempt_already_live(false, true),
            "maps without session-active must not toast-exempt"
        );
        assert!(
            !family_toast_exempt_already_live(true, false),
            "session-active without maps must not toast-exempt"
        );
        assert!(
            family_toast_exempt_already_live(true, true),
            "session-active + maps ⇒ toast-only already-live"
        );
        assert!(!family_toast_exempt_already_live(false, false));
    }

    #[test]
    fn known_gdi_incapable_intact_skips_fail_toast() {
        // 1.0.173: Gidugu intact official TTF ⇒ not failed_names / Couldn't load.
        assert!(
            family_toast_exempt_known_gdi_incapable(
                family_known_gdi_session_incapable("Gidugu"),
                true,
            ),
            "known-incapable + intact must skip fail toast"
        );
        assert!(
            !family_toast_exempt_known_gdi_incapable(
                family_known_gdi_session_incapable("Gidugu"),
                false,
            ),
            "known-incapable without intact is still a real fail"
        );
        assert!(
            !family_toast_exempt_known_gdi_incapable(
                family_known_gdi_session_incapable("Nunito"),
                true,
            ),
            "capable families with intact files still use normal fail path"
        );
        assert!(family_toast_exempt_known_gdi_incapable(true, true));
        assert!(!family_toast_exempt_known_gdi_incapable(true, false));
        assert!(!family_toast_exempt_known_gdi_incapable(false, true));
    }

    #[test]
    fn known_incapable_intact_disk_settled_scan_not_incomplete() {
        // 1.0.175: known-incapable + intact full-size ⇒ verify/scan treats complete;
        // still never session-activated / markLiveActivated.
        assert!(
            family_disk_settled_known_gdi_incapable(
                family_known_gdi_session_incapable("Gidugu"),
                true,
                false,
            ),
            "intact full-size Gidugu must be disk settled"
        );
        assert!(
            !family_disk_settled_known_gdi_incapable(
                family_known_gdi_session_incapable("Gidugu"),
                true,
                true,
            ),
            "undersized remnant must not settle"
        );
        assert!(
            !family_disk_settled_known_gdi_incapable(
                family_known_gdi_session_incapable("Nunito"),
                true,
                false,
            ),
            "capable families are not auto-settled"
        );
        assert!(
            !family_may_claim_session_activated(
                family_known_gdi_session_incapable("Gidugu"),
                0,
            ),
            "Add=0 must not claim Activated"
        );
        assert!(
            family_may_claim_session_activated(
                family_known_gdi_session_incapable("Gidugu"),
                1,
            ),
            "Gidugu Add>0 is live — never from settle alone"
        );
        assert!(family_may_claim_session_activated(false, 1));

        let parent = temp_family_dir("gidugu-disk-settled");
        let dir = parent.join("Gidugu");
        fs::create_dir_all(&dir).unwrap();
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(460988, 0); // full official size, not 24–80KB remnant band
        fs::write(dir.join("gidugu-400-normal.ttf"), &fake).unwrap();
        write_google_planned(&dir, &["gidugu-400-normal.ttf".into()]);
        assert!(!dir_has_undersized_google_static(&dir, "Gidugu"));
        stamp_known_incapable_dir_settled(&dir, "Gidugu");
        assert!(dir_is_complete(&dir), "settle must stamp .complete");
        verify_complete_marker(&dir);
        assert!(
            dir_is_complete(&dir),
            "settled .complete must survive verify (Scan honesty)"
        );
        let has_complete = dir_is_complete(&dir);
        let undersized = dir_has_undersized_google_static(&dir, "Gidugu");
        let incomplete = if family_disk_settled_known_gdi_incapable(
            family_known_gdi_session_incapable("Gidugu"),
            true,
            undersized,
        ) {
            false
        } else {
            !has_complete || undersized
        };
        assert!(!incomplete, "Scan must not flag Repair for settled Gidugu");
        assert!(
            family_toast_exempt_known_gdi_incapable(true, true),
            "1.0.173 toast suppress still intact"
        );
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn ensure_gdi_session_copy_reuses_size_matched_without_loaded() {
        // Skip-copy still works when maps exist and loaded() is empty.
        let parent = temp_family_dir("skip-copy-only");
        let maps = parent.join("gdi-maps");
        fs::create_dir_all(&maps).unwrap();
        let src = parent.join("Face.ttf");
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(4096, 0);
        fs::write(&src, &fake).unwrap();
        let dest = ensure_gdi_session_copy_to(&src, &maps).expect("first copy");
        assert!(dest.is_file());
        let dest2 = ensure_gdi_session_copy_to(&src, &maps).expect("reuse size-matched");
        assert_eq!(dest, dest2);
        // Reuse is skip-copy — still does not imply skip-Add.
        assert!(!face_may_skip_add(false));
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn register_fail_kind_label_in_failed_details() {
        assert_eq!(
            RegisterFailKind::AddReturnedZero.label(),
            "AddFontResourceExW returned 0"
        );
        assert!(family_known_gdi_session_incapable("gidugu"));
        let msg = format!(
            "Gidugu — files on disk 1/1, .complete=no, GDI live 0 ({}), Windows refused this face (known GDI-incapable for session install)",
            RegisterFailKind::AddReturnedZero.label()
        );
        assert!(msg.contains("Windows refused this face"));
        assert!(msg.contains("AddFontResourceExW returned 0"));
    }

    #[test]
    fn fontsource_pin_prefers_npm_over_foundry_version() {
        assert_eq!(fontsource_jsdelivr_pin("5.3.0", "v1.477"), "5.3.0");
        assert_eq!(fontsource_jsdelivr_pin("", "v1.477"), "1.477");
        assert_eq!(fontsource_jsdelivr_pin("latest", "2.76"), "2.76");
    }

    #[test]
    fn parse_metadata_accepts_variablefont_and_bracket_filenames() {
        let meta = r#"
filename: "Sora-VariableFont_wght.ttf"
filename: "Sora-Italic-VariableFont_wght.ttf"
filename: "Sora-Regular.ttf"
filename: "Nunito[wght].ttf"
"#;
        let (_axes, files) = parse_metadata_pb_axes_and_files(meta);
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"Sora-VariableFont_wght.ttf"));
        assert!(names.contains(&"Sora-Italic-VariableFont_wght.ttf"));
        assert!(names.contains(&"Nunito[wght].ttf"));
        assert!(!names.iter().any(|n| n.contains("Regular")));
        let italic = files
            .iter()
            .find(|(n, _)| n.contains("Italic"))
            .map(|(_, i)| *i);
        assert_eq!(italic, Some(true));
    }

    #[test]
    fn pick_subsets_prefers_cjk_over_latin() {
        let all = vec![
            "latin".into(),
            "chinese-hongkong".into(),
            "vietnamese".into(),
        ];
        let got = pick_subsets(&all);
        assert_eq!(got, vec!["chinese-hongkong".to_string()]);
    }

    #[test]
    fn pick_subsets_prefers_emoji_over_latin() {
        // Completeness P0: emoji script subset, never latin-only as enough.
        let all = vec!["latin".into(), "emoji".into()];
        let got = pick_subsets(&all);
        assert_eq!(got, vec!["emoji".to_string()]);
    }

    #[test]
    fn noto_color_emoji_skips_css_latin_plan() {
        assert!(is_noto_color_emoji_family("Noto Color Emoji", "noto-color-emoji"));
        assert!(is_noto_color_emoji_family("noto color emoji", "other-slug"));
        assert!(!is_noto_color_emoji_family("Noto Emoji", "noto-emoji"));
        // Upstream URLs only — never Fontsource latin stub list for color emoji.
        let urls = ttf_urls("noto-color-emoji", "", 400, false, "emoji", 0);
        assert!(urls.iter().all(|u| u.contains("NotoColorEmoji")));
        assert!(urls.iter().any(|u| u.contains("NotoColorEmoji.ttf")));
        assert!(urls.iter().any(|u| u.contains("NotoColorEmoji_WindowsCompatible.ttf")));
        assert!(urls.iter().all(|u| !u.contains("-latin-")));
    }

    #[test]
    fn parse_css_joins_range_weight_tokens() {
        let css = r#"
@font-face {
  font-family: 'Nunito';
  font-style: normal;
  font-weight: 200 1000;
  src: url(https://example.com/nunito.ttf);
}
"#;
        let faces = parse_css_faces(css);
        assert_eq!(faces.len(), 1);
        assert_eq!(faces[0].1, "200-1000");
    }

    #[test]
    fn parse_css_keeps_ttf_and_medium_italic() {
        let css = r#"
@font-face {
  font-family: 'Cormorant Garamond';
  font-style: italic;
  font-weight: 500;
  src: url(https://fonts.gstatic.com/s/cormorantgaramond/v16/foo.ttf) format('truetype');
}
@font-face {
  font-family: 'Cormorant Garamond';
  font-style: normal;
  font-weight: 400;
  src: url(https://fonts.gstatic.com/s/cormorantgaramond/v16/bar.woff2) format('woff2');
}
"#;
        let faces = parse_css_faces(css);
        assert_eq!(faces.len(), 1, "woff2 must be skipped; one TTF kept");
        assert_eq!(faces[0].0, "italic");
        assert_eq!(faces[0].1, "500");
    }

    #[test]
    fn css_range_prefers_full_over_latin() {
        let latin = "font-style: normal; font-weight: 400; unicode-range: U+0000-00FF; src: url(https://x/a.ttf);";
        let full = "font-style: normal; font-weight: 400; src: url(https://x/b.ttf);";
        assert!(css_range_rank(full) > css_range_rank(latin));
    }

    #[test]
    fn axis_richness_ranks_static_above_bare() {
        let static_axis = static_weight_axis();
        assert!(axis_richness(&static_axis) > axis_richness(""));
        assert!(axis_richness(&static_axis) > axis_richness("wght@100..900"));
        assert!(axis_richness("ital,wght@0,100..900;1,100..900") > axis_richness(""));
    }

    #[test]
    fn richer_listing_prefers_face_count_then_axis() {
        // Simulate pick: 18-face static beats 1-face bare even if bare was seen first.
        let bare_len = 1usize;
        let static_len = 18usize;
        let bare_rank = axis_richness("");
        let static_rank = axis_richness(&static_weight_axis());
        let bare_better = bare_len > static_len
            || (bare_len == static_len && bare_rank > static_rank);
        let static_better = static_len > bare_len
            || (static_len == bare_len && static_rank > bare_rank);
        assert!(!bare_better);
        assert!(static_better);
    }

    #[test]
    fn google_face_key_rejects_latin_subset_names() {
        let slug = "chiron-sung-hk";
        assert!(is_google_face_key("chiron-sung-hk-400-normal.ttf", slug));
        assert!(is_google_face_key("chiron-sung-hk-700-italic.ttf", slug));
        assert!(is_google_face_key("chiron-sung-hk-100-900-normal.ttf", slug));
        assert!(
            !is_google_face_key("chiron-sung-hk-latin-400-normal.ttf", slug),
            "*-latin-* must never count toward Google planned"
        );
        assert!(!is_google_face_key(
            "chiron-sung-hk-chinese-hongkong-400-normal.ttf",
            slug
        ));
        assert!(!is_google_face_key("other-family-400-normal.ttf", slug));
    }

    #[test]
    fn latin_padding_does_not_satisfy_google_planned_count() {
        use std::fs;
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "fm-google-keys-{}-{}",
            std::process::id(),
            nanos
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // Dir name slug-hint: "Chiron Sung HK" → chiron-sung-hk
        let family_dir = dir.join("Chiron Sung HK");
        fs::create_dir_all(&family_dir).unwrap();
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(256, 0);
        // Only 1 Google face key + latin padding that would falsely hit planned=2.
        fs::write(family_dir.join("chiron-sung-hk-400-normal.ttf"), &fake).unwrap();
        fs::write(
            family_dir.join("chiron-sung-hk-latin-400-normal.ttf"),
            &fake,
        )
        .unwrap();
        // Key list (not bare count) — verify must require these exact files.
        fs::write(
            family_dir.join(".google-planned"),
            b"chiron-sung-hk-400-normal.ttf\nchiron-sung-hk-700-normal.ttf\n",
        )
        .unwrap();
        fs::write(family_dir.join(".expected"), b"2").unwrap();
        fs::write(family_dir.join(".complete"), b"2").unwrap();
        assert_eq!(count_intact_faces(&family_dir), 2);
        assert_eq!(count_intact_toward_expected(&family_dir), 1);
        verify_complete_marker(&family_dir);
        assert!(
            !family_complete_marker(&family_dir).is_file(),
            "latin padding must not keep a Google-planned .complete"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_requires_listed_keys_not_any_google_shaped() {
        use std::fs;
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "fm-listed-keys-{}-{}",
            std::process::id(),
            nanos
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let family_dir = dir.join("Roboto");
        fs::create_dir_all(&family_dir).unwrap();
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(256, 0);
        // Listed keys want 400 + 700; disk has 400 + wrong google-shaped 500 (pad hole).
        fs::write(family_dir.join("roboto-400-normal.ttf"), &fake).unwrap();
        fs::write(family_dir.join("roboto-500-normal.ttf"), &fake).unwrap();
        fs::write(
            family_dir.join(".google-planned"),
            b"roboto-400-normal.ttf\nroboto-700-normal.ttf\n",
        )
        .unwrap();
        fs::write(family_dir.join(".expected"), b"2").unwrap();
        fs::write(family_dir.join(".complete"), b"2").unwrap();
        assert_eq!(count_intact_google_face_keys(&family_dir, "roboto"), 2);
        assert_eq!(count_intact_toward_expected(&family_dir), 1);
        verify_complete_marker(&family_dir);
        assert!(
            !family_complete_marker(&family_dir).is_file(),
            "any-google-shaped pad must not keep stamp when listed key missing"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_bare_count_google_planned_does_not_trust_pad() {
        use std::fs;
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "fm-legacy-gp-{}-{}",
            std::process::id(),
            nanos
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let family_dir = dir.join("Roboto");
        fs::create_dir_all(&family_dir).unwrap();
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(256, 0);
        fs::write(family_dir.join("roboto-400-normal.ttf"), &fake).unwrap();
        fs::write(family_dir.join("roboto-latin-400-normal.ttf"), &fake).unwrap();
        fs::write(family_dir.join(".google-planned"), b"2").unwrap(); // legacy bare count
        fs::write(family_dir.join(".expected"), b"2").unwrap();
        fs::write(family_dir.join(".complete"), b"2").unwrap();
        assert_eq!(count_intact_toward_expected(&family_dir), 0);
        verify_complete_marker(&family_dir);
        assert!(!family_complete_marker(&family_dir).is_file());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_uses_keys_len_not_understated_expected() {
        use std::fs;
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "fm-understated-{}-{}",
            std::process::id(),
            nanos
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let family_dir = dir.join("Roboto");
        fs::create_dir_all(&family_dir).unwrap();
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(256, 0);
        // One listed key present; .expected understates keys.len()=2 as 1.
        fs::write(family_dir.join("roboto-400-normal.ttf"), &fake).unwrap();
        fs::write(
            family_dir.join(".google-planned"),
            "roboto-400-normal.ttf\nroboto-700-normal.ttf\n".as_bytes(),
        )
        .unwrap();
        fs::write(family_dir.join(".expected"), b"1").unwrap();
        fs::write(family_dir.join(".complete"), b"1").unwrap();
        assert_eq!(count_intact_toward_expected(&family_dir), 1);
        verify_complete_marker(&family_dir);
        assert!(
            !family_complete_marker(&family_dir).is_file(),
            "keys.len() must win over understated .expected"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}


#[cfg(test)]
mod session_sidecar_tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "fm-session-{}-{}-{}",
            label,
            std::process::id(),
            nanos
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parse_session_paths_skips_blank_lines() {
        let got = parse_session_paths_text("a.ttf\n\n  b.ttf  \n\n");
        assert_eq!(got.len(), 2);
        assert!(got[0].ends_with("a.ttf"));
        assert!(got[1].ends_with("b.ttf"));
    }

    #[test]
    fn clear_session_sidecars_removes_paths_and_active() {
        let root = temp_root("clear");
        fs::write(session_paths_file_in(&root), "C:\\a.ttf\n").unwrap();
        fs::write(session_active_file_in(&root), b"[\"Nunito\"]\n").unwrap();
        crate::session_stage::save_session_maps_in(
            &root,
            &[crate::session_stage::FaceMap {
                source: "s".into(),
                stage: "t".into(),
                registry_name: String::new(),
                family: String::new(),
            }],
        );
        assert!(session_paths_file_in(&root).is_file());
        assert!(session_active_file_in(&root).is_file());
        assert!(crate::session_stage::session_maps_file_in(&root).is_file());
        clear_session_sidecars_in(&root);
        assert!(!session_paths_file_in(&root).is_file());
        assert!(!session_active_file_in(&root).is_file());
        assert!(!crate::session_stage::session_maps_file_in(&root).is_file());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn gdi_map_file_name_is_stable_and_not_the_original() {
        let a = PathBuf::from(
            r"C:\Users\Eric\Documents\Font Manager\Open Sauce Sans\open-sauce-sans-400-normal.ttf",
        );
        let b = PathBuf::from(
            "C:/Users/Eric/Documents/Font Manager/Open Sauce Sans/open-sauce-sans-400-normal.ttf",
        );
        let na = gdi_map_file_name(&a);
        let nb = gdi_map_file_name(&b);
        assert_eq!(na, nb, "slash direction must not change the map name");
        assert!(na.ends_with(".ttf"));
        assert!(!na.to_ascii_lowercase().contains("sauce"));
        assert!(!is_gdi_maps_dir_name(&a));
        let map = PathBuf::from(r"C:\Users\Eric\AppData\Local\Font Manager\gdi-maps\abcd.ttf");
        assert!(is_gdi_maps_dir_name(&map));
    }

    #[test]
    fn ensure_gdi_session_copy_leaves_documents_original() {
        let root = temp_root("gdi-copy");
        let src = root.join("Open Sauce Sans").join("open-sauce-sans-400-normal.ttf");
        fs::create_dir_all(src.parent().unwrap()).unwrap();
        fs::write(&src, vec![0u8; 300]).unwrap();
        let maps = root.join("gdi-maps");
        let dest = ensure_gdi_session_copy_to(&src, &maps).expect("stage copy");
        assert!(src.is_file(), "Documents original must remain");
        assert_ne!(dest, src, "GDI must not map the Documents path");
        assert!(dest.is_file());
        assert_eq!(
            fs::metadata(&src).unwrap().len(),
            fs::metadata(&dest).unwrap().len()
        );
        let dest2 = ensure_gdi_session_copy_to(&src, &maps).expect("reuse");
        assert_eq!(dest, dest2, "same size must reuse the map");
        fs::write(&src, vec![1u8; 400]).unwrap();
        let dest3 = ensure_gdi_session_copy_to(&src, &maps).expect("resize");
        assert_eq!(dest3, dest);
        assert_eq!(fs::metadata(&dest3).unwrap().len(), 400);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn save_load_roundtrip_session_paths() {
        let root = temp_root("roundtrip");
        let paths = vec![
            PathBuf::from("C:/Fonts/a.ttf"),
            PathBuf::from("C:/Fonts/b.ttf"),
        ];
        save_session_paths_in(&root, &paths);
        let got = load_session_paths_in(&root);
        assert_eq!(got, paths);
        clear_session_paths_in(&root);
        assert!(load_session_paths_in(&root).is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn merge_unique_paths_dedupes() {
        let a = vec![PathBuf::from("x"), PathBuf::from("y")];
        let b = vec![PathBuf::from("y"), PathBuf::from("z")];
        let m = merge_unique_paths(a, b);
        assert_eq!(m.len(), 3);
        assert_eq!(m[0], PathBuf::from("x"));
        assert_eq!(m[2], PathBuf::from("z"));
    }

    #[test]
    fn plan_clears_both_when_no_locks_remain() {
        let plan = plan_session_end_cleanup(10, &[]);
        assert!(plan.clear_active);
        assert!(plan.clear_paths);
        assert!(plan.clear_maps, "successful unload must clear .session-maps.json");
        assert!(plan.keep_paths.is_empty());
        assert!(plan.fail_loud.is_none());
    }

    #[test]
    fn plan_keeps_locked_paths_and_fails_loud() {
        let locked = vec![PathBuf::from("a.ttf"), PathBuf::from("b.ttf")];
        let plan = plan_session_end_cleanup(100, &locked);
        assert!(plan.clear_active);
        assert!(!plan.clear_paths);
        assert!(!plan.clear_maps, "keep maps for next-boot recovery while locked");
        assert_eq!(plan.keep_paths, locked);
        let msg = plan.fail_loud.expect("fail-loud");
        assert!(msg.contains("2 of 100"));
        assert!(msg.contains("write-locked"));
    }

    #[test]
    fn quit_unload_budget_clamps() {
        // Formula: max(12s, min(180s, path_count * 15ms)).
        // Floor covers empty/small quits; ~2k Activate All ≈ 31.5s; ~11k ≈ 165s.
        assert_eq!(quit_unload_budget_for(0), Duration::from_secs(12));
        assert_eq!(quit_unload_budget_for(100), Duration::from_secs(12));
        // Eric Activate All (~2099): 2099 * 15ms = 31485ms — above floor, under cap
        let two_k = quit_unload_budget_for(2_099);
        assert_eq!(two_k, Duration::from_millis(31_485));
        assert!(two_k > Duration::from_secs(12));
        assert!(two_k < Duration::from_secs(180));
        // ~11k library: 11_000 * 15ms = 165s — far above the old hard 4s kill
        let eleven_k = quit_unload_budget_for(11_000);
        assert!(
            eleven_k > Duration::from_secs(60),
            "11k paths must get well above floor (got {:?})",
            eleven_k
        );
        assert_eq!(eleven_k, Duration::from_secs(165));
        // Cap at 180s
        assert_eq!(quit_unload_budget_for(12_000), Duration::from_secs(180));
        assert_eq!(quit_unload_budget_for(500_000), Duration::from_secs(180));
    }

    #[test]
    fn plan_recover_missing_stage_does_not_nuke_maps_or_active() {
        // Missing stage ⇒ still_locked empty, but must NOT clear maps/active.
        let missing = vec![PathBuf::from(r"C:\missing\stage.ttf")];
        let still = filter_still_write_locked(&missing);
        assert!(still.is_empty(), "absent path is not write-locked");
        let plan = plan_recover_sidecar_clear(&still, true);
        assert!(!plan.clear_maps, "missing stage must not clear .session-maps.json");
        assert!(!plan.clear_active, "missing stage must not clear .session-active.json");
        assert!(!plan.clear_paths);
        assert!(plan.keep_locked_paths.is_empty());
    }

    #[test]
    fn plan_recover_unlocked_existing_clears_paths_only() {
        let plan = plan_recover_sidecar_clear(&[], false);
        assert!(plan.clear_paths, "drop unload ledger after proven unlock");
        assert!(!plan.clear_maps, "preserve maps for re-register");
        assert!(!plan.clear_active, "preserve session-active for re-register");
        assert!(plan.keep_locked_paths.is_empty());
    }

    #[test]
    fn plan_recover_keeps_locked_paths() {
        let locked = vec![PathBuf::from("a.ttf"), PathBuf::from("b.ttf")];
        let plan = plan_recover_sidecar_clear(&locked, false);
        assert!(!plan.clear_paths);
        assert!(!plan.clear_maps);
        assert!(!plan.clear_active);
        assert_eq!(plan.keep_locked_paths, locked);
    }

    #[test]
    fn apply_recover_preserves_maps_and_active_when_missing() {
        let root = temp_root("recover-missing");
        let maps = vec![crate::session_stage::FaceMap {
            source: r"C:\Users\Eric\Documents\Font Manager\A\a.ttf".into(),
            stage: root.join("gone.ttf").to_string_lossy().into(),
            registry_name: String::new(),
            family: "A".into(),
        }];
        crate::session_stage::save_session_maps_in(&root, &maps);
        fs::write(session_active_file_in(&root), b"[\"A\"]\n").unwrap();
        save_session_paths_in(&root, &[root.join("gone.ttf")]);
        let leftover = load_session_paths_in(&root);
        let still = filter_still_write_locked(&leftover);
        assert!(still.is_empty());
        let missing_before = leftover.iter().any(|p| !p.is_file());
        assert!(missing_before);
        let plan = plan_recover_sidecar_clear(&still, missing_before);
        apply_recover_sidecar_plan(&root, &plan);
        assert!(
            crate::session_stage::session_maps_file_in(&root).is_file(),
            "maps must survive missing-stage recover"
        );
        assert!(
            session_active_file_in(&root).is_file(),
            "session-active must survive missing-stage recover"
        );
        // Rebuild can still validate/re-stage from preserved maps.
        let existing = crate::session_stage::load_session_maps_in(&root);
        assert_eq!(existing.len(), 1);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn stale_sidecars_clear_paths_after_unlock_keep_active() {
        // Unit stand-in: existing unlocked path clears ledger only (not active/maps).
        let root = temp_root("stale");
        let fake = root.join("face.ttf");
        fs::write(&fake, b"\x00\x01\x00\x00").unwrap();
        save_session_paths_in(&root, std::slice::from_ref(&fake));
        fs::write(session_active_file_in(&root), b"[\"Roboto\"]\n").unwrap();
        crate::session_stage::save_session_maps_in(
            &root,
            &[crate::session_stage::FaceMap {
                source: "s".into(),
                stage: fake.to_string_lossy().into(),
                registry_name: String::new(),
                family: "Roboto".into(),
            }],
        );
        let leftover = load_session_paths_in(&root);
        assert_eq!(leftover.len(), 1);
        let missing_before = leftover.iter().any(|p| !p.is_file());
        assert!(!missing_before);
        let still = filter_still_write_locked(&leftover);
        assert!(still.is_empty(), "temp file must not be write-locked in tests");
        let plan = plan_recover_sidecar_clear(&still, missing_before);
        apply_recover_sidecar_plan(&root, &plan);
        assert!(!session_paths_file_in(&root).is_file());
        assert!(session_active_file_in(&root).is_file(), "active preserved");
        assert!(crate::session_stage::session_maps_file_in(&root).is_file());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn font_cache_service_names_include_fontcache() {
        let names = font_cache_service_names();
        assert!(names.contains(&"FontCache"));
        assert!(names.contains(&"FontCache3.0.0.0"));
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn font_cache_restart_budget_is_time_bounded() {
        let b = font_cache_restart_budget();
        assert!(b >= Duration::from_secs(2));
        assert!(b <= Duration::from_secs(15), "quit must not hang on FontCache restart");
    }

    #[test]
    fn plan_font_cache_flush_only_when_removes_attempted() {
        assert!(!plan_font_cache_flush(0));
        assert!(plan_font_cache_flush(1));
        assert!(plan_font_cache_flush(50));
    }

    #[test]
    fn font_cache_held_message_matches_product_copy() {
        let msg = font_cache_held_message(7);
        assert!(msg.contains("Font Cache still holding 7 files"));
        assert!(msg.contains("retry as admin or reboot"));
    }

    #[test]
    fn gdi_object_quota_soft_warn_does_not_skip_add() {
        assert_eq!(GDI_OBJECT_DEFAULT_QUOTA, 10_000);
        assert_eq!(GDI_OBJECT_SOFT_WARN, 8_000);
        assert!(!gdi_objects_near_quota(0));
        assert!(!gdi_objects_near_quota(7_999));
        assert!(gdi_objects_near_quota(8_000));
        assert!(gdi_objects_near_quota(10_000));
        let msg = gdi_pressure_message(8_500);
        assert!(msg.contains("8500") || msg.contains("8,500") || msg.contains("8500 GDI") || msg.contains("8500"));
        assert!(msg.contains("10_000") || msg.contains("10000") || msg.contains("10,000"));
        assert!(msg.contains("unchanged"));
        assert!(!msg.to_lowercase().contains("skip add"));
    }

    #[test]
    fn recycle_user_font_dir_removes_family_folder() {
        let root = temp_root("recycle");
        let family = root.join("Open Sauce Sans");
        fs::create_dir_all(&family).unwrap();
        fs::write(family.join("face.ttf"), b"\x00\x01\x00\x00").unwrap();
        recycle_user_font_dir(&family).expect("recycle/remove family dir");
        assert!(!family.exists(), "family folder must be gone (Recycle Bin on Windows)");
        let _ = fs::remove_dir_all(&root);
    }

    /// Card 20 / N1: deleting a family whose name has no ASCII letters or digits
    /// must only remove that family's folder, never the Font Manager root,
    /// `Activated`, `Library`, or sibling families.
    #[test]
    fn delete_non_ascii_family_keeps_root_and_siblings() {
        for name in ["微软雅黑", "خط عربي", "ありがとう"] {
            let root = temp_root("nonascii-delete");
            let sibling = root.join("Roboto");
            let activated_sibling = root.join("Activated").join("Inter");
            let library_sibling = root.join("Library").join("Lora");
            for dir in [&sibling, &activated_sibling, &library_sibling] {
                fs::create_dir_all(dir).unwrap();
                fs::write(dir.join("face.ttf"), b"\x00\x01\x00\x00").unwrap();
            }
            let target = family_dir_in(&root, name);
            fs::create_dir_all(&target).unwrap();
            fs::write(target.join("face.ttf"), b"\x00\x01\x00\x00").unwrap();

            for dir in family_locations_in(&root, name) {
                assert_ne!(dir, root, "{name}: root must never be a family location");
                assert_ne!(dir, root.join("Activated"), "{name}: Activated is not a family");
                assert_ne!(dir, root.join("Library"), "{name}: Library is not a family");
            }
            let purged = purge_family_dirs_in(&root, name);
            assert!(purged.is_ok(), "{name}: delete must succeed: {purged:?}");
            assert!(!target.exists(), "{name}: target family folder must be removed");

            assert!(root.is_dir(), "{name}: Font Manager root must survive");
            assert!(root.join("Activated").is_dir(), "{name}: Activated must survive");
            assert!(root.join("Library").is_dir(), "{name}: Library must survive");
            assert!(sibling.join("face.ttf").is_file(), "{name}: sibling family must survive");
            assert!(activated_sibling.join("face.ttf").is_file(), "{name}: Activated sibling must survive");
            assert!(library_sibling.join("face.ttf").is_file(), "{name}: Library sibling must survive");
            let _ = fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn family_folder_slug_never_empty_and_stable() {
        assert_eq!(family_folder_slug("Open Sans"), "open-sans");
        let cjk = family_folder_slug("微软雅黑");
        assert!(cjk.starts_with("u-") && cjk.len() == 18, "{cjk}");
        assert_eq!(cjk, family_folder_slug(" 微软雅黑 "), "trim-stable");
        assert_ne!(cjk, family_folder_slug("خط عربي"), "distinct names, distinct slugs");
        assert_eq!(slug_family("微软雅黑"), "", "network slug stays ASCII-only");
    }

    #[test]
    fn protected_family_dirs_are_refused() {
        let root = PathBuf::from("/tmp/fm-root");
        assert!(is_protected_family_dir(&root, &root));
        assert!(is_protected_family_dir(&root, &root.join("")));
        assert!(is_protected_family_dir(&root, &root.join("Google")));
        assert!(is_protected_family_dir(&root, &root.join("Uploads")));
        assert!(is_protected_family_dir(&root, &root.join("Activated")));
        assert!(is_protected_family_dir(&root, &root.join("library")));
        assert!(is_protected_family_dir(&root, Path::new("/tmp")));
        assert!(is_protected_family_dir(&root, &root.join("..").join("x")));
        assert!(!is_protected_family_dir(&root, &root.join("Roboto")));
        assert!(!is_protected_family_dir(&root, &root.join("Google").join("Inter")));
        assert!(!is_protected_family_dir(&root, &root.join("Uploads").join("Custom")));
        assert!(!is_protected_family_dir(&root, &root.join("Activated").join("Inter")));
        assert!(purge_family_dirs_in(&root, "   ").is_err());
    }

    #[test]
    fn session_register_workers_bounded() {
        assert_eq!(session_register_workers(0), 1);
        assert_eq!(session_register_workers(1), 1);
        assert_eq!(session_register_workers(3), 3);
        assert_eq!(session_register_workers(100), 6);
    }

    /// Skye P1: Cancel must abort on-disk register queue; Pause waits (download-like).
    #[test]
    fn on_disk_register_gate_cancel_and_pause() {
        assert_eq!(
            on_disk_register_gate(true, false),
            OnDiskRegisterGate::StopCancelled
        );
        assert_eq!(
            on_disk_register_gate(true, true),
            OnDiskRegisterGate::StopCancelled,
            "cancel beats pause"
        );
        assert_eq!(
            on_disk_register_gate(false, true),
            OnDiskRegisterGate::WaitPaused
        );
        assert_eq!(on_disk_register_gate(false, false), OnDiskRegisterGate::Run);
    }

    /// Documents Skye P1: ≤6 parallel `register_intact_family` workers may walk
    /// disk concurrently, but AddFontResourceExW / RemoveFontResourceExW must
    /// not overlap — see `winfont::gdi_api` (Windows). This test only anchors
    /// the worker bound that made overlapping Adds a risk on ~11k-path restore.
    #[test]
    fn parallel_session_register_caps_workers_for_gdi_safety() {
        assert!(
            session_register_workers(11_000) <= 6,
            "session register workers must stay ≤6 so GDI serialization stays bounded"
        );
    }

    #[test]
    fn plan_fail_loud_mentions_font_cache() {
        let locked = vec![PathBuf::from("a.ttf")];
        let plan = plan_session_end_cleanup(10, &locked);
        let msg = plan.fail_loud.expect("fail-loud");
        assert!(msg.contains("Font Cache still holding 1 files"));
    }

    fn c36_fake_face(tag: &str) -> Vec<u8> {
        let mut b = b"\x00\x01\x00\x00".to_vec();
        b.extend_from_slice(tag.as_bytes());
        b.resize(512, 0);
        b
    }

    /// Card 36 / 35-1: two all-non-ASCII families plus a mixed pair ("思源 Sans" vs
    /// Latin "Sans"). Deleting any one must leave the other three families' files.
    #[test]
    fn delete_one_non_ascii_or_mixed_family_keeps_the_others() {
        let fams = ["測試字体", "Тестовый", "思源 Sans", "Sans"];
        for victim in fams {
            let root = temp_root("c36-delete");
            let mut files = Vec::new();
            for fam in fams {
                // Same upload file name for every family: the shared-folder bug also
                // made these overwrite each other on import.
                let path = install_font_file_in(&root, fam, "Regular.ttf", &c36_fake_face(fam))
                    .unwrap_or_else(|e| panic!("{fam}: install failed: {e}"));
                files.push((fam, path));
            }
            for (fam, path) in &files {
                assert!(
                    fs::read(path).unwrap() == c36_fake_face(fam),
                    "{fam}: import must not overwrite another family's file"
                );
            }
            let dirs: HashSet<PathBuf> =
                files.iter().map(|(_, p)| p.parent().unwrap().to_path_buf()).collect();
            assert_eq!(dirs.len(), fams.len(), "every family gets its own folder: {dirs:?}");
            for (fam, path) in &files {
                assert_eq!(dir_family_name(path.parent().unwrap()), *fam, "folder maps back to its family");
            }
            let purged = purge_family_dirs_in(&root, victim);
            assert!(purged.is_ok(), "{victim}: delete must succeed: {purged:?}");
            for (fam, path) in &files {
                if *fam == victim {
                    assert!(!path.exists(), "{victim}: its own file must be removed");
                } else {
                    assert!(path.is_file(), "deleting {victim} removed {fam}'s file {}", path.display());
                    assert!(fs::read(path).unwrap() == c36_fake_face(fam), "{fam}: bytes must be intact");
                }
            }
            let _ = fs::remove_dir_all(&root);
        }
    }

    /// Minimal SFNT whose name table carries `family` (Windows 3/1/0x0409, ID 1),
    /// padded with `salt` so every file has a distinct hash.
    fn c36_named_face(family: &str, salt: &str) -> Vec<u8> {
        let units: Vec<u8> = family.encode_utf16().flat_map(|u| u.to_be_bytes()).collect();
        let mut name = Vec::new();
        name.extend_from_slice(&0u16.to_be_bytes()); // format
        name.extend_from_slice(&1u16.to_be_bytes()); // count
        name.extend_from_slice(&18u16.to_be_bytes()); // stringOffset
        for v in [3u16, 1, 0x0409, 1, units.len() as u16, 0] {
            name.extend_from_slice(&v.to_be_bytes());
        }
        name.extend_from_slice(&units);
        let mut b = b"\x00\x01\x00\x00".to_vec();
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&[0u8; 6]);
        b.extend_from_slice(b"name");
        b.extend_from_slice(&0u32.to_be_bytes());
        b.extend_from_slice(&28u32.to_be_bytes());
        b.extend_from_slice(&(name.len() as u32).to_be_bytes());
        b.extend_from_slice(&name);
        b.extend_from_slice(salt.as_bytes());
        b.resize(b.len().max(512), 0);
        b
    }

    fn c36_tree_hashes(root: &Path) -> Vec<String> {
        use sha2::{Digest, Sha256};
        fn walk(d: &Path, out: &mut Vec<PathBuf>) {
            for e in fs::read_dir(d).unwrap().flatten() {
                let p = e.path();
                let name = p.file_name().unwrap().to_string_lossy().to_string();
                if p.is_dir() {
                    walk(&p, out);
                } else if !name.starts_with('.') {
                    out.push(p);
                }
            }
        }
        let mut files = Vec::new();
        walk(root, &mut files);
        let mut hashes: Vec<String> = files
            .iter()
            .map(|p| format!("{:x}", Sha256::digest(fs::read(p).unwrap())))
            .collect();
        hashes.sort();
        hashes
    }

    #[test]
    fn sfnt_upload_family_reads_the_name_table_not_the_file_name() {
        let root = temp_root("c36-name");
        let p = root.join("renamed-whatever.ttf");
        fs::write(&p, c36_named_face("思源 Sans", "x")).unwrap();
        assert_eq!(sfnt_upload_family(&p).as_deref(), Some("思源 Sans"));
        fs::write(&p, c36_fake_face("no name table")).unwrap();
        assert_eq!(sfnt_upload_family(&p), None);
        let _ = fs::remove_dir_all(&root);
    }

    /// Card 36 migration: legacy shared `font/` and the mixed-name `Sans/` folder are
    /// split into per-family `u-<hash>` folders by name-table family. No file is lost:
    /// count + SHA-256 multiset identical before/after (unmatched files included), and
    /// a second run moves nothing.
    #[test]
    fn migrate_shared_family_folders_keeps_every_file() {
        let root = temp_root("c36-migrate");
        let shared = root.join("font");
        let sans = root.join("Sans");
        let lib_shared = root.join("Library").join("font");
        for d in [&shared, &sans, &lib_shared] {
            fs::create_dir_all(d).unwrap();
        }
        fs::write(shared.join("A-Regular.ttf"), c36_named_face("測試字体", "a1")).unwrap();
        fs::write(shared.join("A-Bold.ttf"), c36_named_face("測試字体", "a2")).unwrap();
        fs::write(shared.join("B.otf"), c36_named_face("Тестовый", "b1")).unwrap();
        fs::write(shared.join("mystery.ttf"), c36_fake_face("unreadable")).unwrap();
        fs::write(shared.join("notes.txt"), b"keep me").unwrap();
        // Destination conflict: the target u-folder already has a file of that name.
        fs::write(shared.join("Conflict.ttf"), c36_named_face("Ελληνικά", "c-src")).unwrap();
        let conflict_dir = root.join(unicode_family_slug("Ελληνικά"));
        fs::create_dir_all(&conflict_dir).unwrap();
        fs::write(conflict_dir.join("Conflict.ttf"), c36_named_face("Ελληνικά", "c-dst")).unwrap();
        fs::write(sans.join("Sans-Regular.ttf"), c36_named_face("Sans", "s1")).unwrap();
        fs::write(sans.join("SiYuan.ttf"), c36_named_face("思源 Sans", "sy1")).unwrap();
        fs::write(lib_shared.join("L.ttf"), c36_named_face("日本語", "l1")).unwrap();
        mark_family_complete(&sans, 2);

        let before = c36_tree_hashes(&root);
        let first = migrate_shared_family_folders(&root);
        let after = c36_tree_hashes(&root);
        assert_eq!(before.len(), 10);
        assert_eq!(before, after, "count + hashes must be identical after migration");
        assert_eq!(
            first,
            SharedFolderMigration { moved: 5, kept_unmatched: 1, kept_conflict: 1, errors: 0 }
        );

        let u = |fam: &str| root.join(unicode_family_slug(fam));
        assert!(u("測試字体").join("A-Regular.ttf").is_file());
        assert!(u("測試字体").join("A-Bold.ttf").is_file());
        assert!(u("Тестовый").join("B.otf").is_file());
        assert!(u("思源 Sans").join("SiYuan.ttf").is_file());
        assert!(root.join("Library").join(unicode_family_slug("日本語")).join("L.ttf").is_file());
        assert_eq!(dir_family_name(&u("測試字体")), "測試字体");
        assert!(sans.join("Sans-Regular.ttf").is_file(), "Latin Sans stays");
        assert!(shared.join("mystery.ttf").is_file(), "unmatched stays in place");
        assert!(shared.join("notes.txt").is_file());
        assert!(shared.join("Conflict.ttf").is_file(), "conflict keeps the source");
        assert!(dir_is_complete(&sans), "source .complete kept (re-stamped)");

        let second = migrate_shared_family_folders(&root);
        assert_eq!(second.moved, 0, "idempotent");
        assert_eq!(c36_tree_hashes(&root), before);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn alias_keys_non_ascii_are_not_lossy() {
        let keys = alias_keys("思源 Sans");
        assert!(keys.iter().any(|k| k.starts_with("u-")), "{keys:?}");
        assert!(keys.iter().any(|k| k.contains('思')), "{keys:?}");
        assert!(!keys.iter().any(|k| k == "sans" || k == "font"), "{keys:?}");
    }

    #[test]
    fn migration_marker_waits_when_a_conflict_was_kept() {
        assert!(migration_complete(&SharedFolderMigration::default()));
        assert!(!migration_complete(&SharedFolderMigration {
            kept_conflict: 1,
            ..SharedFolderMigration::default()
        }));
        assert!(!migration_complete(&SharedFolderMigration {
            errors: 1,
            ..SharedFolderMigration::default()
        }));
    }

    /// Removing the Library / u- folder guards would treat those directories as
    /// family folders and rename a matching non-ASCII face out of them.
    #[test]
    fn migrate_skips_protected_and_u_folders() {
        let root = temp_root("c36-guard");
        let lib = root.join("Library");
        let hashed = root.join("u-abc");
        fs::create_dir_all(&lib).unwrap();
        fs::create_dir_all(&hashed).unwrap();
        fs::write(lib.join("X.ttf"), c36_named_face("Library文", "x")).unwrap();
        fs::write(hashed.join("Y.ttf"), c36_named_face("u-abc文", "y")).unwrap();
        let result = migrate_shared_family_folders(&root);
        assert_eq!(result.moved, 0, "{result:?}");
        assert!(lib.join("X.ttf").is_file(), "Library is not a family folder");
        assert!(hashed.join("Y.ttf").is_file(), "u- folders are already migrated");
        let _ = fs::remove_dir_all(&root);
    }

    /// Card 36 item 2: startup tidy never deletes a font file or a non-empty folder.
    #[test]
    fn boot_tidy_keeps_woff_uploads_and_non_empty_folders() {
        let root = temp_root("c36-tidy");
        let up = root.join("My Upload");
        fs::create_dir_all(&up).unwrap();
        let woff = up.join("My-Upload.woff2");
        fs::write(&woff, b"wOF2 not an sfnt").unwrap();
        fs::write(up.join(".complete"), b"1").unwrap();
        boot_tidy_family_dir(&up, std::slice::from_ref(&woff), false);
        assert!(woff.is_file(), "WOFF2 upload must survive startup");
        assert!(up.is_dir());
        let empty = root.join("Empty");
        fs::create_dir_all(&empty).unwrap();
        fs::write(empty.join(".expected"), b"2").unwrap();
        boot_tidy_family_dir(&empty, &[], false);
        assert!(!empty.exists(), "a folder with only markers may go");
        let _ = fs::remove_dir_all(&root);
    }

    /// Card 36 item 2: prune candidates skip uploads (`.family`), folders whose fonts
    /// name a kept family, and unreadable fonts; only a truly unknown folder qualifies.
    #[test]
    fn prune_candidates_spare_uploads_and_kept_name_tables() {
        let root = temp_root("c36-prune");
        let mk = |folder: &str, file: &str, bytes: Vec<u8>| {
            let d = root.join(folder);
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join(file), bytes).unwrap();
            d
        };
        let upload = install_font_file_in(&root, "測試字体", "x.ttf", &c36_named_face("測試字体", "u"))
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let renamed = mk("Old Folder Name", "a.ttf", c36_named_face("Roboto", "r"));
        let woff = mk("Web Upload", "w.woff2", b"wOF2....".to_vec());
        let gone = mk("Delisted Family", "d.ttf", c36_named_face("Delisted Family", "d"));
        let kept = mk("Roboto", "Roboto.ttf", c36_named_face("Roboto", "k"));
        let keep_keys: HashSet<String> = alias_keys("Roboto").into_iter().collect();
        let dirs = vec![upload.clone(), renamed.clone(), woff.clone(), gone.clone(), kept.clone()];
        let out = prune_candidates(&dirs, &root, &keep_keys);
        assert_eq!(out, vec![gone.clone()], "only the delisted folder is a candidate");
        // And even that is never permanently deleted off Windows.
        assert_eq!(recycle_or_leave(&gone), Ok(cfg!(windows)));
        #[cfg(not(windows))]
        assert!(gone.join("d.ttf").is_file(), "no permanent delete");
        let _ = fs::remove_dir_all(&root);
    }

    /// Card 36: a non-ASCII family resolves to its own u-folder and its legacy JS
    /// upload folder (raw name) — never the lossy `font` / `Sans` folders.
    #[test]
    fn non_ascii_family_locations_own_folders_only() {
        let root = temp_root("c36-loc");
        for d in ["font", "Sans", "測試字体", "思源 Sans"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        fs::create_dir_all(root.join(unicode_family_slug("測試字体"))).unwrap();
        let got = family_locations_in(&root, "測試字体");
        assert!(got.contains(&root.join("測試字体")), "legacy JS folder: {got:?}");
        assert!(got.contains(&root.join(unicode_family_slug("測試字体"))));
        assert!(!got.contains(&root.join("font")), "{got:?}");
        let mixed = family_locations_in(&root, "思源 Sans");
        assert!(mixed.contains(&root.join("思源 Sans")));
        assert!(!mixed.contains(&root.join("Sans")), "{mixed:?}");
        assert_eq!(unicode_family_slug("測試字体"), "u-7acd0fca85b1a525", "JS family-folder.ts vector");
        assert_eq!(unicode_family_slug("Тестовый"), "u-968f7d0b79ea5e3b");
        let _ = fs::remove_dir_all(&root);
    }

    /// Standalone SFNT for face `index` of a TTC (what the JS importer's extractFace
    /// hands to writeAndRegister).
    fn c36_extract_face(ttc: &[u8], index: usize) -> Vec<u8> {
        let be16 = |o: usize| u16::from_be_bytes([ttc[o], ttc[o + 1]]);
        let be32 = |o: usize| u32::from_be_bytes([ttc[o], ttc[o + 1], ttc[o + 2], ttc[o + 3]]) as usize;
        let d = be32(12 + 4 * index);
        let n = usize::from(be16(d + 4));
        let mut out = ttc[d..d + 12 + 16 * n].to_vec();
        for t in 0..n {
            let r = 12 + 16 * t;
            let (off, len) = (be32(d + r + 8), be32(d + r + 12));
            let at = out.len();
            out[r + 8..r + 12].copy_from_slice(&(at as u32).to_be_bytes());
            out.extend_from_slice(&ttc[off..off + len]);
            while !out.len().is_multiple_of(4) {
                out.push(0);
            }
        }
        out
    }

    fn c36_import_and_activate_every_face(ttc: &[u8], stem: &str) -> Vec<(String, PathBuf)> {
        let root = temp_root("c36-ttc");
        let count = u32::from_be_bytes([ttc[8], ttc[9], ttc[10], ttc[11]]) as usize;
        let mut out = Vec::new();
        for i in 0..count {
            let face = c36_extract_face(ttc, i);
            let tmp = root.join(format!(".probe-{i}.ttf"));
            fs::write(&tmp, &face).unwrap();
            let family = sfnt_upload_family(&tmp).expect("face has a name table");
            fs::remove_file(&tmp).unwrap();
            let ext = if &face[0..4] == b"OTTO" { "otf" } else { "ttf" };
            let path = install_font_file_in(&root, &family, &format!("{stem}-{}.{ext}", i + 1), &face)
                .unwrap_or_else(|e| panic!("face {i} ({family}) import: {e}"));
            // Activation entry point used by writeAndRegister (Linux: intact check only;
            // the GDI AddFontResourceExW call is Windows-only).
            register_font_path_in(&path, Some(&family))
                .unwrap_or_else(|e| panic!("face {i} ({family}) activate: {e}"));
            out.push((family, path));
        }
        for (family, path) in &out {
            assert!(path.is_file(), "{family}: file kept");
            assert_eq!(sfnt_upload_family(path).as_deref(), Some(family.as_str()));
        }
        out
    }

    /// Card 36 item 3: all 10 faces of a collection import to Documents and each goes
    /// through the activation entry point (generated Noto-shaped TTC, ~6 KB).
    #[test]
    fn ttc_import_keeps_all_ten_faces_each_activatable() {
        let ttc = crate::parse::tests::c36_build_ttc(&crate::parse::tests::C36_FACES);
        let faces = c36_import_and_activate_every_face(&ttc, "Fixture-Regular");
        assert_eq!(faces.len(), 10);
        let files: HashSet<&PathBuf> = faces.iter().map(|(_, p)| p).collect();
        assert_eq!(files.len(), 10, "no face overwrote another");
        let shared: Vec<&PathBuf> = faces.iter().filter(|(f, _)| f == "Fixture Shared").map(|(_, p)| p).collect();
        assert_eq!(shared.len(), 2);
        assert_eq!(shared[0].parent(), shared[1].parent(), "same family, same folder");
        assert_eq!(count_intact_faces(shared[0].parent().unwrap()), 2);
        if let Some(root) = faces[0].1.parent().and_then(|p| p.parent()).and_then(|p| p.parent()) {
            let _ = fs::remove_dir_all(root);
        }
    }

    /// Same, on the real Noto Sans CJK Regular TTC (opt-in: FM_NOTO_TTC, 19 MB).
    #[test]
    fn real_noto_cjk_ttc_all_ten_faces_import_and_activate() {
        let Some(path) = std::env::var_os("FM_NOTO_TTC") else {
            return;
        };
        let ttc = fs::read(path).unwrap();
        let faces = c36_import_and_activate_every_face(&ttc, "NotoSansCJK-Regular");
        assert_eq!(faces.len(), 10);
        let fams: HashSet<&str> = faces.iter().map(|(f, _)| f.as_str()).collect();
        assert_eq!(fams.len(), 10);
        assert!(faces.iter().all(|(_, p)| p.extension().unwrap() == "otf"));
        if let Some(root) = faces[0].1.parent().and_then(|p| p.parent()).and_then(|p| p.parent()) {
            let _ = fs::remove_dir_all(root);
        }
    }
}

#[cfg(test)]
mod c253_tests {
    use super::*;
    use std::fs;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn c253_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let p = std::env::temp_dir().join(format!("fm-c253-{label}-{nanos}"));
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn mtime(p: &Path) -> SystemTime {
        fs::metadata(p).unwrap().modified().unwrap()
    }

    fn age(p: &Path) {
        let old = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let f = fs::OpenOptions::new().write(true).open(p).unwrap();
        f.set_modified(old).unwrap();
    }

    /// C253 #1: the periodic scan re-stamped `.complete`/`.expected` on every pass,
    /// which fired the recursive managed-root watch and re-ran the scan forever.
    /// A repeat stamp with unchanged contents must not touch the markers.
        /// Live network check (run with `--ignored`): google/fonts repo heals Barlow/Tinos 400.
    #[test]
    #[ignore]
    fn c255_live_google_fonts_repo_full_faces() {
        let client = http_download_client().unwrap();
        let dir = c253_dir("live-repo");
        for (fam, local_glyphs) in [("Barlow", 272u16), ("Tinos", 235u16)] {
            let local = dir.join(format!("{}-400-normal.ttf", slug_family(fam)));
            fs::write(&local, sfnt_with_glyphs(local_glyphs)).unwrap();
            let (lic, folder, statics) = google_fonts_repo_statics(&client, fam).expect("METADATA");
            let (_, _, fname) = statics
                .iter()
                .find(|(st, w, _)| st == "normal" && w == "400")
                .expect("400 normal listed");
            let bytes = google_fonts_variable_cdn_urls(&lic, &folder, fname)
                .iter()
                .find_map(|u| fetch_url_ttf(&client, u))
                .expect("repo face downloads");
            let g = sfnt_num_glyphs_bytes(&bytes).unwrap();
            eprintln!("{fam}: {lic}/{folder}/{fname} {} B, {g} glyphs (local {local_glyphs})", bytes.len());
            assert_eq!(first_acceptable_repair(&[sfnt_with_glyphs(local_glyphs), bytes], &local), Some(1));
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn c255_repair_heals_cut_down_face_from_google_fonts_repo() {
        // Critic 254: CSS serves the latin cut too (272 glyphs) — repo has 694.
        let dir = c253_dir("repo-repair");
        let local = dir.join("barlow-400-normal.ttf");
        fs::write(&local, sfnt_with_glyphs(227)).unwrap();
        let css = sfnt_with_glyphs(227);
        let repo = sfnt_with_glyphs(694);
        assert_eq!(first_acceptable_repair(&[css.clone(), repo], &local), Some(1));
        assert_eq!(first_acceptable_repair(&[css], &local), None);
        let meta = "name: \"Barlow\"\nfonts {\n  name: \"Barlow\"\n  style: \"normal\"\n  weight: 400\n  filename: \"Barlow-Regular.ttf\"\n}\nfonts {\n  name: \"Barlow\"\n  style: \"italic\"\n  weight: 700\n  filename: \"Barlow-BoldItalic.ttf\"\n}\nfonts {\n  style: \"normal\"\n  weight: 400\n  filename: \"Barlow[wght].ttf\"\n}\n";
        let files = parse_metadata_pb_static_files(meta);
        assert_eq!(
            files,
            vec![
                ("normal".into(), "400".into(), "Barlow-Regular.ttf".into()),
                ("italic".into(), "700".into(), "Barlow-BoldItalic.ttf".into()),
            ]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn c253_emoji_and_material_symbols_have_live_upstream_ttf() {
        // googlefonts/noto-emoji moved fonts/ → 2D/fonts/ (old paths 404).
        let emoji = ttf_urls("noto-color-emoji", "", 400, false, "emoji", 0);
        assert!(!emoji.is_empty());
        assert!(
            emoji.iter().all(|u| u.contains("/2D/fonts/NotoColorEmoji")),
            "dead noto-emoji path: {emoji:?}"
        );
        let mono = ttf_urls("noto-emoji", "", 400, false, "emoji", 0);
        assert!(mono.iter().any(|u| u.contains("ofl/notoemoji/NotoEmoji%5Bwght%5D.ttf")));
        assert!(!mono.iter().any(|u| u.contains("@main/fonts/NotoEmoji-Regular.ttf")));
        for (fam, file) in [
            ("Material Symbols Outlined", "MaterialSymbolsOutlined%5BFILL,GRAD,opsz,wght%5D.ttf"),
            ("Material Symbols Rounded", "MaterialSymbolsRounded%5BFILL,GRAD,opsz,wght%5D.ttf"),
            ("Material Symbols Sharp", "MaterialSymbolsSharp%5BFILL,GRAD,opsz,wght%5D.ttf"),
        ] {
            let urls = material_symbols_ttf_urls(fam);
            assert!(urls.iter().any(|u| u.ends_with(file)), "{fam}: {urls:?}");
            assert!(urls.iter().all(|u| u.contains("google/material-design-icons")));
        }
        assert!(material_symbols_ttf_urls("Roboto").is_empty());
    }

    #[test]
    fn c255_material_symbols_never_use_google_endpoints() {
        for fam in ["Material Symbols Outlined", "Material Symbols Rounded", "material symbols sharp"] {
            assert!(!google_endpoints_allowed(fam), "{fam} must not hit fonts.googleapis");
            assert_eq!(effective_fetch_intent(fam, FetchIntent::Google), FetchIntent::Fontsource);
        }
        assert!(google_endpoints_allowed("Roboto"));
        assert_eq!(effective_fetch_intent("Roboto", FetchIntent::Google), FetchIntent::Google);
        assert_eq!(effective_fetch_intent("Material Symbols Outlined", FetchIntent::Local), FetchIntent::Local);
    }

    /// Bench (run with `--ignored --nocapture`): undersized gate over a 2,100-family
    /// library, cold then 2 warm passes (Scan polls repeatedly).
    #[test]
    #[ignore]
    fn c255_bench_undersized_scan() {
        let parent = c253_dir("bench");
        let mut dirs = Vec::new();
        for i in 0..2100 {
            let d = parent.join(format!("Barlow{i}"));
            fs::create_dir_all(&d).unwrap();
            for w in [100, 200, 300, 400, 500, 700] {
                fs::write(d.join(format!("barlow-{w}-normal.ttf")), sfnt_with_glyphs(525)).unwrap();
            }
            dirs.push(d);
        }
        for pass in 0..3 {
            let t = std::time::Instant::now();
            let mut hits = 0;
            for d in &dirs {
                if dir_has_undersized_google_static(d, "Barlow") {
                    hits += 1;
                }
            }
            eprintln!("C255BENCH pass {pass}: {:?} for {} dirs (hits {hits})", t.elapsed(), dirs.len());
        }
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn c255_recovery_refuses_foreign_absolute_session_paths() {
        let own = vec![
            PathBuf::from(r"C:\Users\E\AppData\Local\Temp\fm254\local"),
            PathBuf::from(r"C:\Users\E\AppData\Local\Temp\fm254\docs\Font Manager"),
        ];
        let mine = PathBuf::from(r"C:\Users\E\AppData\Local\Temp\fm254\local\Font Manager\gdi-maps\ab.ttf");
        let mine_docs = PathBuf::from(r"c:/users/e/appdata/local/temp/fm254/docs/font manager/Roboto/r.ttf");
        let foreign = PathBuf::from(r"C:\Users\E\AppData\Local\Font Manager\gdi-maps\ab.ttf");
        let prefix_trick = PathBuf::from(r"C:\Users\E\AppData\Local\Temp\fm254\local2\Font Manager\gdi-maps\x.ttf");
        assert!(recovery_path_owned(&mine, &own));
        assert!(recovery_path_owned(&mine_docs, &own));
        assert!(!recovery_path_owned(&foreign, &own), "another instance's gdi-maps");
        assert!(!recovery_path_owned(&prefix_trick, &own));
        assert!(!recovery_path_owned(&PathBuf::from("gdi-maps\\ab.ttf"), &own), "relative");
        assert!(!recovery_path_owned(&mine, &[]), "no known roots → refuse");
    }

    #[test]
    fn c255_single_family_deactivate_sweeps_failed_activate_leftovers() {
        let docs = PathBuf::from(r"C:\Users\E\Documents\Font Manager");
        let stage = PathBuf::from(r"C:\Users\E\AppData\Local\Font Manager\gdi-maps");
        let foo_vf = stage.join("foo-vf.ttf");
        let bar = stage.join("bar-400.ttf");
        let loaded = vec![foo_vf.clone(), bar.clone()];
        let maps = vec![
            (docs.join("Foo Sans").join("foo-sans-variable-wght.ttf"), foo_vf.clone()),
            (docs.join("Bar").join("bar-400-normal.ttf"), bar.clone()),
        ];
        let got = leftover_loaded_for_family(&alias_keys("Foo Sans"), &loaded, &maps);
        assert_eq!(got, vec![foo_vf], "only Foo Sans's leftover; Bar stays loaded");
        assert!(leftover_loaded_for_family(&alias_keys("Baz"), &loaded, &maps).is_empty());
    }

    #[test]
    fn c253_deactivate_all_unloads_failed_activate_leftovers() {
        let loaded = vec![
            PathBuf::from(r"C:\Users\E\AppData\Local\FontManager\gdi-maps\a\Foo-VF.ttf"),
            PathBuf::from(r"C:\Windows\Fonts\arial.ttf"),
        ];
        let sys = |p: &Path| p.to_string_lossy().to_ascii_lowercase().contains("\\windows\\fonts");
        // Session emptied → the failed-activate leftover must be unloaded, never Windows Fonts.
        assert_eq!(leftover_loaded_after_unload(&[], &loaded, sys), vec![loaded[0].clone()]);
        // Partial Deactivate (session still has families) → touch nothing extra.
        assert!(leftover_loaded_after_unload(&["Roboto".into()], &loaded, sys).is_empty());
    }

    /// Minimal SFNT with a `maxp` table carrying `glyphs`.
    fn sfnt_with_glyphs(glyphs: u16) -> Vec<u8> {
        let mut b = vec![0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00];
        b.extend_from_slice(b"maxp");
        b.extend_from_slice(&[0, 0, 0, 0]);
        b.extend_from_slice(&28u32.to_be_bytes());
        b.extend_from_slice(&6u32.to_be_bytes());
        b.extend_from_slice(&[0x00, 0x00, 0x50, 0x00]);
        b.extend_from_slice(&glyphs.to_be_bytes());
        b.resize(4096, 0);
        b
    }

    #[test]
    fn c253_cut_down_regular_bold_flagged_for_repair() {
        // Barlow (official Google, not allowlisted): 400 is a 227-glyph latin cut vs 525.
        let parent = c253_dir("glyph-outlier");
        let dir = parent.join("Barlow");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("barlow-400-normal.ttf"), sfnt_with_glyphs(227)).unwrap();
        fs::write(dir.join("barlow-500-normal.ttf"), sfnt_with_glyphs(525)).unwrap();
        fs::write(dir.join("barlow-600-normal.ttf"), sfnt_with_glyphs(525)).unwrap();
        assert!(dir_has_undersized_google_static(&dir, "Barlow"), "cut-down 400 must flag the family");
        assert!(face_should_replace_as_tiny_cjk("Barlow", &dir.join("barlow-400-normal.ttf")));
        assert!(!face_should_replace_as_tiny_cjk("Barlow", &dir.join("barlow-500-normal.ttf")));
        // Healthy family: no flag.
        fs::write(dir.join("barlow-400-normal.ttf"), sfnt_with_glyphs(525)).unwrap();
        intact_forget(&dir.join("barlow-400-normal.ttf"));
        assert!(!dir_has_undersized_google_static(&dir, "Barlow"));
        let _ = fs::remove_dir_all(&parent);
    }

    fn fake_ttf(p: &Path) {
        let mut b = b"\x00\x01\x00\x00".to_vec();
        b.resize(4096, 0);
        fs::write(p, &b).unwrap();
    }

    #[test]
    fn c253_google_vf_counts_as_complete_face_set() {
        // Roboto-style: CSS lists 18 statics, VF path writes only roman+italic VF.
        let dir = c253_dir("vf-gate");
        let slug = "roboto";
        let listed: Vec<(String, String, String)> = [100, 200, 300, 400, 500, 600, 700, 800, 900]
            .iter()
            .flat_map(|w| {
                ["normal", "italic"]
                    .iter()
                    .map(move |s| (s.to_string(), w.to_string(), String::new()))
            })
            .collect();
        let vars = vec![
            "roboto-variable-wght-normal.ttf".to_string(),
            "roboto-variable-wght-italic.ttf".to_string(),
        ];
        let inst: Vec<String> = listed
            .iter()
            .map(|(st, w, _)| google_face_filename(slug, w, st))
            .collect();
        write_google_planned(&dir, &merge_variable_into_planned_keys_keep_statics(&inst, &vars));
        fake_ttf(&dir.join(&vars[0]));
        fake_ttf(&dir.join(&vars[1]));
        let expected = listed.len() + vars.len();
        let (planned, intact) = google_complete_gate(&dir, slug, &listed, &vars, expected);
        assert!(
            planned > 0 && intact >= planned,
            "full VF set must be complete, got {intact}/{planned}"
        );
        // Partial VF (italic missing) is still incomplete.
        fs::remove_file(dir.join(&vars[1])).unwrap();
        let (planned, intact) = google_complete_gate(&dir, slug, &listed, &vars, expected);
        assert!(intact < planned, "partial VF must stay incomplete ({intact}/{planned})");
        // Statics-only plans are unchanged: 2 listed, 1 on disk → 1/2.
        let dir2 = c253_dir("static-gate");
        let two = listed[..2].to_vec();
        let keys2: Vec<String> = two.iter().map(|(st, w, _)| google_face_filename(slug, w, st)).collect();
        write_google_planned(&dir2, &keys2);
        fake_ttf(&dir2.join(&keys2[0]));
        assert_eq!(google_complete_gate(&dir2, slug, &two, &[], 2), (2, 1));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&dir2);
    }

    #[test]
    fn c253_settled_stamp_is_idempotent_no_marker_rewrite() {
        let parent = c253_dir("gidugu-loop");
        let dir = parent.join("Gidugu");
        fs::create_dir_all(&dir).unwrap();
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(460_988, 0); // full-size official face (not undersized)
        fs::write(dir.join("gidugu-400-normal.ttf"), &fake).unwrap();
        stamp_known_incapable_dir_settled(&dir, "Gidugu");
        let c = dir.join(".complete");
        let e = dir.join(".expected");
        assert!(c.is_file() && e.is_file(), "first scan stamps Settled");
        age(&c);
        age(&e);
        let (c0, e0) = (mtime(&c), mtime(&e));
        for _ in 0..3 {
            stamp_known_incapable_dir_settled(&dir, "Gidugu");
        }
        assert_eq!(mtime(&c), c0, ".complete rewritten by an unchanged rescan (watch loop)");
        assert_eq!(mtime(&e), e0, ".expected rewritten by an unchanged rescan (watch loop)");
        // A real change still writes.
        mark_family_complete(&dir, 2);
        assert_eq!(fs::read_to_string(&c).unwrap(), "2");
        let _ = fs::remove_dir_all(&parent);
    }
}

#[cfg(test)]
mod c258_tests {
    use super::*;
    use crate::gfsource::{FaceShape, ListingSource};

    fn dir(label: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("fm-c258-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }
    fn sfnt(glyphs: u16) -> Vec<u8> {
        let mut b = vec![0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00];
        b.extend_from_slice(b"maxp");
        b.extend_from_slice(&[0, 0, 0, 0]);
        b.extend_from_slice(&28u32.to_be_bytes());
        b.extend_from_slice(&6u32.to_be_bytes());
        b.extend_from_slice(&[0x00, 0x00, 0x50, 0x00]);
        b.extend_from_slice(&glyphs.to_be_bytes());
        b.resize(4096, 0);
        b
    }
    fn l(url: &str) -> Vec<(String, String, String)> {
        vec![("normal".into(), "400".into(), url.into())]
    }
    fn shape(g: u16, f: &[&str]) -> FaceShape {
        FaceShape { glyphs: g, features: f.iter().map(|s| s.to_string()).collect() }
    }

    /// Item 1: manifest → repo → css2; css2 only when both full sources are empty.
    #[test]
    fn c258_1_listing_order_manifest_repo_css2_last() {
        assert_eq!(
            google_listing_source_order(),
            [ListingSource::Manifest, ListingSource::Repo, ListingSource::Css2]
        );
        let (got, src) = choose_google_listing(l("m"), || panic!("repo not needed"), || panic!("css2 not needed"));
        assert_eq!((got, src), (l("m"), ListingSource::Manifest));
        let (got, src) = choose_google_listing(Vec::new(), || l("r"), || panic!("css2 not needed"));
        assert_eq!((got, src), (l("r"), ListingSource::Repo));
        let (got, src) = choose_google_listing(Vec::new(), Vec::new, || l("c"));
        assert_eq!((got, src), (l("c"), ListingSource::Css2));
        assert!(!ListingSource::Css2.is_full());
    }

    /// Item 1: the a4c24e7 comment claimed css2 Mozilla gives the "full desktop TTF".
    #[test]
    fn c258_1_css2_comment_is_honest() {
        let src = all_activate_sources();
        let at = src.find("const UA_DESKTOP_TTF").unwrap();
        let head = &src[at.saturating_sub(600)..at];
        assert!(!head.contains("returns full desktop TTF"), "misleading css2 comment still present");
    }

    /// Item 2: 1.0.206u VF-Primary by default; Static-first opt-in blocks VF-primary on full-source folders.
    #[test]
    fn c258_2_static_first_blocks_vf_primary() {
        let d = dir("static-first").join("Inter");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("inter-variable-opsz-wght.ttf"), sfnt(2926)).unwrap();
        fs::write(d.join("inter-400-normal.ttf"), sfnt(2900)).unwrap();
        gfsource::set_full_source(&d, ListingSource::Manifest);
        // 1.0.206u default: VF-Primary even when full-source statics exist.
        assert!(!dir_static_first_blocks_vf(&d));
        assert!(dir_vf_primary_ready(&d, "Inter"), "1.0.206u default: VF-primary wins");
        // Static-first opt-in: blocks VF-primary when full-source statics exist.
        gfsource::set_dir_prefer_statics(&d, true);
        assert!(dir_static_first_blocks_vf(&d));
        assert!(!dir_vf_primary_ready(&d, "Inter"), "never both: statics win when Static-first opted in");
        gfsource::clear_full_source(&d);
        assert!(!dir_static_first_blocks_vf(&d));
        let _ = fs::remove_dir_all(d.parent().unwrap());
    }

    /// Item 3: reject downloads short on glyphs or features vs the expected source.
    #[test]
    fn c258_3_reject_short_glyphs_or_features() {
        let full = shape(694, &["smcp", "c2sc", "sups", "kern", "liga"]);
        let css = shape(571, &["kern", "liga"]);
        assert!(!accept_face_shapes(Some(&css), Some(&full), ListingSource::Manifest));
        assert!(accept_face_shapes(Some(&shape(680, &["smcp", "c2sc", "sups", "kern", "liga"])), Some(&full), ListingSource::Manifest));
        // css2 is never a repair over an existing face.
        assert!(!accept_face_shapes(Some(&shape(700, &["kern"])), Some(&css), ListingSource::Css2));
        // Unreadable candidate is rejected.
        assert!(!accept_face_shapes(None, None, ListingSource::Manifest));
        assert!(accept_face_shapes(Some(&full), None, ListingSource::Manifest));
    }

    /// Item 4: a folder with a cut face (or css2 web build / no full-source
    /// provenance) is fixed before anything is loaded into Windows.
    #[test]
    fn c258_4_cut_face_not_preloaded() {
        let d = dir("preload").join("Barlow");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("barlow-400-normal.ttf"), sfnt(272)).unwrap();
        fs::write(d.join("barlow-500-normal.ttf"), sfnt(680)).unwrap();
        assert!(family_dir_needs_fix_before_load(&d, "Barlow"));
        assert!(!face_allowed_for_register(&d, &d.join("barlow-400-normal.ttf"), "Barlow"), "cut face never Added");
        fs::write(d.join("barlow-400-normal.ttf"), sfnt(680)).unwrap();
        // Legacy css2 folder without full-source provenance still needs the upgrade.
        assert!(family_dir_needs_fix_before_load(&d, "Barlow"));
        gfsource::set_full_source(&d, ListingSource::Manifest);
        assert!(!family_dir_needs_fix_before_load(&d, "Barlow"));
        gfsource::set_web_build(&d, true);
        assert!(family_dir_needs_fix_before_load(&d, "Barlow"));
        let _ = fs::remove_dir_all(d.parent().unwrap());
    }

    /// Item 1/3: a css2 web build never stamps `.complete`.
    #[test]
    fn c258_css2_never_complete() {
        let d = dir("webbuild");
        assert!(dir_may_stamp_complete(&d));
        gfsource::set_web_build(&d, true);
        assert!(!dir_may_stamp_complete(&d));
        let _ = fs::remove_dir_all(&d);
    }

    /// Item 5: every font/manifest GET goes through the polite pacer + honest UA.
    #[test]
    fn c258_5_downloads_are_polite() {
        let src = all_activate_sources();
        let body = |name: &str| {
            let at = src.find(name).unwrap();
            &src[at..at + 1500]
        };
        assert!(body("fn fetch_url_ttf(").contains("gfsource::polite_get"));
        assert!(body("fn http_download_client(").contains("gfsource::POLITE_UA"));
        assert!(body("fn stream_url_to_font_file(").contains("gfsource::polite_response"));
    }
}

#[cfg(test)]
mod c260_tests {
    use super::*;
    use crate::gfsource::ListingSource;

    fn dir(label: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("fm-c260-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }
    fn sfnt(glyphs: u16) -> Vec<u8> {
        let mut b = vec![0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00];
        b.extend_from_slice(b"maxp");
        b.extend_from_slice(&[0, 0, 0, 0]);
        b.extend_from_slice(&28u32.to_be_bytes());
        b.extend_from_slice(&6u32.to_be_bytes());
        b.extend_from_slice(&[0x00, 0x00, 0x50, 0x00]);
        b.extend_from_slice(&glyphs.to_be_bytes());
        b.resize(4096, 0);
        b
    }
    fn src() -> String {
        all_activate_sources()
    }
    /// Production code only (before the c258/c260 test modules), CRLF-normalised
    /// (Windows checkouts use CRLF; Card260 first run tripped on it).
    fn code() -> String {
        all_activate_prod_sources()
    }
    fn fn_body(name: &str, len: usize) -> String {
        let s = src();
        let at = s.find(name).unwrap_or_else(|| panic!("{name}"));
        s[at..(at + len).min(s.len())].to_string()
    }

    /// P1 (Skye 259 #7): Repair of a cut / css2 / pre-c258 folder must download, not reregister.
    #[test]
    fn c260_p1_repair_routes_cut_folders_to_download() {
        let d = dir("repair").join("Barlow");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("barlow-400-normal.ttf"), sfnt(272)).unwrap();
        fs::write(d.join("barlow-500-normal.ttf"), sfnt(680)).unwrap();
        assert!(repair_must_download(std::slice::from_ref(&d), "Barlow"), "cut 400 → download path");
        fs::write(d.join("barlow-400-normal.ttf"), sfnt(680)).unwrap();
        assert!(repair_must_download(std::slice::from_ref(&d), "Barlow"), "pre-c258 (no .full-source) → download path");
        gfsource::set_full_source(&d, ListingSource::Manifest);
        assert!(!repair_must_download(std::slice::from_ref(&d), "Barlow"), "healthy full-source folder → reregister is fine");
        // The retry loop asks before reregistering.
        let body = fn_body("pub fn retry_google_downloads_blocking", 2500);
        let ask = body.find("repair_must_download").expect("retry must ask repair_must_download");
        let rereg = body.find("reregister_intact_family").unwrap();
        assert!(ask < rereg);
        let _ = fs::remove_dir_all(d.parent().unwrap());
    }

    /// P1 (Skye 259 #2): VF-Primary default flags statics-only VF family as missing_variable;
    /// Static-first opt-in on a full-source VF-catalog family is not "missing variable".
    #[test]
    fn c260_p1_static_first_vf_family_not_incomplete() {
        let d = dir("scan").join("Inter");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("inter-400-normal.ttf"), sfnt(2900)).unwrap();
        assert!(catalog_variable_expects_public_vf("Inter"));
        assert!(dir_missing_variable(&d, "Inter", false, 1), "legacy statics-only still nags");
        gfsource::set_full_source(&d, ListingSource::Manifest);
        assert!(dir_missing_variable(&d, "Inter", false, 1), "1.0.206u VF-Primary default still expects VF");
        gfsource::set_dir_prefer_statics(&d, true);
        assert!(!dir_missing_variable(&d, "Inter", false, 1), "static-first opt-in full-source is complete");
        // Both Scan paths use the shared helper.
        let s = code();
        assert_eq!(s.matches("catalog_variable_expects_public_vf(family) && !has_variable && intact > 0;").count(), 0);
        assert_eq!(s.matches("catalog_variable_expects_public_vf(&name) && !has_variable && intact > 0;").count(), 0);
        assert_eq!(s.matches("dir_missing_variable(dir, ").count(), 2, "both Scan paths use the helper");
        let _ = fs::remove_dir_all(d.parent().unwrap());
    }

    /// P2 (Skye 259 #5): restore loads the good faces of a partly-cut family.
    #[test]
    fn c260_p2_restore_partial_for_partly_cut() {
        let d = dir("restore").join("Barlow");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("barlow-400-normal.ttf"), sfnt(272)).unwrap();
        fs::write(d.join("barlow-500-normal.ttf"), sfnt(680)).unwrap();
        fs::write(d.join("barlow-600-normal.ttf"), sfnt(680)).unwrap();
        assert_eq!(restore_action(&d, "Barlow"), RestoreAction::Partial { cut: 1 });
        // The cut face itself is still refused for Add.
        assert!(!face_allowed_for_register(&d, &d.join("barlow-400-normal.ttf"), "Barlow"));
        assert!(face_allowed_for_register(&d, &d.join("barlow-500-normal.ttf"), "Barlow"));
        let e = dir("restore-empty");
        assert_eq!(restore_action(&e, "Barlow"), RestoreAction::Skip);
        // Restore surfaces the per-family state (UI data) for partial families.
        assert!(fn_body("pub fn session_begin", 15000).contains("restore_partial_detail"));
        let _ = fs::remove_dir_all(d.parent().unwrap());
        let _ = fs::remove_dir_all(&e);
    }

    /// P2 (Skye 259 #4): every HTTP GET goes through the shared limiter.
    #[test]
    fn c260_p2_all_http_through_limiter() {
        let code = code();
        for raw in ["client.get(", ".get(&url)", ".get(&href)", ".get(&meta_url)", "user_agent(\"FontManager/1.0\")"] {
            assert!(!code.contains(raw), "raw HTTP outside gfsource: {raw}");
        }
    }

    /// P2 (Skye 259 observation): tests never resolve the real %LOCALAPPDATA%.
    #[test]
    fn c260_p2_tests_never_use_real_localappdata() {
        let real = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        let ours = local_app_data().expect("test local app data");
        // On Windows %TEMP% sits under LOCALAPPDATA: require the temp root, and never
        // the real `Font Manager` tree.
        assert!(ours.starts_with(std::env::temp_dir()), "{}", ours.display());
        if let Some(r) = &real {
            assert!(!ours.starts_with(r.join("Font Manager")), "{} is the real Font Manager dir", ours.display());
        }
        guard_test_path(&ours);
        assert!(gdi_maps_root().unwrap().starts_with(&ours));
        assert!(fm_local_root().unwrap().starts_with(&ours));
    }

    /// P2: tests never call the real GDI Add/Remove/broadcast.
    #[test]
    fn c260_p2_tests_never_touch_gdi_session() {
        let c = code();
        let at = c.find("mod winfont {").unwrap();
        let body = &c[at..at + 2600];
        assert!(body.contains("#[cfg(not(test))]\n    #[link(name = \"gdi32\")]"), "gdi32 extern must be cfg(not(test))");
        assert!(body.contains("#[cfg(test)]"));
    }

    #[test]
    #[should_panic(expected = "real user dir")]
    fn c260_p2_guard_panics_on_real_localappdata() {
        let real = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Users\x\AppData\Local"));
        guard_test_path(&real.join("Font Manager").join("gdi-maps").join("x.ttf"));
    }
}

#[cfg(test)]
mod c260f_tests {
    use super::*;

    fn src() -> String {
        all_activate_sources()
    }
    fn body(name: &str, len: usize) -> String {
        let s = src();
        let at = s.find(name).unwrap_or_else(|| panic!("{name}"));
        s[at..(at + len).min(s.len())].to_string()
    }

    /// Desk ruling: Repair never changes the active set.
    #[test]
    fn c260f_repair_inactive_stays_inactive_active_stays_active() {
        let root = std::env::temp_dir().join(format!("fm-c260f-{}", std::process::id()));
        let inactive = root.join("Barlow");
        let active = root.join("Tinos");
        fs::create_dir_all(&inactive).unwrap();
        fs::create_dir_all(&active).unwrap();
        // Inactive cut family: files only.
        mark_repair_files_only("Barlow", std::slice::from_ref(&inactive));
        assert!(repair_files_only("Barlow"));
        assert!(!register_allowed_for_path(&inactive.join("barlow-400-normal.ttf")), "no GDI load");
        assert!(!session_join_after_download("Barlow"), "stays inactive");
        // Active family (never marked): reloads and stays in the session.
        assert!(!repair_files_only("Tinos"));
        assert!(register_allowed_for_path(&active.join("tinos-400-normal.ttf")));
        assert!(session_join_after_download("Tinos"));
        clear_repair_files_only("Barlow");
        assert!(register_allowed_for_path(&inactive.join("barlow-400-normal.ttf")));
        // Wiring: Repair marks inactive families; register + session join + the
        // final register in download_family honour the flag.
        let retry = body("pub fn retry_google_downloads_blocking", 3000);
        assert!(retry.contains("mark_repair_files_only"));
        assert!(retry.contains("load_session_families"));
        assert!(body("fn register_path_detailed(", 400).contains("register_allowed_for_path"));
        assert!(src().contains("if session_join_after_download(&family) {"));
        assert!(src().contains("let (total, reg_cause) = if repair_files_only(family) {"));
        let _ = fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod c262_tests {
    use super::*;

    #[test]
    fn c262_sandbox_root_only_for_sandbox_id_or_env() {
        assert_eq!(sandbox_root_for("app.fontmanager.desktop", None), None, "production unchanged");
        assert_eq!(sandbox_root_for("app.fontmanager.desktop", Some("".into())), None);
        let t = std::env::temp_dir().join("fm-sbx-x");
        assert_eq!(sandbox_root_for("app.fontmanager.desktop", Some(t.clone().into())), Some(t.clone()));
        assert_eq!(sandbox_root_for(SANDBOX_IDENTIFIER, None), Some(std::env::temp_dir().join("fm-sandbox")));
        assert_eq!(sandbox_root_for(SANDBOX_IDENTIFIER, Some(t.clone().into())), Some(t));
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.sandbox.conf.json")).unwrap();
        assert_eq!(conf["identifier"], SANDBOX_IDENTIFIER);
        let src = all_activate_sources();
        let at = src.find("pub fn session_begin").unwrap();
        assert!(src[at..at + 300].contains("if sandbox_root().is_some()"));
    }
}

#[cfg(test)]
mod c263_tests {
    use super::*;

    fn src() -> String {
        all_activate_sources()
    }
    fn body(name: &str, len: usize) -> String {
        let s = src();
        let at = s.find(name).unwrap_or_else(|| panic!("{name}"));
        s[at..(at + len).min(s.len())].to_string()
    }

    /// B1: one Add per face per job — no per-face Add inside the listing download,
    /// and the ready path heals before its single register.
    #[test]
    fn c263_b1_no_duplicate_adds_in_download_path() {
        let dl = body("fn download_listed_faces_to_dir(", 9000);
        let dl = &dl[..dl.find("\nfn fetch_google_family_faces_to_dir").unwrap_or(dl.len())];
        assert!(!dl.contains("register_path("), "per-face Add in download listing");
        let fam = body("fn download_family(", 4000);
        let heal = fam.find("heal_family_google_names(app, family, false)").unwrap();
        let reg = fam.find("existing = register_intact_family(app, family);").unwrap();
        assert!(heal < reg, "heal must run before the one register");
        assert_eq!(fam.matches("register_intact_family(app, family)").count(), 2, "ready + Local only");
    }

    /// B2: stale files-only marks never outlive the job; explicit Activate clears them.
    #[test]
    fn c263_b2_files_only_marks_cleared() {
        let d = std::env::temp_dir().join(format!("fm-c263-{}", std::process::id()));
        mark_repair_files_only("Barlow", std::slice::from_ref(&d));
        mark_repair_files_only("Tinos", &[d.join("t")]);
        clear_all_repair_files_only();
        assert!(!repair_files_only("Barlow") && !repair_files_only("Tinos"));
        assert!(body("fn run_google_bulk(", 300).contains("clear_all_repair_files_only()"));
        assert!(body("pub async fn start_google_downloads(", 600).contains("clear_repair_files_only(f)"));
        let retry = body("pub fn retry_google_downloads_blocking", 2500);
        assert_eq!(retry.matches("load_session_families(&app)").count(), 1);
    }

    /// B5: stale test temp dirs are swept (prefix-only, other pids, old).
    #[test]
    fn c263_b5_sweep_only_stale_prefixed_dirs() {
        let root = std::env::temp_dir().join(format!("fm-c263-sweep-{}", std::process::id()));
        let old = root.join("fm-test-localappdata-1");
        let other = root.join("keep-me");
        let mine = root.join(format!("fm-test-localappdata-{}", std::process::id()));
        for d in [&old, &other, &mine] {
            fs::create_dir_all(d).unwrap();
        }
        std::thread::sleep(Duration::from_millis(30));
        sweep_stale_test_dirs(&root);
        assert!(old.exists(), "a fresh dir (maybe a running test) is kept");
        sweep_stale_test_dirs_older(&root, Duration::ZERO);
        assert!(other.exists() && mine.exists());
        assert!(!old.exists(), "stale test dir swept");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn multi_wave_bulk_and_preexisting_google_plan_adoption() {
        let root = std::env::temp_dir().join(format!("fm-multiwave-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let nunito = root.join("Nunito");
        fs::create_dir_all(&nunito).unwrap();
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(512, 0);
        fs::write(nunito.join("nunito-variable-wght.ttf"), &fake).unwrap();
        assert!(!dir_is_complete(&nunito));
        heal_preexisting_google_plan(&nunito, "Nunito");
        verify_complete_marker(&nunito);
        assert!(
            dir_is_complete(&nunito),
            "pre-existing intact VF folder must adopt plan and stamp .complete without network"
        );
        let bulk_body = body("fn run_google_bulk_inner(", 3800);
        assert!(
            bulk_body.contains("commit_ready_families_at(&app, &ready, index.as_ref(), done_base)"),
            "bulk loop must accumulate done_base across waves"
        );
        assert!(
            !bulk_body.contains("p.done = 0;"),
            "run_google_bulk_inner must never reset p.done mid-job"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn prestage_gdi_bytes_and_fast_unload_keep_stage_contracts() {
        let root = std::env::temp_dir().join(format!("fm-prestage-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let docs = root.join("Documents").join("Font Manager").join("Library").join("Inter");
        let stage_dir = root.join("gdi-maps");
        fs::create_dir_all(&docs).unwrap();
        let src_file = docs.join("inter-regular.ttf");
        let mut fake = b"\x00\x01\x00\x00".to_vec();
        fake.resize(640, 0x2A);
        fs::write(&src_file, &fake).unwrap();
        let staged = prestage_gdi_session_bytes_to(&src_file, &fake, &stage_dir)
            .expect("prestage_gdi_session_bytes_to must return staged path");
        assert!(staged.exists(), "staged GDI file must exist immediately on write");
        assert_eq!(fs::read(&staged).unwrap().len(), fake.len());
        let resolved = ensure_gdi_session_copy_to(&src_file, &stage_dir);
        assert_eq!(resolved, Some(staged), "ensure_gdi_session_copy_to must reuse pre-staged file");

        let unload_body = body("fn unload_now(", 12000);
        assert!(
            unload_body.contains("winfont::unregister_family_keep_stage(t, &mut unloaded_paths)"),
            "unload_now must use O(1) unregister_family_keep_stage so gdi-maps stay warm"
        );
        let lock_check = unload_body
            .find("let mut still = filter_still_write_locked(&unloaded_paths);")
            .expect("unload_now must check filter_still_write_locked first");
        let svc_restart = unload_body
            .find("winfont::restart_font_cache_service(font_cache_restart_budget())")
            .expect("unload_now keeps service restart fallback for locked files");
        assert!(
            lock_check < svc_restart,
            "unload_now must only restart FontCache service when files are actually write-locked"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn consolidate_legacy_roots_merges_and_scan_dedups_cleanly() {
        let root = std::env::temp_dir().join(format!("fm-consolidate-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let act_inter = root.join("Activated").join("Inter");
        let lib_lora = root.join("Library").join("Lora");
        let root_inter = root.join("Inter");
        let root_upload = root.join("MyCustomUpload");
        fs::create_dir_all(&act_inter).unwrap();
        fs::create_dir_all(&lib_lora).unwrap();
        fs::create_dir_all(&root_inter).unwrap();
        fs::create_dir_all(&root_upload).unwrap();

        let mut f1 = b"\x00\x01\x00\x00".to_vec();
        f1.resize(300, 1);
        let mut f2 = b"\x00\x01\x00\x00".to_vec();
        f2.resize(320, 2);
        fs::write(root_inter.join("inter-400.ttf"), &f1).unwrap();
        write_download_source(&root_inter, FetchIntent::Google);
        fs::write(act_inter.join("inter-400.ttf"), b"older-never-overwrite").unwrap();
        fs::write(act_inter.join("inter-700.ttf"), &f2).unwrap();
        fs::write(lib_lora.join("lora-400.ttf"), &f1).unwrap();
        fs::write(root_upload.join("custom.ttf"), &f1).unwrap();
        write_download_source(&root_upload, FetchIntent::Local);

        let moved = consolidate_legacy_roots_in(&root, None);
        assert!(moved >= 1, "must consolidate legacy folders: {moved}");
        let g = google_container_path(&root);
        assert!(g.join("Lora").join("lora-400.ttf").is_file(), "Library/Lora moved to Providers/Google/");
        assert!(g.join("Inter").join("inter-700.ttf").is_file(), "non-conflicting Activated/Inter face merged");
        assert_eq!(fs::read(g.join("Inter").join("inter-400.ttf")).unwrap(), f1, "never overwrite existing face");
        assert!(root.join("Uploads").join("MyCustomUpload").join("custom.ttf").is_file(), "local uploads consolidate into Uploads/");

        // Verify scan dedup helper merges overlapping families accurately.
        let merged = merge_disk_family_rows(vec![
            DiskFamily {
                name: "Inter".into(),
                bytes: 300,
                files: 1,
                corrupt: 0,
                incomplete: false,
                settled: false,
                has_complete: true,
                has_variable: false,
                missing_variable: true,
                undersized: false,
            },
            DiskFamily {
                name: "inter".into(),
                bytes: 500,
                files: 1,
                corrupt: 0,
                incomplete: false,
                settled: false,
                has_complete: true,
                has_variable: true,
                missing_variable: true,
                undersized: false,
            },
        ]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].files, 2);
        assert_eq!(merged[0].bytes, 800);
        assert!(merged[0].has_variable);
        assert!(!merged[0].missing_variable);

        // Verify HKCU session tag & refresh enhancements.
        let ttf_val = crate::session_stage::session_hkcu_value_name(Path::new(r"C:\Stage\abcd1234ef567890.ttf"));
        let otf_val = crate::session_stage::session_hkcu_value_name(Path::new(r"C:\Stage\abcd1234ef567890.otf"));
        assert!(ttf_val.ends_with("(TrueType)"));
        assert!(otf_val.ends_with("(OpenType)"));
        assert!(crate::session_stage::is_fm_session_hkcu_value_name(&ttf_val));
        assert!(!crate::session_stage::is_fm_session_hkcu_value_name("Arial (TrueType)"));

        let work = body("fn sync_documents_vf_policy_work(", 2500);
        assert!(work.contains("consolidate_legacy_roots_in"));
        assert!(work.contains("heal_google_variable_names_in_dir"));
        assert!(work.contains("sweep_gdi_maps_part_files"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fast_session_resume_bypasses_deep_check_only_on_ready_non_web_dirs() {
        let root = std::env::temp_dir().join(format!("fm-fast-resume-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let cfg = root.join("cfg");
        let inter = root.join("Inter");
        fs::create_dir_all(&cfg).unwrap();
        fs::create_dir_all(&inter).unwrap();

        // Default ON (automatic Fast Session Resume on fresh installs & reopen).
        assert!(gfsource::fast_session_resume(Some(&cfg)));
        gfsource::set_fast_session_resume(Some(&cfg), false);
        assert!(cfg.join(gfsource::NO_FAST_RESUME_MARKER).is_file());
        assert!(!cfg.join(gfsource::FAST_RESUME_MARKER).exists());
        assert!(!gfsource::fast_session_resume(Some(&cfg)));
        gfsource::set_fast_session_resume(Some(&cfg), true);
        assert!(cfg.join(gfsource::FAST_RESUME_MARKER).is_file());
        assert!(!cfg.join(gfsource::NO_FAST_RESUME_MARKER).exists());
        assert!(gfsource::fast_session_resume(Some(&cfg)));

        // Fast mode skips SFNT opens when .complete + font file >= 12B exist,
        // while strict mode still detects bad header / missing expected provenance.
        fs::write(inter.join("inter-400.ttf"), b"not-a-valid-sfnt-header-payload").unwrap();
        fs::write(inter.join(".complete"), b"1\n").unwrap();
        assert!(
            family_dir_is_ready_with_mode(&inter, "Inter", true),
            "fast_resume=true trusts .complete + non-empty font file without SFNT parse"
        );
        assert!(
            !family_dir_is_ready_with_mode(&inter, "Inter", false),
            "fast_resume=false runs strict verify and rejects invalid SFNT + bare .complete"
        );

        // .web-build and Gidugu never take the fast bypass.
        let gidugu = root.join("Gidugu");
        fs::create_dir_all(&gidugu).unwrap();
        fs::write(gidugu.join("gidugu-400.ttf"), b"not-a-valid-sfnt-header-payload").unwrap();
        fs::write(gidugu.join(".complete"), b"1\n").unwrap();
        assert!(!family_dir_is_ready_with_mode(&gidugu, "Gidugu", true));
        assert!(family_known_gdi_session_incapable("Gidugu"));

        let web_dir = root.join("Lora");
        fs::create_dir_all(&web_dir).unwrap();
        fs::write(web_dir.join("lora-400.ttf"), b"not-a-valid-sfnt-header-payload").unwrap();
        fs::write(web_dir.join(".complete"), b"1\n").unwrap();
        fs::write(web_dir.join(".web-build"), b"1\n").unwrap();
        assert!(!family_dir_is_ready_with_mode(&web_dir, "Lora", true));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn pure_variable_font_clears_stale_web_build_and_heals_complete() {
        let root = std::env::temp_dir().join(format!("fm-pure-vf-heal-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let dir = root.join("Roboto Flex");
        fs::create_dir_all(&dir).unwrap();

        let mut vf = b"\x00\x01\x00\x00".to_vec();
        vf.resize(2048, 0);
        let vf_name = "roboto-flex-variable-wght.ttf";
        fs::write(dir.join(vf_name), &vf).unwrap();
        write_google_planned(&dir, &[vf_name.to_string()]);
        gfsource::set_web_build(&dir, true);

        assert!(dir_is_pure_variable_font(&dir));
        heal_preexisting_google_plan(&dir, "Roboto Flex");
        assert!(!gfsource::dir_is_web_build(&dir), "stale .web-build cleared on pure VF folder");
        assert!(dir_is_complete(&dir), ".complete stamped on pure VF folder");
        assert!(family_dir_is_ready_with_mode(&dir, "Roboto Flex", false));

        // Planned static folder missing only .complete heals without re-downloading.
        let lekton = root.join("Lekton");
        fs::create_dir_all(&lekton).unwrap();
        fs::write(lekton.join("lekton-400-normal.ttf"), &vf).unwrap();
        gfsource::set_full_source(&lekton, gfsource::ListingSource::Repo);
        write_google_planned(&lekton, &["lekton-400-normal.ttf".to_string()]);
        assert!(!dir_is_complete(&lekton));
        heal_preexisting_google_plan(&lekton, "Lekton");
        assert!(dir_is_complete(&lekton), "intact planned folder heals missing .complete");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_index_stamp_invalidates_on_marker_change_and_formats_lock_holders() {
        let root = std::env::temp_dir().join(format!("fm-scan-idx-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let dir = root.join("Inter");
        fs::create_dir_all(&dir).unwrap();
        let mut ttf = b"\x00\x01\x00\x00".to_vec();
        ttf.resize(512, 0);
        fs::write(dir.join("inter-400-normal.ttf"), &ttf).unwrap();

        let s1 = dir_scan_stamp(&dir).expect("stamp before .complete");
        fs::write(dir.join(".complete"), b"1\n").unwrap();
        let s2 = dir_scan_stamp(&dir).expect("stamp after .complete");
        assert_ne!(s1, s2, "adding .complete must invalidate scan index stamp");

        // Round-trip DiskScanCacheEntry JSON cleanly.
        let idx_file = root.join("scan-index.json");
        let mut map = HashMap::new();
        map.insert(
            dir.to_string_lossy().into_owned(),
            DiskScanCacheEntry {
                stamp: s2,
                row: Some(DiskFamily {
                    name: "Inter".into(),
                    bytes: 512,
                    files: 1,
                    corrupt: 0,
                    incomplete: false,
                    settled: false,
                    has_complete: true,
                    has_variable: false,
                    missing_variable: false,
                    undersized: false,
                }),
            },
        );
        let raw = serde_json::to_vec(&map).unwrap();
        fs::write(&idx_file, &raw).unwrap();
        let decoded: HashMap<String, DiskScanCacheEntry> =
            serde_json::from_slice(&fs::read(&idx_file).unwrap()).unwrap();
        assert_eq!(decoded.len(), 1);

        // Lock holder deduplication & empty-slice safety.
        assert!(winfont::query_file_lock_holders(&[]).is_empty());
        let dedup = winfont::format_lock_holders(&[
            "Illustrator.exe".into(),
            "illustrator.exe".into(),
            "  fontdrvhost.exe ".into(),
            "".into(),
        ]);
        assert_eq!(dedup, vec!["Illustrator.exe", "fontdrvhost.exe"]);

        let _ = fs::remove_dir_all(&root);
    }

    fn temp_v2_dir(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("fm-v2-{label}-{}-{nanos}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn b1_missing_family_reported_not_silently_skipped() {
        let families = vec![
            "Figtree".to_string(),
            "DefinitelyMissingFontXYZ".to_string(),
        ];
        let intents = vec!["google".to_string()];
        let (resolved, unresolved) =
            classify_download_intents(&families, &intents, |_| None);
        assert_eq!(resolved, vec![("Figtree".to_string(), FetchIntent::Google)]);
        assert_eq!(unresolved, vec!["DefinitelyMissingFontXYZ".to_string()]);
    }

    #[test]
    fn b2_b3_vf_purge_skips_local_and_mismatched_win32_family() {
        let parent = temp_v2_dir("vf-purge-safety");
        let fig_vf = include_bytes!("../../../tests/fixtures/Figtree-wght.ttf");
        let patched_fig_vf = crate::namepatch::patch_variable_face(fig_vf, "Figtree", false)
            .expect("patch Figtree variable font");
        let inter_opsz = include_bytes!("../../../tests/fixtures/Inter-opsz-wght.ttf");
        let mut fake_static = b"\x00\x01\x00\x00".to_vec();
        fake_static.resize(256, 0);

        // 1. Local uploaded family keeps sibling static files even with a clean VF.
        let local_dir = parent.join("FigtreeLocal");
        fs::create_dir_all(&local_dir).unwrap();
        fs::write(local_dir.join("figtree-variable-wght.ttf"), &patched_fig_vf).unwrap();
        fs::write(local_dir.join("figtree-400-normal.ttf"), &fake_static).unwrap();
        write_download_source(&local_dir, FetchIntent::Local);
        assert!(
            !vf_primary_purge_safe_for_dir(&local_dir, "Figtree"),
            "local uploads must never purge sibling static faces"
        );
        let (del_local, _) = purge_redundant_statics_in_dir(&local_dir, "Figtree");
        assert_eq!(del_local, 0);
        assert!(local_dir.join("figtree-400-normal.ttf").is_file());

        // 2. Unpatched VF with Win32 nameID 1 ("Figtree Light" != "Figtree") blocks purge.
        let mismatch_dir = parent.join("FigtreeUnpatched");
        fs::create_dir_all(&mismatch_dir).unwrap();
        fs::write(mismatch_dir.join("figtree-variable-wght.ttf"), fig_vf).unwrap();
        fs::write(mismatch_dir.join("figtree-400-normal.ttf"), &fake_static).unwrap();
        write_download_source(&mismatch_dir, FetchIntent::Google);
        assert!(
            !vf_primary_purge_safe_for_dir(&mismatch_dir, "Figtree"),
            "mismatched Win32 family must block static purge"
        );

        // 3. Non-standard variation axis (opsz) blocks purge.
        let opsz_dir = parent.join("Inter");
        fs::create_dir_all(&opsz_dir).unwrap();
        fs::write(opsz_dir.join("inter-variable-opsz-wght.ttf"), inter_opsz).unwrap();
        fs::write(opsz_dir.join("inter-400-normal.ttf"), &fake_static).unwrap();
        write_download_source(&opsz_dir, FetchIntent::Google);
        assert!(
            !vf_primary_purge_safe_for_dir(&opsz_dir, "Inter"),
            "VF with non-standard axis (opsz) must keep statics"
        );

        // 4. Clean name-patched Google standard VF (Figtree wght, Win32 name == Figtree) allows purge.
        let clean_dir = parent.join("Figtree");
        fs::create_dir_all(&clean_dir).unwrap();
        fs::write(clean_dir.join("figtree-variable-wght.ttf"), &patched_fig_vf).unwrap();
        fs::write(clean_dir.join("figtree-400-normal.ttf"), &fake_static).unwrap();
        write_download_source(&clean_dir, FetchIntent::Google);
        mark_family_complete(&clean_dir, 2);
        assert!(vf_primary_purge_safe_for_dir(&clean_dir, "Figtree"));
        let (del_clean, _) = purge_redundant_statics_in_dir(&clean_dir, "Figtree");
        assert_eq!(del_clean, 1);
        assert!(!clean_dir.join("figtree-400-normal.ttf").is_file());
        assert!(clean_dir.join("figtree-variable-wght.ttf").is_file());

        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn b6_leftover_stage_files_excludes_library_and_active_maps() {
        let root = temp_v2_dir("b6-stage-leftovers");
        let docs_dir = root.join("Documents").join("Font Manager").join("Inter");
        let maps_dir = root.join("Local").join("Font Manager").join("gdi-maps");
        fs::create_dir_all(&docs_dir).unwrap();
        fs::create_dir_all(&maps_dir).unwrap();

        let lib_file = docs_dir.join("inter-400-normal.ttf");
        fs::write(&lib_file, b"\x00\x01\x00\x00test").unwrap();

        let active_stage = maps_dir.join("active-face.ttf");
        let stale_stage = maps_dir.join("stale-face.ttf");
        let part_stage = maps_dir.join("half-copy.part");
        fs::write(&active_stage, b"\x00\x01\x00\x00active").unwrap();
        fs::write(&stale_stage, b"\x00\x01\x00\x00stale").unwrap();
        fs::write(&part_stage, b"partial").unwrap();

        let mut active = HashSet::new();
        active.insert(crate::session_stage::normalize_path_key(&active_stage));

        let mut mapped = HashMap::new();
        mapped.insert(
            crate::session_stage::normalize_path_key(&stale_stage),
            (
                lib_file.to_string_lossy().into_owned(),
                "Inter".to_string(),
            ),
        );

        let leftovers = find_leftover_stage_files_in(&maps_dir, &active, &mapped);
        let names: Vec<&str> = leftovers.iter().map(|l| l.file_name.as_str()).collect();
        assert!(!names.contains(&"active-face.ttf"), "active map must be skipped");
        assert!(names.contains(&"stale-face.ttf"), "inactive map must be listed");
        assert!(names.contains(&"half-copy.part"), "part remnant must be listed");

        // Refuse deleting paths outside maps_dir (e.g. Documents library file) or active maps.
        let res = recycle_stage_files_in(
            &maps_dir,
            &active,
            vec![
                lib_file.to_string_lossy().into_owned(),
                active_stage.to_string_lossy().into_owned(),
                stale_stage.to_string_lossy().into_owned(),
            ],
        );
        assert_eq!(res.requested, 3);
        assert_eq!(res.recycled, 1);
        assert_eq!(res.failed.len(), 2);
        assert!(lib_file.is_file(), "Documents library file must never be deleted");
        assert!(active_stage.is_file(), "active session stage file must never be deleted");
        assert!(!stale_stage.is_file(), "inactive stale stage copy removed");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn b13_register_font_path_resolves_family_and_guards_system_fonts() {
        let root = temp_v2_dir("b13-register-path");
        let sys_err = register_font_path_in(Path::new(r"C:\Windows\Fonts\arial.ttf"), Some("Arial"))
            .unwrap_err();
        assert!(sys_err.contains("Windows\\Fonts"), "must block Windows\\Fonts: {sys_err}");

        let bad_file = root.join("corrupt.ttf");
        fs::write(&bad_file, b"not-a-font").unwrap();
        let corrupt_err = register_font_path_in(&bad_file, None).unwrap_err();
        assert!(corrupt_err.contains("intact font"), "must reject corrupt font: {corrupt_err}");

        let marker_dir = root.join("u-custommarker");
        fs::create_dir_all(&marker_dir).unwrap();
        fs::write(marker_dir.join(".family"), b"Custom Marker Family").unwrap();
        let non_sfnt = marker_dir.join("placeholder.ttf");
        fs::write(&non_sfnt, b"placeholder").unwrap();
        assert_eq!(
            resolve_register_font_family(&non_sfnt, Some("  Explicit Family  ")).as_deref(),
            Some("Explicit Family")
        );
        assert_eq!(
            resolve_register_font_family(&non_sfnt, None).as_deref(),
            Some("Custom Marker Family")
        );

        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("tests")
            .join("fixtures")
            .join("Figtree-wght.ttf");
        if fixture.is_file() {
            assert_eq!(
                resolve_register_font_family(&fixture, None).as_deref(),
                Some("Figtree")
            );
        }

        let src = all_activate_sources();
        let at = src.find("pub fn register_font_path(").unwrap();
        let body = &src[at..(at + 500).min(src.len())];
        assert!(body.contains("session_add(&app,"), "register_font_path must update .session-families.json");
        assert!(body.contains("persist_activation_sidecars(&app)"), "register_font_path must persist session sidecars");
        assert!(body.contains("notify_fonts_changed_maybe()"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn v2_google_and_uploads_folder_separation_and_prune_protection() {
        let root = temp_v2_dir("v2-google-uploads-sep");
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("tests")
            .join("fixtures")
            .join("Figtree-wght.ttf");
        let sfnt_bytes = fs::read(&fixture).expect("Figtree-wght.ttf fixture must exist");

        let google_dir = family_dir_in(&root, "Inter");
        assert!(
            google_dir.starts_with(google_container_path(&root)),
            "Google families must live under Providers/Google/: {}",
            google_dir.display()
        );
        fs::create_dir_all(&google_dir).unwrap();
        fs::write(google_dir.join("inter-400.ttf"), &sfnt_bytes).unwrap();
        write_download_source(&google_dir, FetchIntent::Google);

        let upload_path = install_font_file_in(&root, "LocalHero", "LocalHero.ttf", &sfnt_bytes).unwrap();
        assert!(
            upload_path.starts_with(root.join("Uploads")),
            "Added/uploaded fonts must live under Uploads/: {}",
            upload_path.display()
        );

        let upload_dir = upload_path.parent().unwrap().to_path_buf();
        let unknown_google = root.join("Google").join("Figtree");
        fs::create_dir_all(&unknown_google).unwrap();
        fs::write(unknown_google.join("Figtree.ttf"), &sfnt_bytes).unwrap();

        let keep: HashSet<String> = alias_keys("Inter").into_iter().collect();
        let candidates = prune_candidates(&[google_dir, upload_dir.clone(), unknown_google.clone()], &root, &keep);
        assert_eq!(
            candidates,
            vec![unknown_google],
            "prune must spare Uploads/ even when absent from Google keep set"
        );
        assert!(family_locations_in(&root, "LocalHero").contains(&upload_dir));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn uploads_folder_ready_without_google_complete_marker() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!("fm-upload-ready-{}-{nanos}", std::process::id()));
        let upload_dir = root.join(UPLOADS_CONTAINER).join("UploadedHero");
        fs::create_dir_all(&upload_dir).unwrap();
        let mut sfnt = b"\x00\x01\x00\x00".to_vec();
        sfnt.resize(512, 0);
        fs::write(upload_dir.join("UploadedHero.ttf"), &sfnt).unwrap();
        assert!(!dir_is_complete(&upload_dir));
        assert!(
            family_dir_is_ready_with_mode(&upload_dir, "UploadedHero", false),
            "Uploads/<Family> with intact SFNT must restore cleanly without .complete"
        );
        assert!(
            family_dir_is_ready_with_mode(&upload_dir, "UploadedHero", true),
            "Uploads/<Family> fast resume must restore cleanly without .complete"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn parse_live_google_catalog_json_parses_metadata_rows() {
        let mut items = Vec::with_capacity(1005);
        items.push(serde_json::json!({
            "family": "JetBrains Mono",
            "category": "Monospace",
            "popularity": 12,
            "axes": [{ "tag": "wght" }],
            "fonts": { "400": {}, "700": {}, "400i": {} }
        }));
        items.push(serde_json::json!({
            "family": "Noto Serif",
            "category": "Serif",
            "popularity": 5,
            "axes": [],
            "fonts": { "400": {}, "700": {} }
        }));
        items.push(serde_json::json!({
            "family": "Pacifico",
            "category": "Handwriting",
            "popularity": 20,
            "axes": [],
            "fonts": { "400": {} }
        }));
        for i in 0..1002 {
            items.push(serde_json::json!({
                "family": format!("Sample Sans {i}"),
                "category": "Sans Serif",
                "popularity": 100 + i,
                "axes": [],
                "fonts": { "400": {} }
            }));
        }
        let payload = serde_json::json!({ "familyMetadataList": items }).to_string();
        let body = format!("{}{}", ")]}'\n", payload);
        let rows = parse_live_google_catalog_json(&body).expect("parse live catalog");
        assert_eq!(rows.len(), 1005);
        assert_eq!(rows[0][0], "JetBrains Mono");
        assert_eq!(rows[0][1], "mono");
        assert_eq!(rows[0][2], serde_json::json!([400, 700]));
        assert_eq!(rows[0][3], true);
        assert_eq!(rows[0][4], true);
        assert_eq!(rows[0][6], serde_json::json!(["coding"]));
        assert_eq!(rows[1][6], serde_json::json!(["noto"]));
        assert_eq!(rows[2][6], serde_json::json!(["script"]));
    }

    #[test]
    fn cdn_revision_diff_detects_outdated_and_incomplete_google_families() {
        let root = std::env::temp_dir().join(format!("fm-cdn-rev-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let g = google_container_path(&root);
        let up_to_date_dir = g.join("Figtree");
        let stale_dir = g.join("Inter");
        let incomplete_dir = g.join("Roboto");
        fs::create_dir_all(&up_to_date_dir).unwrap();
        fs::create_dir_all(&stale_dir).unwrap();
        fs::create_dir_all(&incomplete_dir).unwrap();

        let fig_bytes = include_bytes!("../../../tests/fixtures/Figtree-wght.ttf");
        let inter_bytes = include_bytes!("../../../tests/fixtures/Inter-opsz-wght.ttf");

        fs::write(up_to_date_dir.join("Figtree-Variable.ttf"), fig_bytes).unwrap();
        mark_family_complete(&up_to_date_dir, 1);
        fs::write(family_cdn_revision_marker(&up_to_date_dir), "2025-05-01").unwrap();

        fs::write(stale_dir.join("Inter-Variable.ttf"), inter_bytes).unwrap();
        mark_family_complete(&stale_dir, 1);
        fs::write(family_cdn_revision_marker(&stale_dir), "2024-01-01").unwrap();

        // Incomplete family has no font files / no .complete marker
        let mut revs = HashMap::new();
        revs.insert("figtree".to_string(), "2025-05-01".to_string());
        revs.insert("inter".to_string(), "2025-06-15".to_string());
        revs.insert("roboto".to_string(), "2025-06-15".to_string());

        let (checked, outdated, incomplete) =
            find_google_families_needing_cdn_update_in(&root, &revs, false);
        assert_eq!(checked, 3);
        assert_eq!(outdated, vec!["Inter".to_string()]);
        assert_eq!(incomplete, vec!["Roboto".to_string()]);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn live_google_directory_accepts_new_google_families_like_agrave_pro_and_firjar() {
        register_live_google_families(["Agrave Pro", "Firjar"]);
        assert!(is_official_google_family("Agrave Pro"));
        assert!(is_official_google_family("agrave pro"));
        assert!(is_official_google_family("Firjar"));
        assert_eq!(infer_fetch_intent("Agrave Pro"), FetchIntent::Google);
        assert_eq!(infer_fetch_intent("Firjar"), FetchIntent::Google);
    }

    #[test]
    fn gdi_hard_ceiling_and_sfnt_magic_and_non_destructive_restore_guard() {
        assert!(!gdi_objects_at_hard_ceiling(8_000));
        assert!(!gdi_objects_at_hard_ceiling(9_199));
        assert!(gdi_objects_at_hard_ceiling(9_200));
        assert!(gdi_objects_at_hard_ceiling(10_000));

        let root = std::env::temp_dir().join(format!("fm-sfnt-guard-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let good = root.join("good.ttf");
        let bad = root.join("bad.ttf");
        fs::write(&good, include_bytes!("../../../tests/fixtures/Figtree-wght.ttf")).unwrap();
        fs::write(&bad, b"<!DOCTYPE html>404 Not Found").unwrap();
        assert!(has_valid_sfnt_magic(&good));
        assert!(!has_valid_sfnt_magic(&bad));
        let _ = fs::remove_dir_all(&root);

        let saved = vec!["Inter".into(), "Roboto".into(), "Missing".into()];
        let ready = vec!["Inter".into()];
        // Cancelled or GDI-capped restore must preserve the entire saved session list.
        assert_eq!(
            merge_session_families_after_restore(&saved, &ready, true, false, |_| false),
            saved
        );
        assert_eq!(
            merge_session_families_after_restore(&saved, &ready, false, true, |_| false),
            saved
        );
        // Normal partial restore keeps ready + families still intact on disk, drops deleted ones.
        assert_eq!(
            merge_session_families_after_restore(&saved, &ready, false, false, |f| f == "Roboto"),
            vec!["Inter".to_string(), "Roboto".to_string()]
        );
    }

    #[test]
    fn session_end_clears_hkcu_drains_gdi_maps_and_broadcasts_wm_fontchange_for_adobe() {
        let src = all_activate_sources();
        let at = src.find("pub fn session_end(app: &AppHandle)").unwrap();
        let end = src[at..].find("\n}\n").map(|i| at + i).unwrap();
        let body = &src[at..end];
        assert!(
            body.contains("winfont::clear_all_hkcu_session_entries()"),
            "session_end must clear HKCU session entries immediately"
        );
        assert!(
            body.contains("stage_paths_from_maps"),
            "session_end must drain all active and persisted session map paths"
        );
        assert!(
            body.contains("winfont::unload_paths(extra.clone(), true)"),
            "session_end must broadcast WM_FONTCHANGE so Adobe drops unloaded session fonts on quit"
        );
        assert!(sandbox_root_for("app.fonths.sandbox", None).is_some());
    }

    #[test]
    fn fonths_providers_google_layout_and_purge_non_google_spares_uploads() {
        let root = std::env::temp_dir().join(format!("fm-fonths-purge-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let g = google_container_path(&root);
        let u = root.join(UPLOADS_CONTAINER);
        let google_ok = g.join("Inter");
        let stray = g.join("StrayCustomFont");
        let upload_keep = u.join("MyBrandFont");
        fs::create_dir_all(&google_ok).unwrap();
        fs::create_dir_all(&stray).unwrap();
        fs::create_dir_all(&upload_keep).unwrap();

        let fig = include_bytes!("../../../tests/fixtures/Figtree-wght.ttf");
        fs::write(google_ok.join("Inter-Variable.ttf"), include_bytes!("../../../tests/fixtures/Inter-opsz-wght.ttf")).unwrap();
        fs::write(stray.join("StrayCustomFont-Regular.ttf"), fig).unwrap();
        fs::write(upload_keep.join("MyBrandFont.ttf"), fig).unwrap();
        write_family_name_marker(&upload_keep, "MyBrandFont");
        write_download_source(&upload_keep, FetchIntent::Local);

        let mut keep: Vec<String> = (0..520).map(|i| format!("CatalogFont{i}")).collect();
        keep.push("Inter".into());

        // Dry run detects StrayCustomFont only (spares Inter & Uploads/MyBrandFont)
        let dry = purge_non_google_fonts_in(&root, &keep, true, true).unwrap();
        assert_eq!(dry.candidates, vec!["StrayCustomFont".to_string()]);
        assert_eq!(dry.removed, 0);
        assert!(stray.exists());

        // Permanent purge removes StrayCustomFont, keeps Inter & Uploads/MyBrandFont
        let real = purge_non_google_fonts_in(&root, &keep, false, true).unwrap();
        assert_eq!(real.removed, 1);
        assert!(!stray.exists());
        assert!(google_ok.exists());
        assert!(upload_keep.exists());

        // Verify Providers\Google path & Noto Emoji compat-outline fallback
        assert_eq!(
            family_dir_for_source_in(&root, "Roboto", false),
            root.join("Providers").join("Google").join("Roboto")
        );
        assert!(is_protected_family_dir(&root, &root.join("Providers")));
        assert!(is_protected_family_dir(&root, &root.join("Providers").join("Google")));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn session_dedupe_and_gdi_state_never_exceed_unique_font_count() {
        let raw = vec![
            " Inter ".to_string(),
            "inter".to_string(),
            "".to_string(),
            "   ".to_string(),
            "Roboto".to_string(),
            "ROBOTO".to_string(),
            "Figtree".to_string(),
        ];
        let deduped = dedupe_family_names(&raw);
        assert_eq!(deduped, vec!["Inter", "Roboto", "Figtree"]);

        let snap = gdi_session_state();
        assert_eq!(snap.live_family_count, snap.live_families.len());
        assert!(snap.gdi_quota >= 10_000);
    }

    #[test]
    fn ensure_gdi_session_copy_rotates_around_locked_dest() {
        let root = std::env::temp_dir().join(format!("fonths-rotate-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let src_dir = root.join("src");
        let maps_root = root.join("gdi-maps");
        fs::create_dir_all(&src_dir).unwrap();
        fs::create_dir_all(&maps_root).unwrap();

        let src = src_dir.join("LockTest.ttf");
        fs::write(&src, include_bytes!("../../../tests/fixtures/Figtree-wght.ttf")).unwrap();

        // Simulate an un-overwritable destination path in `gdi-maps` by occupying `dest`
        // with a non-empty directory so rename/copy onto `dest` fails cleanly cross-platform.
        let blocked_dest = maps_root.join(gdi_map_file_name(&src));
        fs::create_dir_all(blocked_dest.join("locked-handle-sentinel")).unwrap();

        let staged = ensure_gdi_session_copy_to(&src, &maps_root)
            .expect("must rotate to a fresh stage path instead of failing StageCopyFailed");
        assert_ne!(staged, blocked_dest);
        assert!(staged.is_file());
        assert!(staged.starts_with(&maps_root));
        assert!(
            staged
                .file_name()
                .and_then(|s| s.to_str())
                .map(|n| n.contains("-r"))
                .unwrap_or(false),
            "rotated path expected: {}",
            staged.display()
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn prestaged_fast_restore_guards_windows_and_documents_and_respects_session_families() {
        let root = std::env::temp_dir().join(format!("fonths-prestaged-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let docs_src = root.join("Documents").join("Fonths").join("Providers").join("Google").join("Inter");
        let maps_dir = root.join("LocalAppData").join("Fonths").join("gdi-maps");
        fs::create_dir_all(&docs_src).unwrap();
        fs::create_dir_all(&maps_dir).unwrap();

        let src_inter = docs_src.join("Inter-Variable.ttf");
        let stage_inter = maps_dir.join("abcd_inter.ttf");
        fs::write(&src_inter, include_bytes!("../../../tests/fixtures/Inter-opsz-wght.ttf")).unwrap();
        fs::write(&stage_inter, include_bytes!("../../../tests/fixtures/Inter-opsz-wght.ttf")).unwrap();

        let disk_maps = vec![
            crate::session_stage::FaceMap {
                source: src_inter.to_string_lossy().into(),
                stage: stage_inter.to_string_lossy().into(),
                registry_name: String::new(),
                family: "Inter".into(),
            },
            crate::session_stage::FaceMap {
                source: docs_src.join("Missing.ttf").to_string_lossy().into(),
                stage: maps_dir.join("missing.ttf").to_string_lossy().into(),
                registry_name: String::new(),
                family: "MissingMap".into(),
            },
            crate::session_stage::FaceMap {
                source: src_inter.to_string_lossy().into(),
                stage: r"C:\Users\Eric\Documents\Fonths\Providers\Google\Inter\Inter.ttf".into(),
                registry_name: String::new(),
                family: "DocsLeak".into(),
            },
        ];
        let requested = vec!["Inter".into(), "MissingMap".into(), "DocsLeak".into(), "Roboto".into()];
        let (covered, uncovered) = partition_prestaged_targets(&requested, &disk_maps);
        assert_eq!(covered.len(), 1);
        assert_eq!(covered[0].0, "Inter");
        assert_eq!(uncovered, vec!["MissingMap", "DocsLeak", "Roboto"]);

        #[cfg(windows)]
        {
            assert_eq!(
                winfont::register_prestaged_stage_path(
                    Path::new(r"C:\Windows\Fonts\arial.ttf"),
                    &stage_inter
                ),
                winfont::RegisterOutcome::Refused
            );
            assert_eq!(
                winfont::register_prestaged_stage_path(
                    &src_inter,
                    Path::new(r"C:\Users\Eric\Documents\Fonths\Inter\a.ttf")
                ),
                winfont::RegisterOutcome::Refused
            );
            assert_eq!(
                winfont::register_prestaged_stage_path(&src_inter, &maps_dir.join("missing.ttf")),
                winfont::RegisterOutcome::StageCopyFailed
            );
            // Under #[cfg(test)], AddFontResourceExW returns 0 -> must report AddReturnedZero (never Ok)
            assert_eq!(
                winfont::register_prestaged_stage_path(&src_inter, &stage_inter),
                winfont::RegisterOutcome::AddReturnedZero
            );
        }

        let _ = fs::remove_dir_all(&root);
    }
}

