# iPhone build

The iPhone app is built by the **Build iOS IPA** GitHub Actions workflow
(`.github/workflows/ios.yml`) on every push that touches the game, and can
be started by hand from the Actions tab. It produces an ad-hoc signed
`L4D2Rewrite.ipa` as a run artifact. Install it with Sideloadly or AltStore.

To build it yourself on a Mac with Xcode and the Rust `aarch64-apple-ios`
target installed:

```sh
sh ios/build-ipa.sh
```

## What is in the app

The IPA contains the engine, shaders and the small test map shown behind the
menu. It does not contain any Left 4 Dead 2 game files. On first launch:

- **Sign in with Steam** downloads the game files with your own Steam account,
  which must own Left 4 Dead 2. The starter pack holds shared content and
  Dead Center. The other campaigns and optional dialogue are under
  **GAME FILES** in the main menu. Details are in
  [../mobile/steam-installer/README.md](../mobile/steam-installer/README.md).
- Or copy the folders of a PC install into
  *Files → On My iPhone → L4D2 Rewrite → Left 4 Dead 2*.

Close and reopen the app after the files are in place.

## Logs

Each session writes `l4d2r-log.txt` next to the game folder, and the previous
session is kept as `l4d2r-log-previous.txt`. Both can be opened in the Files
app. The log includes memory use during play, which is the most common cause
of the app being closed by iOS. Please attach it to bug reports.

## Performance

The default render scale is 2. A render scale of 3 draws the full panel
resolution and uses much more memory and GPU time.
