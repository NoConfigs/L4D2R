using System;
using System.Drawing;
using System.IO;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;
using System.Windows.Forms;

namespace L4D2R.Launcher
{
    internal static class Program
    {
        [STAThread]
        private static int Main(string[] args)
        {
            if (args.Contains("--launcher-test-build") || args.Contains("--launcher-test-game")) return LauncherTests.Child(args);
            Application.EnableVisualStyles(); Application.SetCompatibleTextRenderingDefault(false);
            if (args.Contains("--self-test")) return LauncherTests.Run();
            if (args.Contains("--smoke-game")) return LauncherTests.SmokeGame();
            if (args.Contains("--visual-probe")) return LauncherTests.VisualProbe(args);
            string root = Path.GetDirectoryName(Application.ExecutablePath);
            // A local mutex prevents two launchers from compiling/replacing the same build.
            string identity = Convert.ToBase64String(System.Security.Cryptography.SHA256.Create().ComputeHash(System.Text.Encoding.UTF8.GetBytes(root.ToUpperInvariant()))).Replace('/', '_');
            bool first;
            using (var mutex = new Mutex(true, "Local\\L4D2RLauncher_" + identity, out first))
            {
                if (!first) { MessageBox.Show("L4D2R Launcher is already open.", "L4D2R"); return 0; }
                Application.Run(new LauncherForm(root));
            }
            return 0;
        }
    }

    internal sealed class LauncherForm : Form
    {
        private readonly LauncherService service;
        private VideoSettings video;
        private readonly TextBox game = new TextBox();
        private readonly ModernButton browse = new ModernButton(), detect = new ModernButton(), play = new ModernButton(), rebuild = new ModernButton(), save = new ModernButton(), cancel = new ModernButton();
        private readonly ModernButton videoNav = new ModernButton(), logNav = new ModernButton(), androidNav = new ModernButton(), buildApk = new ModernButton();
        private readonly ComboBox apkMode = new ChoiceBox();
        private readonly CheckBox apkInstall = new ModernSwitch();
        private readonly Panel androidPage = new Panel();
        private readonly CheckBox lab = new ModernSwitch(), vsync = new ModernSwitch(), fps = new ModernSwitch();
        private readonly Label status = new Label();
        private readonly RichTextBox log = new RichTextBox();
        private readonly Panel tabs = new Panel();
        private readonly Panel videoPage = new Panel(), logPage = new Panel();
        private readonly ComboBox aspect = new ChoiceBox(), resolution = new ChoiceBox(), display = new ChoiceBox(), aa = new ChoiceBox(), filtering = new ChoiceBox(), shader = new ChoiceBox(), effects = new ChoiceBox(), detail = new ChoiceBox();
        private readonly PercentSlider brightness = new PercentSlider(), grain = new PercentSlider();
        private readonly System.Windows.Forms.Timer timer = new System.Windows.Forms.Timer();
        private bool busy, wasRunning, loading, closingAfterCancel;

        internal LauncherForm(string root)
        {
            service = new LauncherService(root);
            Text = "L4D2R Launcher"; StartPosition = FormStartPosition.CenterScreen;
            ClientSize = new Size(1100, 820); MinimumSize = new Size(1040, 780);
            FormBorderStyle = FormBorderStyle.None; Padding = new Padding(1);
            AutoScaleMode = AutoScaleMode.Dpi; BackColor = Theme.Border; ForeColor = Theme.Text;
            Font = Theme.Body(10); DoubleBuffered = true;
            // The exe's icon (launcher/app.ico) also shows in the taskbar.
            try { Icon = Icon.ExtractAssociatedIcon(Application.ExecutablePath); } catch (ArgumentException) { }
            var frame = new TableLayoutPanel { Dock = DockStyle.Fill, ColumnCount = 1, RowCount = 2, Margin = Padding.Empty, BackColor = Theme.Background };
            frame.RowStyles.Add(new RowStyle(SizeType.Absolute, 34)); frame.RowStyles.Add(new RowStyle(SizeType.Percent, 100)); Controls.Add(frame);

            // Title strip.
            var caption = new GrainPanel { Dock = DockStyle.Fill, Base = Theme.Sidebar, Margin = Padding.Empty };
            var captionText = new Label { Text = "L4D2R  ·  LAUNCHER", Font = Theme.Display(9.5f), ForeColor = Theme.Disabled, AutoSize = false, Dock = DockStyle.Fill, Padding = new Padding(18, 9, 0, 0), BackColor = Color.Transparent };
            caption.Controls.Add(captionText);
            var close = new ModernButton { Text = "✕", Width = 46, Dock = DockStyle.Right, Kind = ButtonKind.CaptionClose, Font = new Font("Segoe UI", 10), AccessibleName = "Close launcher" };
            var minimize = new ModernButton { Text = "—", Width = 46, Dock = DockStyle.Right, Kind = ButtonKind.Caption, Font = new Font("Segoe UI", 10), AccessibleName = "Minimize launcher" };
            close.Click += (s, e) => Close(); minimize.Click += (s, e) => WindowState = FormWindowState.Minimized;
            caption.Controls.Add(minimize); caption.Controls.Add(close); frame.Controls.Add(caption, 0, 0);
            caption.MouseDown += DragWindow; captionText.MouseDown += DragWindow;

            var body = new TableLayoutPanel { Dock = DockStyle.Fill, ColumnCount = 2, RowCount = 1, Margin = Padding.Empty };
            body.ColumnStyles.Add(new ColumnStyle(SizeType.Absolute, 300)); body.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100)); frame.Controls.Add(body, 0, 1);

            // Left column: the logo, then the menu in big condensed capitals.
            var sidebar = new GrainPanel { Dock = DockStyle.Fill, Base = Theme.Sidebar, Margin = Padding.Empty };
            var logo = new PictureBox { Location = new Point(26, 24), Size = new Size(248, 151), SizeMode = PictureBoxSizeMode.Zoom, BackColor = Color.Transparent, Image = LoadLogo() };
            sidebar.Controls.Add(logo);
            videoNav.Text = "Video"; logNav.Text = "Build log"; androidNav.Text = "Android";
            int navY = 226;
            foreach (var nav in new[] { videoNav, logNav, androidNav })
            {
                nav.Kind = ButtonKind.Nav; nav.Font = Theme.Display(21); nav.Size = new Size(286, 50); nav.Location = new Point(0, navY); navY += 54;
                sidebar.Controls.Add(nav);
            }
            videoNav.Click += (s, e) => SelectPage(videoPage); logNav.Click += (s, e) => SelectPage(logPage); androidNav.Click += (s, e) => SelectPage(androidPage);
            var sidebarFoot = new Label { Text = "LOCAL BUILD  ·  WINDOWS", Font = Theme.Display(9), ForeColor = Theme.Disabled, Dock = DockStyle.Bottom, Height = 46, Padding = new Padding(20, 0, 0, 0), BackColor = Color.Transparent };
            sidebar.Controls.Add(sidebarFoot); body.Controls.Add(sidebar, 0, 0);
            sidebar.Paint += (s, e) => { using (var pen = new Pen(Theme.Rule)) e.Graphics.DrawLine(pen, sidebar.Width - 1, 0, sidebar.Width - 1, sidebar.Height); };

            // Main column.
            var main = new GrainPanel { Dock = DockStyle.Fill, Base = Theme.Background, Margin = Padding.Empty };
            body.Controls.Add(main, 1, 0);
            var layout = new TableLayoutPanel { Dock = DockStyle.Fill, Padding = new Padding(34, 24, 34, 20), ColumnCount = 1, RowCount = 5, Margin = Padding.Empty, BackColor = Color.Transparent };
            layout.RowStyles.Add(new RowStyle(SizeType.Absolute, 44)); layout.RowStyles.Add(new RowStyle(SizeType.Absolute, 46)); layout.RowStyles.Add(new RowStyle(SizeType.Absolute, 26)); layout.RowStyles.Add(new RowStyle(SizeType.Percent, 100)); layout.RowStyles.Add(new RowStyle(SizeType.Absolute, 104));
            main.Controls.Add(layout);
            layout.Controls.Add(new SectionTitle("Game files") { Dock = DockStyle.Fill, Margin = Padding.Empty }, 0, 0);
            var path = new TableLayoutPanel { Dock = DockStyle.Fill, ColumnCount = 3, RowCount = 1, Margin = Padding.Empty, BackColor = Color.Transparent };
            path.RowStyles.Add(new RowStyle(SizeType.Percent, 100));
            path.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100)); path.ColumnStyles.Add(new ColumnStyle(SizeType.Absolute, 112)); path.ColumnStyles.Add(new ColumnStyle(SizeType.Absolute, 132));
            var pathField = new Panel { Dock = DockStyle.Fill, BackColor = Theme.Input, Padding = new Padding(12, 13, 8, 4), Margin = new Padding(0, 0, 8, 0) };
            pathField.Paint += (s, e) => { using (var pen = new Pen(game.Focused ? Theme.Accent : Theme.Border)) e.Graphics.DrawRectangle(pen, 0, 0, pathField.Width - 1, pathField.Height - 1); };
            game.Dock = DockStyle.Fill; game.BorderStyle = BorderStyle.None; game.BackColor = Theme.Input; game.ForeColor = Theme.Text; game.Font = Theme.Body(10); pathField.Controls.Add(game);
            game.GotFocus += (s, e) => pathField.Invalidate(); game.LostFocus += (s, e) => pathField.Invalidate();
            ButtonStyle(browse, "Browse"); ButtonStyle(detect, "Auto-detect"); browse.Margin = new Padding(0, 0, 8, 0); detect.Margin = Padding.Empty;
            path.Controls.Add(pathField, 0, 0); path.Controls.Add(browse, 1, 0); path.Controls.Add(detect, 2, 0); layout.Controls.Add(path, 0, 1);

            tabs.Dock = DockStyle.Fill; tabs.BackColor = Color.Transparent; tabs.Margin = Padding.Empty;
            videoPage.Dock = logPage.Dock = androidPage.Dock = DockStyle.Fill; videoPage.BackColor = logPage.BackColor = androidPage.BackColor = Color.Transparent;
            tabs.Controls.Add(androidPage); tabs.Controls.Add(logPage); tabs.Controls.Add(videoPage);
            layout.Controls.Add(tabs, 0, 3);

            // Android page.
            var androidLayout = new TableLayoutPanel { Dock = DockStyle.Fill, ColumnCount = 2, RowCount = 5, Margin = Padding.Empty, BackColor = Color.Transparent };
            androidLayout.ColumnStyles.Add(new ColumnStyle(SizeType.Absolute, 170)); androidLayout.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100));
            androidLayout.RowStyles.Add(new RowStyle(SizeType.Absolute, 48)); androidLayout.RowStyles.Add(new RowStyle(SizeType.Absolute, 104)); androidLayout.RowStyles.Add(new RowStyle(SizeType.Absolute, 48)); androidLayout.RowStyles.Add(new RowStyle(SizeType.Absolute, 44)); androidLayout.RowStyles.Add(new RowStyle(SizeType.Percent, 100));
            var androidTitle = new SectionTitle("Android build") { Dock = DockStyle.Fill, Margin = Padding.Empty }; androidLayout.Controls.Add(androidTitle, 0, 0); androidLayout.SetColumnSpan(androidTitle, 2);
            var androidInfo = new Label { Dock = DockStyle.Fill, Font = Theme.Body(10), ForeColor = Theme.Muted, BackColor = Color.Transparent, Padding = new Padding(0, 8, 0, 0), Text =
                "Builds artifacts\\android\\L4D2Rewrite.apk (arm64, touch controls). The first build installs the Android SDK, NDK and a JDK in your user folder (about 4 GB) and accepts the Android SDK license.\n\n" +
                "APK only leaves the game data out. Push copies your Left 4 Dead 2 folders to the phone over USB (USB debugging on). Bundle packs them into the APK, which only works under 4 GB." };
            androidLayout.Controls.Add(androidInfo, 0, 1); androidLayout.SetColumnSpan(androidInfo, 2);
            Combo(apkMode, new[] { "APK only", "Push data to phone (USB)", "Bundle data into APK" }); apkMode.SelectedIndex = 0; apkMode.AccessibleName = "Android game data";
            androidLayout.Controls.Add(FieldLabel("Game data"), 0, 2); androidLayout.Controls.Add(apkMode, 1, 2);
            apkInstall.Text = "Install on the connected phone"; apkInstall.Dock = DockStyle.Fill; androidLayout.Controls.Add(apkInstall, 0, 3); androidLayout.SetColumnSpan(apkInstall, 2);
            ButtonStyle(buildApk, "Build APK"); buildApk.Kind = ButtonKind.Primary; buildApk.Dock = DockStyle.None; buildApk.Size = new Size(180, 46); buildApk.Font = Theme.Display(15); buildApk.Margin = new Padding(0, 12, 0, 0);
            var apkActions = new FlowLayoutPanel { Dock = DockStyle.Fill, Margin = Padding.Empty, BackColor = Color.Transparent }; apkActions.Controls.Add(buildApk); androidLayout.Controls.Add(apkActions, 0, 4); androidLayout.SetColumnSpan(apkActions, 2);
            androidPage.Controls.Add(androidLayout);
            buildApk.Click += async (s, e) => await BuildApk();

            // Video page.
            var videoLayout = new TableLayoutPanel { Dock = DockStyle.Fill, ColumnCount = 1, RowCount = 3, Margin = Padding.Empty, BackColor = Color.Transparent };
            videoLayout.RowStyles.Add(new RowStyle(SizeType.Absolute, 48)); videoLayout.RowStyles.Add(new RowStyle(SizeType.Percent, 100)); videoLayout.RowStyles.Add(new RowStyle(SizeType.Absolute, 26));
            videoLayout.Controls.Add(new SectionTitle("Video") { Dock = DockStyle.Fill, Margin = Padding.Empty }, 0, 0);
            var grid = new TableLayoutPanel { Dock = DockStyle.Fill, ColumnCount = 4, RowCount = 6, Margin = Padding.Empty, BackColor = Color.Transparent };
            for (int i = 0; i < 4; i++) grid.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, i % 2 == 0 ? 20 : 30));
            for (int i = 0; i < 6; i++) grid.RowStyles.Add(new RowStyle(SizeType.Percent, 100f / 6));
            videoLayout.Controls.Add(grid, 0, 1); videoPage.Controls.Add(videoLayout);
            Combo(aspect, new[] { "Widescreen 16:9", "Normal 4:3", "Widescreen 16:10" });
            Combo(resolution, VideoSettings.Resolutions[0]); Combo(display, new[] { "Windowed", "Borderless", "Fullscreen" });
            Combo(aa, new[] { "None", "2x MSAA", "4x MSAA", "8x MSAA" });
            Combo(filtering, new[] { "Bilinear", "Trilinear", "Anisotropic 2x", "Anisotropic 4x", "Anisotropic 8x", "Anisotropic 16x" });
            foreach (var box in new[] { shader, effects, detail }) Combo(box, new[] { "Low", "Medium", "High" });
            foreach (var number in new[] { brightness, grain }) { number.Minimum = 0; number.Maximum = 150; number.Increment = 5; number.Anchor = AnchorStyles.Left | AnchorStyles.Right; number.Height = 34; number.Margin = new Padding(0, 4, 0, 4); }
            brightness.Minimum = 50; grain.Maximum = 100;
            Row(grid, 0, "Aspect ratio", aspect, "Resolution", resolution);
            Row(grid, 1, "Display mode", display, "Anti-aliasing", aa);
            Row(grid, 2, "Brightness", brightness, "Film grain", grain);
            Row(grid, 3, "Filtering", filtering, "Shader detail", shader);
            Row(grid, 4, "Effect detail", effects, "Model / texture", detail);
            vsync.Text = "Vertical sync"; fps.Text = "Show FPS"; vsync.Dock = fps.Dock = DockStyle.Fill;
            grid.Controls.Add(vsync, 0, 5); grid.SetColumnSpan(vsync, 2); grid.Controls.Add(fps, 2, 5); grid.SetColumnSpan(fps, 2);
            videoLayout.Controls.Add(new Label { Text = "Applies on the next launch. MSAA falls back to the highest level your GPU supports.", Dock = DockStyle.Fill, Font = Theme.Body(9), ForeColor = Theme.Disabled, BackColor = Color.Transparent, TextAlign = ContentAlignment.BottomLeft }, 0, 2);

            // Build log page.
            var logLayout = new TableLayoutPanel { Dock = DockStyle.Fill, ColumnCount = 1, RowCount = 2, Margin = Padding.Empty, BackColor = Color.Transparent };
            logLayout.RowStyles.Add(new RowStyle(SizeType.Absolute, 48)); logLayout.RowStyles.Add(new RowStyle(SizeType.Percent, 100));
            logLayout.Controls.Add(new SectionTitle("Build log") { Dock = DockStyle.Fill, Margin = Padding.Empty }, 0, 0);
            var logFrame = new Panel { Dock = DockStyle.Fill, BackColor = Theme.Card, Padding = new Padding(12, 10, 6, 8), Margin = new Padding(0, 6, 0, 0) };
            logFrame.Paint += (s, e) => { using (var pen = new Pen(Theme.Border)) e.Graphics.DrawRectangle(pen, 0, 0, logFrame.Width - 1, logFrame.Height - 1); };
            log.Dock = DockStyle.Fill; log.ReadOnly = true; log.BackColor = Theme.Card; log.ForeColor = Theme.Muted; log.BorderStyle = BorderStyle.None; log.Font = new Font("Consolas", 9);
            log.Text = "Build output appears here.\n\nThe saved build is reused each time you play. Rebuild compiles updated source.";
            logFrame.Controls.Add(log); logLayout.Controls.Add(logFrame, 0, 1); logPage.Controls.Add(logLayout);

            // Footer: status line, then the actions with PLAY on the right.
            var footer = new TableLayoutPanel { Dock = DockStyle.Fill, ColumnCount = 1, RowCount = 2, Margin = Padding.Empty, Padding = new Padding(0, 10, 0, 0), BackColor = Color.Transparent };
            footer.RowStyles.Add(new RowStyle(SizeType.Absolute, 34)); footer.RowStyles.Add(new RowStyle(SizeType.Percent, 100));
            footer.Paint += (s, e) => Theme.HairLine(e.Graphics, 0, 2, footer.Width);
            status.Dock = DockStyle.Fill; status.Padding = new Padding(0, 8, 0, 0); status.Font = Theme.Body(10); status.AutoEllipsis = true; status.BackColor = Color.Transparent; footer.Controls.Add(status, 0, 0);
            var actions = new FlowLayoutPanel { Dock = DockStyle.Fill, FlowDirection = FlowDirection.RightToLeft, WrapContents = false, Margin = Padding.Empty, BackColor = Color.Transparent };
            ButtonStyle(play, "Play"); play.Kind = ButtonKind.Primary; play.Width = 220; play.Height = 54; play.Font = Theme.Display(22);
            ButtonStyle(save, "Save settings"); save.Width = 140; ButtonStyle(rebuild, "Rebuild"); rebuild.Width = 110; ButtonStyle(cancel, "Cancel build"); cancel.Width = 140; cancel.Visible = false;
            foreach (var b in new[] { save, rebuild, cancel }) { b.Height = 54; }
            lab.Text = "Test map"; lab.Width = 130; lab.Height = 54;
            foreach (Control c in new Control[] { play, save, rebuild, cancel, lab }) { c.Margin = new Padding(10, 0, 0, 0); actions.Controls.Add(c); } footer.Controls.Add(actions, 0, 1); layout.Controls.Add(footer, 0, 4);
            SelectPage(videoPage);
            aspect.SelectedIndexChanged += (s, e) => { if (loading || aspect.SelectedIndex < 0) return; int previous = resolution.SelectedIndex; resolution.Items.Clear(); resolution.Items.AddRange(VideoSettings.Resolutions[aspect.SelectedIndex]); resolution.SelectedIndex = Math.Max(0, Math.Min(previous, resolution.Items.Count - 1)); };
            browse.Click += (s, e) => BrowseGame();
            detect.Click += (s, e) => { try { string found = Installation.Detect(service.Root); if (found == null) SetStatus("L4D2 not found in Steam libraries. Browse to its installation folder.", true); else game.Text = found; } catch (Exception ex) { Error(ex); } };
            game.TextChanged += (s, e) => RefreshState();
            save.Click += (s, e) => { try { SaveSettings(); SetStatus("Video settings saved for your next launch.", false); } catch (Exception ex) { Error(ex); } };
            play.Click += async (s, e) => await Play();
            rebuild.Click += async (s, e) => await Build(false);
            cancel.Click += (s, e) => { cancel.Enabled = false; service.CancelBuild(); SetStatus("Cancelling build…", false); };
            FormClosing += BeforeClosing;
            FormClosed += (s, e) => timer.Dispose();
            timer.Interval = 1500; timer.Tick += (s, e) => { bool running = service.Running(); if (wasRunning && !running) LoadSettings(); wasRunning = running; if (!busy) RefreshState(); }; timer.Start();
            try { game.Text = Installation.Detect(root) ?? ""; LoadSettings(); RefreshState(); }
            catch (Exception ex) { Error(ex); }
        }
        private static void ButtonStyle(Button button, string text) { button.Text = text; button.Dock = DockStyle.Fill; button.Height = 46; button.FlatStyle = FlatStyle.Flat; button.BackColor = Theme.Input; button.ForeColor = Theme.Text; button.FlatAppearance.BorderSize = 0; button.Cursor = Cursors.Hand; }
        private static Label FieldLabel(string text)
        {
            return new Label { Text = text.ToUpperInvariant(), Dock = DockStyle.Fill, TextAlign = ContentAlignment.MiddleLeft, ForeColor = Theme.Muted, BackColor = Color.Transparent, Font = Theme.Display(11.5f) };
        }
        /// The L4D2R logo embedded in the exe (launcher/assets/logo.png).
        private static Image LoadLogo()
        {
            using (var stream = typeof(LauncherForm).Assembly.GetManifestResourceStream("L4D2R.Launcher.logo.png"))
                return stream == null ? null : new Bitmap(stream);
        }
        private static void Combo(ComboBox box, string[] values)
        {
            box.DropDownStyle = ComboBoxStyle.DropDownList; box.Items.AddRange(values); box.Anchor = AnchorStyles.Left | AnchorStyles.Right; box.Font = Theme.Body(10.5f);
            box.BackColor = Theme.Input; box.ForeColor = Theme.Text; box.DrawMode = DrawMode.OwnerDrawFixed; box.ItemHeight = 30; box.Margin = new Padding(0, 4, 0, 4); box.AccessibleName = "Video setting";
            box.DrawItem += (s, e) => {
                bool selected = (e.State & DrawItemState.Selected) != 0;
                using (var brush = new SolidBrush(selected ? Theme.Accent : Theme.Input)) e.Graphics.FillRectangle(brush, e.Bounds);
                if (e.Index >= 0) TextRenderer.DrawText(e.Graphics, box.Items[e.Index].ToString(), box.Font, new Rectangle(e.Bounds.X + 8, e.Bounds.Y, e.Bounds.Width - 8, e.Bounds.Height), selected ? Color.White : Theme.Text, TextFormatFlags.Left | TextFormatFlags.VerticalCenter | TextFormatFlags.EndEllipsis);
                e.DrawFocusRectangle();
            };
        }
        private static void Row(TableLayoutPanel panel, int row, string left, Control a, string right, Control b)
        {
            a.AccessibleName = left; b.AccessibleName = right;
            panel.Controls.Add(FieldLabel(left), 0, row); panel.Controls.Add(a, 1, row);
            var second = FieldLabel(right); second.Padding = new Padding(20, 0, 0, 0);
            panel.Controls.Add(second, 2, row); panel.Controls.Add(b, 3, row);
        }
        private void SelectPage(Panel page)
        {
            videoPage.Visible = page == videoPage; logPage.Visible = page == logPage; androidPage.Visible = page == androidPage; page.BringToFront();
            videoNav.Selected = page == videoPage; logNav.Selected = page == logPage; androidNav.Selected = page == androidPage;
        }
        [System.Runtime.InteropServices.DllImport("user32.dll")] private static extern bool ReleaseCapture();
        [System.Runtime.InteropServices.DllImport("user32.dll")] private static extern IntPtr SendMessage(IntPtr window, int message, IntPtr wParam, IntPtr lParam);
        private void DragWindow(object sender, MouseEventArgs e)
        {
            if (e.Button != MouseButtons.Left) return;
            ReleaseCapture(); SendMessage(Handle, 0xA1, new IntPtr(2), IntPtr.Zero);
        }
        protected override void WndProc(ref Message message)
        {
            base.WndProc(ref message);
            if (message.Msg != 0x84 || WindowState != FormWindowState.Normal) return;
            long coordinates = message.LParam.ToInt64();
            Point p = PointToClient(new Point(unchecked((short)(coordinates & 0xffff)), unchecked((short)((coordinates >> 16) & 0xffff))));
            bool left = p.X < 7, right = p.X >= ClientSize.Width - 7, top = p.Y < 7, bottom = p.Y >= ClientSize.Height - 7;
            int hit = top && left ? 13 : top && right ? 14 : bottom && left ? 16 : bottom && right ? 17 : left ? 10 : right ? 11 : top ? 12 : bottom ? 15 : 1;
            if (hit != 1) message.Result = new IntPtr(hit);
        }
        private void BrowseGame()
        {
            using (var picker = new FolderBrowserDialog { Description = "Select your Left 4 Dead 2 Steam installation folder", ShowNewFolderButton = false, SelectedPath = Installation.Resolve(game.Text) ?? "" })
                if (picker.ShowDialog(this) == DialogResult.OK) { game.Text = picker.SelectedPath; if (Installation.Resolve(game.Text) == null) SetStatus("That folder is missing the L4D2 executable, base VPK or Hotel map.", true); }
        }
        internal void LoadSettings()
        {
            loading = true;
            try
            {
                video = VideoSettings.Load(service.Root); var g = video.Graphics;
                aspect.SelectedIndex = VideoSettings.Index(g, "aspect", 0, 2); resolution.Items.Clear(); resolution.Items.AddRange(VideoSettings.Resolutions[aspect.SelectedIndex]); resolution.SelectedIndex = VideoSettings.Index(g, "resolution", 0, resolution.Items.Count - 1);
                display.SelectedIndex = VideoSettings.Index(g, "display", 0, 2); aa.SelectedIndex = Array.IndexOf(VideoSettings.Samples, Int32.Parse(g["samples"])); filtering.SelectedIndex = Array.IndexOf(VideoSettings.Filters, Int32.Parse(g["filtering"]));
                shader.SelectedIndex = Int32.Parse(g["shader"]); effects.SelectedIndex = Int32.Parse(g["effects"]); detail.SelectedIndex = Int32.Parse(g["detail"]);
                brightness.Value = (decimal)(VideoSettings.Number(g, "brightness", 1, .5, 1.5) * 100); grain.Value = (decimal)(VideoSettings.Number(g, "grain", 0, 0, 1) * 100);
                vsync.Checked = VideoSettings.Bool(video.Display, "vsync"); fps.Checked = VideoSettings.Bool(video.Display, "show_fps");
            }
            finally { loading = false; }
        }
        internal void SaveSettings()
        {
            if (service.Running()) throw new InvalidOperationException("Close the game before changing its video settings here.");
            // Preserve FOV and any future keys, including changes made since this window opened.
            video = VideoSettings.Load(service.Root); var g = video.Graphics;
            VideoSettings.Set(g, "aspect", aspect.SelectedIndex); VideoSettings.Set(g, "resolution", resolution.SelectedIndex); VideoSettings.Set(g, "display", display.SelectedIndex);
            VideoSettings.Set(g, "samples", VideoSettings.Samples[aa.SelectedIndex]); VideoSettings.Set(g, "filtering", VideoSettings.Filters[filtering.SelectedIndex]);
            VideoSettings.Set(g, "brightness", (double)brightness.Value / 100); VideoSettings.Set(g, "grain", (double)grain.Value / 100);
            VideoSettings.Set(g, "shader", shader.SelectedIndex); VideoSettings.Set(g, "effects", effects.SelectedIndex); VideoSettings.Set(g, "detail", detail.SelectedIndex);
            video.Display["vsync"] = vsync.Checked ? "true" : "false"; video.Display["show_fps"] = fps.Checked ? "true" : "false"; video.Save(service.Root);
        }
        private void SetStatus(string text, bool error) { status.Text = text; status.ForeColor = error ? Theme.AccentHot : Theme.Muted; }
        private void RefreshState()
        {
            if (busy) return;
            bool valid = Installation.Resolve(game.Text) != null, running = service.Running();
            play.Text = service.Built ? "Play" : "Build & play"; play.Enabled = valid && !running; rebuild.Enabled = valid && !running; save.Enabled = !running; videoPage.Enabled = !running; game.Enabled = browse.Enabled = detect.Enabled = !running;
            SetStatus(running ? "L4D2R is running. Settings unlock when the game closes." : !valid ? "Select your L4D2 folder to get started." : service.Built ? "Ready to play — using the saved build." : "Ready for the first build. Rust and C++ build tools are required.", false);
        }
        private void Error(Exception ex) { SetStatus(ex.Message, true); AppendLog(ex.Message); }
        private void AppendLog(string text)
        {
            if (IsDisposed || Disposing) return;
            if (InvokeRequired) { try { BeginInvoke(new Action<string>(AppendLog), text); } catch (InvalidOperationException) { } return; }
            if (log.TextLength > 150000) log.Text = log.Text.Substring(log.TextLength - 100000);
            log.AppendText(text + Environment.NewLine); log.SelectionStart = log.TextLength; log.ScrollToCaret();
        }
        private async Task Play()
        {
            try
            {
                SaveSettings(); service.SaveGame(game.Text);
                if (!service.Built) { if (!await Build(true)) return; }
                using (var process = service.StartGame(game.Text, lab.Checked)) { wasRunning = true; }
                RefreshState();
            }
            catch (Exception ex) { Error(ex); }
        }
        private async Task<bool> Build(bool fromPlay)
        {
            if (busy) return false;
            try
            {
                service.SaveGame(game.Text); SaveSettings(); busy = true; save.Visible = rebuild.Visible = false;
                play.Enabled = rebuild.Enabled = save.Enabled = videoPage.Enabled = game.Enabled = browse.Enabled = detect.Enabled = lab.Enabled = false;
                cancel.Visible = cancel.Enabled = true; SelectPage(logPage); log.Clear(); SetStatus("Building… you can follow progress below.", false);
                await service.BuildAsync(AppendLog);
                SetStatus(fromPlay ? "Build complete. Starting game…" : "Build complete. Ready to play.", false); return true;
            }
            catch (OperationCanceledException ex) { Error(ex); return false; }
            catch (Exception ex) { Error(ex); return false; }
            finally
            {
                busy = false; cancel.Visible = false; save.Visible = rebuild.Visible = true; lab.Enabled = true;
                bool valid = Installation.Resolve(game.Text) != null;
                play.Text = service.Built ? "Play" : "Build & play"; play.Enabled = rebuild.Enabled = valid;
                save.Enabled = videoPage.Enabled = game.Enabled = browse.Enabled = detect.Enabled = true;
                if (closingAfterCancel) Close();
            }
        }
        private async Task BuildApk()
        {
            if (busy) return;
            try
            {
                busy = true; save.Visible = rebuild.Visible = false;
                play.Enabled = rebuild.Enabled = save.Enabled = buildApk.Enabled = game.Enabled = browse.Enabled = detect.Enabled = lab.Enabled = false;
                cancel.Visible = cancel.Enabled = true; SelectPage(logPage); log.Clear(); SetStatus("Building the Android APK. Follow progress below.", false);
                await service.BuildApkAsync(AppendLog, game.Text, apkMode.SelectedIndex, apkInstall.Checked);
                SetStatus("APK ready: " + service.ApkOutput, false);
            }
            catch (OperationCanceledException ex) { Error(ex); }
            catch (Exception ex) { Error(ex); }
            finally
            {
                busy = false; cancel.Visible = false; save.Visible = rebuild.Visible = true; lab.Enabled = buildApk.Enabled = true;
                bool valid = Installation.Resolve(game.Text) != null;
                play.Enabled = rebuild.Enabled = valid; save.Enabled = game.Enabled = browse.Enabled = detect.Enabled = true;
                if (closingAfterCancel) Close();
            }
        }
        private void BeforeClosing(object sender, FormClosingEventArgs e)
        {
            if (!busy) return;
            e.Cancel = true; closingAfterCancel = true; cancel.Enabled = false; service.CancelBuild(); SetStatus("Cancelling the build before closing…", false);
        }
        /// Self-test captures of the other pages (build log, Android).
        internal void SavePagePreviews(string directory)
        {
            SelectPage(logPage); Application.DoEvents(); SavePreview(System.IO.Path.Combine(directory, "launcher-preview-log.png"));
            SelectPage(androidPage); Application.DoEvents(); SavePreview(System.IO.Path.Combine(directory, "launcher-preview-android.png"));
            SelectPage(videoPage); Application.DoEvents();
        }
        internal void SavePreview(string path)
        {
            using (var bitmap = new Bitmap(Width, Height)) { DrawToBitmap(bitmap, new Rectangle(0, 0, Width, Height)); bitmap.Save(path); }
        }
    }

    // Paint the closed field ourselves so native Windows themes cannot hide
    // light text on a white field. The native popup, keyboard and accessibility stay intact.
    internal sealed class ChoiceBox : ComboBox
    {
        internal ChoiceBox() { SetStyle(ControlStyles.UserPaint | ControlStyles.AllPaintingInWmPaint | ControlStyles.OptimizedDoubleBuffer, true); }
        protected override void OnPaint(PaintEventArgs e)
        {
            var g = e.Graphics;
            var r = new Rectangle(0, 0, Math.Max(1, Width - 1), Math.Max(1, Height - 1));
            using (var fill = new SolidBrush(Theme.Input)) g.FillRectangle(fill, r);
            using (var border = new Pen(Focused ? Theme.Accent : Theme.Border)) g.DrawRectangle(border, r);
            int center = Height / 2;
            g.SmoothingMode = System.Drawing.Drawing2D.SmoothingMode.AntiAlias;
            using (var pen = new Pen(Enabled ? Theme.Accent : Theme.Disabled, 2f)) g.DrawLines(pen, new[] { new Point(Width - 22, center - 3), new Point(Width - 17, center + 2), new Point(Width - 12, center - 3) });
            var text = new Rectangle(11, 0, Math.Max(0, ClientSize.Width - 39), ClientSize.Height);
            TextRenderer.DrawText(g, Text, Font, text, Enabled ? Theme.Text : Theme.Disabled, TextFormatFlags.Left | TextFormatFlags.VerticalCenter | TextFormatFlags.EndEllipsis);
        }
        protected override void OnSelectedIndexChanged(EventArgs e) { base.OnSelectedIndexChanged(e); Invalidate(); }
        protected override void OnEnabledChanged(EventArgs e) { base.OnEnabledChanged(e); Invalidate(); }
    }
}
