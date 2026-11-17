using System;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Drawing.Imaging;
using System.Drawing.Text;
using System.Linq;
using System.Windows.Forms;

namespace L4D2R.Launcher
{
    // Left 4 Dead 2 menu look: near-black grained backdrop, warm off-white
    // condensed capitals, blood red for selection and the primary action,
    // square edges and hairline rules instead of rounded cards.
    internal static class Theme
    {
        internal static readonly Color Background = Color.FromArgb(10, 9, 9);
        internal static readonly Color Sidebar = Color.FromArgb(6, 5, 5);
        internal static readonly Color Card = Color.FromArgb(16, 14, 14);
        internal static readonly Color Input = Color.FromArgb(22, 19, 19);
        internal static readonly Color Border = Color.FromArgb(48, 42, 40);
        internal static readonly Color Rule = Color.FromArgb(36, 31, 30);
        internal static readonly Color Text = Color.FromArgb(236, 230, 220);
        internal static readonly Color Muted = Color.FromArgb(143, 135, 128);
        internal static readonly Color Disabled = Color.FromArgb(80, 74, 70);
        internal static readonly Color Accent = Color.FromArgb(193, 18, 31);
        internal static readonly Color AccentHot = Color.FromArgb(227, 38, 47);
        internal static readonly Color Selected = Color.FromArgb(52, 10, 12);

        private static readonly string[] installed = new InstalledFontCollection().Families.Select(f => f.Name).ToArray();
        private static string Pick(params string[] names) { return names.FirstOrDefault(n => installed.Contains(n)) ?? "Segoe UI"; }
        // L4D2's UI uses a heavy condensed grotesque; Bahnschrift Condensed
        // ships with Windows 10/11 and is the closest stock match.
        internal static readonly string DisplayFamily = Pick("Bahnschrift SemiBold Condensed", "Bahnschrift Condensed", "Arial Narrow", "Segoe UI Semibold");
        internal static readonly string BodyFamily = Pick("Bahnschrift SemiCondensed", "Bahnschrift", "Segoe UI");
        internal static Font Display(float size) { return new Font(DisplayFamily, size, FontStyle.Regular, GraphicsUnit.Point); }
        internal static Font Body(float size) { return new Font(BodyFamily, size, FontStyle.Regular, GraphicsUnit.Point); }

        private static Bitmap grain;
        // Film grain tile (fixed seed): faint speckles and scratches, painted
        // over every surface so the dark panels read as worn, not flat.
        internal static Bitmap Grain
        {
            get
            {
                if (grain != null) return grain;
                var rng = new Random(4242);
                grain = new Bitmap(256, 256, PixelFormat.Format32bppArgb);
                for (int y = 0; y < 256; y++)
                    for (int x = 0; x < 256; x++)
                    {
                        int n = rng.Next(256);
                        int a = n > 246 ? 22 : n < 10 ? 18 : rng.Next(7);
                        int v = n > 128 ? 255 : 0;
                        grain.SetPixel(x, y, Color.FromArgb(a, v, v, v));
                    }
                using (var g = Graphics.FromImage(grain))
                using (var scratch = new Pen(Color.FromArgb(10, 255, 255, 255)))
                    for (int i = 0; i < 6; i++) { int x = rng.Next(256); g.DrawLine(scratch, x, 0, x + rng.Next(-30, 30), 256); }
                return grain;
            }
        }
        // Paint the backdrop for `control` at its place in the window, so
        // grain and vignette continue seamlessly across child controls.
        internal static void Backdrop(Graphics g, Control control, Color baseColor)
        {
            g.Clear(baseColor);
            Form form = control.FindForm();
            Point offset = form == null ? Point.Empty : form.PointToClient(control.PointToScreen(Point.Empty));
            using (var brush = new TextureBrush(Grain, WrapMode.Tile))
            {
                brush.TranslateTransform(-offset.X, -offset.Y);
                g.FillRectangle(brush, control.ClientRectangle);
            }
            if (form == null) return;
            // Vignette: darker toward the window edges.
            var size = form.ClientSize;
            using (var path = new GraphicsPath())
            {
                path.AddEllipse(-size.Width * 0.25f - offset.X, -size.Height * 0.3f - offset.Y, size.Width * 1.5f, size.Height * 1.6f);
                using (var vignette = new PathGradientBrush(path))
                {
                    vignette.CenterColor = Color.FromArgb(0, 0, 0, 0);
                    vignette.SurroundColors = new[] { Color.FromArgb(150, 0, 0, 0) };
                    vignette.FocusScales = new PointF(0.55f, 0.55f);
                    var clip = g.Clip; g.FillRectangle(vignette, control.ClientRectangle); g.Clip = clip;
                }
            }
        }
        /// Base colour under `control`: the nearest GrainPanel's, or the first
        /// opaque ancestor's (transparent layout panels have none of their own).
        internal static Color BaseOf(Control control)
        {
            for (Control p = control.Parent; p != null; p = p.Parent)
            {
                var grain = p as GrainPanel;
                if (grain != null) return grain.Base;
                if (p.BackColor.A == 255) return p.BackColor;
            }
            return Background;
        }
        internal static void HairLine(Graphics g, int x, int y, int width)
        {
            using (var pen = new Pen(Theme.Rule)) g.DrawLine(pen, x, y, x + width, y);
        }
    }

    /// Panel painting the shared backdrop (grain + vignette).
    internal class GrainPanel : Panel
    {
        internal Color Base = Theme.Background;
        internal GrainPanel() { DoubleBuffered = true; ResizeRedraw = true; SetStyle(ControlStyles.AllPaintingInWmPaint | ControlStyles.UserPaint | ControlStyles.OptimizedDoubleBuffer, true); }
        protected override void OnPaintBackground(PaintEventArgs e) { Theme.Backdrop(e.Graphics, this, Base); }
    }

    /// Section heading: condensed capitals over a short red stroke and a
    /// hairline rule, as L4D2's option pages head their groups.
    internal sealed class SectionTitle : Control
    {
        internal SectionTitle(string text) { Text = text; Font = Theme.Display(15); ForeColor = Theme.Text; Height = 38; SetStyle(ControlStyles.UserPaint | ControlStyles.AllPaintingInWmPaint | ControlStyles.OptimizedDoubleBuffer | ControlStyles.ResizeRedraw | ControlStyles.SupportsTransparentBackColor, true); BackColor = Color.Transparent; }
        protected override void OnPaint(PaintEventArgs e)
        {
            e.Graphics.TextRenderingHint = TextRenderingHint.AntiAliasGridFit;
            TextRenderer.DrawText(e.Graphics, Text.ToUpperInvariant(), Font, new Rectangle(0, 2, Width, Height - 10), ForeColor, TextFormatFlags.Left | TextFormatFlags.Top | TextFormatFlags.NoPadding);
            int y = Height - 4;
            Theme.HairLine(e.Graphics, 0, y, Width);
            using (var red = new SolidBrush(Theme.Accent)) e.Graphics.FillRectangle(red, 0, y - 1, 34, 3);
        }
    }

    internal enum ButtonKind { Secondary, Primary, Nav, Caption, CaptionClose }

    internal sealed class ModernButton : Button
    {
        internal int Radius = 0;
        internal ButtonKind Kind = ButtonKind.Secondary;
        private bool hover, pressed, selected;
        internal bool Selected { get { return selected; } set { selected = value; Invalidate(); } }
        internal ModernButton()
        {
            SetStyle(ControlStyles.UserPaint | ControlStyles.AllPaintingInWmPaint | ControlStyles.OptimizedDoubleBuffer | ControlStyles.ResizeRedraw, true);
            FlatStyle = FlatStyle.Flat; FlatAppearance.BorderSize = 0; BackColor = Theme.Input; ForeColor = Theme.Text; Cursor = Cursors.Hand;
            Font = Theme.Display(11.5f);
        }
        protected override void OnMouseEnter(EventArgs e) { hover = true; Invalidate(); base.OnMouseEnter(e); }
        protected override void OnMouseLeave(EventArgs e) { hover = pressed = false; Invalidate(); base.OnMouseLeave(e); }
        protected override void OnMouseDown(MouseEventArgs e) { pressed = true; Invalidate(); base.OnMouseDown(e); }
        protected override void OnMouseUp(MouseEventArgs e) { pressed = false; Invalidate(); base.OnMouseUp(e); }
        protected override void OnGotFocus(EventArgs e) { Invalidate(); base.OnGotFocus(e); }
        protected override void OnLostFocus(EventArgs e) { Invalidate(); base.OnLostFocus(e); }
        protected override void OnEnabledChanged(EventArgs e) { Invalidate(); base.OnEnabledChanged(e); }
        protected override void OnPaintBackground(PaintEventArgs e)
        {
            if (Parent != null) Theme.Backdrop(e.Graphics, this, Theme.BaseOf(this));
            else e.Graphics.Clear(Theme.Background);
        }
        protected override void OnPaint(PaintEventArgs e)
        {
            var g = e.Graphics; g.SmoothingMode = SmoothingMode.None;
            // Button is an opaque control: OnPaintBackground never runs and the
            // shared double buffer still holds other controls' pixels, so the
            // backdrop is painted here first.
            Theme.Backdrop(g, this, Theme.BaseOf(this));
            var r = new Rectangle(0, 0, Width - 1, Height - 1);
            string label = Text.ToUpperInvariant();
            var flags = TextFormatFlags.VerticalCenter | TextFormatFlags.EndEllipsis | TextFormatFlags.NoPadding;
            switch (Kind)
            {
                case ButtonKind.Primary:
                {
                    Color fill = !Enabled ? Theme.Selected : pressed ? Theme.Accent : hover ? Theme.AccentHot : Theme.Accent;
                    using (var brush = new LinearGradientBrush(r, ControlPaint.Light(fill, 0.08f), ControlPaint.Dark(fill, 0.08f), LinearGradientMode.Vertical)) g.FillRectangle(brush, r);
                    using (var tex = new TextureBrush(Theme.Grain)) g.FillRectangle(tex, r);
                    if (Focused) using (var pen = new Pen(Theme.Text)) g.DrawRectangle(pen, 2, 2, Width - 5, Height - 5);
                    TextRenderer.DrawText(g, label, Font, r, Enabled ? Color.White : Theme.Muted, flags | TextFormatFlags.HorizontalCenter);
                    break;
                }
                case ButtonKind.Nav:
                {
                    // L4D2 main menu: the chosen entry sits on a red bar that
                    // fades out to the right; hover just brightens the text.
                    if (Selected)
                        using (var bar = new LinearGradientBrush(r, Color.FromArgb(200, Theme.Accent), Color.FromArgb(0, Theme.Accent), LinearGradientMode.Horizontal))
                            g.FillRectangle(bar, 0, 4, Width, Height - 8);
                    Color c = !Enabled ? Theme.Disabled : Selected ? Color.White : hover ? Theme.Text : Theme.Muted;
                    TextRenderer.DrawText(g, label, Font, new Rectangle(18, 0, Width - 18, Height), c, flags | TextFormatFlags.Left);
                    if (Focused && !Selected) using (var pen = new Pen(Theme.Border)) g.DrawRectangle(pen, r);
                    break;
                }
                case ButtonKind.Caption:
                case ButtonKind.CaptionClose:
                {
                    if (hover) using (var brush = new SolidBrush(Kind == ButtonKind.CaptionClose ? Theme.Accent : Theme.Input)) g.FillRectangle(brush, r);
                    TextRenderer.DrawText(g, Text, Font, r, hover ? Color.White : Theme.Muted, flags | TextFormatFlags.HorizontalCenter);
                    break;
                }
                default:
                {
                    using (var brush = new SolidBrush(Color.FromArgb(hover && Enabled ? 200 : 150, Theme.Input))) g.FillRectangle(brush, r);
                    using (var pen = new Pen(!Enabled ? Theme.Rule : Focused || hover ? Theme.Accent : Theme.Border)) g.DrawRectangle(pen, r);
                    TextRenderer.DrawText(g, label, Font, r, Enabled ? (hover ? Color.White : Theme.Text) : Theme.Disabled, flags | TextFormatFlags.HorizontalCenter);
                    break;
                }
            }
        }
    }

    /// Checkbox in the L4D2 options style: a small square that fills red.
    internal sealed class ModernSwitch : CheckBox
    {
        private bool hover;
        internal ModernSwitch()
        {
            SetStyle(ControlStyles.UserPaint | ControlStyles.AllPaintingInWmPaint | ControlStyles.OptimizedDoubleBuffer | ControlStyles.ResizeRedraw, true);
            Cursor = Cursors.Hand; ForeColor = Theme.Text; Font = Theme.Display(11.5f);
        }
        protected override void OnMouseEnter(EventArgs e) { hover = true; Invalidate(); base.OnMouseEnter(e); }
        protected override void OnMouseLeave(EventArgs e) { hover = false; Invalidate(); base.OnMouseLeave(e); }
        protected override void OnCheckedChanged(EventArgs e) { base.OnCheckedChanged(e); Invalidate(); }
        protected override void OnPaintBackground(PaintEventArgs e) { if (Parent != null) Theme.Backdrop(e.Graphics, this, Theme.BaseOf(this)); }
        protected override void OnPaint(PaintEventArgs e)
        {
            var g = e.Graphics;
            Theme.Backdrop(g, this, Theme.BaseOf(this));
            int y = (Height - 16) / 2;
            using (var fill = new SolidBrush(Theme.Input)) g.FillRectangle(fill, 1, y, 16, 16);
            using (var pen = new Pen(!Enabled ? Theme.Rule : hover || Focused ? Theme.AccentHot : Theme.Border)) g.DrawRectangle(pen, 1, y, 16, 16);
            if (Checked) using (var red = new SolidBrush(Enabled ? Theme.Accent : Theme.Disabled)) g.FillRectangle(red, 5, y + 4, 9, 9);
            TextRenderer.DrawText(g, Text.ToUpperInvariant(), Font, new Rectangle(26, 0, Math.Max(0, Width - 28), Height), Enabled ? (hover ? Color.White : Theme.Text) : Theme.Disabled, TextFormatFlags.Left | TextFormatFlags.VerticalCenter | TextFormatFlags.EndEllipsis);
        }
    }

    /// Thin slider: hairline track, red fill and a square notch.
    internal sealed class PercentSlider : Control
    {
        internal decimal Minimum = 0, Maximum = 100, Increment = 5;
        private decimal value;
        internal decimal Value { get { return value; } set { this.value = Math.Max(Minimum, Math.Min(Maximum, value)); Invalidate(); } }
        internal PercentSlider()
        {
            SetStyle(ControlStyles.UserPaint | ControlStyles.AllPaintingInWmPaint | ControlStyles.OptimizedDoubleBuffer | ControlStyles.ResizeRedraw | ControlStyles.Selectable, true);
            TabStop = true; Cursor = Cursors.Hand; AccessibleRole = AccessibleRole.Slider; Font = Theme.Body(10);
        }
        private void At(int x)
        {
            decimal fraction = Math.Max(0, Math.Min(1, (decimal)(x - 5) / Math.Max(1, Width - 62)));
            Value = Minimum + Math.Round((Maximum - Minimum) * fraction / Increment) * Increment;
        }
        protected override void OnMouseDown(MouseEventArgs e) { base.OnMouseDown(e); if (e.Button == MouseButtons.Left) { Focus(); Capture = true; At(e.X); } }
        protected override void OnMouseMove(MouseEventArgs e) { base.OnMouseMove(e); if (Capture && e.Button == MouseButtons.Left) At(e.X); }
        protected override void OnMouseUp(MouseEventArgs e) { base.OnMouseUp(e); Capture = false; }
        protected override bool IsInputKey(Keys keyData) { return keyData == Keys.Left || keyData == Keys.Right || keyData == Keys.Home || keyData == Keys.End || base.IsInputKey(keyData); }
        protected override void OnKeyDown(KeyEventArgs e)
        {
            if (e.KeyCode == Keys.Left) { Value -= Increment; e.Handled = true; }
            if (e.KeyCode == Keys.Right) { Value += Increment; e.Handled = true; }
            if (e.KeyCode == Keys.Home) { Value = Minimum; e.Handled = true; }
            if (e.KeyCode == Keys.End) { Value = Maximum; e.Handled = true; }
            base.OnKeyDown(e);
        }
        protected override void OnGotFocus(EventArgs e) { Invalidate(); base.OnGotFocus(e); }
        protected override void OnLostFocus(EventArgs e) { Invalidate(); base.OnLostFocus(e); }
        protected override void OnPaintBackground(PaintEventArgs e) { if (Parent != null) Theme.Backdrop(e.Graphics, this, Theme.BaseOf(this)); }
        protected override void OnPaint(PaintEventArgs e)
        {
            var g = e.Graphics;
            Theme.Backdrop(g, this, Theme.BaseOf(this));
            float width = Math.Max(1, Width - 62), x = 5 + width * (float)((Value - Minimum) / Math.Max(1, Maximum - Minimum)); int y = Height / 2;
            using (var track = new SolidBrush(Theme.Border)) g.FillRectangle(track, 5, y - 1, width, 2);
            using (var fill = new SolidBrush(Enabled ? Theme.Accent : Theme.Disabled)) g.FillRectangle(fill, 5, y - 1, x - 5, 2);
            using (var notch = new SolidBrush(Enabled ? (Focused ? Color.White : Theme.Text) : Theme.Disabled)) g.FillRectangle(notch, x - 3, y - 7, 6, 14);
            TextRenderer.DrawText(g, ((int)Value).ToString() + "%", Font, new Rectangle(Width - 50, 0, 48, Height), Enabled ? Theme.Text : Theme.Disabled, TextFormatFlags.Right | TextFormatFlags.VerticalCenter);
        }
    }
}
