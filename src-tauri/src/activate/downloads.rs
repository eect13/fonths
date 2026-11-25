//! Google Fonts & Fontsource activation queues, retries, repairs, and progress events.

#[allow(unused_imports)]
use super::*;
#[allow(unused_imports)]
use std::collections::{HashMap, HashSet, VecDeque};
#[allow(unused_imports)]
use std::fs;
#[allow(unused_imports)]
use std::io::{ErrorKind, Read, Write};
#[allow(unused_imports)]
use std::path::{Path, PathBuf};
#[allow(unused_imports)]
use std::process::Command;
#[allow(unused_imports)]
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
#[allow(unused_imports)]
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
#[allow(unused_imports)]
use std::thread;
#[allow(unused_imports)]
use std::time::{Duration, Instant};
#[allow(unused_imports)]
use rayon::prelude::*;
#[allow(unused_imports)]
use serde::Serialize;
#[allow(unused_imports)]
use tauri::{AppHandle, Emitter, Manager};

pub fn forget_queued(family: &str) {
    if let Ok(mut queued) = bulk().queued.lock() {
        queued.remove(&family.trim().to_lowercase());
    }
    forget_fetch_intent(family);
}

pub fn remember_failed(family: &str, reason: &str) {
    if let Ok(mut p) = bulk().progress.lock() {
        if !p.failed_names.iter().any(|n| n.eq_ignore_ascii_case(family)) {
            p.failed_names.push(family.to_string());
            p.failed_details.push(format!("{family}: {reason}"));
        }
    }
}

#[derive(Clone, Serialize)]
pub struct GoogleDlProgress {
    pub running: bool,
    pub done: u32,
    pub total: u32,
    pub failed: u32,
    pub current: String,
    pub failed_names: Vec<String>,
    pub failed_details: Vec<String>,
    pub paused: bool,
    pub ready_names: Vec<String>,
    pub skipped: u32,
    /// Quiet settle (e.g. Gidugu known GDI-incapable + intact): clear pending, no toast.
    #[serde(default)]
    pub settled_names: Vec<String>,
    /// "download" | "remove" — JS bar uses this so Deactivate is not labelled Downloading.
    #[serde(default)]
    pub kind: String,
}

pub struct Bulk {
    pub cancel: AtomicBool,
    /// Documents refresh only. Never shared with Activate (`cancel` / "Stopping…").
    pub docs_cancel: AtomicBool,
    pub pause: AtomicBool,
    pub running: AtomicBool,
    /// 1.0.207: true while sync_documents_vf_policy is in scan/purge. Not an Escape hook.
    pub docs_vf_session_live: AtomicBool,
    pub bust: AtomicBool,
    pub progress: Mutex<GoogleDlProgress>,
    pub pending: Mutex<VecDeque<String>>,
    /// Live worker download queue drained immediately on `cancel_google_downloads`.
    pub active_queue: Mutex<Option<Arc<Mutex<VecDeque<String>>>>>,
    pub queued: Mutex<HashSet<String>>,
    pub denied: Mutex<HashSet<String>>,
    /// Per-family Activate intent (google | fontsource | local).
    pub intents: Mutex<HashMap<String, FetchIntent>>,
    pub circuits: Mutex<HashMap<&'static str, CdnGate>>,
    /// How many `ready_names` the last running progress event already sent.
    /// Events send only the new tail. A poll still returns the full list.
    pub ready_emitted: AtomicUsize,
}

pub fn bulk() -> &'static Bulk {
    static BULK: OnceLock<Bulk> = OnceLock::new();
    BULK.get_or_init(|| Bulk {
        cancel: AtomicBool::new(false),
        docs_cancel: AtomicBool::new(false),
        pause: AtomicBool::new(false),
        running: AtomicBool::new(false),
        docs_vf_session_live: AtomicBool::new(false),
        bust: AtomicBool::new(false),
        progress: Mutex::new(GoogleDlProgress {
            running: false,
            done: 0,
            total: 0,
            failed: 0,
            current: String::new(),
            failed_names: Vec::new(),
            failed_details: Vec::new(),
            paused: false,
            ready_names: Vec::new(),
            skipped: 0,
            settled_names: Vec::new(),
            kind: String::new(),
        }),
        pending: Mutex::new(VecDeque::new()),
        active_queue: Mutex::new(None),
        queued: Mutex::new(HashSet::new()),
        denied: Mutex::new(HashSet::new()),
        intents: Mutex::new(HashMap::new()),
        circuits: Mutex::new(HashMap::new()),
        ready_emitted: AtomicUsize::new(0),
    })
}

pub fn accept_new_families(families: Vec<String>) -> Vec<String> {
    let state = bulk();
    let Ok(mut queued) = state.queued.lock() else {
        return families;
    };
    let mut fresh = Vec::new();
    for family in families {
        let key = family.trim().to_lowercase();
        if key.is_empty() {
            continue;
        }
        if queued.insert(key.clone()) {
            fresh.push(family);
        }
    }
    if let Ok(mut denied) = state.denied.lock() {
        for family in &fresh {
            denied.remove(&family.trim().to_lowercase());
        }
    }
    fresh
}

/// Small catalog lookups (CSS, METADATA.pb, Fontsource API). A hung CDN must not
/// hold a whole-catalog worker for the 300s TTF body timeout.
pub const CATALOG_PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// Names not yet included in a running progress event.
pub fn ready_emit_delta(names: &[String], already_sent: usize) -> (Vec<String>, usize) {
    let n = names.len();
    let sent = if already_sent > n { 0 } else { already_sent };
    (names[sent..].to_vec(), n)
}

/// false = cancel. true = keep going (pause waits here, between faces, not inside a socket).
pub fn catalog_download_allowed() -> bool {
    loop {
        if bulk().cancel.load(Ordering::SeqCst) {
            return false;
        }
        if !bulk().pause.load(Ordering::SeqCst) {
            return true;
        }
        if let Ok(mut p) = bulk().progress.lock() {
            p.paused = true;
            p.running = true;
        }
        thread::sleep(Duration::from_millis(200));
    }
}

pub fn note_catalog_current(label: &str) {
    if let Ok(mut p) = bulk().progress.lock() {
        if p.current != label {
            p.current = label.to_string();
        }
    }
}

#[cfg(test)]
mod ready_emit_delta_tests {
    use super::ready_emit_delta;

    #[test]
    fn tail_and_reset_when_the_job_list_shrinks() {
        let names = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let (delta, next) = ready_emit_delta(&names, 0);
        assert_eq!(delta, names);
        assert_eq!(next, 3);
        let (delta, next) = ready_emit_delta(&names, 2);
        assert_eq!(delta, vec!["C".to_string()]);
        assert_eq!(next, 3);
        let restarted = vec!["A".to_string()];
        let (delta, next) = ready_emit_delta(&restarted, 3);
        assert_eq!(delta, restarted);
        assert_eq!(next, 1);
    }
}

pub fn emit_progress(app: &AppHandle) {
    emit_progress_throttled(app, false);
}

pub fn emit_progress_force(app: &AppHandle) {
    emit_progress_throttled(app, true);
}

pub fn emit_progress_throttle_ms() -> u64 {
    350
}

/// 1.0.206ae: docs VF purge must not flood WebView (~2k dirs).
/// Harder than Activate's 350ms so Cancel stays hittable (no Not Responding).
pub fn docs_vf_emit_progress_throttle_ms() -> u64 {
    900
}

/// Also emit at most every N dirs even if wall-clock fires sooner after a stall.
pub fn docs_vf_emit_every_n_dirs() -> u32 {
    48
}

pub fn emit_progress_throttled(app: &AppHandle, force: bool) {
    static LAST: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    let last = LAST.get_or_init(|| Mutex::new(None));
    let idle = bulk()
        .progress
        .lock()
        .map(|p| !p.running && !p.paused)
        .unwrap_or(false);
    let force = force || idle;
    if !force {
        if let Ok(mut g) = last.lock() {
            if let Some(t) = *g {
                if t.elapsed() < Duration::from_millis(emit_progress_throttle_ms()) {
                    return;
                }
            }
            *g = Some(Instant::now());
        }
    } else if let Ok(mut g) = last.lock() {
        *g = Some(Instant::now());
    }
    if let Ok(p) = bulk().progress.lock() {
        let mut light = p.clone();
        let active = p.running || p.paused;
        if active && !force {
            let (delta, next) = ready_emit_delta(&p.ready_names, bulk().ready_emitted.load(Ordering::SeqCst));
            light.ready_names = delta;
            bulk().ready_emitted.store(next, Ordering::SeqCst);
        } else {
            bulk().ready_emitted.store(p.ready_names.len(), Ordering::SeqCst);
        }
        let _ = app.emit("font-download", light);
    }
}

pub fn download_family(
    app: &AppHandle,
    client: &reqwest::blocking::Client,
    family: &str,
) -> Result<(usize, HealStats), String> {
    let key = family.trim().to_lowercase();
    if bulk().denied.lock().map(|d| d.contains(&key)).unwrap_or(false) {
        return Err("deactivated".into());
    }
    let slug = slug_family(family);
    if slug.is_empty() {
        return Err("empty family name".into());
    }
    let bust = bulk().bust.load(Ordering::SeqCst);
    // Clear Sans: replace Fontsource WOFF/odd ~67KB shreds with Intel TTFs.
    let clear_sans_needs_heal = is_clear_sans_family(family) && clear_sans_library_needs_heal(app, family);
    if clear_sans_needs_heal {
        clear_complete_markers_for_family(app, family);
        // Drop Fontsource shreds so Intel TTFs can replace them (skip-intact would keep Add=0 bodies).
        for dir in family_locations(app, family) {
            let mut files = Vec::new();
            walk_font_files(&dir, &mut files);
            for path in files {
                if clear_sans_face_needs_intel_heal(&path) {
                    let _ = delete_font_file(&path);
                }
            }
        }
    }
    // Always rewrite Clear Sans `.expected` to Intel 8 (even when shreds already healed).
    if is_clear_sans_family(family) {
        for dir in family_locations(app, family) {
            heal_clear_sans_expected_plan(&dir, family);
        }
    }
    let intent_early = effective_fetch_intent(family, intent_for_family(family));
    // c258 item 4: never pre-load a cut / css2 / pre-c258 folder into Windows —
    // fix it first; the final register below loads the fixed files.
    let fix_first = matches!(intent_early, FetchIntent::Google)
        && family_locations(app, family)
            .iter()
            .any(|d| family_dir_needs_fix_before_load(d, family));
    // c263: heal first, then one register — Add-then-heal-then-re-Add loaded faces twice.
    let mut existing = 0usize;
    if !fix_first && !bust && !clear_sans_needs_heal && family_is_ready(app, family) {
        // Complete folders: Google intent may still pull missing catalog VF + name heal.
        // Fontsource intent must not call Google CSS2 / desktop VF ensure.
        let mut heal = HealStats::default();
        if matches!(intent_early, FetchIntent::Google) {
            let (_, h) = ensure_catalog_variable_faces(app, client, family);
            heal.add(h);
            heal.add(heal_family_google_names(app, family, false));
        }
        existing = register_intact_family(app, family);
        if existing > 0 {
            return Ok((existing, heal));
        }
    }
    if bust {
        purge_family_files(app, family);
    }
    let intent = effective_fetch_intent(family, intent_for_family(family));
    if matches!(intent, FetchIntent::Local) {
        let total = register_intact_family(app, family);
        if total > 0 {
            return Ok((total, HealStats::default()));
        }
        return Err("local family has no installable faces".into());
    }
    let root = family_dir_for_intent(app, family, intent)?;
    fs::create_dir_all(&root).map_err(|e| format!("could not create folder: {e}"))?;
    let mut wrote = 0usize;
    let mut locked = false;
    let mut planned = 0usize;
    let mut version = String::new();
    // Stamp chosen source so register only Adds faces from this Activate card.
    write_download_source(&root, intent);

    // Hard separation: Google Activate = Google faces only (no Fontsource fill).
    // Fontsource Activate = Fontsource only (no Google CSS2 / desktop fetch).
    // Completeness P0: Noto Color Emoji = full upstream color TTF only — never
    // Google CSS unicode-range / latin stubs in the planned set.
    let (mut google_wrote, mut google_listed, mut google_var_files, mut heal) =
        if matches!(intent, FetchIntent::Google) && !is_noto_color_emoji_family(family, &slug) {
            fetch_google_family_faces_to_dir(client, family, &slug, &root)
        } else {
            (0, Vec::new(), Vec::new(), HealStats::default())
        };
    // 1.0.206: emoji P0 — prefer full color-capable upstream TTF (not CSS WOFF2 /
    // unicode-range shreds). Live only if later Add>0; allowlist Settled if Add=0.
    if matches!(intent, FetchIntent::Google) && is_emoji_session_family(family) {
        let emoji_n = pull_emoji_upstream_color_ttf(client, family, &slug, &root);
        google_wrote = google_wrote.saturating_add(emoji_n);
        if emoji_n > 0 {
            let key = if is_noto_color_emoji_family(family, &slug) {
                format!("{slug}.ttf")
            } else {
                format!("{slug}-400-normal.ttf")
            };
            // Color emoji: planned = upstream face only (drop any CSS instance keys).
            if is_noto_color_emoji_family(family, &slug) {
                google_listed.clear();
                google_var_files.clear();
                // Drop latin/CSS stubs that may pre-exist from older installs.
                let _ = purge_known_incapable_fontsource_remnants(&root, family);
            }
            // Track via var_files so planned keys match on-disk name (not CSS face matrix).
            if !google_var_files.iter().any(|k| k == &key) {
                google_var_files.push(key);
            }
        }
    }
    // If Google CSS listed faces but nothing intact landed, clear the plan — do NOT
    // Fontsource-fill on Google intent (hard separation). Repair can retry Google.
    if matches!(intent, FetchIntent::Google) && !google_listed.is_empty() && google_wrote == 0 {
        let keys_probe: Vec<String> = google_listed
            .iter()
            .map(|(style, weight, _)| google_face_filename(&slug, weight, style))
            .chain(google_var_files.iter().cloned())
            .collect();
        if count_intact_planned_keys(&root, &keys_probe) == 0 {
            google_listed.clear();
            google_var_files.clear();
        }
    }
    let google_instance_expected = google_listed.len();
    let google_expected = google_instance_expected.saturating_add(google_var_files.len());
    wrote = wrote.saturating_add(google_wrote);
    if matches!(intent, FetchIntent::Google) && google_expected > 0 {
        planned = google_expected;
        let instance_keys: Vec<String> = google_listed
            .iter()
            .map(|(style, weight, _)| google_face_filename(&slug, weight, style))
            .collect();
        // 1.0.206u Skye P1: vars first but keep statics in plan/on disk until
        // VarsOnly Add>0 (commit_vf_primary_after_successful_add). Do not purge here.
        let keys = merge_variable_into_planned_keys_keep_statics(&instance_keys, &google_var_files);
        write_google_planned(&root, &keys);
        clear_fontsource_planned(&root);
        // Only purge *unplanned* leftovers (latin shreds) — never drop planned statics
        // before register (would gut Add=0 fallback).
        if google_wrote > 0 || count_intact_planned_keys(&root, &keys) > 0 {
            purge_unplanned_font_files(&root, &keys);
        }
    } else if matches!(intent, FetchIntent::Google) {
        if count_intact_faces(&root) == 0 {
            clear_google_planned(&root);
        }
    } else {
        // Fontsource intent: never keep a stale Google plan that would confuse register.
        clear_google_planned(&root);
    }

    // Fontsource fill only on Fontsource intent — never as Google fallback.
    let need_fontsource = matches!(intent, FetchIntent::Fontsource);
    let mut fontsource_keys: Vec<String> = Vec::new();
    if need_fontsource && slug == "clear-sans" {
        // Intel-only planned set (8). Never Fontsource face-matrix (10).
        planned = clear_sans_intel_planned_count();
        version = CLEAR_SANS_INTEL_PIN.to_string();
        let keys = clear_sans_intel_face_keys();
        fontsource_keys = keys.clone();
        for (weight, italic) in CLEAR_SANS_INTEL_FACES {
            if bulk().cancel.load(Ordering::SeqCst) {
                break;
            }
            let style = if *italic { "italic" } else { "normal" };
            let name = google_face_filename(&slug, &weight.to_string(), style);
            let path = root.join(&name);
            if !bust && ttf_intact(&path) && !clear_sans_face_needs_intel_heal(&path) {
                register_path(&path);
                wrote = wrote.saturating_add(1);
                continue;
            }
            match fetch_ttf_to_file(client, &slug, &version, *weight, *italic, "latin", &path) {
                Ok(()) => wrote = wrote.saturating_add(1),
                Err(_) => {}
            }
        }
        // Drop any leftover ThinItalic / LightItalic / Fontsource shreds outside the plan.
        purge_unplanned_font_files(&root, &keys);
        write_expected_faces(&root, planned);
        write_fontsource_planned(&root, &fontsource_keys);
    } else if need_fontsource && !material_symbols_ttf_urls(family).is_empty() {
        // c253 #7: Fontsource ships WOFF2 only — pull the official VF TTF instead.
        let key = sanitize(&format!("{slug}-variable-fill-grad-opsz-wght.ttf"));
        let path = root.join(&key);
        planned = 1;
        fontsource_keys = vec![key.clone()];
        if !bust && ttf_intact(&path) {
            wrote = wrote.saturating_add(1);
        } else {
            for url in material_symbols_ttf_urls(family) {
                if !catalog_download_allowed() {
                    break;
                }
                let Some(bytes) = fetch_url_ttf(client, &url) else {
                    continue;
                };
                // Full VF is MBs; reject stubs / error pages.
                if bytes.len() < 1024 * 1024 {
                    continue;
                }
                if write_font_file(&path, &bytes).is_ok() && ttf_intact(&path) {
                    wrote = wrote.saturating_add(1);
                    break;
                }
            }
        }
        // Never drop existing copies unless the official TTF actually landed.
        if wrote > 0 {
            purge_unplanned_font_files(&root, &fontsource_keys);
        }
        write_fontsource_planned(&root, &fontsource_keys);
    } else if need_fontsource {
        if let Some((all_subsets, weights, meta_styles, fs_ver)) = fontsource_meta(client, &slug) {
            version = fs_ver.clone();
            let mut subsets = pick_subsets(&all_subsets);
            if subsets.is_empty() {
                subsets.push("latin".into());
            }
            let weights = pick_fontsource_weights(&weights);
            // Use advertised styles only — never invent normal for italic-only packages.
            let styles: Vec<bool> = if slug.contains("emoji") {
                vec![false]
            } else {
                meta_styles
            };
            let fs_expected = subsets.len().saturating_mul(weights.len()).saturating_mul(styles.len());
            planned = fs_expected;
            for subset in &subsets {
                for weight in &weights {
                    for italic in &styles {
                        let style = if *italic { "italic" } else { "normal" };
                        fontsource_keys.push(fontsource_face_filename(&slug, subset, *weight, style));
                    }
                }
            }
            fontsource_keys.sort();
            fontsource_keys.dedup();
            let fs_wrote = pull_fontsource_subset_to_dir(
                client, &slug, &version, &subsets, &weights, &styles, &root,
            );
            // CJK honesty: metadata listed chinese-* but FS only dropped latin → do not
            // claim a Fontsource expected set when nothing usable landed.
            if all_subsets.iter().any(|s| is_cjk_subset(s)) {
                let mut files = Vec::new();
                walk_font_files(&root, &mut files);
                let fs_names: Vec<_> = files
                    .iter()
                    .filter_map(|p| p.file_name().and_then(|s| s.to_str()).map(|s| s.to_string()))
                    .filter(|n| n.contains(&slug))
                    .collect();
                if !fs_names.is_empty()
                    && fs_names
                        .iter()
                        .all(|n| filename_has_latin_subset(n, &slug))
                {
                    planned = 0;
                }
            }
            wrote = wrote.saturating_add(fs_wrote);
            write_fontsource_planned(&root, &fontsource_keys);
        }
    }

    // Legacy Fontsource packs used `*-latin-*` names — strip when we have a
    // replacement set (Google wrote, or Fontsource-only path). Never strip when
    // Google listed then failed with zero writes — that left folders empty (1.0.148+).
    if google_wrote > 0 || google_expected == 0 {
        purge_latin_named_files(&root);
    }

    // Reconcile with on-disk intact after streaming paths.
    let intact_now = count_intact_faces(&root);
    wrote = intact_now.max(wrote);
    if wrote == 0 {
        // Surface lock suspicion when Documents had prior files we could not replace.
        locked = root
            .read_dir()
            .ok()
            .map(|rd| rd.flatten().any(|e| e.path().extension().is_some()))
            .unwrap_or(false)
            && !bust;
    }
    if wrote > 0 && !version.is_empty() {
        let _ = fs::write(root.join(".fontsource-version"), version.as_bytes());
    }
    if planned > 0 {
        write_expected_faces(&root, planned);
    }
    // Fontsource emoji path: ensure full upstream color TTF landed (ttf_urls special-case).
    if matches!(intent, FetchIntent::Fontsource) && is_emoji_session_family(family) {
        let emoji_n = pull_emoji_upstream_color_ttf(client, family, &slug, &root);
        wrote = wrote.saturating_add(emoji_n);
        if emoji_n > 0 && is_noto_color_emoji_family(family, &slug) {
            // Completeness: purge latin/FS stubs; planned = upstream color face only.
            let _ = purge_known_incapable_fontsource_remnants(&root, family);
            let key = format!("{slug}.ttf");
            fontsource_keys.clear();
            fontsource_keys.push(key);
            planned = 1;
            write_fontsource_planned(&root, &fontsource_keys);
            write_expected_faces(&root, planned);
        }
    }
    if bulk().cancel.load(Ordering::SeqCst) {
        clear_complete_marker(&root);
        return Err("cancelled".into());
    }
    if needs_compat_pack(&slug) && (wrote > 0 || existing > 0 || family_has_intact(app, family)) {
        install_compat_pack(client, &root, family, &slug);
    }
    if bulk().cancel.load(Ordering::SeqCst) {
        clear_complete_marker(&root);
        return Err("cancelled".into());
    }
    let (total, reg_cause) = if repair_files_only(family) {
        // Inactive family Repair: files replaced, stays unloaded.
        (count_intact_faces(&root), RegisterFailKind::NoneTried)
    } else {
        register_intact_family_detailed(app, family)
    };
    if bulk().cancel.load(Ordering::SeqCst) {
        clear_complete_marker(&root);
        return Err("cancelled".into());
    }
    if total == 0 {
        let intact_now = count_intact_faces(&root);
        let real_add_zero = matches!(reg_cause, RegisterFailKind::AddReturnedZero);
        if (real_add_zero || family_known_gdi_session_incapable(family))
            && family_disk_settled_known_gdi_incapable(
                family_may_settle_add_zero(family),
                intact_now > 0,
                dir_has_undersized_google_static(&root, family),
            )
        {
            // Disk settled only after genuine Add=0 (never on transient quota/stage/lock error).
            // Err so caller quiet-settles — never Activated / ready_names.
            stamp_settle_after_add_zero(app, family);
            return Err(format_register_zero_detail_with(app, family, reg_cause));
        }
        clear_complete_marker(&root);
        if locked {
            return Err("files locked — close Word or Adobe, then Retry".into());
        }
        if intact_now > 0 {
            // Skip-intact Fontsource path can hit this: files on disk, GDI 0.
            // Clear sticky .complete; split stage vs Add vs unloading (Skye P0).
            return Err(note_register_zero(app, family, reg_cause));
        }
        return Err("no installable TTF/OTF (Google CSS + Fontsource yielded none)".into());
    }
    // Gate .complete on Google face keys when Google planned the set — never let
    // Fontsource *-latin-* extras stamp complete over missing Google faces.
    let intact_for_complete = if slug == "clear-sans" {
        count_intact_planned_keys(&root, &clear_sans_intel_face_keys())
    } else if google_expected > 0 {
        let (p, intact) =
            google_complete_gate(&root, &slug, &google_listed, &google_var_files, google_expected);
        planned = p;
        intact
    } else {
        count_intact_faces(&root)
    };
    if planned == 0
        && total > 0
        && intact_for_complete > 0
        && !dir_only_latin_fontsource_names(&root)
        && !dir_has_undersized_google_static(&root, family)
    {
        if let Some(existing_keys) = read_google_planned_keys(&root) {
            let intact_planned = count_intact_planned_keys(&root, &existing_keys);
            if !existing_keys.is_empty() && intact_planned >= existing_keys.len() {
                planned = existing_keys.len();
            }
        } else if dir_vf_primary_ready(&root, family) {
            let mut files = Vec::new();
            walk_font_files(&root, &mut files);
            let var_files: Vec<String> = files
                .iter()
                .filter_map(|p| {
                    let name = p.file_name()?.to_str()?;
                    if is_variable_face_filename(name) && ttf_intact(p) {
                        Some(name.to_string())
                    } else {
                        None
                    }
                })
                .collect();
            if !var_files.is_empty() {
                write_google_planned(&root, &var_files);
                planned = intact_for_complete;
            }
        } else {
            let inst_keys = collect_intact_google_instance_keys(&root);
            let floor_ok = google_catalog_face_floor(family)
                .map(|fl| inst_keys.len() >= fl)
                .unwrap_or(true);
            if !inst_keys.is_empty() && floor_ok {
                write_google_planned(&root, &inst_keys);
                planned = intact_for_complete;
            }
        }
    }
    if planned > 0 && intact_for_complete >= planned {
        if slug == "clear-sans" {
            // Force Intel plan count even if a stale Fontsource planned leaked.
            planned = clear_sans_intel_planned_count();
        }
        mark_family_complete(&root, planned);
        // Intact heals during download already in `heal`; fold any remaining
        // mashed statics (vars counted via download_google / ensure paths).
        heal.add(heal_family_google_names(app, family, false));
        Ok((total, heal))
    } else {
        clear_complete_marker(&root);
        if slug == "clear-sans" {
            // Persist Intel expected so UI does not stick on 8/10 after a partial.
            write_expected_faces(&root, clear_sans_intel_planned_count());
        }
        if planned == 0 && total > 0 && intact_for_complete > 0 {
            heal.add(heal_family_google_names(app, family, false));
            Ok((total, heal))
        } else if planned == 0 {
            Err("could not enumerate full face set — Repair".into())
        } else {
            Err(format!(
                "incomplete face set ({intact_for_complete}/{planned}) — Repair"
            ))
        }
    }
}

/// `.complete` gate for a Google-planned set → (planned, intact).
pub fn google_complete_gate(
    root: &Path,
    slug: &str,
    google_listed: &[(String, String, String)],
    google_var_files: &[String],
    google_expected: usize,
) -> (usize, usize) {
    // c253 #3: the VF path never writes the CSS static instances, so a full
    // intact VF set (roman + italic when planned) *is* the complete face set.
    if !google_var_files.is_empty()
        && google_var_files.iter().all(|n| ttf_intact(&root.join(n)))
    {
        return (google_var_files.len(), google_var_files.len());
    }
    // Count instance faces + variable files from the planned key list.
    let intact = if let Some(keys) = read_google_planned_keys(root) {
        count_intact_planned_keys(root, &keys)
    } else {
        count_intact_google_listed_keys(root, slug, google_listed).saturating_add(
            google_var_files
                .iter()
                .filter(|n| ttf_intact(&root.join(n)))
                .count(),
        )
    };
    (google_expected, intact)
}

pub const DOWNLOAD_WORKERS: usize = FACE_STREAM_SLOTS; // was 3; waits unbounded → match stream slots

pub fn take_next_family(queue: &Mutex<VecDeque<String>>) -> Option<String> {
    let state = bulk();
    // If new waves arrived in `state.pending`, yield back to `run_google_bulk_inner`
    // so ready-on-disk families in the new waves go through fast 6-worker parallel
    // registration (`split_ready_missing` + `commit_ready_families_at`) first.
    if state
        .pending
        .lock()
        .map(|extra| !extra.is_empty())
        .unwrap_or(false)
    {
        return None;
    }
    queue.lock().ok().and_then(|mut q| q.pop_front())
}

pub fn drain_download_queue(
    app: AppHandle,
    client: Arc<reqwest::blocking::Client>,
    queue: Arc<Mutex<VecDeque<String>>>,
) -> HealStats {
    let state = bulk();
    // Coalesce name-heal across families — one toast with totals, not per-family storm.
    let mut heal_acc = HealStats::default();
    loop {
        if state.cancel.load(Ordering::SeqCst) {
            return heal_acc;
        }
        if state.pause.load(Ordering::SeqCst) {
            if let Ok(mut p) = state.progress.lock() {
                p.paused = true;
                p.running = true;
            }
            emit_progress(&app);
            thread::sleep(Duration::from_millis(200));
            continue;
        }
        if let Ok(mut p) = state.progress.lock() {
            p.paused = false;
        }
        let Some(family) = take_next_family(&queue) else {
            return heal_acc;
        };
        let denied = state
            .denied
            .lock()
            .map(|d| d.contains(&family.trim().to_lowercase()))
            .unwrap_or(false);
        if denied {
            forget_queued(&family);
            if let Ok(mut p) = state.progress.lock() {
                p.done += 1;
            }
            emit_progress(&app);
            continue;
        }
        {
            let mut p = state.progress.lock().unwrap();
            p.current = family.clone();
            p.running = true;
        }
        emit_progress(&app);
        let already = family_is_ready(&app, &family) && !state.bust.load(Ordering::SeqCst);
        let result = if already {
            // Already-complete: Google intent may pull missing catalog vars + heal names.
            // Fontsource intent must not hit Google CSS2 / desktop VF ensure.
            // Aggregate — do not emit per family (toast storm).
            let mut heal = HealStats::default();
            if matches!(intent_for_family(&family), FetchIntent::Google) {
                let (_, h) = ensure_catalog_variable_faces(&app, &client, &family);
                heal.add(h);
                heal.add(heal_family_google_names(&app, &family, false));
            }
            heal_acc.add(heal);
            let (n, cause) = if repair_files_only(&family) {
                (1, RegisterFailKind::NoneTried)
            } else {
                register_intact_family_detailed(&app, &family)
            };
            if n == 0 {
                let real_add_zero = matches!(cause, RegisterFailKind::AddReturnedZero);
                if (real_add_zero || family_known_gdi_session_incapable(&family))
                    && family_may_settle_add_zero(&family)
                    && family_has_intact(&app, &family)
                {
                    // Keep / stamp disk settled only after genuine Add=0 — never fake Live or settle on quota/lock error.
                    stamp_settle_after_add_zero(&app, &family);
                    Err(format_register_zero_detail_with(&app, &family, cause))
                } else {
                    Err(note_register_zero(&app, &family, cause))
                }
            } else {
                Ok(n)
            }
        } else {
            match download_family(&app, &client, &family) {
                Ok((n, heal)) => {
                    heal_acc.add(heal);
                    Ok(n)
                }
                Err(e) => Err(e),
            }
        };
        let cancelled = state.cancel.load(Ordering::SeqCst)
            || matches!(&result, Err(reason) if reason == "cancelled" || reason == "deactivated");
        if cancelled {
            forget_queued(&family);
            if state.cancel.load(Ordering::SeqCst) {
                return heal_acc;
            }
            continue;
        }
        match &result {
            Err(reason) => {
                forget_queued(&family);
                if suppress_fail_toast_known_incapable(&app, &family) {
                    // Intact official TTF + known GDI-incapable: disk settled + quiet toast.
                    stamp_known_incapable_disk_settled(&app, &family);
                    note_settled_quiet(&family);
                } else {
                    remember_failed(&family, reason);
                }
            }
            Ok(_) => {
                forget_queued(&family);
                if session_join_after_download(&family) {
                    session_add(&app, &[family.clone()]);
                    notify_fonts_changed_maybe();
                    if let Ok(mut p) = state.progress.lock() {
                        if !p.ready_names.iter().any(|n| n.eq_ignore_ascii_case(&family)) {
                            p.ready_names.push(family.clone());
                        }
                        if already {
                            p.skipped += 1;
                        }
                    }
                }
            }
        }
        clear_repair_files_only(&family);
        {
            let mut p = state.progress.lock().unwrap();
            p.done += 1;
            if result.is_err() && !suppress_fail_toast_known_incapable(&app, &family) {
                p.failed += 1;
            }
        }
        emit_progress(&app);
    }
}

pub fn run_google_bulk(app: AppHandle, families: Vec<String>) {
    run_google_bulk_inner(app, families);
    // c263: a cancelled / finished job never leaves a files-only Repair mark behind
    // (a stale mark made a later Activate of that family silently not load).
    clear_all_repair_files_only();
}

pub fn run_google_bulk_inner(app: AppHandle, families: Vec<String>) {
    let state = bulk();
    let mut batch = families;
    let mut missing_queue: VecDeque<String> = VecDeque::new();
    let mut client: Option<Arc<reqwest::blocking::Client>> = None;
    let mut did_download_work = false;

    loop {
        if state.cancel.load(Ordering::SeqCst) {
            break;
        }
        if let Ok(mut pending) = state.pending.lock() {
            batch.extend(pending.drain(..));
        }
        if batch.is_empty() && missing_queue.is_empty() {
            // Brief grace wait for multi-wave `activateInWaves` (`setTimeout(0)`) ticks.
            thread::sleep(Duration::from_millis(50));
            if let Ok(mut pending) = state.pending.lock() {
                batch.extend(pending.drain(..));
            }
            if batch.is_empty() && missing_queue.is_empty() {
                state.running.store(false, Ordering::SeqCst);
                if !state.cancel.load(Ordering::SeqCst) {
                    let late: Vec<String> = state
                        .pending
                        .lock()
                        .map(|mut p| p.drain(..).collect())
                        .unwrap_or_default();
                    if !late.is_empty() && !state.running.swap(true, Ordering::SeqCst) {
                        batch = late;
                        if let Ok(mut p) = state.progress.lock() {
                            p.running = true;
                        }
                        continue;
                    }
                }
                break;
            }
        }

        if !batch.is_empty() {
            let bust = state.bust.load(Ordering::SeqCst);
            let current_batch = std::mem::take(&mut batch);
            let (ready, missing, index) = split_ready_missing(&app, current_batch, bust);
            missing_queue.extend(missing);

            if !ready.is_empty() || !missing_queue.is_empty() {
                let done_base = if let Ok(mut p) = state.progress.lock() {
                    let base = p.done as usize;
                    let pending_len = state.pending.lock().map(|q| q.len()).unwrap_or(0);
                    let want_total =
                        (base + ready.len() + missing_queue.len() + pending_len) as u32;
                    if p.total < want_total {
                        p.total = want_total;
                    }
                    p.kind = "download".into();
                    p.current = if ready.is_empty() {
                        missing_queue
                            .front()
                            .cloned()
                            .unwrap_or_else(|| "Downloading…".into())
                    } else if missing_queue.is_empty() && pending_len == 0 {
                        format!("Registering {} already on disk…", ready.len())
                    } else {
                        format!("Registering {} intact files…", ready.len())
                    };
                    p.running = true;
                    base
                } else {
                    0
                };
                emit_progress(&app);
                if !ready.is_empty() {
                    commit_ready_families_at(&app, &ready, index.as_ref(), done_base);
                    emit_progress(&app);
                }
            }

            // If more waves arrived in `state.pending` while splitting/registering, process
            // their ready-on-disk families first before launching network download workers.
            if state
                .pending
                .lock()
                .map(|p| !p.is_empty())
                .unwrap_or(false)
            {
                continue;
            }
        }

        if missing_queue.is_empty() {
            continue;
        }

        let dl_client = match &client {
            Some(c) => c.clone(),
            None => match http_download_client() {
                Some(c) => {
                    let arc = Arc::new(c);
                    client = Some(arc.clone());
                    arc
                }
                None => {
                    for fam in missing_queue.drain(..) {
                        forget_queued(&fam);
                    }
                    break;
                }
            },
        };

        did_download_work = true;
        let queue = Arc::new(Mutex::new(std::mem::take(&mut missing_queue)));
        if let Ok(mut aq) = state.active_queue.lock() {
            *aq = Some(queue.clone());
        }
        let mut joins = Vec::with_capacity(DOWNLOAD_WORKERS);
        for _ in 0..DOWNLOAD_WORKERS {
            let app = app.clone();
            let dl_client = dl_client.clone();
            let queue = queue.clone();
            joins.push(thread::spawn(move || drain_download_queue(app, dl_client, queue)));
        }
        let mut drain_heal = HealStats::default();
        for j in joins {
            if let Ok(h) = j.join() {
                drain_heal.add(h);
            }
        }
        if let Ok(mut aq) = state.active_queue.lock() {
            *aq = None;
        }
        // One name-heal emit per drain wave (totals), not per family.
        emit_name_heal(&app, drain_heal);
        if !state.cancel.load(Ordering::SeqCst) {
            if let Ok(mut remaining) = queue.lock() {
                missing_queue.extend(remaining.drain(..));
            }
        }
    }

    if state.cancel.load(Ordering::SeqCst) {
        for fam in batch.drain(..).chain(missing_queue.drain(..)) {
            forget_queued(&fam);
        }
        if let Ok(mut pending) = state.pending.lock() {
            for fam in pending.drain(..) {
                forget_queued(&fam);
            }
        }
    }

    if did_download_work {
        notify_fonts_changed();
        #[cfg(windows)]
        persist_activation_sidecars(&app);
    }
    if let Ok(mut p) = state.progress.lock() {
        p.running = false;
        p.current.clear();
        if p.done == 0 && p.failed == 0 && p.ready_names.is_empty() {
            p.total = 0;
        }
    }
    state.running.store(false, Ordering::SeqCst);
    state.bust.store(false, Ordering::SeqCst);
    emit_progress(&app);
}

#[derive(Clone, Serialize)]
pub struct ActivationPlan {
    pub ready: Vec<String>,
    pub missing: Vec<String>,
    pub corrupt: u32,
    pub scanned: u32,
    pub on_disk: Vec<String>,
}

/// Fast folder walk. Does not download. Intact files stay put. Corrupt files are dropped.
#[tauri::command]
pub async fn plan_google_activation(app: AppHandle, families: Vec<String>) -> Result<ActivationPlan, String> {
    tauri::async_runtime::spawn_blocking(move || plan_google_activation_blocking(app, families))
        .await
        .map_err(|e| e.to_string())?
}

pub fn plan_google_activation_blocking(app: AppHandle, families: Vec<String>) -> Result<ActivationPlan, String> {
    let (ready, missing, _) = split_ready_missing(&app, families, false);
    Ok(ActivationPlan {
        scanned: (ready.len() + missing.len()) as u32,
        ready: ready.clone(),
        missing,
        corrupt: 0,
        on_disk: ready,
    })
}

#[tauri::command]
pub async fn read_family_font(app: AppHandle, family: String, italic: Option<bool>) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || read_family_font_blocking(app, family, italic))
        .await
        .map_err(|e| e.to_string())?
}

pub fn read_family_font_blocking(app: AppHandle, family: String, italic: Option<bool>) -> Result<String, String> {
    let want_italic = italic.unwrap_or(false);
    // Prefer variable faces when present (axes), then static instances as backup.
    let mut var_hit: Option<PathBuf> = None;
    let mut static_hit: Option<PathBuf> = None;
    let mut roman = None;
    for dir in family_locations(&app, &family) {
        let mut files = Vec::new();
        walk_font_files(&dir, &mut files);
        sort_faces_var_first(&mut files);
        for path in files {
            if !ttf_intact(&path) {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let is_var = is_variable_face_filename(&name);
            let is_italic = name.contains("italic") || name.contains("oblique");
            let style_ok = if want_italic { is_italic } else { !is_italic };
            if style_ok {
                if is_var && var_hit.is_none() {
                    var_hit = Some(path.clone());
                } else if !is_var && static_hit.is_none() {
                    static_hit = Some(path.clone());
                }
            }
            if roman.is_none() {
                roman = Some(path);
            }
        }
    }
    var_hit
        .or(static_hit)
        .or(roman)
        .map(|p| p.to_string_lossy().into_owned())
        .ok_or_else(|| "no font file on disk".into())
}

#[tauri::command]
pub async fn list_family_font_files(app: AppHandle, family: String) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || list_family_font_files_blocking(app, family))
        .await
        .map_err(|e| e.to_string())?
}

pub fn list_family_font_files_blocking(app: AppHandle, family: String) -> Result<Vec<String>, String> {
    let mut allowed = Vec::new();
    let mut fallback = Vec::new();
    let mut seen = HashSet::new();
    for dir in family_locations(&app, &family) {
        let mut files = Vec::new();
        walk_font_files(&dir, &mut files);
        sort_faces_var_first(&mut files);
        for path in files {
            if !ttf_intact(&path) {
                continue;
            }
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if is_compat_sidecar_filename(name) {
                continue;
            }
            let key = name.to_ascii_lowercase();
            if !seen.insert(key) {
                continue;
            }
            let s = path.to_string_lossy().into_owned();
            if face_allowed_for_register(&dir, &path, &family) {
                allowed.push(s.clone());
            }
            fallback.push(s);
        }
    }
    if !allowed.is_empty() {
        Ok(allowed)
    } else {
        Ok(fallback)
    }
}

#[tauri::command]
pub async fn retry_google_downloads(app: AppHandle, families: Vec<String>) -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(move || retry_google_downloads_blocking(app, families))
        .await
        .map_err(|e| e.to_string())?
}

pub fn retry_google_downloads_blocking(app: AppHandle, families: Vec<String>) -> Result<usize, String> {
    let mut locked = Vec::new();
    let mut need_fetch = Vec::new();
    let mut live: Vec<String> = Vec::new();
    let mut reregistered = 0usize;
    // c263: read the session once, not once per family (160-family Repair).
    let active_before = load_session_families(&app);
    for family in &families {
        forget_queued(family);
        if family_has_intact(&app, family) {
            // c260 P1: cut / css2 / pre-c258 Google folders go through the Activate
            // download path (manifest → repo → css2, reject checks) — never reregister
            // the old files.
            if matches!(effective_fetch_intent(family, intent_for_family(family)), FetchIntent::Google)
                && repair_must_download(&family_locations(&app, family), family)
            {
                // c260 follow-up: Repair only replaces files; inactive stays inactive.
                if !active_before.iter().any(|a| a.eq_ignore_ascii_case(family)) {
                    mark_repair_files_only(family, &family_locations(&app, family));
                }
                need_fetch.push(family.clone());
                continue;
            }
            // Clear Sans: heal odd Fontsource shreds from Intel before re-Add.
            if is_clear_sans_family(family) && clear_sans_library_needs_heal(&app, family) {
                need_fetch.push(family.clone());
                continue;
            }
            // Settled / already-refused known-incapable: quiet — no Add churn loop.
            if family_early_skip_known_incapable(&app, family) {
                stamp_known_incapable_disk_settled(&app, family);
                note_settled_quiet(family);
                continue;
            }
            // First try this session: sanitize+Add once (no auto 2015/Fontsource chase).
            let (n, cause) = reregister_intact_family(&app, family);
            if n > 0 {
                reregistered += 1;
                live.push(family.clone());
                if let Ok(mut p) = bulk().progress.lock() {
                    p.failed_names.retain(|n| !n.eq_ignore_ascii_case(family));
                    p.failed_details
                        .retain(|d| !d.to_ascii_lowercase().starts_with(&family.to_ascii_lowercase()));
                    p.settled_names
                        .retain(|n| !n.eq_ignore_ascii_case(family));
                    if !p.ready_names.iter().any(|n| n.eq_ignore_ascii_case(family)) {
                        p.ready_names.push(family.clone());
                    }
                    p.failed = p.failed_names.len() as u32;
                }
                continue;
            }
            let honesty = family_disk_honesty(&app, family);
            if retry_gidugu_settle_without_refetch(
                n,
                family_may_settle_add_zero(family),
                honesty.intact > 0,
                honesty.undersized,
            ) {
                note_session_gdi_refused(family);
                stamp_settle_after_add_zero(&app, family);
                note_settled_quiet(family);
                continue;
            }
            // Still GDI 0 — clear sticky .complete; honest cause; no wipe.
            let detail = note_register_zero(&app, family, cause);
            if let Ok(mut p) = bulk().progress.lock() {
                if !p.failed_names.iter().any(|n| n.eq_ignore_ascii_case(family)) {
                    p.failed_names.push(family.clone());
                    p.failed_details.push(detail);
                }
                p.failed = p.failed_names.len() as u32;
            }
            // Missing faces / catalog VF — non-bust fetch (ensure), not skip-as-done.
            if family_is_incomplete(&app, family)
                || family_needs_variable_backfill(&app, family)
            {
                need_fetch.push(family.clone());
            }
            continue;
        }
        // Empty / missing folder — purge is a no-op; queue fetch.
        if let Err(err) = purge_family_files_result(&app, family) {
            if err.contains("locked") {
                locked.push(family.clone());
                continue;
            }
        }
        need_fetch.push(family.clone());
    }
    if !live.is_empty() {
        session_add(&app, &live);
        notify_fonts_changed();
        #[cfg(windows)]
        persist_activation_sidecars(&app);
        emit_progress(&app);
    }
    if !locked.is_empty() && need_fetch.is_empty() && reregistered == 0 {
        return Err(format!(
            "files locked — close Word or Adobe, then Retry ({})",
            locked.join(", ")
        ));
    }
    if need_fetch.is_empty() {
        emit_progress(&app);
        return Ok(reregistered);
    }
    // Do not bust/wipe intact faces — skip-intact download fills missing/corrupt only.
    bulk().bust.store(false, Ordering::SeqCst);
    if !bulk().running.load(Ordering::SeqCst) {
        reset_circuits();
    }
    let added = start_google_downloads_blocking(app, need_fetch, None)?;
    Ok(added.saturating_add(reregistered))
}

/// Repair result: queued downloads + name-heal outcomes (healed vs locked skips).
#[derive(Debug, Clone, Serialize)]
pub struct RepairResult {
    pub queued: usize,
    pub healed: usize,
    pub locked: usize,
    pub write_failed: usize,
    pub var_ensured: usize,
}

/// Re-fetch families that have partial faces (no `.complete`).
/// Complete catalog-variable folders missing `*-variable-*` get vars added in place
/// (no full bust); incomplete families still go through Retry/bust.
/// Replace tiny latin-subset statics in-place for known CJK families without
/// wiping the folder or redownloading the whole library. Returns faces replaced.
/// c255: Repair candidate gate — upstream clearly larger (full vs latin/subset
/// remnant) or (c253 #5) carrying more glyphs than the local cut-down face.
pub fn repair_bytes_acceptable(bytes: &[u8], local: &Path) -> bool {
    let local_len = fs::metadata(local).map(|m| m.len()).unwrap_or(0);
    let more_glyphs = match (sfnt_num_glyphs_bytes(bytes), sfnt_num_glyphs_path(local)) {
        (Some(up), Some(l)) => up > l,
        _ => false,
    };
    more_glyphs
        || !((bytes.len() as u64) <= UNDERSIZED_GOOGLE_FACE_MAX_BYTES
            || (local_len > 0 && (bytes.len() as u64) < local_len.saturating_mul(2)))
}

/// First acceptable Repair candidate (Google CSS, then google/fonts repo).
pub fn first_acceptable_repair(cands: &[Vec<u8>], local: &Path) -> Option<usize> {
    cands.iter().position(|b| repair_bytes_acceptable(b, local))
}

/// google/fonts METADATA.pb static faces → (style, weight, filename). VFs skipped.
pub fn parse_metadata_pb_static_files(text: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let (mut in_font, mut style, mut weight, mut file) = (false, None, None, None);
    let val = |l: &str, k: &str| -> Option<String> {
        l.strip_prefix(k).map(|v| v.trim().trim_matches('"').to_string())
    };
    for line in text.lines() {
        let l = line.trim();
        if l == "fonts {" {
            in_font = true;
            (style, weight, file) = (None, None, None);
        } else if in_font && l == "}" {
            in_font = false;
            if let (Some(st), Some(w), Some(f)) = (style.take(), weight.take(), file.take()) {
                let f: String = f;
                if f.to_ascii_lowercase().ends_with(".ttf") && !is_variable_face_filename(&f) {
                    out.push((st, w, f));
                }
            }
        } else if in_font {
            if let Some(v) = val(l, "style:") {
                style = Some(v);
            } else if let Some(v) = val(l, "weight:") {
                weight = Some(v);
            } else if let Some(v) = val(l, "filename:") {
                file = Some(v);
            }
        }
    }
    out
}

/// c255 P1: google/fonts repo static faces for `family` → (license, folder, statics).
/// Google CSS can itself serve the latin cut; the repo has the full face.
pub fn google_fonts_repo_statics(
    client: &reqwest::blocking::Client,
    family: &str,
) -> Option<(String, String, Vec<(String, String, String)>)> {
    google_fonts_repo_statics_pref(client, family, None)
}

/// Repo statics with the licence folder tried first when known (from the
/// manifest's OFL.txt / LICENSE.txt / UFL.txt — the shipped catalog has none).
pub fn google_fonts_repo_statics_pref(
    client: &reqwest::blocking::Client,
    family: &str,
    pref: Option<&str>,
) -> Option<(String, String, Vec<(String, String, String)>)> {
    let mut lics: Vec<&str> = pref.into_iter().collect();
    for l in ["ofl", "apache", "ufl"] {
        if !lics.contains(&l) {
            lics.push(l);
        }
    }
    for lic in lics {
        for folder in google_fonts_repo_folders(family) {
            for meta_url in google_fonts_metadata_cdn_urls(lic, &folder) {
                if !catalog_download_allowed() {
                    return None;
                }
                let opts = gfsource::PoliteOpts {
                    ua: None,
                    timeout: Some(CATALOG_PROBE_TIMEOUT),
                    cdn_lane: false,
                };
                let Some(bytes) = gfsource::polite_fetch(client, &meta_url, &opts, &cancel_requested).ok() else {
                    continue;
                };
                let Ok(text) = String::from_utf8(bytes) else { continue };
                let files = parse_metadata_pb_static_files(&text);
                if !files.is_empty() {
                    return Some((lic.to_string(), folder.clone(), files));
                }
            }
        }
    }
    None
}

pub fn replace_tiny_cjk_static_faces(
    app: &AppHandle,
    client: &reqwest::blocking::Client,
    family: &str,
) -> usize {
    // Gidugu / CJK latin shreds, or (c253 #5) any official Google glyph-count outlier;
    // per-face gate is face_should_replace_as_tiny_cjk.
    if !is_official_google_family(family) {
        return 0;
    }
    let slug = slug_family(family);
    if slug.is_empty() {
        return 0;
    }
    let Ok(root) = family_dir(app, family) else {
        return 0;
    };
    if !root.is_dir() {
        return 0;
    }
    let mut files = Vec::new();
    walk_font_files(&root, &mut files);
    let mut tiny_keys: Vec<(String, String, PathBuf)> = Vec::new(); // weight, style, path
    for p in &files {
        let Some(name) = p.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if is_variable_face_filename(name) || filename_has_latin_subset(name, &slug) {
            continue;
        }
        if !face_should_replace_as_tiny_cjk(family, p) {
            continue;
        }
        if let Some((weight, style)) = parse_google_instance_face_name(&slug, name) {
            tiny_keys.push((weight, style, p.clone()));
        }
    }
    if tiny_keys.is_empty() {
        return 0;
    }
    // c258: manifest → repo → css2; a css2 pick is never a repair (stays `.web-build`).
    let (listed, listed_src, _) = discover_richest_google_listing(client, family);
    if listed.is_empty() {
        return 0;
    }
    let mut replaced = 0usize;
    let mut repo: Option<Option<(String, String, Vec<(String, String, String)>)>> = None;
    for (weight, style, path) in tiny_keys {
        if bulk().cancel.load(Ordering::SeqCst) {
            break;
        }
        let want = (style.to_ascii_lowercase(), weight.clone());
        let url = listed.iter().find_map(|(st, wt, u)| {
            if st.to_ascii_lowercase() == want.0 && wt == &want.1 {
                Some(u.clone())
            } else {
                None
            }
        });
        let Some(url) = url else {
            continue;
        };
        let mut cands: Vec<Vec<u8>> = Vec::new();
        if let Some(b) = fetch_url_ttf(client, &url) {
            cands.push(b);
        }
        let local_len = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if first_acceptable_repair(&cands, &path).is_none() {
            // c255 P1: CSS served the same cut — source the full face from google/fonts.
            if repo.is_none() {
                repo = Some(google_fonts_repo_statics(client, family));
            }
            if let Some(Some((lic, folder, statics))) = &repo {
                let hit = statics.iter().find(|(st, wt, _)| {
                    st.eq_ignore_ascii_case(&want.0) && wt == &want.1
                });
                if let Some((_, _, fname)) = hit {
                    for u in google_fonts_variable_cdn_urls(lic, folder, fname) {
                        if let Some(b) = fetch_url_ttf(client, &u) {
                            cands.push(b);
                            break;
                        }
                    }
                }
            }
        }
        let Some(pick) = first_acceptable_repair(&cands, &path) else {
            continue;
        };
        let pick_url = if pick == 0 { Some(url.clone()) } else { None };
        let pick_is_css2 = pick == 0 && !listed_src.is_full();
        let bytes = cands.swap_remove(pick);
        eprintln!(
            "{family} — undersized vs Google (latin/subset remnant): replacing {local_len}B face with {}B",
            bytes.len()
        );
        let patched = crate::namepatch::patch_google_instance_face(
            &bytes, family, &weight, &style,
        )
        .unwrap_or(bytes);
        intact_forget(&path);
        if write_font_file(&path, &patched).is_ok() {
            replaced = replaced.saturating_add(1);
            if let (Some(u), Some(name)) = (&pick_url, path.file_name().and_then(|n| n.to_str())) {
                gfsource::record_source_url(&root, name, u);
            }
            if pick_is_css2 {
                gfsource::set_web_build(&root, true);
            }
            let _ = register_family_path(family, &path);
        }
    }
    replaced
}

// Name-heal soft-fails on locked faces (Illustrator/fontdrvhost) but returns
// `locked` so the UI can fail loud — never looks like silent success.

/// Allowlisted Settled affordance (1.0.188: no Fontsource download; purge remnants).
/// Activated only if Add>0 — never from settle alone. Offer no longer fetches FS TTFs.
#[derive(Clone, Serialize)]
pub struct FontsourceOfferResult {
    pub family: String,
    pub added: usize,
    pub settled: bool,
    pub message: String,
}

/// Fontsource static TTF URLs for an allowlisted known-GDI-incapable family (slug + subset plan).
#[cfg_attr(not(test), allow(dead_code))]
pub fn fontsource_gdi_offer_ttf_urls(family: &str) -> Vec<String> {
    let slug = fontsource_gdi_offer_slug(family);
    let mut urls = Vec::new();
    for subset in fontsource_gdi_offer_subsets(family) {
        urls.extend(ttf_urls(&slug, "latest", 400, false, subset, 0));
    }
    urls
}

#[cfg(test)]
pub fn gidugu_fontsource_ttf_urls() -> Vec<String> {
    fontsource_gdi_offer_ttf_urls("Gidugu")
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn fontsource_offer_activated_only_if_add(added: usize) -> bool {
    added > 0
}

/// Soft Settled Power Retry: clear this-session Add=0 refuse so register tries Add again (1.0.206f).
#[tauri::command]
pub fn clear_session_gdi_refused_family(family: String) -> Result<(), String> {
    let family = family.trim();
    if family.is_empty() {
        return Err("family required".into());
    }
    clear_session_gdi_refused(family);
    Ok(())
}

#[tauri::command]
pub async fn try_fontsource_gdi_offer(app: AppHandle, family: String) -> Result<FontsourceOfferResult, String> {
    tauri::async_runtime::spawn_blocking(move || try_fontsource_gdi_offer_blocking(app, family))
        .await
        .map_err(|e| e.to_string())?
}

pub fn try_fontsource_gdi_offer_blocking(app: AppHandle, family: String) -> Result<FontsourceOfferResult, String> {
    let family = family.trim().to_string();
    if family.is_empty() {
        return Err("family required".into());
    }
    if !family_known_gdi_session_incapable(&family) {
        return Err(format!("{family} is not on the known GDI-incapable list"));
    }
    // 1.0.188: do not download Fontsource TTFs for allowlisted families (already
    // know Add=0 on Eric’s class of machines). Purge any prior FS remnant; Settled only.
    for dir in family_locations(&app, &family) {
        let _ = purge_known_incapable_fontsource_remnants(&dir, &family);
    }
    note_session_gdi_refused(&family);
    stamp_known_incapable_disk_settled(&app, &family);
    note_settled_quiet(&family);
    emit_progress(&app);
    Ok(FontsourceOfferResult {
        family,
        added: 0,
        settled: true,
        message: "On disk · Settled (not Activated). Fontsource download skipped for known GDI-incapable fonts.".into(),
    })
}

#[tauri::command]
pub async fn repair_incomplete_families(
    app: AppHandle,
    families: Vec<String>,
) -> Result<RepairResult, String> {
    tauri::async_runtime::spawn_blocking(move || repair_incomplete_families_blocking(app, families))
        .await
        .map_err(|e| e.to_string())?
}

pub fn repair_incomplete_families_blocking(
    app: AppHandle,
    families: Vec<String>,
) -> Result<RepairResult, String> {
    let mut targets = Vec::new();
    let mut heal = HealStats::default();
    let mut var_ensured = 0usize;
    let client = http_download_client();
    if families.is_empty() {
        for_family_dirs(&app, |dir| {
            if let Some(name) = dir.file_name().and_then(|s| s.to_str()) {
                heal_clear_sans_expected_plan(dir, name);
                if family_known_gdi_session_incapable(name) {
                    let _ = purge_known_incapable_fontsource_remnants(dir, name);
                }
            }
            verify_complete_marker(dir);
            if let Some(name) = dir.file_name().and_then(|s| s.to_str()) {
                stamp_known_incapable_dir_settled(dir, name);
            }
            if dir_has_intact(dir) && !dir_is_complete(dir) {
                if let Some(name) = dir.file_name().and_then(|s| s.to_str()) {
                    // Soft full-face without provenance: exclude Repair (try-Add first).
                    if soft_full_face_exclude_repair(dir, name) {
                        // skip
                    } else {
                        targets.push(name.to_string());
                    }
                }
            } else if dir_is_complete(dir) {
                // Complete Google folders: name-heal + pull missing catalog vars (no bust).
                if let Some(name) = dir.file_name().and_then(|s| s.to_str()) {
                    if is_official_google_family(name) || family_ensures_google_vf(name) {
                        if let Some(ref c) = client {
                            let _ = replace_tiny_cjk_static_faces(&app, c, name);
                            let (ve, eh) = ensure_catalog_variable_faces(&app, c, name);
                            var_ensured = var_ensured.saturating_add(ve);
                            heal.add(eh);
                            // Vars counted via ensure — heal static instances only.
                            heal.add(heal_google_names_in_dir(dir, name, false));
                        } else {
                            heal.add(heal_google_instance_names_in_dir(dir, name));
                        }
                    }
                }
            }
        });
    } else {
        for family in families {
            if family_known_gdi_session_incapable(&family) {
                for dir in family_locations(&app, &family) {
                    let _ = purge_known_incapable_fontsource_remnants(&dir, &family);
                }
                stamp_known_incapable_disk_settled(&app, &family);
            }
            if family_soft_full_face_exclude_repair(&app, &family) {
                // Soft full-face without provenance: try-Add first, never Repair re-download.
            } else if family_is_incomplete(&app, &family) || !family_is_ready(&app, &family) {
                targets.push(family);
            } else {
                if let Some(ref c) = client {
                    let _ = replace_tiny_cjk_static_faces(&app, c, &family);
                    let (ve, eh) = ensure_catalog_variable_faces(&app, c, &family);
                    var_ensured = var_ensured.saturating_add(ve);
                    heal.add(eh);
                    heal.add(heal_family_google_names(&app, &family, false));
                } else {
                    heal.add(heal_family_google_instance_names(&app, &family));
                }
            }
        }
    }
    // Repair returns HealStats in RepairResult for the frontend toast (no
    // name-heal event — would double-fire with the invoke result handler).
    if targets.is_empty() {
        return Ok(RepairResult {
            queued: 0,
            healed: heal.healed,
            locked: heal.locked,
            write_failed: heal.write_failed,
            var_ensured,
        });
    }
    let n = retry_google_downloads_blocking(app, targets)?;
    Ok(RepairResult {
        queued: n,
        healed: heal.healed,
        locked: heal.locked,
        write_failed: heal.write_failed,
        var_ensured,
    })
}

#[tauri::command]
pub fn skip_google_failures(families: Vec<String>) -> Result<usize, String> {
    let n = families.len();
    for family in &families {
        forget_queued(family);
        if let Ok(mut denied) = bulk().denied.lock() {
            denied.insert(family.trim().to_lowercase());
        }
    }
    if let Ok(mut p) = bulk().progress.lock() {
        p.failed_names.clear();
        p.failed_details.clear();
        p.settled_names.clear();
        p.failed = 0;
        if !p.running {
            p.current.clear();
        }
    }
    Ok(n)
}

/// Resolve Activate fetch intent for resume when the JS store row is missing.
/// Returns "google" | "fontsource" | "local" | null (skip — never guess google).
#[tauri::command]
pub fn resolve_family_fetch_intent(app: AppHandle, family: String) -> Option<String> {
    resolve_fetch_intent_from_disk(&app, &family).map(fetch_intent_label).map(|s| s.to_string())
}

#[tauri::command]
pub async fn start_google_downloads(
    app: AppHandle,
    families: Vec<String>,
    // Parallel to `families`: "google" | "fontsource" | "local". Missing → disk/catalog resolve.
    intents: Option<Vec<String>>,
) -> Result<usize, String> {
    // c263: an explicit Activate wins over a pending files-only Repair mark.
    for f in &families {
        clear_repair_files_only(f);
    }
    tauri::async_runtime::spawn_blocking(move || start_google_downloads_blocking(app, families, intents))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
pub fn classify_download_intents<F>(
    families: &[String],
    intents: &[String],
    resolve_from_disk: F,
) -> (Vec<(String, FetchIntent)>, Vec<String>)
where
    F: Fn(&str) -> Option<FetchIntent>,
{
    let mut resolved = Vec::with_capacity(families.len());
    let mut unresolved = Vec::new();
    for (i, family) in families.iter().enumerate() {
        let t = family.trim();
        if t.is_empty() {
            continue;
        }
        let explicit = intents.get(i).and_then(|s| parse_fetch_intent(s));
        match explicit.or_else(|| resolve_from_disk(t)) {
            Some(intent) => resolved.push((t.to_string(), intent)),
            None => unresolved.push(t.to_string()),
        }
    }
    (resolved, unresolved)
}

pub fn start_google_downloads_blocking(
    app: AppHandle,
    families: Vec<String>,
    // Parallel to `families`: "google" | "fontsource" | "local". Missing → disk/catalog resolve.
    intents: Option<Vec<String>>,
) -> Result<usize, String> {
    if families.is_empty() {
        return Ok(0);
    }
    let intent_list = intents.unwrap_or_default();
    // 1.0.206h: explicit intent OR disk/catalog resolve — never infer_fetch_intent after resolve None.
    let mut resolved: Vec<String> = Vec::with_capacity(families.len());
    let mut unresolved: Vec<String> = Vec::new();
    for (i, family) in families.iter().enumerate() {
        let explicit = intent_list.get(i).and_then(|s| parse_fetch_intent(s));
        let intent = match explicit {
            Some(i) => i,
            None => match {
                let r = resolve_fetch_intent_from_disk(&app, family);
                if r.is_none() && !family.trim().is_empty() {
                    unresolved.push(family.trim().to_string());
                }
                r
            } {
                Some(i) => i,
                None => continue, // skip ambiguous — never blind-infer Google/Fontsource
            },
        };
        remember_fetch_intent(family, intent);
        resolved.push(family.clone());
    }
    let fresh = accept_new_families(resolved);
    if fresh.is_empty() && unresolved.is_empty() {
        return Ok(0);
    }
    let added = fresh.len();
    let state = bulk();
    // If a previous cancelled bulk run is still unwinding, wait briefly so a brand-new
    // user Activate does not append into a dying cancelled worker (which drains & discards pending).
    if state.running.load(Ordering::SeqCst) && state.cancel.load(Ordering::SeqCst) {
        let deadline = std::time::Instant::now() + Duration::from_millis(500);
        while state.running.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(15));
        }
    }
    let already_running = if !fresh.is_empty() {
        state.running.swap(true, Ordering::SeqCst)
    } else {
        state.running.load(Ordering::SeqCst)
    };
    {
        let mut p = state.progress.lock().map_err(|e| e.to_string())?;
        p.running = !fresh.is_empty() || already_running;
        p.kind = "download".into();
        if !already_running {
            p.current = if fresh.is_empty() {
                String::new()
            } else {
                "Scanning Documents…".into()
            };
            p.done = 0;
            p.failed = 0;
            p.skipped = 0;
            p.total = added as u32;
            p.failed_names.clear();
            p.failed_details.clear();
            p.settled_names.clear();
            p.paused = false;
            p.ready_names.clear();
            reset_circuits();
        } else {
            if !fresh.is_empty() && p.current.is_empty() {
                p.current = "Scanning Documents…".into();
            }
            p.total += added as u32;
        }
        for u in &unresolved {
            p.failed = p.failed.saturating_add(1);
            p.done = p.done.saturating_add(1);
            p.total = p.total.saturating_add(1);
            if !p.failed_names.iter().any(|n| n.eq_ignore_ascii_case(u)) {
                p.failed_names.push(u.clone());
                p.failed_details.push(format!(
                    "{u} — Not found (no file on disk and no catalog source)"
                ));
            }
        }
    }
    if fresh.is_empty() {
        emit_progress(&app);
        return Ok(0);
    }
    if already_running {
        if let Ok(mut pending) = state.pending.lock() {
            pending.extend(fresh);
        }
        emit_progress(&app);
        return Ok(added);
    }
    state.cancel.store(false, Ordering::SeqCst);
    state.pause.store(false, Ordering::SeqCst);
    emit_progress(&app);
    thread::spawn(move || run_google_bulk(app, fresh));
    Ok(added)
}

#[tauri::command]
pub fn pause_google_downloads(app: AppHandle) -> Result<(), String> {
    let state = bulk();
    state.pause.store(true, Ordering::SeqCst);
    if let Ok(mut p) = state.progress.lock() {
        p.paused = true;
        p.running = true;
    }
    emit_progress(&app);
    Ok(())
}

#[tauri::command]
pub fn resume_google_downloads(app: AppHandle) -> Result<(), String> {
    let state = bulk();
    state.pause.store(false, Ordering::SeqCst);
    if let Ok(mut p) = state.progress.lock() {
        p.paused = false;
        p.running = state.running.load(Ordering::SeqCst);
        // Keep done/total/skipped — Resume must not zero the bar.
    }
    emit_progress(&app);
    Ok(())
}

/// Activate / download cancel. Does not abort a Documents refresh (`docs_cancel`)
/// and does not emit `docs-vf-cancel-requested` (that mis-labeled Activate as refresh).
#[tauri::command]
pub async fn cancel_google_downloads(app: AppHandle) -> Result<(), String> {
    let state = bulk();
    state.cancel.store(true, Ordering::SeqCst);
    state.pause.store(false, Ordering::SeqCst);
    if let Ok(mut pending) = state.pending.lock() {
        pending.clear();
    }
    if let Ok(aq) = state.active_queue.lock() {
        if let Some(ref q) = *aq {
            if let Ok(mut guard) = q.lock() {
                guard.clear();
            }
        }
    }
    if let Ok(mut queued) = state.queued.lock() {
        queued.clear();
    }
    if let Ok(mut p) = state.progress.lock() {
        p.paused = false;
        // Docs refresh has its own label. Only stamp Stopping… when Activate/download owns the bar.
        if !state.docs_vf_session_live.load(Ordering::SeqCst) {
            p.running = false;
            p.current = "Stopping…".into();
        }
    }
    emit_progress_force(&app);
    Ok(())
}

/// 1.0.204: Deactivate while download running — drop only those families from the queue.
#[tauri::command]
pub fn drop_google_download_families(families: Vec<String>) -> Result<u32, String> {
    let state = bulk();
    let keys: std::collections::HashSet<String> = families
        .iter()
        .map(|f| f.trim().to_lowercase())
        .filter(|k| !k.is_empty())
        .collect();
    if keys.is_empty() {
        return Ok(0);
    }
    let mut dropped = 0u32;
    if let Ok(mut pending) = state.pending.lock() {
        let before = pending.len();
        pending.retain(|f| !keys.contains(&f.trim().to_lowercase()));
        dropped = dropped.saturating_add((before - pending.len()) as u32);
    }
    if let Ok(aq) = state.active_queue.lock() {
        if let Some(ref q) = *aq {
            if let Ok(mut guard) = q.lock() {
                let before = guard.len();
                guard.retain(|f| !keys.contains(&f.trim().to_lowercase()));
                dropped = dropped.saturating_add((before - guard.len()) as u32);
            }
        }
    }
    if let Ok(mut queued) = state.queued.lock() {
        for key in &keys {
            if queued.remove(key) {
                dropped = dropped.saturating_add(1);
            }
        }
    }
    if let Ok(mut intents) = state.intents.lock() {
        for key in &keys {
            intents.remove(key);
        }
    }
    if let Ok(mut denied) = state.denied.lock() {
        for key in &keys {
            denied.insert(key.clone());
        }
    }
    if let Ok(mut p) = state.progress.lock() {
        if p.total > dropped {
            p.total = p.total.saturating_sub(dropped);
        }
    }
    Ok(dropped)
}

#[tauri::command]
pub fn google_download_progress() -> GoogleDlProgress {
    bulk()
        .progress
        .lock()
        .map(|p| p.clone())
        .unwrap_or(GoogleDlProgress {
            running: false,
            done: 0,
            total: 0,
            failed: 0,
            current: String::new(),
            failed_names: Vec::new(),
            failed_details: Vec::new(),
            paused: false,
            ready_names: Vec::new(),
            skipped: 0,
            settled_names: Vec::new(),
            kind: String::new(),
        })
}

