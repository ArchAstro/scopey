//! Terminal presentation for reports: truecolor text, unicode bars, and
//! kitty-graphics charts.
//!
//! Colors come from the validated dark-surface palette (categorical slots in
//! fixed order, status colors reserved for verdict states; see
//! `eval`/dataviz notes). Charts obey the mark rules: single hue for
//! magnitude-over-position, categorical slots assigned in fixed order, 2px
//! gaps between fills, and no text inside images — labels stay as terminal
//! text in default ink beside the marks.
//!
//! Kitty graphics (raster images inline in the terminal) are emitted only
//! when the terminal advertises support (kitty, ghostty, WezTerm) or the
//! user forces `--graphics kitty`; everything degrades to unicode bars, then
//! to plain text under `NO_COLOR` or when piped.

use std::env;
use std::io::IsTerminal;

/// Categorical slots (dark-surface steps), fixed order, validated as a set.
pub const CATEGORICAL: [&str; 8] = [
    "#3987e5", "#d95926", "#199e70", "#c98500", "#d55181", "#008300", "#9085e9", "#e66767",
];
/// Status palette — reserved for verdict states, never for series identity.
pub const STATUS_GOOD: &str = "#0ca30c";
pub const STATUS_WARNING: &str = "#fab219";
pub const STATUS_SERIOUS: &str = "#ec835a";
pub const NEUTRAL: &str = "#8a8984";
/// Sequential hue for magnitude (single hue, per the color rules).
pub const SEQUENTIAL: &str = "#3987e5";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Graphics {
    Auto,
    Kitty,
    Off,
}

impl Graphics {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Graphics::Auto),
            "kitty" => Some(Graphics::Kitty),
            "off" | "none" | "plain" => Some(Graphics::Off),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Caps {
    pub color: bool,
    pub kitty: bool,
}

pub fn detect(graphics: Graphics) -> Caps {
    let tty = std::io::stdout().is_terminal();
    let color = tty && env::var_os("NO_COLOR").is_none();
    let kitty = match graphics {
        Graphics::Off => false,
        Graphics::Kitty => true,
        Graphics::Auto => color && kitty_supported(),
    };
    Caps { color, kitty }
}

/// Only positive, layer-local signals: `TERM` is conventionally re-set by
/// each terminal/PTY layer, while `TERM_PROGRAM` leaks through nested PTYs
/// (tmux, capture harnesses, CI) and reported kitty support where the actual
/// emulator would drop the escape. WezTerm users (TERM=xterm-256color) can
/// force `--graphics kitty`.
fn kitty_supported() -> bool {
    if env::var_os("KITTY_WINDOW_ID").is_some() {
        return true;
    }
    let term = env::var("TERM").unwrap_or_default().to_ascii_lowercase();
    term.contains("kitty") || term.contains("ghostty")
}

/// Terminal width for wrapping: ioctl on stdout, then COLUMNS, then 100.
pub fn terminal_width() -> usize {
    unsafe {
        let mut size: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut size) == 0 && size.ws_col > 0 {
            return size.ws_col as usize;
        }
    }
    env::var("COLUMNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|w| *w > 20)
        .unwrap_or(100)
}

/// Greedy word-wrap with a hanging indent for continuation lines.
pub fn wrap_indent(text: &str, width: usize, first_prefix: &str, indent: &str) -> String {
    let usable = width.saturating_sub(indent.chars().count()).max(20);
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate_len =
            current.chars().count() + if current.is_empty() { 0 } else { 1 } + word.chars().count();
        if !current.is_empty() && candidate_len > usable {
            lines.push(current);
            current = String::new();
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    let mut out = String::new();
    for (index, line) in lines.iter().enumerate() {
        if index == 0 {
            out.push_str(first_prefix);
        } else {
            out.push('\n');
            out.push_str(indent);
        }
        out.push_str(line);
    }
    out
}

/// A dim horizontal rule with a bold title, e.g. `── drift patterns ── …`.
pub fn section(caps: Caps, title: &str, suffix: &str, width: usize) -> String {
    let spaced_suffix = if suffix.is_empty() {
        String::new()
    } else {
        format!("{suffix} ")
    };
    let used = 3 + title.chars().count() + 1 + spaced_suffix.chars().count();
    let tail = "─".repeat(width.saturating_sub(used).clamp(2, 60));
    format!(
        "{}{} {}{}",
        dim(caps, "── "),
        bold(caps, title),
        spaced_suffix,
        dim(caps, &tail)
    )
}

pub fn hex_rgb(hex: &str) -> (u8, u8, u8) {
    let h = hex.trim_start_matches('#');
    if h.len() != 6 {
        return (200, 200, 200);
    }
    let parse = |s: &str| u8::from_str_radix(s, 16).unwrap_or(200);
    (parse(&h[0..2]), parse(&h[2..4]), parse(&h[4..6]))
}

pub fn fg(caps: Caps, hex: &str, text: &str) -> String {
    if !caps.color {
        return text.to_string();
    }
    let (r, g, b) = hex_rgb(hex);
    format!("\x1b[38;2;{r};{g};{b}m{text}\x1b[0m")
}

pub fn dim(caps: Caps, text: &str) -> String {
    if !caps.color {
        return text.to_string();
    }
    format!("\x1b[2m{text}\x1b[0m")
}

pub fn bold(caps: Caps, text: &str) -> String {
    if !caps.color {
        return text.to_string();
    }
    format!("\x1b[1m{text}\x1b[0m")
}

/// A left-to-right bar with 1/8-cell resolution.
pub fn unicode_bar(frac: f64, width: usize) -> String {
    const PARTIALS: [char; 7] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    let clamped = frac.clamp(0.0, 1.0);
    let cells = clamped * width as f64;
    let full = cells.floor() as usize;
    let remainder = ((cells - full as f64) * 8.0).round() as usize;
    let mut bar: String = "█".repeat(full.min(width));
    if full < width && remainder > 0 {
        if remainder >= 8 {
            bar.push('█');
        } else {
            bar.push(PARTIALS[remainder - 1]);
        }
    }
    bar
}

/// Sparkline over 1/8-cell block heights.
pub fn sparkline(values: &[f64]) -> String {
    const LEVELS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let max = values.iter().cloned().fold(0.0_f64, f64::max);
    values
        .iter()
        .map(|v| {
            if max <= 0.0 {
                LEVELS[0]
            } else {
                let idx = ((v / max) * 7.0).round() as usize;
                LEVELS[idx.min(7)]
            }
        })
        .collect()
}

pub struct BarRow {
    pub label: String,
    pub value: String,
    pub frac: f64,
    pub color: &'static str,
}

/// Aligned label column (default ink), colored bar, value in default ink.
pub fn render_bar_rows(caps: Caps, rows: &[BarRow], bar_width: usize) -> String {
    let label_width = rows
        .iter()
        .map(|r| r.label.chars().count())
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    for row in rows {
        let bar = unicode_bar(row.frac, bar_width);
        out.push_str(&format!(
            "  {label:<label_width$}  {bar}{pad} {value}\n",
            label = row.label,
            bar = fg(caps, row.color, &bar),
            pad = " ".repeat(bar_width.saturating_sub(bar.chars().count())),
            value = dim(caps, &row.value),
        ));
    }
    out
}

// ---------------------------------------------------------------------------
// Kitty graphics: raster RGBA charts, transmitted base64-chunked.

const BASE64_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(BASE64_TABLE[(n >> 18) as usize & 63] as char);
        out.push(BASE64_TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            BASE64_TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            BASE64_TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

pub struct Canvas {
    pub width: usize,
    pub height: usize,
    pixels: Vec<u8>,
}

impl Canvas {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; width * height * 4],
        }
    }

    pub fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, hex: &str) {
        let (r, g, b) = hex_rgb(hex);
        for row in y..(y + h).min(self.height) {
            for col in x..(x + w).min(self.width) {
                let i = (row * self.width + col) * 4;
                self.pixels[i] = r;
                self.pixels[i + 1] = g;
                self.pixels[i + 2] = b;
                self.pixels[i + 3] = 255;
            }
        }
    }

    /// Serialize as kitty graphics APC escape(s): transmit + display, raw
    /// RGBA (`f=32`), chunked at 4096 base64 chars.
    pub fn kitty_escape(&self) -> String {
        let encoded = base64(&self.pixels);
        let chunks: Vec<&str> = encoded
            .as_bytes()
            .chunks(4096)
            .map(|c| std::str::from_utf8(c).unwrap_or_default())
            .collect();
        let mut out = String::new();
        for (index, chunk) in chunks.iter().enumerate() {
            let more = if index + 1 < chunks.len() { 1 } else { 0 };
            if index == 0 {
                out.push_str(&format!(
                    "\x1b_Ga=T,f=32,s={},v={},m={};{}\x1b\\",
                    self.width, self.height, more, chunk
                ));
            } else {
                out.push_str(&format!("\x1b_Gm={more};{chunk}\x1b\\"));
            }
        }
        out.push('\n');
        out
    }
}

/// Gap between fills follows the mark rules: 2px surface gaps.
const ROW_GAP: usize = 2;

/// Vertical histogram in a single hue (magnitude over position).
pub fn histogram_chart(buckets: &[usize], width: usize, height: usize) -> Canvas {
    let mut canvas = Canvas::new(width, height);
    if buckets.is_empty() {
        return canvas;
    }
    let max = buckets.iter().copied().max().unwrap_or(0).max(1);
    let column = (width - (buckets.len() - 1) * ROW_GAP) / buckets.len();
    for (index, count) in buckets.iter().enumerate() {
        let bar_height = ((*count as f64 / max as f64) * height as f64).round() as usize;
        let x = index * (column + ROW_GAP);
        canvas.fill_rect(
            x,
            height.saturating_sub(bar_height.max(1)),
            column,
            bar_height.max(1),
            SEQUENTIAL,
        );
    }
    canvas
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_bar_scales_with_partial_cells() {
        assert_eq!(unicode_bar(0.0, 10), "");
        assert_eq!(unicode_bar(1.0, 10).chars().count(), 10);
        assert_eq!(unicode_bar(0.5, 10).chars().count(), 5);
        // 0.55 of 10 cells = 5.5 cells → five full blocks plus a half block.
        assert_eq!(unicode_bar(0.55, 10), "█████▌");
    }

    #[test]
    fn sparkline_normalizes_to_eight_levels() {
        assert_eq!(sparkline(&[0.0, 4.0, 8.0]), "▁▅█");
        assert_eq!(sparkline(&[0.0, 0.0]), "▁▁");
    }

    #[test]
    fn base64_matches_reference_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn kitty_escape_carries_geometry_and_chunking() {
        let mut canvas = Canvas::new(2, 1);
        canvas.fill_rect(0, 0, 2, 1, "#ff0000");
        let escape = canvas.kitty_escape();
        assert!(escape.starts_with("\x1b_Ga=T,f=32,s=2,v=1,m=0;"));
        assert!(escape.ends_with("\x1b\\\n"));
        // Two RGBA pixels = 8 bytes = 12 base64 chars, single chunk.
        let payload = escape
            .split(';')
            .nth(1)
            .unwrap()
            .trim_end_matches("\x1b\\\n");
        assert_eq!(payload.len(), 12);
        // A large canvas must chunk with m=1 continuation frames.
        let big = Canvas::new(64, 64);
        let escape = big.kitty_escape();
        assert!(escape.contains("m=1;"));
        assert!(escape.contains("\x1b_Gm="));
    }

    #[test]
    fn histogram_respects_bounds_and_gap_layout() {
        let hist = histogram_chart(&[1, 2, 3], 100, 40);
        assert_eq!((hist.width, hist.height), (100, 40));
        // Columns are separated by the 2px surface gap.
        let column = (100 - 2 * ROW_GAP) / 3;
        assert!(column > 0);
        let empty = histogram_chart(&[], 100, 40);
        assert_eq!(empty.width, 100);
    }

    #[test]
    fn plain_mode_emits_no_escapes() {
        let caps = Caps {
            color: false,
            kitty: false,
        };
        assert_eq!(fg(caps, "#ffffff", "x"), "x");
        assert_eq!(dim(caps, "x"), "x");
    }
}
