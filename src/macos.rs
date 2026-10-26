use crate::{Browser, BrowserOptions, Error, ErrorKind, Result, TargetType};
use objc2::rc::Retained;
use objc2_app_kit::{NSWorkspace, NSWorkspaceLaunchOptions};
use objc2_core_foundation::{CFOptionFlags, CFString};
use objc2_core_services::{kMDItemPath, MDItem, MDQuery, MDQueryOptionFlags};
use objc2_foundation::{
    NSArray, NSDictionary, NSError, NSFileManager, NSSearchPathDirectory, NSSearchPathDomainMask,
    NSString, NSURL,
};
use std::fmt::Display;
use std::path::{Path, PathBuf};

/// Deal with opening of browsers on macOS using the AppKit `NSWorkspace` APIs.
pub(super) fn open_browser_internal(
    browser: Browser,
    target: &TargetType,
    options: &BrowserOptions,
) -> Result<()> {
    let workspace = NSWorkspace::sharedWorkspace();

    // Resolve the URL of the browser application we want to launch. Rather than
    // relying on hardcoded application paths, we locate applications by their
    // bundle identifiers (or, for the default browser, ask the system which
    // application handles https urls).
    let app_url = match browser {
        Browser::Default => resolve_default_browser(&workspace),
        _ => {
            let bundle_id = bundle_id_for_browser(browser)
                .ok_or_else(|| Error::new(ErrorKind::NotFound, "browser not supported on macos"))?;
            let bundle_id = NSString::from_str(bundle_id);
            workspace.URLForApplicationWithBundleIdentifier(&bundle_id)
        }
    }
    .ok_or_else(|| Error::new(ErrorKind::NotFound, "browser not found"))?;

    launch(&workspace, &app_url, &browser, target, options)
}

/// Opens `target` with the application `app`: the path of a `.app` bundle,
/// else a bundle identifier, else a name, as [`resolve_app`] finds it.
pub(super) fn open_with_app_internal(
    app: &str,
    target: &TargetType,
    options: &BrowserOptions,
) -> Result<()> {
    let workspace = NSWorkspace::sharedWorkspace();
    let app_url = resolve_app(&workspace, app, target)?;
    launch(&workspace, &app_url, &app, target, options)
}

/// Shows `path`, an existing file or folder, selected in a Finder window.
pub(super) fn reveal_internal(path: &Path) -> Result<()> {
    std::fs::metadata(path)?;
    let path = path
        .to_str()
        .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "path is not valid unicode"))?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(path));
    NSWorkspace::sharedWorkspace()
        .activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[url]));
    Ok(())
}

/// The application `app` names, tried in this order:
///
/// 1. the path of a `.app` bundle;
/// 2. a bundle identifier, through Launch Services;
/// 3. a name, the bundle's file name without `.app`: first among the
///    applications that open `target`, then by a scan of the standard
///    application folders ([`scan_app_folders`]), then through Spotlight
///    ([`spotlight_apps`]) when the scan finds none.
///
/// The first step of 3 that finds the name decides it, and a name that two
/// applications of that step share is refused, with both paths.
fn resolve_app(workspace: &NSWorkspace, app: &str, target: &TargetType) -> Result<Retained<NSURL>> {
    if app.ends_with(".app") && Path::new(app).is_dir() {
        return Ok(NSURL::fileURLWithPath(&NSString::from_str(app)));
    }
    if let Some(url) = workspace.URLForApplicationWithBundleIdentifier(&NSString::from_str(app)) {
        return Ok(url);
    }
    let target_url = NSURL::URLWithString(&NSString::from_str(target.as_ref()))
        .ok_or_else(|| Error::other("failed to create target NSURL"))?;
    let bundle = format!("{app}.app");
    let mut found: Vec<PathBuf> = workspace
        .URLsForApplicationsToOpenURL(&target_url)
        .to_vec()
        .into_iter()
        .filter_map(|url| url.path())
        .map(|path| PathBuf::from(path.to_string()))
        .filter(|path| path.file_name().is_some_and(|name| name == bundle.as_str()))
        .collect();
    if found.is_empty() {
        found = scan_app_folders(&app_folders(), &bundle);
    }
    if found.is_empty() {
        found = spotlight_apps(&bundle)?;
    }
    let path = pick(app, found)?;
    Ok(NSURL::fileURLWithPath(&NSString::from_str(
        &path.to_string_lossy(),
    )))
}

/// The one path in `found`, or an error: `NotFound` when it is empty, and
/// `InvalidInput`, naming every path, when two differ.
///
/// Each path is first resolved through its symbolic links, so one bundle
/// reached by two paths counts once: `/Applications/Safari.app` links to the
/// copy under `/System/Cryptexes`. A path that cannot be resolved stays as it
/// is.
fn pick(app: &str, found: Vec<PathBuf>) -> Result<PathBuf> {
    let mut found: Vec<PathBuf> = found
        .into_iter()
        .map(|path| std::fs::canonicalize(&path).unwrap_or(path))
        .collect();
    found.sort();
    found.dedup();
    match found.len() {
        0 => Err(Error::new(
            ErrorKind::NotFound,
            format!("no application is named {app}"),
        )),
        1 => Ok(found.remove(0)),
        _ => {
            let paths: Vec<String> = found
                .iter()
                .map(|path| path.display().to_string())
                .collect();
            Err(Error::new(
                ErrorKind::InvalidInput,
                format!(
                    "more than one application is named {app}: {}",
                    paths.join(", ")
                ),
            ))
        }
    }
}

/// The standard application folders of every domain, as the system lists
/// them, such as `/Applications` and `~/Applications`.
fn app_folders() -> Vec<PathBuf> {
    NSFileManager::defaultManager()
        .URLsForDirectory_inDomains(
            NSSearchPathDirectory::ApplicationDirectory,
            NSSearchPathDomainMask::AllDomainsMask,
        )
        .to_vec()
        .into_iter()
        .filter_map(|url| url.path())
        .map(|path| PathBuf::from(path.to_string()))
        .collect()
}

/// Every bundle named `bundle` in `folders` or one folder down, such as
/// `/Applications/Utilities`. A folder that cannot be read is skipped, and
/// nothing inside a `.app` bundle is read.
fn scan_app_folders(folders: &[PathBuf], bundle: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for folder in folders {
        for path in entries(folder) {
            if path.file_name().is_some_and(|name| name == bundle) {
                found.push(path);
            } else if path.extension().is_none_or(|ext| ext != "app") && path.is_dir() {
                found.extend(
                    entries(&path)
                        .filter(|nested| nested.file_name().is_some_and(|name| name == bundle)),
                );
            }
        }
    }
    found
}

/// The paths in `folder`, or none when it cannot be read.
fn entries(folder: &Path) -> impl Iterator<Item = PathBuf> {
    std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
}

/// Every application bundle named `bundle` that Spotlight has indexed,
/// through a synchronous `MDQuery`, which needs no run loop.
fn spotlight_apps(bundle: &str) -> Result<Vec<PathBuf>> {
    let mut name = String::with_capacity(bundle.len());
    for c in bundle.chars() {
        // A query string quotes its value; `*` in a value is a wildcard.
        if matches!(c, '\\' | '"' | '*') {
            name.push('\\');
        }
        name.push(c);
    }
    let expression = CFString::from_str(&format!(
        "kMDItemContentType == \"com.apple.application-bundle\" && kMDItemFSName == \"{name}\""
    ));
    // SAFETY: the default allocator, a query string, and no value-list or
    // sorting attributes, all of which `MDQueryCreate` accepts as NULL.
    let query = unsafe { MDQuery::new(None, Some(&*expression), None, None) }
        .ok_or_else(|| Error::other("failed to create the Spotlight query"))?;
    // SAFETY: a query created above and not yet executed.
    let executed = unsafe { query.execute(MDQueryOptionFlags::Synchronous.0 as CFOptionFlags) };
    if !executed {
        return Err(Error::other("the Spotlight query failed"));
    }
    // SAFETY: an immutable constant of the Metadata framework.
    let path_key =
        unsafe { kMDItemPath }.ok_or_else(|| Error::other("kMDItemPath is not defined"))?;
    let mut found = Vec::new();
    // SAFETY: a synchronous query no longer changes its results, so each
    // index below the count holds an `MDItem`.
    let count = unsafe { query.result_count() };
    for index in 0..count {
        let item = unsafe { query.result_at_index(index) }.cast::<MDItem>();
        let Some(item) = (unsafe { item.as_ref() }) else {
            continue;
        };
        let path = unsafe { item.attribute(Some(path_key)) };
        if let Some(path) = path
            .as_deref()
            .and_then(|value| value.downcast_ref::<CFString>())
        {
            let path = PathBuf::from(path.to_string());
            if path.file_name().is_some_and(|name| name == bundle) {
                found.push(path);
            }
        }
    }
    Ok(found)
}

/// Opens `target` with the application at `app_url`, which `label` names in
/// logs.
// We intentionally use the deprecated synchronous launch API (see the note at
// the launch site below), so silence the deprecation warnings for this fn.
#[allow(deprecated)]
fn launch(
    workspace: &NSWorkspace,
    app_url: &NSURL,
    label: &dyn Display,
    target: &TargetType,
    options: &BrowserOptions,
) -> Result<()> {
    let browser = label;

    // handle dry-run scenario: the application was located above, so we know it
    // exists without actually launching it.
    if options.dry_run {
        log::debug!("dry-run: not actually opening the browser {browser}");
        return Ok(());
    }

    // create the NSURL for the target we want to open
    let target_string = NSString::from_str(target.as_ref());
    let target_url = NSURL::URLWithString(&target_string)
        .ok_or_else(|| Error::other("failed to create target NSURL"))?;
    let urls = NSArray::from_retained_slice(&[target_url]);

    // configure the launch. We use the (deprecated) synchronous launch API
    // rather than the newer completion-handler based one, as the latter relies
    // on the caller running a main run loop to actually dispatch the launch,
    // which isn't the case for library consumers (and breaks in headless
    // environments such as CI). This is the direct equivalent of the Launch
    // Services `LSOpenFromURLSpec` call we previously used.
    let mut launch_options = NSWorkspaceLaunchOptions::Default;
    if options.dont_switch {
        // don't bring the browser to the foreground
        launch_options |= NSWorkspaceLaunchOptions::WithoutActivation;
    }
    let config = NSDictionary::new();

    // launch the browser
    log::trace!("about to start browser: {browser} for {target} using app at {app_url:?}");
    let result = unsafe {
        workspace.openURLs_withApplicationAtURL_options_configuration_error(
            &urls,
            app_url,
            launch_options,
            &config,
        )
    };
    match result {
        Ok(running_app) => {
            log::trace!(
                "launched {browser}: bundle_id={:?}, url={:?}, finished_launching={}",
                running_app.bundleIdentifier(),
                running_app.bundleURL(),
                running_app.isFinishedLaunching(),
            );
            Ok(())
        }
        Err(err) => Err(map_launch_error(&err)),
    }
}

/// Maps an `NSError` returned while launching the browser to our [`Error`] type,
/// preserving the `NotFound`/`PermissionDenied` distinctions where possible.
fn map_launch_error(err: &NSError) -> Error {
    // For the `NSOSStatusErrorDomain`, `code` is the underlying Launch Services
    // `OSStatus`. See the `Result Codes` section of:
    // https://developer.apple.com/documentation/coreservices/launch_services
    let kind = match err.code() {
        // -43 is file not found, while -10814 is launch services err code
        -43 | -10814 => ErrorKind::NotFound,
        -10826 => ErrorKind::PermissionDenied,
        _ => ErrorKind::Other,
    };
    Error::new(kind, err.localizedDescription().to_string())
}

/// Determine the URL of the user's default browser, falling back to Safari if
/// detection fails for any reason.
fn resolve_default_browser(workspace: &NSWorkspace) -> Option<objc2::rc::Retained<NSURL>> {
    let https = NSString::from_str("https://");
    let resolved =
        NSURL::URLWithString(&https).and_then(|url| workspace.URLForApplicationToOpenURL(&url));

    match resolved {
        Some(url) => {
            log::trace!("default browser is {url:?}");
            Some(url)
        }
        None => {
            log::error!("failed to get default browser, falling back to Safari");
            let safari = NSString::from_str(SAFARI_BUNDLE_ID);
            workspace.URLForApplicationWithBundleIdentifier(&safari)
        }
    }
}

/// Maps a [`Browser`] to its macOS bundle identifier. Returns `None` for
/// browsers that aren't supported on macOS.
fn bundle_id_for_browser(browser: Browser) -> Option<&'static str> {
    match browser {
        Browser::Firefox => Some("org.mozilla.firefox"),
        Browser::Chrome => Some("com.google.Chrome"),
        Browser::Opera => Some("com.operasoftware.Opera"),
        Browser::Safari => Some(SAFARI_BUNDLE_ID),
        _ => None,
    }
}

/// We assume Safari to be the default browser, if deductions fail for any reason
const SAFARI_BUNDLE_ID: &str = "com.apple.Safari";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_non_existing_browser() {
        let _ = env_logger::try_init();
        if let Err(err) = open_browser_internal(
            Browser::Opera,
            &TargetType::try_from("https://github.com").expect("failed to parse url"),
            &BrowserOptions::default(),
        ) {
            assert_eq!(err.kind(), ErrorKind::NotFound);
        } else {
            panic!("expected opening non-existing browser to fail");
        }
    }

    #[test]
    fn test_existence() {
        let _ = env_logger::try_init();
        assert!(Browser::Safari.exists());
        assert!(!Browser::Opera.exists());
    }

    #[test]
    fn test_scan_finds_a_bundle_at_the_top_and_one_folder_down() {
        let root = std::env::temp_dir().join(format!("webbrowser-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in [
            "Twice.app",
            "Vendor/Twice.app",
            "Other.app/Twice.app",
            "a/b/Twice.app",
        ] {
            std::fs::create_dir_all(root.join(dir)).expect("failed to create a test bundle");
        }
        let mut found = scan_app_folders(std::slice::from_ref(&root), "Twice.app");
        found.sort();
        std::fs::remove_dir_all(&root).expect("failed to remove the test folder");
        assert_eq!(
            found,
            [root.join("Twice.app"), root.join("Vendor/Twice.app")]
        );
    }

    #[test]
    fn test_a_bundle_reached_through_a_symlink_counts_once() {
        let root = std::env::temp_dir().join(format!("webbrowser-pick-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let bundle = root.join("System/Twice.app");
        std::fs::create_dir_all(&bundle).expect("failed to create a test bundle");
        std::os::unix::fs::symlink(&bundle, root.join("Twice.app"))
            .expect("failed to link the test bundle");
        let found = scan_app_folders(std::slice::from_ref(&root), "Twice.app");
        let count = found.len();
        let picked = pick("Twice", found);
        let expected = std::fs::canonicalize(bundle);
        std::fs::remove_dir_all(root).expect("failed to remove the test folder");
        assert_eq!(count, 2);
        assert_eq!(
            picked.expect("expected one application"),
            expected.expect("failed to resolve the test bundle")
        );
    }

    #[test]
    fn test_a_shared_name_is_refused_with_every_path() {
        let found = vec![
            PathBuf::from("/b/Twice.app"),
            PathBuf::from("/a/Twice.app"),
            PathBuf::from("/b/Twice.app"),
        ];
        let err = pick("Twice", found).expect_err("expected a shared name to be refused");
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        assert_eq!(
            err.to_string(),
            "more than one application is named Twice: /a/Twice.app, /b/Twice.app"
        );
    }

    #[test]
    fn test_spotlight_finds_only_the_bundle_it_was_asked_for() {
        let found = spotlight_apps("Safari.app").expect("failed to run the Spotlight query");
        assert!(
            found.iter().all(|path| path.ends_with("Safari.app")),
            "{found:?}"
        );
        let missing = spotlight_apps("webbrowser-test-no-such-app.app")
            .expect("failed to run the Spotlight query");
        assert!(missing.is_empty(), "{missing:?}");
    }

    /// Times one lookup on its own, for the benchmark of the two: its first
    /// call in this process, then the median and slowest of 20 more.
    /// `WEBBROWSER_LOOKUP` is `scan` or `spotlight`, and
    /// `WEBBROWSER_LOOKUP_NAME` the name to find.
    #[test]
    #[ignore]
    fn lookup_timing() {
        let lookup = std::env::var("WEBBROWSER_LOOKUP").expect("WEBBROWSER_LOOKUP is not set");
        let name =
            std::env::var("WEBBROWSER_LOOKUP_NAME").expect("WEBBROWSER_LOOKUP_NAME is not set");
        let bundle = format!("{name}.app");
        let find = || match lookup.as_str() {
            "scan" => scan_app_folders(&app_folders(), &bundle),
            "spotlight" => spotlight_apps(&bundle).expect("failed to run the Spotlight query"),
            other => panic!("unknown lookup {other}"),
        };
        let start = std::time::Instant::now();
        let found = find();
        let cold = start.elapsed();
        let mut warm: Vec<_> = (0..20)
            .map(|_| {
                let start = std::time::Instant::now();
                find();
                start.elapsed()
            })
            .collect();
        warm.sort();
        println!(
            "lookup={lookup} name={name} found={found:?} cold_us={} warm_p50_us={} warm_max_us={} folders={:?}",
            cold.as_micros(),
            warm[10].as_micros(),
            warm[19].as_micros(),
            app_folders(),
        );
    }
}
