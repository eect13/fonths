//! c258 tests for `gfsource`. Live ones are `#[ignore]` (run with `--ignored`).
use super::gfsource::*;
use std::time::{Duration, Instant};

const INTER_SNIP: &str = ")]}'\n{\"zipName\":\"Inter.zip\",\"manifest\":{\"files\":[{\"filename\":\"OFL.txt\",\"contents\":\"Copyright Inter\"},{\"filename\":\"README.txt\",\"contents\":\"r\"}],\"fileRefs\":[\
{\"filename\":\"static/Inter_24pt-Regular.ttf\",\"url\":\"https://fonts.gstatic.com/s/inter/v20/b.ttf\"},\
{\"filename\":\"static/Inter_18pt-Regular.ttf\",\"url\":\"https://fonts.gstatic.com/s/inter/v20/a.ttf\"},\
{\"filename\":\"static/Inter_18pt-BoldItalic.ttf\",\"url\":\"https://fonts.gstatic.com/s/inter/v20/c.ttf\"},\
{\"filename\":\"static/Inter_18pt-Italic.ttf\",\"url\":\"https://fonts.gstatic.com/s/inter/v20/d.ttf\"},\
{\"filename\":\"Inter-VariableFont_opsz,wght.ttf\",\"url\":\"https://fonts.gstatic.com/s/inter/v20/v.ttf\"},\
{\"filename\":\"Inter-Italic-VariableFont_opsz,wght.ttf\",\"url\":\"https://fonts.gstatic.com/s/inter/v20/vi.ttf\"}]}}";

#[test]
fn c258_manifest_parse_strips_xssi_keeps_licence() {
    let m = parse_manifest(INTER_SNIP).expect("manifest");
    assert_eq!(m.refs.len(), 6);
    assert_eq!(manifest_license_dir(&m), Some("ofl"));
    assert_eq!(manifest_license_files(&m), vec![("OFL.txt".to_string(), "Copyright Inter".to_string())]);
    assert!(parse_manifest("<html>spa</html>").is_none());
}

#[test]
fn c258_manifest_statics_one_per_face_smallest_opsz() {
    let m = parse_manifest(INTER_SNIP).unwrap();
    let l = manifest_static_listing(&m, "Inter");
    assert_eq!(
        l,
        vec![
            ("normal".into(), "400".into(), "https://fonts.gstatic.com/s/inter/v20/a.ttf".into()),
            ("italic".into(), "400".into(), "https://fonts.gstatic.com/s/inter/v20/d.ttf".into()),
            ("italic".into(), "700".into(), "https://fonts.gstatic.com/s/inter/v20/c.ttf".into()),
        ]
    );
    let v = manifest_variable_refs(&m);
    assert_eq!(v.len(), 2);
    assert!(v.iter().any(|(_, _, it)| *it));
}

#[test]
fn c258_manifest_statics_skip_width_siblings() {
    let t = "{\"manifest\":{\"files\":[],\"fileRefs\":[\
{\"filename\":\"static/Roboto-Regular.ttf\",\"url\":\"https://x/r\"},\
{\"filename\":\"static/Roboto_Condensed-Regular.ttf\",\"url\":\"https://x/c\"}]}}";
    let l = manifest_static_listing(&parse_manifest(t).unwrap(), "Roboto");
    assert_eq!(l, vec![("normal".into(), "400".into(), "https://x/r".into())]);
}

#[test]
fn c258_download_list_url_encodes() {
    assert_eq!(download_list_url("Noto Sans JP"), "https://fonts.google.com/download/list?family=Noto+Sans+JP");
    assert_eq!(download_list_url("M PLUS 1p"), "https://fonts.google.com/download/list?family=M+PLUS+1p");
}

#[test]
fn c258_shape_rejects_short_glyphs_or_features() {
    let full = FaceShape { glyphs: 694, features: ["smcp", "c2sc", "kern", "liga"].iter().map(|s| s.to_string()).collect() };
    let css = FaceShape { glyphs: 571, features: ["kern", "liga"].iter().map(|s| s.to_string()).collect() };
    let drift = FaceShape { glyphs: 680, features: full.features.clone() };
    assert!(!shape_not_worse(&css, &full));
    assert!(shape_not_worse(&drift, &full));
    assert!(shape_not_worse(&full, &css));
}

#[test]
fn c258_backoff_and_status() {
    assert_eq!(backoff_delay(0, None), Duration::from_secs(1));
    assert_eq!(backoff_delay(3, None), Duration::from_secs(8));
    assert_eq!(backoff_delay(10, None), Duration::from_secs(60));
    assert_eq!(backoff_delay(0, Some(600)), Duration::from_secs(60));
    for c in [403, 429, 500, 503] {
        assert!(status_backs_off(c));
    }
    assert!(!status_backs_off(404));
    assert!(POLITE_UA.contains("github.com/eect13/fonths") && !POLITE_UA.contains('@'));
}

#[test]
fn c258_pacer_caps_concurrency_and_spacing() {
    let start = Instant::now();
    let peak = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hs: Vec<_> = (0..6)
        .map(|_| {
            let peak = peak.clone();
            std::thread::spawn(move || {
                let _s = Slot::acquire();
                peak.fetch_max(Slot::in_use(), std::sync::atomic::Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(30));
            })
        })
        .collect();
    for h in hs {
        h.join().unwrap();
    }
    assert!(peak.load(std::sync::atomic::Ordering::SeqCst) <= MAX_CONCURRENT);
    // 6 starts on the metadata lane, ≥100 ms apart → ≥500 ms.
    assert!(start.elapsed() >= Duration::from_millis(480), "{:?}", start.elapsed());

    // CDN binary lane permits up to CDN_MAX_CONCURRENT concurrent slots with CDN_MIN_SPACING.
    let cdn_peak = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let cdn_hs: Vec<_> = (0..8)
        .map(|_| {
            let cdn_peak = cdn_peak.clone();
            std::thread::spawn(move || {
                let _s = Slot::acquire_cdn();
                cdn_peak.fetch_max(Slot::cdn_in_use(), std::sync::atomic::Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(20));
            })
        })
        .collect();
    for h in cdn_hs {
        h.join().unwrap();
    }
    assert!(cdn_peak.load(std::sync::atomic::Ordering::SeqCst) <= CDN_MAX_CONCURRENT);
}

#[test]
fn c258_caches() {
    let d = std::env::temp_dir().join(format!("c258-cache-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let p = d.join("x.json");
    assert!(!cache_fresh(&p, std::time::SystemTime::now()));
    std::fs::write(&p, "x").unwrap();
    assert!(cache_fresh(&p, std::time::SystemTime::now()));
    assert!(!cache_fresh(&p, std::time::SystemTime::now() + MANIFEST_MAX_AGE + Duration::from_secs(1)));
    std::fs::write(p.with_extension("rev"), "2026-04-01").unwrap();
    assert!(cache_fresh_for_revision(&p, std::time::SystemTime::now(), Some("2026-04-01")));
    assert!(!cache_fresh_for_revision(&p, std::time::SystemTime::now(), Some("2026-05-01")));
    assert!(is_cdn_binary_url("https://fonts.gstatic.com/s/inter/v18/UcC73FwrK3iLTeHuS_nVMrMxCp50SjIa1ZL7.ttf"));
    assert!(is_cdn_binary_url("https://cdn.jsdelivr.net/gh/google/fonts@main/ofl/inter/Inter.ttf"));
    assert!(!is_cdn_binary_url("https://fonts.google.com/metadata/fonts"));
    record_source_url(&d, "barlow-400-normal.ttf", "https://u/1");
    record_source_url(&d, "barlow-700-normal.ttf", "https://u/2");
    record_source_url(&d, "barlow-400-normal.ttf", "https://u/3");
    assert_eq!(source_url_for(&d, "barlow-400-normal.ttf").as_deref(), Some("https://u/3"));
    assert_eq!(read_source_urls(&d).len(), 2);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn c258_static_first_never_both() {
    assert!(!use_variable(false, true));
    assert!(use_variable(false, false));
    assert!(use_variable(true, true));
}

#[test]
fn vf_primary_default_and_static_first_opt_in_plus_manifest_circuit() {
    let d = std::env::temp_dir().join(format!("fm-vf-policy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    // 1.0.206u default: VF-Primary when no marker exists.
    assert!(prefer_variable(Some(&d)));
    set_prefer_statics(Some(&d), true);
    assert!(!prefer_variable(Some(&d)));
    set_prefer_statics(Some(&d), false);
    assert!(prefer_variable(Some(&d)));

    // Manifest negative-cache & circuit breaker.
    reset_manifest_circuit();
    clear_manifest_negative_cache();
    assert!(manifest_circuit_allow());
    assert!(!manifest_missed_this_session("NonExistentTestFamily"));
    note_manifest_miss("NonExistentTestFamily");
    assert!(manifest_missed_this_session("NonExistentTestFamily"));
    for _ in 0..5 {
        note_manifest_failure();
    }
    assert!(!manifest_circuit_allow(), "5 consecutive manifest failures trip circuit");
    reset_manifest_circuit();
    assert!(manifest_circuit_allow(), "reset_manifest_circuit re-opens gate");
    clear_manifest_negative_cache();
    let _ = std::fs::remove_dir_all(&d);
}

// ------------------------------------------------------------ live (run once)

fn client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder().timeout(Duration::from_secs(300)).build().unwrap()
}
fn no_cancel() -> bool {
    false
}
fn fetch(c: &reqwest::blocking::Client, url: &str) -> Vec<u8> {
    polite_get(c, url, &no_cancel).expect(url)
}
fn manifest(c: &reqwest::blocking::Client, fam: &str) -> Manifest {
    fetch_manifest_cached(c, fam, None, &no_cancel).expect(fam)
}

#[test]
#[ignore]
fn c258_live_barlow_regular_full() {
    let c = client();
    let m = manifest(&c, "Barlow");
    let l = manifest_static_listing(&m, "Barlow");
    assert_eq!(l.len(), 18, "Barlow statics");
    let (_, _, url) = l.iter().find(|(s, w, _)| s == "normal" && w == "400").unwrap();
    let b = fetch(&c, url);
    let g = num_glyphs(&b).unwrap();
    let f = feature_tags(&b);
    eprintln!("LIVE Barlow-Regular {} B glyphs={g} cmap={} feats={}: {:?}", b.len(), cmap_count(&b), f.len(), f);
    assert!(g >= 680);
    assert!(f.contains("smcp"));
}

#[test]
#[ignore]
fn c258_live_inter_vf_has_fvar() {
    let c = client();
    let m = manifest(&c, "Inter");
    let v = manifest_variable_refs(&m);
    let (n, url, _) = v.iter().find(|(_, _, it)| !*it).unwrap();
    let b = fetch(&c, url);
    eprintln!("LIVE Inter VF {n} {} B glyphs={:?} fvar={}", b.len(), num_glyphs(&b), has_fvar(&b));
    assert!(has_fvar(&b));
    assert_eq!(manifest_static_listing(&m, "Inter").len(), 18);
}

#[test]
#[ignore]
fn c258_live_noto_sans_jp_cmap() {
    let c = client();
    let m = manifest(&c, "Noto Sans JP");
    let l = manifest_static_listing(&m, "NotoSansJP");
    let (_, _, url) = l.iter().find(|(s, w, _)| s == "normal" && w == "400").unwrap();
    let b = fetch(&c, url);
    let n = cmap_count(&b);
    eprintln!("LIVE NotoSansJP-Regular {} B glyphs={:?} cmap={n}", b.len(), num_glyphs(&b));
    assert!(n >= 16_732);
}

#[test]
#[ignore]
fn c258_live_fira_sans_full_features_vs_repo_and_css2() {
    let c = client();
    let m = manifest(&c, "Fira Sans");
    let l = manifest_static_listing(&m, "FiraSans");
    let (_, _, url) = l.iter().find(|(s, w, _)| s == "normal" && w == "400").unwrap();
    let man = face_shape(&fetch(&c, url)).unwrap();
    let repo = face_shape(&fetch(&c, "https://cdn.jsdelivr.net/gh/google/fonts@main/ofl/firasans/FiraSans-Regular.ttf")).unwrap();
    eprintln!("LIVE FiraSans manifest glyphs={} feats={} | repo glyphs={} feats={}", man.glyphs, man.features.len(), repo.glyphs, repo.features.len());
    eprintln!("  missing vs repo: {:?}", repo.features.difference(&man.features).collect::<Vec<_>>());
    assert!(shape_not_worse(&man, &repo), "manifest Fira Sans must carry the repo feature set");
    for t in ["case", "dlig", "onum", "lnum", "smcp", "c2sc", "frac", "sups"] {
        assert!(man.features.contains(t), "{t}");
    }
    // css2 (Mozilla UA) is the cut build: the shape gate must reject it.
    let css = String::from_utf8(
        c.get("https://fonts.googleapis.com/css2?family=Fira+Sans:wght@400")
            .header("User-Agent", "Mozilla/5.0")
            .send().unwrap().bytes().unwrap().to_vec(),
    ).unwrap();
    let u = css.split("url(").nth(1).and_then(|s| s.split(')').next()).unwrap().to_string();
    let cut = face_shape(&fetch(&c, &u)).unwrap();
    eprintln!("LIVE FiraSans css2 glyphs={} feats={}", cut.glyphs, cut.features.len());
    assert!(!shape_not_worse(&cut, &man));
}

/// c263: the Google Fonts source module — order and laziness behind the trait.
struct Fake {
    manifest: Option<Manifest>,
    repo: Listing,
    css: Listing,
    calls: std::cell::RefCell<Vec<&'static str>>,
}
impl GoogleFontsSource for Fake {
    fn manifest(&self, _f: &str) -> Option<Manifest> {
        self.calls.borrow_mut().push("manifest");
        self.manifest.clone()
    }
    fn pascal(&self, _f: &str) -> String {
        "Inter".into()
    }
    fn repo_listing(&self, _f: &str, lic: Option<&str>) -> Listing {
        self.calls.borrow_mut().push(if lic == Some("ofl") { "repo:ofl" } else { "repo" });
        self.repo.clone()
    }
    fn css2_listing(&self, _f: &str) -> Listing {
        self.calls.borrow_mut().push("css2");
        self.css.clone()
    }
}
fn one(u: &str) -> Listing {
    vec![("normal".into(), "400".into(), u.into())]
}

#[test]
fn c263_source_module_order_and_fallbacks() {
    assert_eq!(SOURCE_ORDER, [ListingSource::Manifest, ListingSource::Repo, ListingSource::Css2]);
    let m = parse_manifest(INTER_SNIP).unwrap();
    let f = Fake { manifest: Some(m), repo: one("r"), css: one("c"), calls: Default::default() };
    let (l, s, man) = discover_listing(&f, "Inter");
    assert_eq!(s, ListingSource::Manifest);
    assert!(man.is_some() && !l.is_empty());
    assert_eq!(*f.calls.borrow(), vec!["manifest"], "repo/css2 never asked");
    // Manifest down: repo (with the manifest licence dir when known), css2 untouched.
    let f = Fake { manifest: None, repo: one("r"), css: one("c"), calls: Default::default() };
    assert_eq!(discover_listing(&f, "Inter").1, ListingSource::Repo);
    assert_eq!(*f.calls.borrow(), vec!["manifest", "repo"]);
    // Both full sources empty: css2 last resort.
    let f = Fake { manifest: None, repo: Vec::new(), css: one("c"), calls: Default::default() };
    let (l, s, _) = discover_listing(&f, "Inter");
    assert_eq!((l, s), (one("c"), ListingSource::Css2));
    assert!(!s.is_full());
}
