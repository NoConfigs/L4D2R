# L4D2R Launcher (Windows)

Open **L4D2R Launcher.exe** beside the rewrite source. Select an installed Left 4 Dead 2 folder, choose video settings, then select **Build & Play**. The launcher remembers the installation and installs the release game to `runtime/l4d2-rewrite.exe`. Future **Play** clicks start that EXE directly. **Test map** starts the laboratory instead of the main menu. **Rebuild** is explicit, for example after downloading updated source.

The first game build requires Rust 1.95+ (Windows MSVC), Visual Studio C++ Build Tools and a Windows SDK. Cargo downloads dependencies on the first build; later play does not require Cargo. The launcher itself uses Windows .NET Framework 4.x, with no Bevy renderer or Rust compilation needed to open it. Keep it beside `Cargo.toml`, `src`, `assets`, `maps`, `vendor` and `build.rs`; it is not an installer or a game-asset bundle.

Video controls use the same `runtime/graphics-options.ini` and `runtime/display-options.ini` as the game. Settings apply on the next game launch. Existing FOV and unknown settings are preserved. Unsupported MSAA requests use the game's hardware fallback. Settings and rebuilding are disabled while this copy of the game runs. The launcher refreshes saved settings when the game closes.

Build output appears in the **Build log** tab and `runtime/launcher-build.log`. Cancelling or failing a rebuild preserves the previous playable EXE. Closing the launcher during a build cancels its compiler process tree first. Closing it while playing leaves the game running.

**Android** support is coming soon; the tab is greyed out for now.

**Look and branding.** The launcher follows the L4D2 menus:
- a grained near-black backdrop with a vignette;
- Bahnschrift Condensed capitals (falling back to Arial Narrow or Segoe UI);
- a blood-red selection bar and PLAY button;
- square controls with hairline rules.

The logo is embedded from `launcher/assets/logo.png`, and `launcher/app.ico` is the exe, window and taskbar icon. Both are built from the full logo PNG with `tools/make-launcher-icon.ps1 -Source <logo.png>`.

To rebuild the launcher source:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools/build-launcher.ps1 -Test
```

The executable self-tests use a unique temporary fixture with a fake game/compiler, covering folder validation, settings persistence, process arguments, first build, repeat play, failure/cancellation recovery and target-directory overrides. They also create a real form and save `local-research/launcher/launcher-preview.png`. They do not change your installed game or rebuild the real rewrite.

For an optional integration check, run `& '.\L4D2R Launcher.exe' --smoke-game`. It uses your existing runtime, saves the detected game directory and captures the actual menu to `local-research/launcher/game-menu.png`, without rebuilding or changing saved video settings. Leave the game closed before running it.
