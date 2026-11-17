using System;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;
using System.Windows.Forms;

namespace L4D2R.Launcher
{
    // Isolated executable-level tests. Never build, launch or edit the user's game.
    internal static class LauncherTests
    {
        internal static int Child(string[] args)
        {
            string root = Environment.CurrentDirectory;
            if (args.Contains("--launcher-test-game"))
            {
                Files.Write(Path.Combine(root, "runtime/launch-probe.txt"), root + "\n" + String.Join("\n", args));
                if (args.Contains("--wait")) Thread.Sleep(2500);
                return 0;
            }
            if (args.Contains("--wait")) { Thread.Sleep(30000); return 0; }
            Console.WriteLine("test compiler output"); Console.Error.WriteLine("test compiler diagnostic");
            if (args.Contains("--fail")) return 7;
            string output = Path.Combine(root, "target/release/l4d2-rewrite.exe");
            Directory.CreateDirectory(Path.GetDirectoryName(output));
            File.Copy(Application.ExecutablePath, output, true);
            return 0;
        }
        private static void Assert(bool condition, string message) { if (!condition) throw new Exception(message); }
        [System.Runtime.InteropServices.DllImport("user32.dll")] private static extern IntPtr SendMessage(IntPtr hWnd, int msg, IntPtr wParam, IntPtr lParam);
        /// Real-window capture of hover, leave and click states: posts the
        /// mouse messages Windows would send and copies the composited screen.
        internal static int VisualProbe(string[] args)
        {
            string root = Path.GetDirectoryName(Application.ExecutablePath);
            string dir = Path.Combine(root, "local-research/launcher/probe");
            Directory.CreateDirectory(dir);
            foreach (var f in Directory.GetFiles(dir, "*.png")) File.Delete(f);
            using (var form = new LauncherForm(root))
            {
                form.StartPosition = FormStartPosition.Manual; form.Location = new System.Drawing.Point(40, 40); form.TopMost = true;
                form.Show(); Settle(500);
                int n = 0;
                Action<string> shot = name =>
                {
                    Settle(250);
                    var origin = form.PointToScreen(System.Drawing.Point.Empty);
                    using (var bmp = new System.Drawing.Bitmap(form.ClientSize.Width, form.ClientSize.Height))
                    {
                        using (var g = System.Drawing.Graphics.FromImage(bmp)) g.CopyFromScreen(origin, System.Drawing.Point.Empty, form.ClientSize);
                        bmp.Save(Path.Combine(dir, string.Format("{0:00}-{1}.png", n++, name)));
                    }
                };
                Func<string, Control> field = name => (Control)typeof(LauncherForm).GetField(name, System.Reflection.BindingFlags.NonPublic | System.Reflection.BindingFlags.Instance).GetValue(form);
                Func<Control, IntPtr> centre = c => (IntPtr)((c.Height / 2 << 16) | (c.Width / 2));
                shot("initial");
                foreach (var name in new[] { "logNav", "androidNav", "videoNav", "play", "rebuild", "save", "browse", "lab" })
                {
                    var c = field(name);
                    SendMessage(c.Handle, 0x0200, IntPtr.Zero, centre(c)); // WM_MOUSEMOVE
                    shot("hover-" + name);
                    if (name.EndsWith("Nav"))
                    {
                        SendMessage(c.Handle, 0x0201, (IntPtr)1, centre(c)); // WM_LBUTTONDOWN
                        shot("down-" + name);
                        SendMessage(c.Handle, 0x0202, IntPtr.Zero, centre(c)); // WM_LBUTTONUP
                        shot("clicked-" + name);
                    }
                    SendMessage(c.Handle, 0x02A3, IntPtr.Zero, IntPtr.Zero); // WM_MOUSELEAVE
                    shot("left-" + name);
                }
                form.Close();
            }
            return 0;
        }
        private static void Settle(int ms)
        {
            var until = DateTime.UtcNow.AddMilliseconds(ms);
            while (DateTime.UtcNow < until) { Application.DoEvents(); Thread.Sleep(15); }
        }
        internal static int SmokeGame()
        {
            string root = Path.GetDirectoryName(Application.ExecutablePath);
            string report = Path.Combine(root, "local-research/launcher/game-smoke.txt");
            try
            {
                var service = new LauncherService(root);
                string game = Installation.Detect(root);
                if (!service.Built || game == null) throw new InvalidOperationException("A built runtime and installed L4D2 are needed for this optional smoke check.");
                DateTime built = File.GetLastWriteTimeUtc(service.Binary);
                using (Process process = service.StartGame(game, false, "--capture-menu --capture-size \"960 540\" --capture-msaa 1 --capture-detail 1 --capture-frames 180 --capture-output \"local-research/launcher/game-menu.png\""))
                {
                    if (!process.WaitForExit(180000)) { process.Kill(); throw new Exception("Game smoke check timed out."); }
                    Assert(process.ExitCode == 0, "Actual game exited successfully");
                }
                Assert(File.Exists(Path.Combine(root, "local-research/launcher/game-menu.png")), "Actual game captured menu");
                Assert(File.GetLastWriteTimeUtc(service.Binary) == built, "Actual game launch reused runtime without rebuilding");
                Files.Write(report, "PASS actual runtime menu launched through launcher service; captured menu; reused existing EXE without rebuilding.\n");
                return 0;
            }
            catch (Exception error) { Files.Write(report, "FAIL " + error); return 1; }
        }
        internal static int Run()
        {
            string project = Path.GetDirectoryName(Application.ExecutablePath);
            string evidence = Path.Combine(project, "local-research/launcher"); Directory.CreateDirectory(evidence);
            string fixture = Path.Combine(Path.GetTempPath(), "L4D2R launcher test " + Guid.NewGuid().ToString("N"));
            Directory.CreateDirectory(fixture);
            string oldTarget = Environment.GetEnvironmentVariable("CARGO_TARGET_DIR");
            CultureInfo oldCulture = Thread.CurrentThread.CurrentCulture;
            var report = new System.Collections.Generic.List<string>();
            try
            {
                Environment.SetEnvironmentVariable("CARGO_TARGET_DIR", null);
                string game = Path.GetFullPath(Path.Combine(fixture, "Steam library/Left 4 Dead 2"));
                Files.Write(Path.Combine(game, "left4dead2.exe"), "fixture");
                Files.Write(Path.Combine(game, "left4dead2/pak01_dir.vpk"), "fixture");
                Assert(Installation.Resolve(game) == null, "Reject incomplete installation");
                Files.Write(Path.Combine(game, "left4dead2/maps/c1m1_hotel.bsp"), "fixture");
                Assert(Installation.Resolve(game) == game, "Resolve root");
                Assert(Installation.Resolve(Path.Combine(game, "left4dead2")) == game, "Resolve content folder");
                Assert(Installation.Resolve(Path.Combine(game, "left4dead2.exe")) == game, "Resolve executable");
                Assert(Installation.Resolve("\0invalid") == null, "Reject invalid path");
                Assert(Installation.Libraries("\"path\" \"D:\\\\Steam Library\"").Single() == "D:\\Steam Library", "Parse escaped Steam library path");
                report.Add("PASS installation validation and Steam library parsing");

                Thread.CurrentThread.CurrentCulture = CultureInfo.GetCultureInfo("fr-FR");
                var settings = new VideoSettings("brightness=NaN\ngrain=Infinity\nsamples=3\naspect=1\nresolution=900\nshader=-2\nfuture_key=kept\n", "camera_fov=103.5\nviewmodel_fov=61\nvsync=true\n");
                Assert(settings.Graphics["brightness"] == "1" && settings.Graphics["grain"] == "0", "Reject nonfinite settings");
                Assert(settings.Graphics["samples"] == "4" && settings.Graphics["resolution"] == "3" && settings.Graphics["shader"] == "0", "Clamp settings to actual game options");
                settings.Save(fixture); var loaded = VideoSettings.Load(fixture);
                Assert(loaded.Display["camera_fov"] == "103.5" && loaded.Display["viewmodel_fov"] == "61" && loaded.Graphics["future_key"] == "kept", "Preserve FOV/unknown values using invariant numbers");
                Assert(loaded.Display["vsync"] == "true", "Roundtrip VSync");
                report.Add("PASS video settings roundtrip, validation, culture and FOV preservation");

                Assert(LauncherService.Quote("a b\\") == "\"a b\\\\\"", "Quote trailing slash");
                Assert(LauncherService.Quote("x\"y") == "\"x\\\"y\"", "Quote embedded double quote");
                Files.Write(Path.Combine(fixture, "Cargo.toml"), "fixture");
                var service = new LauncherService(fixture);
                Assert(!service.Built, "Missing build requests first build");
                service.SaveGame(game);
                Assert(Installation.Detect(fixture) == game, "Saved installation rediscovered");
                service.BuildAsync(s => { }, Application.ExecutablePath, "--launcher-test-build").GetAwaiter().GetResult();
                Assert(service.Built, "First build installs runtime EXE");
                string buildLog = Files.Read(Path.Combine(fixture, "runtime/launcher-build.log"));
                Assert(buildLog.Contains("Build complete") && buildLog.Contains("test compiler diagnostic"), "Both build streams reach log");
                DateTime built = File.GetLastWriteTimeUtc(service.Binary);
                for (int i = 0; i < 2; i++)
                {
                    using (Process process = service.StartGame(game, i == 1, "--launcher-test-game")) { Assert(process.WaitForExit(10000) && process.ExitCode == 0, "Launch child"); }
                    string probe = Files.Read(Path.Combine(fixture, "runtime/launch-probe.txt"));
                    Assert(probe.StartsWith(fixture + "\n") && probe.EndsWith(game), "Correct cwd and quoted installation argument");
                    Assert(probe.Contains("--test-map") == (i == 1), "Menu/test map arguments");
                    Assert(File.GetLastWriteTimeUtc(service.Binary) == built, "Repeat play does not rebuild");
                }
                report.Add("PASS build/install and repeated menu/test-map launch without rebuilding");
                using (Process process = service.StartGame(game, false, "--launcher-test-game --wait"))
                {
                    Thread.Sleep(250);
                    Assert(service.Running(), "Recognize running runtime by normalized executable path");
                    try { service.StartGame(game, false); throw new Exception("Duplicate game launch accepted"); } catch (InvalidOperationException) { }
                    try { service.BuildAsync(s => { }, Application.ExecutablePath, "--launcher-test-build").GetAwaiter().GetResult(); throw new Exception("Build while playing accepted"); } catch (InvalidOperationException) { }
                    Assert(process.WaitForExit(10000), "Running game probe exited");
                }
                Assert(!service.Running(), "Unlock after game exits");
                report.Add("PASS running-game detection and duplicate-launch/rebuild guards");
                try { service.BuildAsync(s => { }, Application.ExecutablePath, "--launcher-test-build --fail").GetAwaiter().GetResult(); throw new Exception("Failed build accepted"); }
                catch (InvalidOperationException) { Assert(File.GetLastWriteTimeUtc(service.Binary) == built, "Failed rebuild preserves playable EXE"); }
                Task pending = service.BuildAsync(s => { }, Application.ExecutablePath, "--launcher-test-build --wait");
                Thread.Sleep(500); service.CancelBuild();
                try { pending.GetAwaiter().GetResult(); throw new Exception("Cancelled build accepted"); }
                catch (OperationCanceledException) { Assert(File.GetLastWriteTimeUtc(service.Binary) == built, "Cancelled rebuild preserves playable EXE"); }
                report.Add("PASS failed/cancelled rebuild preserves last good runtime");
                var apkLines = new System.Collections.Generic.List<string>();
                service.BuildApkAsync(t => { lock (apkLines) apkLines.Add(t); }, game, 1, true, Application.ExecutablePath, "--launcher-test-build").GetAwaiter().GetResult();
                string apkLog = Files.Read(Path.Combine(fixture, "runtime/launcher-apk.log"));
                Assert(apkLog.Contains("APK ready") && apkLog.Contains("test compiler diagnostic"), "APK build streams reach its log");
                try { service.BuildApkAsync(t => { }, game, 0, false, Application.ExecutablePath, "--launcher-test-build --fail").GetAwaiter().GetResult(); throw new Exception("Failed APK build accepted"); }
                catch (InvalidOperationException) { }
                Assert(LauncherService.ApkModes.Length == 3 && LauncherService.ApkModes[2] == "Bundle", "APK data modes match build-apk.ps1");
                report.Add("PASS Android APK build runs the script tool, logs both streams and reports failure");

                Environment.SetEnvironmentVariable("CARGO_TARGET_DIR", "custom target");
                Assert(service.BuildOutput() == Path.GetFullPath(Path.Combine(fixture, "custom target/release/l4d2-rewrite.exe")), "Relative Cargo target override");
                Environment.SetEnvironmentVariable("CARGO_TARGET_DIR", Path.Combine(fixture, "absolute target"));
                Assert(service.BuildOutput() == Path.GetFullPath(Path.Combine(fixture, "absolute target/release/l4d2-rewrite.exe")), "Absolute Cargo target override");
                Environment.SetEnvironmentVariable("CARGO_TARGET_DIR", null);
                report.Add("PASS Cargo target directory overrides");

                using (var form = new LauncherForm(fixture))
                {
                    form.Show(); Application.DoEvents(); form.SaveSettings();
                    var check = VideoSettings.Load(fixture);
                    Assert(check.Display["camera_fov"] == "103.5" && check.Graphics["future_key"] == "kept", "UI saves preserve game-only settings");
                    form.SavePreview(Path.Combine(evidence, "launcher-preview.png")); form.SavePagePreviews(evidence); form.Close();
                }
                report.Add("PASS actual WinForms creation, settings save and visual capture");
                report.Add("ALL TESTS PASSED");
                return 0;
            }
            catch (Exception error) { report.Add("FAIL " + error); return 1; }
            finally
            {
                Thread.CurrentThread.CurrentCulture = oldCulture;
                Environment.SetEnvironmentVariable("CARGO_TARGET_DIR", oldTarget);
                File.WriteAllLines(Path.Combine(evidence, "self-test.txt"), report, Files.Utf8);
                // Keep the unique fixture for diagnosing failures; never delete user paths.
                File.WriteAllText(Path.Combine(evidence, "fixture-path.txt"), fixture, Files.Utf8);
            }
        }
    }
}
