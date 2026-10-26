#[cfg(target_os = "macos")]
mod common;

#[cfg(target_os = "macos")]
mod tests {
    const TEST_PLATFORM: &str = "macos";

    use super::common::*;
    use serial_test::serial;
    use webbrowser::Browser;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[serial]
    async fn test_open_default() {
        check_browser(Browser::Default, TEST_PLATFORM).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[serial]
    async fn test_open_safari() {
        check_browser(Browser::Safari, TEST_PLATFORM).await;
    }

    // #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    // #[serial]
    // #[ignore]
    // async fn test_open_firefox() {
    //     check_browser(Browser::Firefox, TEST_PLATFORM).await;
    // }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[serial]
    #[ignore]
    async fn test_open_chrome() {
        check_browser(Browser::Chrome, TEST_PLATFORM).await;
    }

    #[test]
    fn test_existence_default() {
        assert!(Browser::is_available(), "should have found a browser");
    }

    #[test]
    fn test_existence_safari() {
        assert!(Browser::Safari.exists(), "should have found Safari");
    }

    #[test]
    fn test_non_existence_webpositive() {
        assert!(
            !Browser::WebPositive.exists(),
            "should not have found WebPositive",
        );
    }

    #[cfg(not(feature = "hardened"))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[serial]
    async fn test_local_file_abs_path() {
        check_local_file(Browser::Default, None, |pb| {
            pb.as_os_str().to_string_lossy().into()
        })
        .await;
    }

    #[cfg(not(feature = "hardened"))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[serial]
    async fn test_local_file_rel_path() {
        let cwd = std::env::current_dir().expect("unable to get current dir");
        check_local_file(Browser::Default, None, |pb| {
            pb.strip_prefix(cwd)
                .expect("strip prefix failed")
                .as_os_str()
                .to_string_lossy()
                .into()
        })
        .await;
    }

    #[cfg(not(feature = "hardened"))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[serial]
    async fn test_local_file_uri() {
        check_local_file(Browser::Default, None, |pb| {
            url::Url::from_file_path(pb)
                .expect("failed to convert path to url")
                .to_string()
        })
        .await;
    }

    #[cfg(feature = "hardened")]
    #[test]
    fn test_hardened_mode() {
        let err = webbrowser::open("file:///etc/passwd")
            .expect_err("expected non-http url to fail in hardened mode");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        let err = webbrowser::open_with_app("Safari", "file:///etc/passwd")
            .expect_err("expected non-http url to fail in hardened mode");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn test_reveal_missing_path() {
        let err = webbrowser::reveal(std::path::Path::new("/webbrowser-test/no-such-path"))
            .expect_err("expected a missing path to fail");
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn test_open_with_missing_app() {
        let err = webbrowser::open_with_app("webbrowser-test-no-such-app", "https://github.com")
            .expect_err("expected a missing application to fail");
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn test_open_with_app_finds_a_path_a_bundle_id_and_a_name() {
        let mut options = webbrowser::BrowserOptions::new();
        options.with_dry_run(true);
        for app in ["/Applications/Safari.app", "com.apple.Safari", "Safari"] {
            let opened =
                webbrowser::open_with_app_with_options(app, "https://github.com", &options);
            assert!(opened.is_ok(), "{app}: {opened:?}");
        }
    }

    #[test]
    fn test_open_with_app_finds_a_name_that_opens_no_url() {
        let mut options = webbrowser::BrowserOptions::new();
        options.with_dry_run(true);
        let opened =
            webbrowser::open_with_app_with_options("Calculator", "https://github.com", &options);
        assert!(opened.is_ok(), "{opened:?}");
    }
}
