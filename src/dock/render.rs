//! Small Direct2D renderer, separate from the launcher's renderer.

use super::logic::Part;
use super::status::Status;
use super::tray::Shown;
use super::{Item, logic};
use crate::indexer::IconPixels;
use crate::theme::Palette;
use crate::tray_host::Key;
use std::collections::HashMap;
use std::sync::Arc;
use windows::Win32::Foundation::{D2DERR_RECREATE_TARGET, HWND, RECT};
use windows::Win32::Graphics::Direct2D::{Common::*, *};
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, GetForegroundWindow};
use windows::core::{Interface, PCWSTR, Result, w};

/// Vertical layout, in DIPs, shared by the app slots and the status parts.
/// Icons sit a little above center to leave room for the running indicator.
use super::logic::{ICON_CENTER_Y, INDICATOR_TOP, TILE_BOTTOM, TILE_RADIUS, TILE_TOP};
const UPPER: (f32, f32) = (12.0, 33.0);
const LOWER: (f32, f32) = (32.0, 52.0);
/// Extra icon scale at the crest of the macOS-style magnification wave.
const MAGNIFY: f32 = 0.12;

struct Target {
    rt: ID2D1RenderTarget,
    /// Set for a window-backed target, whose pixel size can change.
    hwnd: Option<ID2D1HwndRenderTarget>,
    brush: ID2D1SolidColorBrush,
    bitmaps: HashMap<String, Bitmap>,
    /// Notification-area icons, by their tray identity.
    tray: HashMap<Key, (Arc<IconPixels>, ID2D1Bitmap)>,
    size: D2D_SIZE_U,
}

/// A cached app icon, ready to draw.
struct Bitmap {
    pixels: Arc<IconPixels>,
    bitmap: ID2D1Bitmap,
    /// Nearly black artwork needs a lit plate behind it to read on the glass.
    dark: bool,
}

struct Formats {
    /// A fallback tile's initial.
    letter: IDWriteTextFormat,
    time: IDWriteTextFormat,
    small: IDWriteTextFormat,
    /// The terminal prompt in the search slot.
    prompt: IDWriteTextFormat,
    /// Segoe Fluent Icons, for the status glyphs.
    glyph: IDWriteTextFormat,
    chevron: IDWriteTextFormat,
    /// Tooltips: left-aligned and wrapping.
    tip: IDWriteTextFormat,
    /// One line of the window list, clipped at the card's edge.
    row: IDWriteTextFormat,
}

pub struct Renderer {
    factory: ID2D1Factory,
    dw: IDWriteFactory,
    formats: Formats,
    display_font: PCWSTR,
    icon_font: PCWSTR,
    mono_font: PCWSTR,
    format_scale: f32,
    target: Option<Target>,
    shared_target: Option<Target>,
}

pub struct Frame<'a> {
    pub layout: logic::AppLayout,
    pub dpi: u32,
    pub palette: &'a Palette,
    pub items: &'a [Item],
    pub icons: &'a HashMap<String, Arc<IconPixels>>,
    pub hover: Option<usize>,
    pub hover_levels: &'a [f32],
    pub app_icon_size: f32,
    pub layout_scale: f32,
    pub capacity: usize,
    pub scroll: usize,
    pub status: &'a Status,
    pub hover_part: Option<Part>,
    pub parts: &'a [Part],
    /// Every shown notification icon, and which of them sit in the dock.
    pub tray: &'a [Shown],
    pub inline: &'a [usize],
    pub flyout_open: bool,
}

/// The notification-area flyout's grid.
pub struct FlyoutFrame<'a> {
    pub dpi: u32,
    pub palette: &'a Palette,
    pub tray: &'a [Shown],
    pub hover: Option<usize>,
}

fn create_formats(dw: &IDWriteFactory, display: PCWSTR, icons: PCWSTR, mono: PCWSTR, scale: f32) -> Result<Formats> {
    unsafe {
        let format = |family: PCWSTR, weight, size| -> Result<IDWriteTextFormat> {
            let format = dw.CreateTextFormat(
                family,
                None,
                weight,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size * scale,
                w!("en-us"),
            )?;
            format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            Ok(format)
        };
        let tip = dw.CreateTextFormat(
            mono,
            None,
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            12.0 * scale,
            w!("en-us"),
        )?;
        tip.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
        tip.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR)?;
        tip.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
        let row = dw.CreateTextFormat(
            mono,
            None,
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            11.5 * scale,
            w!("en-us"),
        )?;
        row.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
        Ok(Formats {
            letter: format(display, DWRITE_FONT_WEIGHT_SEMI_BOLD, 15.0)?,
            time: format(mono, DWRITE_FONT_WEIGHT_SEMI_BOLD, 13.5)?,
            small: format(mono, DWRITE_FONT_WEIGHT_NORMAL, 11.0)?,
            prompt: format(mono, DWRITE_FONT_WEIGHT_SEMI_BOLD, 15.0)?,
            glyph: format(icons, DWRITE_FONT_WEIGHT_NORMAL, 17.0)?,
            chevron: format(icons, DWRITE_FONT_WEIGHT_NORMAL, 12.0)?,
            tip,
            row,
        })
    }
}

impl Frame<'_> {
    fn count(&self) -> usize {
        self.items.len().saturating_sub(self.scroll).min(self.capacity)
    }

    /// Rounds a DIP coordinate to the nearest physical pixel, for crisp edges.
    fn snap(&self, dip: f32) -> f32 {
        let scale = self.dpi as f32 / 96.0;
        (dip * scale).round() / scale
    }

    fn snap_rect(&self, left: f32, top: f32, right: f32, bottom: f32) -> D2D_RECT_F {
        rect(self.snap(left), self.snap(top), self.snap(right), self.snap(bottom))
    }

    /// One physical pixel, in DIPs.
    fn hairline(&self) -> f32 {
        96.0 / self.dpi as f32
    }
}

impl Renderer {
    pub fn new() -> Result<Self> {
        unsafe {
            let factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let fonts = crate::render::Fonts::installed(&dw);
            let formats = create_formats(&dw, fonts.display, fonts.icons, fonts.mono, 1.0)?;
            Ok(Self {
                factory,
                formats,
                display_font: fonts.display,
                icon_font: fonts.icons,
                mono_font: fonts.mono,
                format_scale: 1.0,
                dw,
                target: None,
                shared_target: None,
            })
        }
    }

    pub fn draw(&mut self, hwnd: HWND, frame: &Frame) -> Result<()> {
        self.update_formats(frame.layout_scale)?;
        self.paint(hwnd, frame.dpi, frame.palette, |target, formats| {
            dock_background(
                target,
                frame.palette,
                frame.layout_scale,
                frame.layout.total_width(frame.parts, frame.layout_scale),
                logic::height(frame.layout_scale),
                true,
            );
            unsafe { draw(target, formats, frame) }.and_then(|()| draw_status(target, formats, frame))
        })
    }

    pub fn draw_on(&mut self, rt: &ID2D1HwndRenderTarget, frame: &Frame) -> Result<()> {
        self.update_formats(frame.layout_scale)?;
        if self.shared_target.as_ref().is_none_or(|t| t.hwnd.as_ref() != Some(rt)) {
            self.shared_target = Some(Target {
                rt: rt.cast()?,
                hwnd: Some(rt.clone()),
                brush: unsafe { rt.CreateSolidColorBrush(&frame.palette.text, None)? },
                bitmaps: HashMap::new(),
                tray: HashMap::new(),
                size: D2D_SIZE_U::default(),
            });
        }
        let target = self.shared_target.as_mut().unwrap();
        dock_background(
            target,
            frame.palette,
            frame.layout_scale,
            frame.layout.total_width(frame.parts, frame.layout_scale),
            logic::height(frame.layout_scale),
            false,
        );
        unsafe { draw(target, &self.formats, frame) }.and_then(|()| draw_status(target, &self.formats, frame))
    }

    fn update_formats(&mut self, scale: f32) -> Result<()> {
        if self.format_scale != scale {
            self.formats = create_formats(&self.dw, self.display_font, self.icon_font, self.mono_font, scale)?;
            self.format_scale = scale;
        }
        Ok(())
    }

    pub fn draw_flyout(&mut self, hwnd: HWND, frame: &FlyoutFrame) -> Result<()> {
        self.paint(hwnd, frame.dpi, frame.palette, |target, _| draw_flyout(target, frame))
    }

    pub fn draw_tip(&mut self, hwnd: HWND, dpi: u32, palette: &Palette, tip: &str) -> Result<()> {
        self.paint(hwnd, dpi, palette, |target, formats| {
            background(target, palette, 1.0);
            let size = unsafe { target.rt.GetSize() };
            unsafe { target.brush.SetColor(&palette.text) };
            text(
                &target.rt,
                &target.brush,
                &formats.tip,
                tip,
                rect(TIP_PAD.0, TIP_PAD.1, size.width - TIP_PAD.0, size.height),
            );
            Ok(())
        })
    }

    pub fn draw_volume(
        &mut self,
        hwnd: HWND,
        dpi: u32,
        p: &Palette,
        level: Option<(u32, bool)>,
        mute_focus: bool,
    ) -> Result<()> {
        self.paint(hwnd, dpi, p, |target, formats| {
            background(target, p, 1.0);
            let (percent, muted) = level.unwrap_or((0, true));
            let rt = &target.rt;
            let brush = &target.brush;
            let top = super::volume::TRACK_TOP;
            let bottom = super::volume::TRACK_BOTTOM;
            let y = bottom - percent as f32 / 100.0 * (bottom - top);
            unsafe {
                brush.SetColor(&p.subtext);
                text(
                    rt,
                    brush,
                    &formats.small,
                    &level.map_or_else(|| "--".into(), |_| format!("{percent}%")),
                    rect(4.0, 4.0, 60.0, 28.0),
                );
                brush.SetColor(&p.scrollbar);
                rt.FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: rect(29.0, top, 35.0, bottom),
                        radiusX: 3.0,
                        radiusY: 3.0,
                    },
                    brush,
                );
                brush.SetColor(&p.accent);
                rt.FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: rect(29.0, y, 35.0, bottom),
                        radiusX: 3.0,
                        radiusY: 3.0,
                    },
                    brush,
                );
                rt.FillEllipse(
                    &D2D1_ELLIPSE {
                        point: windows_numerics::Vector2 { X: 32.0, Y: y },
                        radiusX: 6.0,
                        radiusY: 6.0,
                    },
                    brush,
                );
                brush.SetColor(if mute_focus { &p.row_selected } else { &p.tile });
                rt.FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: rect(12.0, 192.0, 52.0, 224.0),
                        radiusX: 8.0,
                        radiusY: 8.0,
                    },
                    brush,
                );
                brush.SetColor(&p.text);
                text(
                    rt,
                    brush,
                    &formats.glyph,
                    volume_glyph(percent, muted),
                    rect(12.0, 192.0, 52.0, 224.0),
                );
                if mute_focus {
                    brush.SetColor(&p.accent);
                    rt.DrawRoundedRectangle(
                        &D2D1_ROUNDED_RECT {
                            rect: rect(12.0, 192.0, 52.0, 224.0),
                            radiusX: 8.0,
                            radiusY: 8.0,
                        },
                        brush,
                        1.0,
                        None,
                    );
                }
            }
            Ok(())
        })
    }

    /// A tooltip's window size in DIPs, wrapping past `max` DIPs of text.
    pub fn measure_tip(&self, tip: &str, max: f32) -> Result<(f32, f32)> {
        let wide: Vec<u16> = tip.encode_utf16().collect();
        unsafe {
            let layout = self.dw.CreateTextLayout(&wide, &self.formats.tip, max, 1000.0)?;
            let mut metrics = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut metrics)?;
            Ok((
                metrics.widthIncludingTrailingWhitespace.ceil() + 2.0 * TIP_PAD.0,
                metrics.height.ceil() + 2.0 * TIP_PAD.1,
            ))
        }
    }

    /// Draws an app's windows as a list, highlighting the row under the pointer.
    pub fn draw_windows(
        &mut self,
        hwnd: HWND,
        dpi: u32,
        palette: &Palette,
        rows: &[String],
        hover: Option<usize>,
    ) -> Result<()> {
        self.paint(hwnd, dpi, palette, |target, formats| {
            background(target, palette, 1.0);
            let size = unsafe { target.rt.GetSize() };
            for (index, row) in rows.iter().enumerate() {
                let top = logic::LIST_PAD.1 + index as f32 * logic::LIST_ROW;
                let line = rect(
                    logic::LIST_PAD.0,
                    top,
                    size.width - logic::LIST_PAD.0,
                    top + logic::LIST_ROW,
                );
                if hover == Some(index) {
                    unsafe {
                        target.brush.SetColor(&palette.row_selected);
                        target.rt.FillRoundedRectangle(&rounded(line, 4.0), &target.brush);
                    }
                }
                unsafe { target.brush.SetColor(&palette.text) };
                let wide: Vec<u16> = row.encode_utf16().collect();
                unsafe {
                    target.rt.DrawText(
                        &wide,
                        &formats.row,
                        &line,
                        &target.brush,
                        D2D1_DRAW_TEXT_OPTIONS_CLIP,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );
                }
            }
            Ok(())
        })
    }

    /// The window list's window size in DIPs.
    pub fn measure_windows(&self, rows: &[String]) -> Result<(f32, f32)> {
        let mut width = 0.0f32;
        for row in rows {
            let wide: Vec<u16> = row.encode_utf16().collect();
            unsafe {
                let layout = self
                    .dw
                    .CreateTextLayout(&wide, &self.formats.row, logic::LIST_TEXT_MAX, 1000.0)?;
                let mut metrics = DWRITE_TEXT_METRICS::default();
                layout.GetMetrics(&mut metrics)?;
                width = width.max(metrics.widthIncludingTrailingWhitespace);
            }
        }
        Ok((
            width.ceil().min(logic::LIST_TEXT_MAX) + 2.0 * logic::LIST_PAD.0,
            rows.len() as f32 * logic::LIST_ROW + 2.0 * logic::LIST_PAD.1,
        ))
    }

    /// Draws the desktop pill: a dot per desktop, the current one long, then its name.
    pub fn draw_desktops(
        &mut self,
        hwnd: HWND,
        dpi: u32,
        palette: &Palette,
        (current, count): (usize, usize),
        name: &str,
    ) -> Result<()> {
        self.paint(hwnd, dpi, palette, |target, formats| {
            background(target, palette, 1.0);
            let size = unsafe { target.rt.GetSize() };
            let middle = size.height / 2.0;
            let mut x = DESKTOP_PAD;
            for index in 0..count {
                let (width, color) = if index == current {
                    (DESKTOP_DOT_CURRENT, palette.text)
                } else {
                    (
                        DESKTOP_DOT,
                        D2D1_COLOR_F {
                            a: 0.35,
                            ..palette.text
                        },
                    )
                };
                unsafe {
                    target.brush.SetColor(&color);
                    target.rt.FillRoundedRectangle(
                        &rounded(
                            rect(x, middle - DESKTOP_DOT / 2.0, x + width, middle + DESKTOP_DOT / 2.0),
                            DESKTOP_DOT / 2.0,
                        ),
                        &target.brush,
                    );
                }
                x += width + DESKTOP_GAP;
            }
            unsafe { target.brush.SetColor(&palette.text) };
            let left = x - DESKTOP_GAP + DESKTOP_LABEL_GAP;
            text(
                &target.rt,
                &target.brush,
                &formats.tip,
                name,
                rect(left, TIP_PAD.1, size.width, size.height),
            );
            Ok(())
        })
    }

    /// The desktop pill's window size in DIPs.
    pub fn measure_desktops(&self, count: usize, name: &str) -> Result<(f32, f32)> {
        let (label, height) = self.measure_tip(name, 1000.0)?;
        let dots = count as f32 * (DESKTOP_DOT + DESKTOP_GAP) - DESKTOP_GAP + DESKTOP_DOT_CURRENT - DESKTOP_DOT;
        let label = label - 2.0 * TIP_PAD.0;
        Ok((2.0 * DESKTOP_PAD + dots + DESKTOP_LABEL_GAP + label, height))
    }

    /// Renders the dock onto a bitmap target for the preview test, over the
    /// given desktop color. The dock's own fill is what unifies the look; only
    /// the blurred acrylic of a live window is missing.
    #[cfg(test)]
    pub fn draw_to(&mut self, rt: &ID2D1RenderTarget, frame: &Frame, desktop: D2D1_COLOR_F) -> Result<()> {
        self.update_formats(frame.layout_scale)?;
        let mut target = Target {
            rt: rt.clone(),
            hwnd: None,
            brush: unsafe { rt.CreateSolidColorBrush(&frame.palette.text, None)? },
            bitmaps: HashMap::new(),
            tray: HashMap::new(),
            size: D2D_SIZE_U::default(),
        };
        unsafe {
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            rt.BeginDraw();
            rt.Clear(Some(&desktop));
        }
        dock_background(
            &target,
            frame.palette,
            frame.layout_scale,
            frame.layout.total_width(frame.parts, frame.layout_scale),
            logic::height(frame.layout_scale),
            false,
        );
        let drawn = unsafe { draw(&mut target, &self.formats, frame) }
            .and_then(|()| draw_status(&mut target, &self.formats, frame));
        let ended = unsafe { rt.EndDraw(None, None) };
        drawn.and(ended)
    }

    /// Sizes the target to the window, then draws with `f` between BeginDraw and EndDraw.
    fn paint(
        &mut self,
        hwnd: HWND,
        dpi: u32,
        palette: &Palette,
        f: impl FnOnce(&mut Target, &Formats) -> Result<()>,
    ) -> Result<()> {
        unsafe {
            let mut client = RECT::default();
            GetClientRect(hwnd, &mut client)?;
            let size = D2D_SIZE_U {
                width: client.right.max(1) as u32,
                height: client.bottom.max(1) as u32,
            };
            if self.target.is_none() {
                let rt = self.factory.CreateHwndRenderTarget(
                    &D2D1_RENDER_TARGET_PROPERTIES {
                        r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                        pixelFormat: D2D1_PIXEL_FORMAT {
                            format: DXGI_FORMAT_B8G8R8A8_UNORM,
                            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                        },
                        ..Default::default()
                    },
                    &D2D1_HWND_RENDER_TARGET_PROPERTIES {
                        hwnd,
                        pixelSize: size,
                        presentOptions: D2D1_PRESENT_OPTIONS_IMMEDIATELY,
                    },
                )?;
                rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
                self.target = Some(Target {
                    brush: rt.CreateSolidColorBrush(&palette.text, None)?,
                    rt: rt.cast()?,
                    hwnd: Some(rt),
                    bitmaps: HashMap::new(),
                    tray: HashMap::new(),
                    size,
                });
            }
            let target = self.target.as_mut().unwrap();
            if let Some(rt) = &target.hwnd
                && (target.size.width != size.width || target.size.height != size.height)
            {
                rt.Resize(&size)?;
                target.size = size;
            }
            target.rt.SetDpi(dpi as f32, dpi as f32);
            target.rt.BeginDraw();
            let drawn = f(target, &self.formats);
            let ended = target.rt.EndDraw(None, None);
            let result = drawn.and(ended);
            if result.as_ref().is_err_and(|e| e.code() == D2DERR_RECREATE_TARGET) {
                self.target = None;
                return Ok(());
            }
            result
        }
    }
}

/// A tooltip's text inset, in DIPs.
const TIP_PAD: (f32, f32) = (8.0, 5.0);
/// The desktop pill's dots and spacing, in DIPs.
const DESKTOP_PAD: f32 = 10.0;
const DESKTOP_DOT: f32 = 6.0;
const DESKTOP_DOT_CURRENT: f32 = 16.0;
const DESKTOP_GAP: f32 = 5.0;
const DESKTOP_LABEL_GAP: f32 = 9.0;

fn rect(left: f32, top: f32, right: f32, bottom: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left,
        top,
        right,
        bottom,
    }
}
fn rounded(rect: D2D_RECT_F, radius: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT {
        rect,
        radiusX: radius,
        radiusY: radius,
    }
}

/// The rounded highlight behind a hovered or active slot or part.
fn tile(target: &Target, frame: &Frame, left: f32, right: f32, color: &D2D1_COLOR_F) {
    unsafe {
        target.brush.SetColor(color);
        target.rt.FillRoundedRectangle(
            &rounded(
                frame.snap_rect(
                    left,
                    TILE_TOP * frame.layout_scale,
                    right,
                    TILE_BOTTOM * frame.layout_scale,
                ),
                TILE_RADIUS * frame.layout_scale,
            ),
            &target.brush,
        );
    }
}

/// DWM rounds the corners and draws the backdrop; this only tints it.
fn background(target: &Target, palette: &Palette, layout_scale: f32) {
    crate::theme::paint_background(&target.rt, &target.brush, palette, 8.0 * layout_scale);
}

/// The dock uses the same tinted glass, accent and text colors as search.
/// `clear` resets a dedicated window's target.
fn dock_background(target: &Target, palette: &Palette, layout_scale: f32, width: f32, height: f32, clear: bool) {
    let radius = logic::RADIUS * layout_scale;
    if clear {
        crate::theme::paint_background(&target.rt, &target.brush, palette, radius);
    } else {
        unsafe {
            target.brush.SetColor(&D2D1_COLOR_F {
                a: 0.94,
                ..palette.background_solid
            });
            target
                .rt
                .FillRoundedRectangle(&rounded(rect(0.0, 0.0, width, height), radius), &target.brush);
        }
    }
    let (mut dpi, mut dpi_y) = (96.0, 96.0);
    unsafe { target.rt.GetDpi(&mut dpi, &mut dpi_y) };
    let pixel = 96.0 / dpi;
    unsafe {
        target.brush.SetColor(&D2D1_COLOR_F {
            a: 0.035,
            ..palette.accent
        });
        target.rt.FillRoundedRectangle(
            &rounded(rect(1.0 * pixel, 1.0 * pixel, width - pixel, height - pixel), radius),
            &target.brush,
        );
        target.brush.SetColor(&D2D1_COLOR_F {
            a: 0.42,
            ..palette.accent
        });
        target.rt.DrawRoundedRectangle(
            &rounded(
                rect(pixel / 2.0, pixel / 2.0, width - pixel / 2.0, height - pixel / 2.0),
                radius,
            ),
            &target.brush,
            pixel,
            None,
        );
        // A soft highlight gives the theme-colored rim a glass edge.
        target.brush.SetColor(&D2D1_COLOR_F {
            a: 0.15,
            ..palette.text
        });
        target
            .rt
            .FillRectangle(&rect(radius, pixel, width - radius, pixel * 2.0), &target.brush);
    }
}

/// Draws a notification icon centered on (`x`, `y`). Its pixels are made
/// for this DPI, so they map one to one onto the screen.
fn tray_icon(target: &mut Target, dpi: u32, palette: &Palette, shown: &Shown, x: f32, y: f32) -> Result<()> {
    let Some(pixels) = &shown.pixels else { return Ok(()) };
    let key = shown.icon.key();
    let scale = dpi as f32 / 96.0;
    let stale = target.tray.get(&key).is_none_or(|(old, _)| !Arc::ptr_eq(old, pixels));
    unsafe {
        if stale {
            let bitmap = target.rt.CreateBitmap(
                D2D_SIZE_U {
                    width: pixels.width,
                    height: pixels.height,
                },
                Some(pixels.bgra.as_ptr().cast()),
                pixels.width * 4,
                &D2D1_BITMAP_PROPERTIES {
                    pixelFormat: D2D1_PIXEL_FORMAT {
                        format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                    },
                    dpiX: 96.0,
                    dpiY: 96.0,
                },
            )?;
            target.tray.insert(key, (pixels.clone(), bitmap));
        }
        let (width, height) = (pixels.width as f32 / scale, pixels.height as f32 / scale);
        let left = ((x - width / 2.0) * scale).round() / scale;
        let top = ((y - height / 2.0) * scale).round() / scale;
        if is_dark(pixels) {
            target.brush.SetColor(&palette.icon_plate);
            target.rt.FillRoundedRectangle(
                &rounded(rect(left - 1.0, top - 1.0, left + width + 1.0, top + height + 1.0), 4.0),
                &target.brush,
            );
        }
        target.rt.DrawBitmap(
            &target.tray[&key].1,
            Some(&rect(left, top, left + width, top + height)),
            1.0,
            D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
            None,
        );
    }
    Ok(())
}

fn draw_flyout(target: &mut Target, frame: &FlyoutFrame) -> Result<()> {
    background(target, frame.palette, 1.0);
    target
        .tray
        .retain(|key, _| frame.tray.iter().any(|shown| shown.icon.key() == *key));
    let scale = frame.dpi as f32 / 96.0;
    let snap = |dip: f32| (dip * scale).round() / scale;
    let cell = logic::FLYOUT_CELL;
    for (index, shown) in frame.tray.iter().enumerate() {
        let (left, top) = logic::flyout_cell(index);
        if frame.hover == Some(index) {
            unsafe {
                target.brush.SetColor(&frame.palette.row_selected);
                target.rt.FillRoundedRectangle(
                    &rounded(
                        rect(
                            snap(left + 2.0),
                            snap(top + 2.0),
                            snap(left + cell - 2.0),
                            snap(top + cell - 2.0),
                        ),
                        TILE_RADIUS,
                    ),
                    &target.brush,
                );
            }
        }
        tray_icon(
            target,
            frame.dpi,
            frame.palette,
            shown,
            left + cell / 2.0,
            top + cell / 2.0,
        )?;
    }
    Ok(())
}

/// A one-pixel vertical rule at `x`, between the apps and the status.
fn separator(target: &Target, frame: &Frame, x: f32) {
    let x = frame.snap(x);
    unsafe {
        target.brush.SetColor(&frame.palette.divider);
        target.rt.FillRectangle(
            &frame.snap_rect(
                x,
                20.0 * frame.layout_scale,
                x + frame.hairline(),
                44.0 * frame.layout_scale,
            ),
            &target.brush,
        );
    }
}

unsafe fn draw(target: &mut Target, formats: &Formats, frame: &Frame) -> Result<()> {
    unsafe {
        let Frame {
            palette: p,
            items,
            icons,
            hover,
            app_icon_size,
            scroll,
            ..
        } = frame;
        let running = frame.count();
        if frame.layout.divider() {
            separator(
                target,
                frame,
                logic::left_scaled(frame.layout.pinned + 1, frame.layout_scale)
                    + logic::separator_width(frame.layout_scale) / 2.0,
            );
        }
        let foreground = GetForegroundWindow().0 as isize;
        target.bitmaps.retain(|key, _| icons.contains_key(key));
        let scale = frame.layout_scale;
        let slot = logic::slot(scale);
        // A soft accent glow follows the magnification wave. Drawn in its
        // own pass so neighboring glows never sit on top of an icon.
        for index in 1..=running {
            let progress = frame.hover_levels.get(index).copied().unwrap_or(0.0);
            if progress <= 0.001 {
                continue;
            }
            let x = frame.layout.left(index, scale);
            let icon_size = *app_icon_size * (1.0 + MAGNIFY * progress);
            let bottom = ICON_CENTER_Y * scale + app_icon_size / 2.0;
            let top = bottom - icon_size;
            let pad = 3.0 * scale + 2.0 * progress * scale;
            target.brush.SetColor(&D2D1_COLOR_F {
                a: 0.12 * progress,
                ..p.accent
            });
            target.rt.FillRoundedRectangle(
                &rounded(
                    frame.snap_rect(
                        x + (slot - icon_size) / 2.0 - pad,
                        (top - pad).max(scale),
                        x + (slot + icon_size) / 2.0 + pad,
                        bottom + 2.0 * scale,
                    ),
                    11.0 * scale,
                ),
                &target.brush,
            );
        }
        for index in 0..=running {
            let x = frame.layout.left(index, scale);
            let progress = frame.hover_levels.get(index).copied().unwrap_or(0.0);
            let eased = progress * progress * (3.0 - 2.0 * progress);
            let item = index.checked_sub(1).and_then(|i| items.get(i + scroll));
            let active = item.is_some_and(|item| item.windows.iter().any(|window| window.hwnd == foreground));
            if index == 0 {
                // A terminal prompt stands in for the search glyph.
                if *hover == Some(0) {
                    tile(target, frame, x + 4.0 * scale, x + slot - 4.0 * scale, &p.row_selected);
                }
                target.brush.SetColor(&D2D1_COLOR_F { a: 0.88, ..p.accent });
                text(
                    &target.rt,
                    &target.brush,
                    &formats.prompt,
                    ">_",
                    frame.snap_rect(x, TILE_TOP * scale, x + slot, TILE_BOTTOM * scale),
                );
                continue;
            }
            let icon_size = *app_icon_size * (1.0 + MAGNIFY * eased);
            let icon_bottom = ICON_CENTER_Y * scale + app_icon_size / 2.0;
            let icon_top = icon_bottom - icon_size;
            let icon_rect = frame.snap_rect(
                x + (slot - icon_size) / 2.0,
                icon_top,
                x + (slot + icon_size) / 2.0,
                icon_bottom,
            );
            let Some(item) = item else {
                continue;
            };
            let fallback = item
                .app
                .path
                .as_ref()
                .map(|path| format!("exe:{}", path.to_lowercase()));
            let icon = icons
                .get_key_value(&item.app.id)
                .or_else(|| fallback.as_ref().and_then(|key| icons.get_key_value(key)));
            if let Some((key, pixels)) = icon {
                let stale = target
                    .bitmaps
                    .get(key)
                    .is_none_or(|old| !Arc::ptr_eq(&old.pixels, pixels));
                if stale {
                    let bitmap = target.rt.CreateBitmap(
                        D2D_SIZE_U {
                            width: pixels.width,
                            height: pixels.height,
                        },
                        Some(pixels.bgra.as_ptr().cast()),
                        pixels.width * 4,
                        &D2D1_BITMAP_PROPERTIES {
                            pixelFormat: D2D1_PIXEL_FORMAT {
                                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                            },
                            dpiX: 96.0,
                            dpiY: 96.0,
                        },
                    )?;
                    target.bitmaps.insert(
                        key.clone(),
                        Bitmap {
                            pixels: pixels.clone(),
                            bitmap,
                            dark: is_dark(pixels),
                        },
                    );
                }
                if target.bitmaps[key].dark {
                    // Black artwork needs something lit to sit on. Only dark
                    // icons get a plate, so the glass stays free of tile boxes.
                    // The plate has to be nearly opaque: at 0.15 the black
                    // ChatGPT mark read as an empty square on the dark glass.
                    target.brush.SetColor(&p.icon_plate);
                    target
                        .rt
                        .FillRoundedRectangle(&rounded(icon_rect, icon_size * 0.22), &target.brush);
                }
                target.rt.DrawBitmap(
                    &target.bitmaps[key].bitmap,
                    Some(&icon_rect),
                    1.0,
                    D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                    None,
                );
            } else {
                target.brush.SetColor(&p.tile);
                target.rt.FillRoundedRectangle(&rounded(icon_rect, 6.0), &target.brush);
                target.brush.SetColor(&p.text);
                let initial = item.app.name.chars().next().unwrap_or('?').to_uppercase().to_string();
                text(&target.rt, &target.brush, &formats.letter, &initial, icon_rect);
            }
            if !item.windows.is_empty() {
                // A dot for running apps, a longer accent dash for the foreground one, and
                // a second dot when the app has more than one window.
                const GAP: f32 = 2.0;
                let marks: &[f32] = match (active, item.windows.len() > 1) {
                    (false, false) => &[3.0],
                    (false, true) => &[3.0, 3.0],
                    (true, false) => &[12.0],
                    (true, true) => &[12.0, 3.0],
                };
                let total = marks.iter().sum::<f32>() + GAP * (marks.len() - 1) as f32;
                let mut left = x + slot / 2.0 - total / 2.0 * scale;
                for &width in marks {
                    let bar = frame.snap_rect(
                        left,
                        INDICATOR_TOP * scale,
                        left + width * scale,
                        (INDICATOR_TOP + 3.0) * scale,
                    );
                    if active {
                        // A glow lifts the active app's dash off the glass.
                        target.brush.SetColor(&D2D1_COLOR_F { a: 0.18, ..p.accent });
                        target.rt.FillRoundedRectangle(
                            &rounded(
                                D2D_RECT_F {
                                    left: bar.left - 3.0 * scale,
                                    top: bar.top - 1.0 * scale,
                                    right: bar.right + 3.0 * scale,
                                    bottom: bar.bottom + 1.0 * scale,
                                },
                                4.0 * scale,
                            ),
                            &target.brush,
                        );
                    }
                    target.brush.SetColor(if active { &p.accent } else { &p.subtext });
                    target
                        .rt
                        .FillRoundedRectangle(&rounded(bar, 1.5 * scale), &target.brush);
                    left += (width + GAP) * scale;
                }
            }
        }
        Ok(())
    }
}

/// Alpha-weighted luminance, so a mostly transparent icon is judged by its ink.
fn is_dark(pixels: &IconPixels) -> bool {
    let mut weight = 0.0f32;
    let mut luma = 0.0f32;
    for px in pixels.bgra.as_chunks::<4>().0 {
        let alpha = px[3] as f32 / 255.0;
        weight += alpha;
        luma += alpha * (0.2126 * px[2] as f32 + 0.7152 * px[1] as f32 + 0.0722 * px[0] as f32);
    }
    weight > 0.0 && luma / (weight * 255.0) < 0.18
}

fn text(
    rt: &ID2D1RenderTarget,
    brush: &ID2D1SolidColorBrush,
    format: &IDWriteTextFormat,
    text: &str,
    area: D2D_RECT_F,
) {
    let text: Vec<u16> = text.encode_utf16().collect();
    unsafe {
        rt.DrawText(
            &text,
            format,
            &area,
            brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
            DWRITE_MEASURING_MODE_NATURAL,
        );
    }
}

/// Segoe Fluent Icons speaker: muted, silent, or one to three waves.
fn volume_glyph(percent: u32, muted: bool) -> &'static str {
    match (muted, percent) {
        (true, _) => "\u{E74F}",
        (_, 0) => "\u{E992}",
        (_, 1..=33) => "\u{E993}",
        (_, 34..=66) => "\u{E994}",
        _ => "\u{E995}",
    }
}

/// Segoe Fluent Icons battery in tenths, from Battery0 (U+EBA0) or BatteryCharging0 (U+EBAB).
fn battery_glyph(percent: u32, charging: bool) -> String {
    let first = if charging { 0xEBAB } else { 0xEBA0 };
    char::from_u32(first + (percent.min(100) + 5) / 10).map_or_else(String::new, String::from)
}

/// A separator, then the volume, battery and clock, each with a second line.
fn draw_status(target: &mut Target, formats: &Formats, frame: &Frame) -> Result<()> {
    let Frame {
        palette: p,
        status,
        hover_part,
        parts,
        ..
    } = frame;
    target
        .tray
        .retain(|key, _| frame.tray.iter().any(|shown| shown.icon.key() == *key));
    separator(
        target,
        frame,
        frame.layout.width(frame.layout_scale) - logic::pad(frame.layout_scale)
            + logic::separator_width(frame.layout_scale) / 2.0,
    );
    for &part in parts.iter() {
        let Some(left) = frame.layout.part_left(part, parts, frame.layout_scale) else {
            continue;
        };
        let right = left + part.width_scaled(frame.layout_scale);
        if *hover_part == Some(part) || part == Part::Chevron && frame.flyout_open {
            tile(
                target,
                frame,
                left + 2.0 * frame.layout_scale,
                right - 2.0 * frame.layout_scale,
                &p.row_selected,
            );
        }
        match part {
            Part::Chevron | Part::Power | Part::ShowDesktop => {
                unsafe { target.brush.SetColor(&p.text) };
                let glyph = match part {
                    Part::Power => "\u{E7E8}",
                    Part::ShowDesktop => "\u{E977}",
                    _ if frame.flyout_open => "\u{E70D}",
                    _ => "\u{E70E}",
                };
                let area = rect(
                    left,
                    TILE_TOP * frame.layout_scale,
                    right,
                    TILE_BOTTOM * frame.layout_scale,
                );
                text(
                    &target.rt,
                    &target.brush,
                    if part != Part::Chevron {
                        &formats.glyph
                    } else {
                        &formats.chevron
                    },
                    glyph,
                    area,
                );
                continue;
            }
            Part::Tray(index) => {
                if let Some(shown) = frame.inline.get(index).and_then(|&i| frame.tray.get(i)) {
                    let middle = (TILE_TOP + TILE_BOTTOM) / 2.0 * frame.layout_scale;
                    tray_icon(
                        target,
                        frame.dpi,
                        p,
                        shown,
                        left + part.width_scaled(frame.layout_scale) / 2.0,
                        middle,
                    )?;
                }
                continue;
            }
            _ => {}
        }
        let (rt, brush) = (&target.rt, &target.brush);
        let upper = rect(left, UPPER.0 * frame.layout_scale, right, UPPER.1 * frame.layout_scale);
        let lower = rect(left, LOWER.0 * frame.layout_scale, right, LOWER.1 * frame.layout_scale);
        unsafe {
            match part {
                Part::Volume => {
                    let Some(volume) = status.volume else { continue };
                    brush.SetColor(&p.text);
                    text(
                        rt,
                        brush,
                        &formats.glyph,
                        volume_glyph(volume.percent, volume.muted),
                        upper,
                    );
                    brush.SetColor(&p.subtext);
                    text(rt, brush, &formats.small, &format!("{}%", volume.percent), lower);
                }
                Part::Battery => {
                    let Some(level) = status.battery else { continue };
                    let low = level.percent <= 20 && !level.charging;
                    brush.SetColor(if low { &RED } else { &p.text });
                    text(
                        rt,
                        brush,
                        &formats.glyph,
                        &battery_glyph(level.percent, level.charging),
                        upper,
                    );
                    brush.SetColor(&p.subtext);
                    text(rt, brush, &formats.small, &format!("{}%", level.percent), lower);
                }
                Part::Clock => {
                    brush.SetColor(&p.text);
                    text(rt, brush, &formats.time, &status.time, upper);
                    brush.SetColor(&p.subtext);
                    text(rt, brush, &formats.small, &status.date, lower);
                }
                Part::Chevron | Part::Tray(_) | Part::Power | Part::ShowDesktop => {}
            }
        }
    }
    Ok(())
}

const RED: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 0.91,
    g: 0.30,
    b: 0.27,
    a: 1.0,
};

#[cfg(test)]
mod probe {
    use super::*;
    use crate::commands::open_windows::Window;
    use crate::dock::Item as DockItem;
    use crate::dock::logic::AppLayout;
    use crate::dock::status::{Battery, Volume};
    use crate::indexer::AppEntry;
    use std::path::PathBuf;
    use windows::Win32::Graphics::Imaging::*;
    use windows::Win32::System::Com::*;
    use windows::core::{GUID, HSTRING};

    #[test]
    fn dark_icons_are_told_apart_from_colorful_ones() {
        let icon = |rgb: [u8; 3]| IconPixels {
            width: 2,
            height: 2,
            bgra: (0..4).flat_map(|_| [rgb[2], rgb[1], rgb[0], 255]).collect(),
        };
        assert!(is_dark(&icon([0, 0, 0])), "pure black needs a plate");
        assert!(is_dark(&icon([20, 24, 22])), "near black needs a plate");
        assert!(!is_dark(&icon([59, 255, 146])), "the accent does not");
        assert!(!is_dark(&icon([240, 240, 240])), "white does not");
        let clear = IconPixels {
            width: 2,
            height: 2,
            bgra: vec![0; 16],
        };
        assert!(!is_dark(&clear), "a fully transparent icon has no ink");
    }

    /// Draws the dock through the real renderer into a PNG, so the shell, the
    /// magnification wave and the status parts can be inspected without a
    /// screen capture.
    #[test]
    #[ignore = "writes a preview PNG"]
    fn renders_the_dock_to_a_png() {
        std::thread::spawn(|| unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
            let entries = [
                ("notepad", "Notepad", "System32\\notepad.exe", 1),
                ("explorer", "File Explorer", "explorer.exe", 2),
                ("cmd", "Command Prompt", "System32\\cmd.exe", 1),
                ("mspaint", "Paint", "System32\\mspaint.exe", 2),
                ("calc", "Calculator", "System32\\calc.exe", 1),
                ("snipping", "Snipping Tool", "System32\\SnippingTool.exe", 1),
            ];
            let mut items = Vec::new();
            let mut icons = HashMap::new();
            for (id, name, relative, windows) in entries {
                let path = PathBuf::from(&windir).join(relative).to_string_lossy().into_owned();
                if let Some(pixels) = crate::desk::icons::load(&path, 64) {
                    icons.insert(id.to_string(), Arc::new(pixels));
                }
                let windows = (0..windows)
                    .map(|index| Window {
                        // Zero stands for the foreground window in a locked
                        // session, so one app shows its active indicator.
                        hwnd: if id == "calc" && index == 0 { 0 } else { index + 1 },
                        title: format!("{name} {index}"),
                        exe: path.clone(),
                    })
                    .collect();
                items.push(DockItem {
                    app: AppEntry {
                        name: name.into(),
                        id: id.into(),
                        path: Some(path),
                    },
                    windows,
                });
            }
            // A pure-black icon with alpha, like ChatGPT's: it vanishes on the
            // dark glass unless the renderer gives it a plate.
            let mut ink = vec![0u8; 64 * 64 * 4];
            for y in 0..64i32 {
                for x in 0..64i32 {
                    let (dx, dy) = (x - 32, y - 32);
                    if dx * dx + dy * dy <= 26 * 26 {
                        let at = ((y * 64 + x) * 4) as usize;
                        ink[at] = 0;
                        ink[at + 1] = 0;
                        ink[at + 2] = 0;
                        ink[at + 3] = 255;
                    }
                }
            }
            icons.insert(
                "chatgpt".into(),
                Arc::new(IconPixels {
                    width: 64,
                    height: 64,
                    bgra: ink,
                }),
            );
            items.push(DockItem {
                app: AppEntry {
                    name: "ChatGPT".into(),
                    id: "chatgpt".into(),
                    path: None,
                },
                windows: Vec::new(),
            });
            let parts = logic::parts(None, true, true);
            let status = Status {
                time: "23:58".into(),
                date: "Thu 12 Nov".into(),
                battery: Some(Battery {
                    percent: 82,
                    charging: false,
                }),
                volume: Some(Volume {
                    percent: 64,
                    muted: false,
                }),
                minute: 0,
            };
            let levels = [0.0, 0.11, 0.45, 1.0, 0.45, 0.11, 0.0];
            let palette = Palette::new(true, true);
            let frame = Frame {
                layout: AppLayout {
                    count: items.len(),
                    pinned: 3,
                },
                dpi: 144,
                palette: &palette,
                items: &items,
                icons: &icons,
                hover: Some(3),
                hover_levels: &levels,
                app_icon_size: 32.0,
                layout_scale: 1.0,
                capacity: items.len(),
                scroll: 0,
                status: &status,
                hover_part: None,
                parts: &parts,
                tray: &[],
                inline: &[],
                flyout_open: false,
            };

            let dpi = 144.0;
            let width = (frame.layout.total_width(&parts, 1.0) * dpi / 96.0).ceil() as u32;
            let height = (logic::height(1.0) * dpi / 96.0).ceil() as u32;
            let factory: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory2, None, CLSCTX_INPROC_SERVER).unwrap();
            let bitmap = factory
                .CreateBitmap(width, height, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnLoad)
                .unwrap();
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).unwrap();
            let rt = d2d
                .CreateWicBitmapRenderTarget(
                    &bitmap,
                    &D2D1_RENDER_TARGET_PROPERTIES {
                        pixelFormat: D2D1_PIXEL_FORMAT {
                            format: DXGI_FORMAT_B8G8R8A8_UNORM,
                            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                        },
                        dpiX: dpi,
                        dpiY: dpi,
                        ..Default::default()
                    },
                )
                .unwrap();
            let mut renderer = Renderer::new().unwrap();
            let desktop = D2D1_COLOR_F {
                r: 0.08,
                g: 0.10,
                b: 0.13,
                a: 1.0,
            };
            renderer.draw_to(&rt, &frame, desktop).unwrap();

            let path = std::env::temp_dir().join("opencode").join("dock-render.png");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let stream = factory.CreateStream().unwrap();
            stream
                .InitializeFromFilename(&HSTRING::from(path.as_os_str()), 0x4000_0000)
                .unwrap();
            let encoder = factory
                .CreateEncoder(&GUID_ContainerFormatPng, &GUID::zeroed())
                .unwrap();
            encoder.Initialize(&stream, WICBitmapEncoderNoCache).unwrap();
            let mut png: Option<IWICBitmapFrameEncode> = None;
            encoder.CreateNewFrame(&mut png, std::ptr::null_mut()).unwrap();
            let png = png.unwrap();
            png.Initialize(None).unwrap();
            png.SetSize(width, height).unwrap();
            let mut format = GUID_WICPixelFormat32bppPBGRA;
            png.SetPixelFormat(&mut format).unwrap();
            png.WriteSource(&bitmap, std::ptr::null()).unwrap();
            png.Commit().unwrap();
            encoder.Commit().unwrap();
            println!("saved {} ({width}x{height}, {} apps)", path.display(), items.len());
        })
        .join()
        .unwrap();
    }

    /// Reports how dark each pinned app's icon is, and whether it even loads,
    /// so a black-on-black icon can be told apart from a missing one.
    #[test]
    #[ignore = "reads the live pins and their real icons"]
    fn prints_pin_icon_contrast() {
        std::thread::spawn(|| unsafe {
            use windows::Win32::UI::Shell::{IShellItem, SHCreateItemFromParsingName};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let pins = crate::dock::pins::load(&crate::util::data_dir().join("dock-pins.toml"));
            for pin in pins {
                let parsing = match &pin.path {
                    Some(path) => path.clone(),
                    None => format!("shell:AppsFolder\\{}", pin.id),
                };
                let item: Result<IShellItem> = SHCreateItemFromParsingName(&HSTRING::from(parsing.as_str()), None);
                let item = item.ok();
                for size in [32, 48, 64, 128, 256] {
                    let pixels = item.as_ref().and_then(|item| crate::indexer::load_icon(item, size));
                    let Some(pixels) = pixels else {
                        println!("{:24} {size:3} NO ICON", pin.name);
                        continue;
                    };
                    let mut weight = 0.0;
                    let mut luma = 0.0;
                    let mut bright = 0;
                    let mut opaque = 0;
                    for px in pixels.bgra.as_chunks::<4>().0 {
                        let a = px[3] as f32 / 255.0;
                        let l = (0.0722 * px[2] as f32 + 0.7152 * px[1] as f32 + 0.2126 * px[0] as f32) / 255.0;
                        weight += a;
                        luma += l * a;
                        if a > 0.5 {
                            opaque += 1;
                            if l > 0.6 {
                                bright += 1;
                            }
                        }
                    }
                    println!(
                        "{:24} {size:3} coverage={:.2} luma={:.3} bright={:.0}%",
                        pin.name,
                        opaque as f32 / (pixels.width * pixels.height) as f32,
                        if weight > 0.0 { luma / weight } else { 0.0 },
                        bright as f32 / opaque.max(1) as f32 * 100.0,
                    );
                }
            }
        })
        .join()
        .unwrap();
    }
}
