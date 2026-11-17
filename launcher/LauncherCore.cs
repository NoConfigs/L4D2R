using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Text;
using System.Text.RegularExpressions;
using System.Threading.Tasks;
using Microsoft.Win32;

namespace L4D2R.Launcher
{
    internal static class Files
    {
        internal static readonly Encoding Utf8 = new UTF8Encoding(false);
        internal static string Read(string path) { return File.Exists(path) ? File.ReadAllText(path, Utf8) : ""; }
        internal static void Write(string path, string text)
        {
            Directory.CreateDirectory(Path.GetDirectoryName(path));
            string temporary = path + ".launcher-tmp";
            File.WriteAllText(temporary, text, Utf8);
            try
            {
                if (File.Exists(path)) File.Replace(temporary, path, null);
                else File.Move(temporary, path);
            }
            finally { if (File.Exists(temporary)) File.Delete(temporary); }
        }
    }

    internal sealed class VideoSettings
    {
        internal static readonly string[][] Resolutions = {
            new[] { "1280 x 720", "1600 x 900", "1920 x 1080", "2560 x 1440", "3840 x 2160" },
            new[] { "1024 x 768", "1280 x 960", "1600 x 1200", "1920 x 1440" },
            new[] { "1280 x 800", "1440 x 900", "1680 x 1050", "1920 x 1200", "2560 x 1600" }
        };
        internal static readonly int[] Samples = { 1, 2, 4, 8 };
        internal static readonly int[] Filters = { 0, 1, 2, 4, 8, 16 };
        internal readonly Dictionary<string, string> Graphics, Display;
        internal VideoSettings(string graphics, string display)
        {
            Graphics = Parse(graphics); Display = Parse(display);
            Set(Graphics, "brightness", Number(Graphics, "brightness", 1, .5, 1.5));
            Set(Graphics, "grain", Number(Graphics, "grain", 0, 0, 1));
            Set(Graphics, "samples", Choice(Graphics, "samples", 4, Samples));
            Set(Graphics, "filtering", Choice(Graphics, "filtering", 8, Filters));
            foreach (string key in new[] { "shader", "effects", "detail" }) Set(Graphics, key, Index(Graphics, key, 2, 2));
            int aspect = Index(Graphics, "aspect", 0, 2);
            Set(Graphics, "aspect", aspect);
            Set(Graphics, "resolution", Index(Graphics, "resolution", 0, Resolutions[aspect].Length - 1));
            Set(Graphics, "display", Index(Graphics, "display", 0, 2));
            Set(Display, "camera_fov", Number(Display, "camera_fov", 90, 1.01, 178.99));
            Set(Display, "viewmodel_fov", Number(Display, "viewmodel_fov", 51, 1.01, 178.99));
            foreach (string key in new[] { "vsync", "show_fps" }) Display[key] = Bool(Display, key) ? "true" : "false";
        }
        internal static VideoSettings Load(string root)
        {
            return new VideoSettings(Files.Read(Path.Combine(root, "runtime/graphics-options.ini")), Files.Read(Path.Combine(root, "runtime/display-options.ini")));
        }
        internal static Dictionary<string, string> Parse(string text)
        {
            var result = new Dictionary<string, string>(StringComparer.Ordinal);
            foreach (string line in text.Split('\n'))
            {
                int at = line.IndexOf('=');
                if (at > 0 && !result.ContainsKey(line.Substring(0, at))) result[line.Substring(0, at)] = line.Substring(at + 1).Trim();
            }
            return result;
        }
        internal static double Number(Dictionary<string, string> values, string key, double fallback, double min, double max)
        {
            string raw; double value;
            return values.TryGetValue(key, out raw) && Double.TryParse(raw, NumberStyles.Float, CultureInfo.InvariantCulture, out value) && !Double.IsNaN(value) && !Double.IsInfinity(value)
                ? Math.Max(min, Math.Min(max, value)) : fallback;
        }
        internal static int Index(Dictionary<string, string> values, string key, int fallback, int max) { return (int)Number(values, key, fallback, 0, max); }
        internal static int Choice(Dictionary<string, string> values, string key, int fallback, int[] choices)
        {
            int n = (int)Number(values, key, fallback, 0, 100);
            return choices.Contains(n) ? n : fallback;
        }
        internal static bool Bool(Dictionary<string, string> values, string key) { string value; return values.TryGetValue(key, out value) && value == "true"; }
        internal static void Set(Dictionary<string, string> values, string key, double value) { values[key] = value.ToString("0.########", CultureInfo.InvariantCulture); }
        internal static string Encode(Dictionary<string, string> values) { return String.Join("\n", values.Select(p => p.Key + "=" + p.Value)) + "\n"; }
        internal void Save(string root)
        {
            Files.Write(Path.Combine(root, "runtime/graphics-options.ini"), Encode(Graphics));
            Files.Write(Path.Combine(root, "runtime/display-options.ini"), Encode(Display));
        }
    }

    internal static class Installation
    {
        internal static string Resolve(string candidate)
        {
            try
            {
                if (String.IsNullOrWhiteSpace(candidate)) return null;
                string root = Path.GetFullPath(candidate.Trim().Trim('"').TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar));
                if (File.Exists(root) && String.Equals(Path.GetFileName(root), "left4dead2.exe", StringComparison.OrdinalIgnoreCase)) root = Path.GetDirectoryName(root);
                if (String.Equals(Path.GetFileName(root), "left4dead2", StringComparison.OrdinalIgnoreCase)) root = Path.GetDirectoryName(root);
                return new[] { "left4dead2.exe", "left4dead2/pak01_dir.vpk", "left4dead2/maps/c1m1_hotel.bsp" }.All(p => File.Exists(Path.Combine(root, p))) ? root : null;
            }
            catch (ArgumentException) { return null; }
            catch (IOException) { return null; }
            catch (UnauthorizedAccessException) { return null; }
            catch (NotSupportedException) { return null; }
        }
        internal static IEnumerable<string> Libraries(string vdf)
        {
            foreach (Match match in Regex.Matches(vdf, "\"path\"\\s+\"([^\"\\r\\n]+)\"", RegexOptions.IgnoreCase))
                yield return match.Groups[1].Value.Replace("\\\\", "\\");
        }
        internal static string Detect(string project)
        {
            string saved = Resolve(Files.Read(Path.Combine(project, "runtime/game-dir.txt")));
            if (saved != null) return saved;
            var steam = new List<string>();
            using (RegistryKey key = Registry.CurrentUser.OpenSubKey("Software\\Valve\\Steam"))
            {
                if (key != null && key.GetValue("SteamPath") is string) steam.Add((string)key.GetValue("SteamPath"));
            }
            steam.Add(Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFilesX86), "Steam"));
            steam.Add(Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles), "Steam"));
            var libraries = new List<string>(steam);
            foreach (string root in steam.Distinct(StringComparer.OrdinalIgnoreCase))
            {
                string vdf = Path.Combine(root, "steamapps/libraryfolders.vdf");
                try { libraries.AddRange(Libraries(Files.Read(vdf))); } catch (IOException) { }
            }
            return libraries.Select(p => Resolve(Path.Combine(p, "steamapps/common/Left 4 Dead 2"))).FirstOrDefault(p => p != null);
        }
    }

    internal sealed class LauncherService
    {
        internal readonly string Root;
        internal string Binary { get { return Path.GetFullPath(Path.Combine(Root, "runtime/l4d2-rewrite.exe")); } }
        internal bool Built { get { return File.Exists(Binary) && new FileInfo(Binary).Length > 0; } }
        internal Process BuildProcess { get; private set; }
        internal bool Cancelled { get; private set; }
        internal LauncherService(string root) { Root = Path.GetFullPath(root); }
        internal static string Quote(string value)
        {
            // CommandLineToArgvW / MSVC escaping, including trailing backslashes.
            var b = new StringBuilder("\""); int slashes = 0;
            foreach (char c in value)
            {
                if (c == '\\') { slashes++; continue; }
                if (c == '"') b.Append('\\', slashes * 2 + 1).Append(c);
                else b.Append('\\', slashes).Append(c);
                slashes = 0;
            }
            return b.Append('\\', slashes * 2).Append('"').ToString();
        }
        internal static string FindTool(string name)
        {
            var dirs = new List<string>();
            string cargoHome = Environment.GetEnvironmentVariable("CARGO_HOME");
            if (!String.IsNullOrEmpty(cargoHome)) dirs.Add(Path.Combine(cargoHome, "bin"));
            dirs.Add(Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), ".cargo/bin"));
            dirs.AddRange((Environment.GetEnvironmentVariable("PATH") ?? "").Split(Path.PathSeparator));
            foreach (string dir in dirs)
            {
                try { string path = Path.Combine(dir.Trim('"'), name + ".exe"); if (File.Exists(path)) return path; }
                catch (ArgumentException) { }
            }
            return null;
        }
        internal bool Running()
        {
            foreach (Process process in Process.GetProcessesByName("l4d2-rewrite"))
                using (process)
                {
                    try { if (String.Equals(process.MainModule.FileName, Binary, StringComparison.OrdinalIgnoreCase)) return true; }
                    catch (System.ComponentModel.Win32Exception) { }
                    catch (InvalidOperationException) { }
                }
            return false;
        }
        internal void SaveGame(string game)
        {
            string valid = Installation.Resolve(game);
            if (valid == null) throw new InvalidOperationException("Select a complete Left 4 Dead 2 installation first.");
            Files.Write(Path.Combine(Root, "runtime/game-dir.txt"), valid);
        }
        internal string BuildOutput()
        {
            string target = Environment.GetEnvironmentVariable("CARGO_TARGET_DIR");
            if (String.IsNullOrWhiteSpace(target)) target = "target";
            if (!Path.IsPathRooted(target)) target = Path.Combine(Root, target);
            return Path.GetFullPath(Path.Combine(target, "release/l4d2-rewrite.exe"));
        }
        internal async Task BuildAsync(Action<string> log, string testTool = null, string testArguments = null)
        {
            if (BuildProcess != null) throw new InvalidOperationException("A build is already running.");
            if (Running()) throw new InvalidOperationException("Close the game before rebuilding it.");
            if (!File.Exists(Path.Combine(Root, "Cargo.toml"))) throw new InvalidOperationException("Keep L4D2R Launcher.exe alongside Cargo.toml and the rewrite source folders.");
            string tool = testTool ?? FindTool("cargo");
            if (tool == null) throw new InvalidOperationException("The first build needs Rust 1.95 or newer and Visual Studio C++ Build Tools with a Windows SDK. Install those, then reopen this launcher. Later launches do not need to build again.");
            Cancelled = false;
            Directory.CreateDirectory(Path.Combine(Root, "runtime"));
            Directory.CreateDirectory(Path.Combine(Root, "local-research"));
            string logPath = Path.Combine(Root, "runtime/launcher-build.log");
            var logLock = new object();
            using (var writer = new StreamWriter(logPath, false, Files.Utf8))
            using (var process = new Process())
            {
                Action<string> line = text => { if (text == null) return; lock (logLock) { writer.WriteLine(text); writer.Flush(); } log(text); };
                process.StartInfo = new ProcessStartInfo(tool, testArguments ?? "build --locked --release --bin l4d2-rewrite") {
                    WorkingDirectory = Root, UseShellExecute = false, CreateNoWindow = true,
                    RedirectStandardOutput = true, RedirectStandardError = true
                };
                process.StartInfo.EnvironmentVariables["CARGO_TERM_COLOR"] = "never";
                process.OutputDataReceived += (s, e) => line(e.Data);
                process.ErrorDataReceived += (s, e) => line(e.Data);
                BuildProcess = process;
                try
                {
                    line("Building L4D2R. First builds download dependencies and may take several minutes.");
                    process.Start(); process.BeginOutputReadLine(); process.BeginErrorReadLine();
                    await Task.Run(() => process.WaitForExit());
                    if (Cancelled) throw new OperationCanceledException("Build cancelled. Your previous playable build is kept.");
                    if (process.ExitCode != 0) throw new InvalidOperationException("Build failed. See the Build log tab or runtime/launcher-build.log. Check Rust 1.95+, C++ Build Tools and the Windows SDK are installed; dependency downloads need internet on the first build.");
                    string output = BuildOutput();
                    if (!File.Exists(output) || new FileInfo(output).Length == 0) throw new IOException("Cargo finished without creating " + output);
                    string temporary = Binary + ".launcher-tmp";
                    try
                    {
                        File.Copy(output, temporary, true);
                        if (File.Exists(Binary)) File.Replace(temporary, Binary, null);
                        else File.Move(temporary, Binary);
                    }
                    finally { if (File.Exists(temporary)) File.Delete(temporary); }
                    // Steamworks: the exe loads steam_api64.dll from its own
                    // folder; Cargo leaves it in steamworks-sys's build output.
                    string builds = Path.Combine(Path.GetDirectoryName(output), "build");
                    FileInfo steamDll = Directory.Exists(builds) ? new DirectoryInfo(builds).GetFiles("steam_api64.dll", SearchOption.AllDirectories).OrderByDescending(f => f.LastWriteTimeUtc).FirstOrDefault() : null;
                    if (steamDll != null) File.Copy(steamDll.FullName, Path.Combine(Path.GetDirectoryName(Binary), "steam_api64.dll"), true);
                    else line("Warning: steam_api64.dll was not found in the build output; the game may not start.");
                    line("Build complete. Future launches use this saved EXE.");
                }
                finally { BuildProcess = null; }
            }
        }
        /// Android APK modes (tools/build-apk.ps1 -GameData).
        internal static readonly string[] ApkModes = { "None", "Push", "Bundle" };
        internal string ApkOutput { get { return Path.Combine(Root, "artifacts\\android\\L4D2Rewrite.apk"); } }
        /// Runs tools/build-apk.ps1, which installs the Android toolchain on first use.
        internal async Task BuildApkAsync(Action<string> log, string game, int mode, bool install, string testTool = null, string testArguments = null)
        {
            if (BuildProcess != null) throw new InvalidOperationException("A build is already running.");
            string script = Path.Combine(Root, "tools\\build-apk.ps1");
            if (testTool == null && !File.Exists(script)) throw new InvalidOperationException("tools\\build-apk.ps1 is missing beside the launcher.");
            string resolved = Installation.Resolve(game);
            if (resolved == null) throw new InvalidOperationException("Select a complete Left 4 Dead 2 installation first.");
            Cancelled = false;
            Directory.CreateDirectory(Path.Combine(Root, "runtime"));
            string logPath = Path.Combine(Root, "runtime/launcher-apk.log");
            var logLock = new object();
            string arguments = testArguments ?? ("-NoProfile -ExecutionPolicy Bypass -File " + Quote(script) + " -GameData " + ApkModes[mode]
                + " -GameDir " + Quote(resolved) + (install ? " -Install" : ""));
            using (var writer = new StreamWriter(logPath, false, Files.Utf8))
            using (var process = new Process())
            {
                Action<string> line = text => { if (text == null) return; lock (logLock) { writer.WriteLine(text); writer.Flush(); } log(text); };
                process.StartInfo = new ProcessStartInfo(testTool ?? "powershell.exe", arguments) {
                    WorkingDirectory = Root, UseShellExecute = false, CreateNoWindow = true,
                    RedirectStandardOutput = true, RedirectStandardError = true
                };
                process.StartInfo.EnvironmentVariables["CARGO_TERM_COLOR"] = "never";
                process.OutputDataReceived += (s, e) => line(e.Data);
                process.ErrorDataReceived += (s, e) => line(e.Data);
                BuildProcess = process;
                try
                {
                    line("Building the Android APK. The first build installs the Android toolchain (~4 GB) and compiles the game for arm64, which takes a while.");
                    process.Start(); process.BeginOutputReadLine(); process.BeginErrorReadLine();
                    await Task.Run(() => process.WaitForExit());
                    if (Cancelled) throw new OperationCanceledException("APK build cancelled.");
                    if (process.ExitCode != 0) throw new InvalidOperationException("APK build failed. See the Build activity tab or runtime/launcher-apk.log.");
                    line("APK ready: " + ApkOutput);
                }
                finally { BuildProcess = null; }
            }
        }
        internal void CancelBuild()
        {
            Process process = BuildProcess;
            if (process == null) return;
            Cancelled = true;
            try
            {
                // Kill only this launcher's compiler process tree, not other Cargo jobs.
                Process.Start(new ProcessStartInfo("taskkill.exe", "/PID " + process.Id + " /T /F") { UseShellExecute = false, CreateNoWindow = true });
            }
            catch (InvalidOperationException) { }
        }
        internal Process StartGame(string game, bool lab, string testArgument = null)
        {
            if (!Built) throw new InvalidOperationException("Build the rewrite first.");
            if (Running()) throw new InvalidOperationException("L4D2R is already running.");
            SaveGame(game);
            Directory.CreateDirectory(Path.Combine(Root, "local-research/parity"));
            string arguments = (testArgument == null ? "" : testArgument + " ") + (lab ? "--test-map " : "") + "--game-dir " + Quote(Installation.Resolve(game));
            return Process.Start(new ProcessStartInfo(Binary, arguments) { WorkingDirectory = Root, UseShellExecute = false, CreateNoWindow = true });
        }
    }
}
