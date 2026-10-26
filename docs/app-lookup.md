# Finding an application by name on macOS

`open_with_app` takes the path of a `.app` bundle, a bundle identifier, or a name.
This fork adds the name lookup that reaches every installed application, for a-process.

## The decision

The lead relayed the operator's answer on 2026-10-25, in these words:

> **Decision:** scan the standard app folders first, and fall back to Spotlight when the scan finds nothing. objc2-core-services is approved as a fork dependency for the Spotlight fallback.

## The order

1. The path of a `.app` bundle.
2. A bundle identifier, through Launch Services (`URLForApplicationWithBundleIdentifier:`).
3. A name, the bundle's file name without `.app`:
   1. among the applications that open the URL (`URLsForApplicationsToOpenURL:`);
   2. by a scan of the standard application folders that `NSFileManager` lists for every domain, and one folder down, such as `/Applications/Utilities`;
   3. through Spotlight, a synchronous `MDQuery` on `kMDItemContentType` and `kMDItemFSName`, only when the scan finds none.

The first step of 3 that finds the name decides it.
A name that two applications of that step share is refused with `InvalidInput`, and the error names every path.
Each path is resolved through its symbolic links first, so one bundle reached by two paths counts once: `/Applications/Safari.app` links to the copy under `/System/Cryptexes`, and the scan finds both.

Each lookup is its own function in `src/macos.rs` (`scan_app_folders`, `spotlight_apps`), so the ignored `lookup_timing` test can time one alone:

```sh
WEBBROWSER_LOOKUP=scan WEBBROWSER_LOOKUP_NAME=Safari cargo test --lib -- --ignored --exact os::tests::lookup_timing --nocapture
```
