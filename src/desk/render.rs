//! Draws desk fences with Direct2D, and answers where each part is.
//!
//! Fence geometry is in device-independent pixels relative to the monitor's
//! top-left corner, which is also the desk window's origin. Keeping the hit
//! testing here means the window code stays about input, not layout.
//!
//! The look follows Pickets (formerly OpenFences, MIT): a rounded card with a
//! slightly lighter title bar, a count badge, a roll-up button, and a grid of
//! icon tiles that highlight under the pointer. Icons sit directly on the
//! card, and a name that does not fit wraps to two lines.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use windows::Win32::Foundation::{COLORREF, D2DERR_RECREATE_TARGET, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, GetWindowRect, ULW_ALPHA, UpdateLayeredWindow};
use windows::core::{Result, w};
use windows_numerics::Vector2;

use crate::desk::{DETAIL_HEADER_HEIGHT, DETAIL_ROW_HEIGHT, FenceView, PAD, Placed, TAB_HEIGHT, TITLE_HEIGHT};
use crate::indexer::IconPixels;
use crate::render::Fonts;
use crate::theme::Palette;
use crate::util::wide;

/// Corner radius of a fence card, in DIPs.
pub const RADIUS: f32 = 10.0;
/// Radius of an item's hover tile, in DIPs.
const TILE_RADIUS: f32 = 6.0;
/// Inset of a hover tile inside its cell, in DIPs.
const TILE_INSET: f32 = 3.0;
/// Space above an icon inside its cell, in DIPs.
const ICON_TOP: f32 = 6.0;
/// Space between an icon and its name, in DIPs.
const LABEL_GAP: f32 = 5.0;
/// Height of the two-line name area, in DIPs.
const LABEL_H: f32 = 28.0;
/// Left inset of the title, in DIPs.
const TITLE_INSET: f32 = 10.0;
/// The roll-up button, in DIPs.
const BUTTON_W: f32 = 28.0;
const BUTTON_H: f32 = 24.0;
const BUTTON_GAP: f32 = 4.0;
/// Width kept free at the right of the title for the count and button.
const CHEVRON: f32 = BUTTON_W + 2.0 * BUTTON_GAP + 24.0;

/// The part of the desk at a point, in DIPs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    /// The fence's title bar, which drags the fence.
    Title(usize),
    /// Back to the previous folder in a browsable fence.
    NavigateBack(usize),
    /// The roll-up button in the title bar.
    Collapse(usize),
    Tab(usize, usize),
    TabBar(usize),
    NewTab(usize),
    /// Body of a fence, away from any item.
    Fence(usize),
    /// Item `slot` of a fence.
    Item(usize, usize),
    /// Resize grip at the lower-right corner.
    Resize(usize),
    /// Scrollbar within a fence's contents.
    Scrollbar(usize),
}

pub struct Frame<'a> {
    pub palette: &'a Palette,
    pub fences: &'a [Placed],
    pub icons: &'a HashMap<String, Arc<IconPixels>>,
    pub show_labels: bool,
    pub hover: Option<Hit>,
}

/// The fence at `(x, y)` DIPs, topmost first.
pub fn hit(fences: &[Placed], x: f32, y: f32) -> Option<Hit> {
    for (index, fence) in fences.iter().enumerate().rev() {
        let (left, top) = (fence.x as f32, fence.y as f32);
        let (width, height) = (fence.width as f32, fence.height as f32);
        if !(left..left + width).contains(&x) || !(top..top + height).contains(&y) {
            continue;
        }
        let title = (top..top + TITLE_HEIGHT as f32).contains(&y);
        if title {
            if fence.browsing && x < left + 34.0 {
                return Some(Hit::NavigateBack(index));
            }
            if x >= left + width - CHEVRON {
                return Some(Hit::Collapse(index));
            }
            return Some(Hit::Title(index));
        }
        let tabs_top = top + TITLE_HEIGHT as f32;
        if !fence.collapsed && (tabs_top..tabs_top + TAB_HEIGHT as f32).contains(&y) {
            let mut x_cursor = left + 7.0;
            for (tab_index, name) in fence.tabs.iter().enumerate() {
                let tab_width = (name.chars().count() as f32 * 7.5 + 24.0).clamp(50.0, 150.0);
                if (x_cursor..x_cursor + tab_width).contains(&x) {
                    return Some(Hit::Tab(index, tab_index));
                }
                x_cursor += tab_width + 4.0;
            }
            if fence.tabs.len() < 8 && (x_cursor..x_cursor + 22.0).contains(&x) {
                return Some(Hit::NewTab(index));
            }
            return Some(Hit::TabBar(index));
        }
        if fence.collapsed {
            return Some(Hit::Fence(index));
        }
        if x >= left + width - 16.0 && y >= top + height - 16.0 {
            return Some(Hit::Resize(index));
        }
        if fence.max_scroll() > 0
            && x >= left + width - 8.0
            && x < left + width - 1.0
            && y >= top + TITLE_HEIGHT as f32 + TAB_HEIGHT as f32 + 5.0
            && y < top + height - 21.0
        {
            return Some(Hit::Scrollbar(index));
        }
        if fence.view == FenceView::Details {
            let row_top = top + TITLE_HEIGHT as f32 + TAB_HEIGHT as f32 + PAD as f32 + DETAIL_HEADER_HEIGHT as f32
                - fence.scroll_y as f32;
            let body_top = top + TITLE_HEIGHT as f32 + TAB_HEIGHT as f32 + PAD as f32 + DETAIL_HEADER_HEIGHT as f32;
            if y >= body_top {
                let slot = ((y - row_top) / DETAIL_ROW_HEIGHT as f32).floor() as usize;
                if slot < fence.items.len() {
                    return Some(Hit::Item(index, slot));
                }
            }
        } else {
            for slot in 0..fence.items.len() {
                let (cell_x, cell_y, cell_w, cell_h) = cell(fence, slot);
                if (cell_x..cell_x + cell_w).contains(&x) && (cell_y..cell_y + cell_h).contains(&y) {
                    return Some(Hit::Item(index, slot));
                }
            }
        }
        return Some(Hit::Fence(index));
    }
    None
}

/// The rectangle of one item's cell: left, top, width, height in DIPs.
fn cell(fence: &Placed, slot: usize) -> (f32, f32, f32, f32) {
    let columns = fence.columns.max(1) as usize;
    let cell_w = (fence.width as f32 - 2.0 * PAD as f32) / columns as f32;
    let cell_h = fence.cell_height() as f32;
    let column = slot % columns;
    let row = slot / columns;
    (
        fence.x as f32 + PAD as f32 + column as f32 * cell_w,
        fence.y as f32 + TITLE_HEIGHT as f32 + TAB_HEIGHT as f32 + PAD as f32 + row as f32 * cell_h
            - fence.scroll_y as f32,
        cell_w,
        cell_h,
    )
}

/// Colors from Pickets' fence palette. The body and title stay opaque so the
/// card reads the same over any wallpaper.
#[derive(Clone, Copy)]
struct Look {
    body: D2D1_COLOR_F,
    title: D2D1_COLOR_F,
    edge: D2D1_COLOR_F,
    title_text: D2D1_COLOR_F,
    label: D2D1_COLOR_F,
    hint: D2D1_COLOR_F,
    hover: D2D1_COLOR_F,
    well: D2D1_COLOR_F,
}

fn solid(hex: u32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((hex >> 16) & 0xFF) as f32 / 255.0,
        g: ((hex >> 8) & 0xFF) as f32 / 255.0,
        b: (hex & 0xFF) as f32 / 255.0,
        a: 1.0,
    }
}

fn veil(hex: u32, alpha: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { a: alpha, ..solid(hex) }
}

fn tinted(base: u32, tint: Option<u32>, amount: f32) -> D2D1_COLOR_F {
    let Some(tint) = tint else { return solid(base) };
    let channel = |shift| {
        let base = ((base >> shift) & 0xFFu32) as f32;
        let tint = ((tint >> shift) & 0xFFu32) as f32;
        (base + (tint - base) * amount).round() as u32
    };
    solid((channel(16) << 16) | (channel(8) << 8) | channel(0))
}

fn look(palette: &Palette, tint: Option<u32>) -> Look {
    // Light text means a dark theme; this is the same test the app uses.
    if palette.text.r > 0.5 {
        Look {
            body: tinted(0x202020, tint, 0.16),
            title: tinted(0x2B2B2B, tint, 0.32),
            edge: solid(0x3A3A3A),
            title_text: solid(0xE0E0E0),
            label: solid(0xEAEAEA),
            hint: solid(0x9AA3B5),
            hover: veil(0xFFFFFF, 0.13),
            well: veil(0xFFFFFF, 0.06),
        }
    } else {
        Look {
            body: tinted(0xF4F5F7, tint, 0.12),
            title: tinted(0xE3E6EB, tint, 0.28),
            edge: solid(0xC9CFD9),
            title_text: solid(0x1F232B),
            label: solid(0x1A1D24),
            hint: solid(0x5C6577),
            hover: veil(0x000000, 0.09),
            well: veil(0x000000, 0.05),
        }
    }
}

fn icon_key(path: &str, thumbnail: bool) -> String {
    format!("{}:{path}", if thumbnail { "thumb" } else { "icon" })
}

struct Target {
    rt: Option<ID2D1DCRenderTarget>,
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    size: D2D_SIZE_U,
    brush: ID2D1SolidColorBrush,
    bitmaps: HashMap<String, (Arc<IconPixels>, ID2D1Bitmap)>,
}

impl Drop for Target {
    fn drop(&mut self) {
        // Release Direct2D's reference to the selected DIB before deleting its
        // device context and bitmap.
        self.rt.take();
        unsafe {
            let _ = SelectObject(self.dc, self.previous);
            let _ = DeleteDC(self.dc);
            let _ = DeleteObject(self.bitmap.into());
        }
    }
}

pub struct Renderer {
    d2d: ID2D1Factory,
    dwrite: IDWriteFactory,
    title: IDWriteTextFormat,
    label: IDWriteTextFormat,
    count: IDWriteTextFormat,
    glyph: IDWriteTextFormat,
    letter: IDWriteTextFormat,
    target: Option<Target>,
    dpi: f32,
}

impl Renderer {
    pub fn new() -> Result<Self> {
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let fonts = Fonts::installed(&dwrite);
            let title = format(&dwrite, fonts.text, 12.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?;
            let label = format(&dwrite, fonts.text, 11.0, DWRITE_FONT_WEIGHT_NORMAL)?;
            let count = format(&dwrite, fonts.text, 11.0, DWRITE_FONT_WEIGHT_NORMAL)?;
            let glyph = format(&dwrite, fonts.icons, 11.0, DWRITE_FONT_WEIGHT_NORMAL)?;
            let letter = format(&dwrite, fonts.text, 20.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?;

            // Names wrap to at most the two lines the cell reserves.
            label.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
            label.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            label.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            label.SetLineSpacing(DWRITE_LINE_SPACING_METHOD_UNIFORM, 14.0, 11.0)?;
            count.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING)?;
            glyph.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            letter.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;

            let title = trimmed(&dwrite, title)?;
            let label = trimmed(&dwrite, label)?;
            let count = trimmed(&dwrite, count)?;
            Ok(Self {
                d2d,
                dwrite,
                title,
                label,
                count,
                glyph,
                letter,
                target: None,
                dpi: 96.0,
            })
        }
    }

    pub fn set_dpi(&mut self, dpi: u32) {
        self.dpi = dpi as f32;
        if let Some(target) = &self.target {
            if let Some(rt) = &target.rt {
                unsafe { rt.SetDpi(self.dpi, self.dpi) };
            }
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if self
            .target
            .as_ref()
            .is_some_and(|target| target.size.width != width || target.size.height != height)
        {
            self.target = None;
        }
    }

    pub fn draw(&mut self, hwnd: HWND, frame: &Frame<'_>) -> Result<()> {
        // A lost device leaves the window blank. Recreate the target and paint
        // the frame once more instead of waiting for the next invalidation.
        for attempt in 0..2 {
            let mut client = RECT::default();
            unsafe { GetClientRect(hwnd, &mut client)? };
            let size = D2D_SIZE_U {
                width: (client.right - client.left).max(1) as u32,
                height: (client.bottom - client.top).max(1) as u32,
            };
            if self
                .target
                .as_ref()
                .is_none_or(|target| target.size.width != size.width || target.size.height != size.height)
            {
                self.target = Some(self.create_target(size)?);
            }
            let target = self.target.as_mut().unwrap();
            let rt = target.rt.as_ref().unwrap().clone();
            let result: Result<()> = unsafe {
                let bound = RECT {
                    left: 0,
                    top: 0,
                    right: size.width as i32,
                    bottom: size.height as i32,
                };
                rt.BindDC(target.dc, &bound)?;
                rt.BeginDraw();
                // A layered window preserves these premultiplied alpha pixels
                // so the desktop wallpaper shows through between and within
                // the fence fills.
                let clear = D2D1_COLOR_F {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.0,
                };
                rt.Clear(Some(&clear));
                draw_fences(
                    &target.brush,
                    &rt,
                    frame,
                    &self.title,
                    &self.label,
                    &self.count,
                    &self.glyph,
                    &self.letter,
                    &mut target.bitmaps,
                );
                rt.EndDraw(None, None)?;
                let mut window = RECT::default();
                GetWindowRect(hwnd, &mut window)?;
                let destination = POINT {
                    x: window.left,
                    y: window.top,
                };
                let origin = POINT { x: 0, y: 0 };
                let layered_size = SIZE {
                    cx: size.width as i32,
                    cy: size.height as i32,
                };
                let blend = BLENDFUNCTION {
                    BlendOp: AC_SRC_OVER as u8,
                    BlendFlags: 0,
                    SourceConstantAlpha: 255,
                    AlphaFormat: AC_SRC_ALPHA as u8,
                };
                UpdateLayeredWindow(
                    hwnd,
                    None,
                    Some(&destination),
                    Some(&layered_size),
                    Some(target.dc),
                    Some(&origin),
                    COLORREF(0),
                    Some(&blend),
                    ULW_ALPHA,
                )?;
                Ok(())
            };
            match &result {
                Err(error) if error.code() == D2DERR_RECREATE_TARGET && attempt == 0 => {
                    self.target = None;
                }
                _ => return result,
            }
        }
        Ok(())
    }

    /// Draws onto any render target. The preview test uses this with a WIC
    /// bitmap so desk geometry and colors can be checked without a screen.
    #[cfg(test)]
    pub fn draw_to(
        &self,
        rt: &ID2D1RenderTarget,
        frame: &Frame<'_>,
        bitmaps: &mut HashMap<String, (Arc<IconPixels>, ID2D1Bitmap)>,
        clear: D2D1_COLOR_F,
    ) -> Result<()> {
        unsafe {
            let white = D2D1_COLOR_F {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 1.0,
            };
            let brush = rt.CreateSolidColorBrush(&white, None)?;
            rt.BeginDraw();
            rt.Clear(Some(&clear));
            draw_fences(
                &brush,
                rt,
                frame,
                &self.title,
                &self.label,
                &self.count,
                &self.glyph,
                &self.letter,
                bitmaps,
            );
            rt.EndDraw(None, None)
        }
    }

    fn create_target(&self, size: D2D_SIZE_U) -> Result<Target> {
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.0.is_null() {
                return Err(windows::core::Error::from_thread());
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: size.width as i32,
                    biHeight: -(size.height as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bitmap = match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(bitmap) => bitmap,
                Err(error) => {
                    let _ = DeleteDC(dc);
                    return Err(error);
                }
            };
            let previous = SelectObject(dc, bitmap.into());
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                ..Default::default()
            };
            let rt = match self.d2d.CreateDCRenderTarget(&props) {
                Ok(rt) => rt,
                Err(error) => {
                    let _ = SelectObject(dc, previous);
                    let _ = DeleteDC(dc);
                    let _ = DeleteObject(bitmap.into());
                    return Err(error);
                }
            };
            rt.SetDpi(self.dpi, self.dpi);
            let white = D2D1_COLOR_F {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 1.0,
            };
            let brush = match rt.CreateSolidColorBrush(&white, None) {
                Ok(brush) => brush,
                Err(error) => {
                    drop(rt);
                    let _ = SelectObject(dc, previous);
                    let _ = DeleteDC(dc);
                    let _ = DeleteObject(bitmap.into());
                    return Err(error);
                }
            };
            Ok(Target {
                rt: Some(rt),
                dc,
                bitmap,
                previous,
                size,
                brush,
                bitmaps: HashMap::new(),
            })
        }
    }
}

unsafe fn format(
    dw: &IDWriteFactory,
    family: windows::core::PCWSTR,
    size: f32,
    weight: DWRITE_FONT_WEIGHT,
) -> Result<IDWriteTextFormat> {
    unsafe {
        let format = dw.CreateTextFormat(
            family,
            None,
            weight,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            w!("en-us"),
        )?;
        format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        Ok(format)
    }
}

unsafe fn trimmed(dw: &IDWriteFactory, format: IDWriteTextFormat) -> Result<IDWriteTextFormat> {
    unsafe {
        let sign = dw.CreateEllipsisTrimmingSign(&format)?;
        let trimming = DWRITE_TRIMMING {
            granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
            delimiter: 0,
            delimiterCount: 0,
        };
        format.SetTrimming(&trimming, &sign)?;
        Ok(format)
    }
}

fn bounds(left: f32, top: f32, right: f32, bottom: f32) -> D2D_RECT_F {
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

unsafe fn text(
    rt: &ID2D1RenderTarget,
    format: &IDWriteTextFormat,
    value: &str,
    rect: D2D_RECT_F,
    brush: &ID2D1SolidColorBrush,
) {
    unsafe {
        rt.DrawText(
            &wide(value),
            format,
            &rect,
            brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
            DWRITE_MEASURING_MODE_NATURAL,
        );
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn draw_fences(
    brush: &ID2D1SolidColorBrush,
    rt: &ID2D1RenderTarget,
    frame: &Frame<'_>,
    title_format: &IDWriteTextFormat,
    label_format: &IDWriteTextFormat,
    count_format: &IDWriteTextFormat,
    glyph_format: &IDWriteTextFormat,
    letter_format: &IDWriteTextFormat,
    bitmaps: &mut HashMap<String, (Arc<IconPixels>, ID2D1Bitmap)>,
) {
    unsafe {
        for (index, fence) in frame.fences.iter().enumerate() {
            let mut p = look(frame.palette, fence.tint);
            let alpha = 1.0 - fence.transparency as f32 / 100.0;
            p.body.a = alpha;
            p.title.a = alpha;
            p.edge.a = alpha.max(0.24);
            let (left, top) = (fence.x as f32, fence.y as f32);
            let (width, height) = (fence.width as f32, fence.height as f32);
            let card = bounds(left, top, left + width, top + height);
            brush.SetColor(&p.body);
            rt.FillRoundedRectangle(&rounded(card, RADIUS), brush);

            // The window region rounds the card's top corners. Draw the title
            // fill once: overlapping translucent shapes create a horizontal
            // seam through the bar.
            let title_bottom = top + TITLE_HEIGHT as f32;
            brush.SetColor(&p.title);
            rt.FillRectangle(&bounds(left, top, left + width, title_bottom), brush);

            if !fence.collapsed {
                let tabs_bottom = title_bottom + TAB_HEIGHT as f32;
                brush.SetColor(&p.well);
                rt.FillRectangle(
                    &bounds(left + 1.0, title_bottom, left + width - 1.0, tabs_bottom),
                    brush,
                );
                let mut tab_x = left + 7.0;
                for (tab_index, name) in fence.tabs.iter().enumerate() {
                    let tab_width = (name.chars().count() as f32 * 7.5 + 24.0).clamp(50.0, 150.0);
                    let tab_rect = bounds(tab_x, title_bottom + 3.0, tab_x + tab_width, tabs_bottom - 3.0);
                    if tab_index == fence.active_tab {
                        brush.SetColor(&p.hover);
                        rt.FillRoundedRectangle(&rounded(tab_rect, 6.0), brush);
                    }
                    brush.SetColor(if tab_index == fence.active_tab {
                        &p.title_text
                    } else {
                        &p.hint
                    });
                    text(rt, label_format, name, tab_rect, brush);
                    tab_x += tab_width + 4.0;
                }
                if fence.tabs.len() < 8 {
                    let plus = bounds(tab_x, title_bottom + 2.5, tab_x + 22.0, tabs_bottom - 2.5);
                    if frame.hover == Some(Hit::NewTab(index)) {
                        brush.SetColor(&p.hover);
                        rt.FillRoundedRectangle(&rounded(plus, 6.0), brush);
                    }
                    brush.SetColor(&p.title_text);
                    let cx = tab_x + 11.0;
                    let cy = title_bottom + TAB_HEIGHT as f32 / 2.0;
                    rt.DrawLine(Vector2::new(cx - 4.0, cy), Vector2::new(cx + 4.0, cy), brush, 1.8, None);
                    rt.DrawLine(Vector2::new(cx, cy - 4.0), Vector2::new(cx, cy + 4.0), brush, 1.8, None);
                } else {
                    brush.SetColor(&p.hint);
                    let cx = tab_x + 11.0;
                    let cy = title_bottom + TAB_HEIGHT as f32 / 2.0;
                    rt.DrawLine(Vector2::new(cx - 4.0, cy), Vector2::new(cx + 4.0, cy), brush, 1.8, None);
                    rt.DrawLine(Vector2::new(cx, cy - 4.0), Vector2::new(cx, cy + 4.0), brush, 1.8, None);
                }
                let clip = bounds(left + 1.0, tabs_bottom, left + width - 1.0, top + height - 1.0);
                let _ = rt.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
                match fence.view {
                    FenceView::Details => draw_details(
                        brush,
                        rt,
                        frame,
                        fence,
                        index,
                        left,
                        top,
                        width,
                        label_format,
                        count_format,
                        letter_format,
                        bitmaps,
                        &p,
                    ),
                    FenceView::Icons | FenceView::Thumbnails => {
                        for (slot, item) in fence.items.iter().enumerate() {
                            let (cell_x, cell_y, cell_w, cell_h) = cell(fence, slot);
                            if frame.hover == Some(Hit::Item(index, slot)) {
                                brush.SetColor(&p.hover);
                                rt.FillRoundedRectangle(
                                    &rounded(
                                        bounds(
                                            cell_x + TILE_INSET,
                                            cell_y + TILE_INSET,
                                            cell_x + cell_w - TILE_INSET,
                                            cell_y + cell_h - TILE_INSET,
                                        ),
                                        TILE_RADIUS,
                                    ),
                                    brush,
                                );
                            }
                            draw_item(
                                brush,
                                rt,
                                frame,
                                item,
                                bounds(cell_x, cell_y, cell_x + cell_w, cell_y + cell_h),
                                &p,
                                label_format,
                                letter_format,
                                bitmaps,
                                fence.icon_size,
                                fence.view == FenceView::Thumbnails,
                            );
                        }
                    }
                }
                let _ = rt.PopAxisAlignedClip();
                draw_scrollbar(brush, rt, fence, left, top, width, height, &p);
                brush.SetColor(&p.hint);
                for offset in [0.0_f32, 5.0, 10.0] {
                    let x = left + width - 5.0 - offset;
                    let y = top + height - 5.0;
                    rt.DrawLine(Vector2::new(x - 7.0, y), Vector2::new(x, y - 7.0), brush, 1.0, None);
                }
            }

            // Name, count, and roll-up button.
            if fence.browsing {
                let back = bounds(left + 4.0, top + 5.0, left + 31.0, top + TITLE_HEIGHT as f32 - 5.0);
                if frame.hover == Some(Hit::NavigateBack(index)) {
                    brush.SetColor(&p.hover);
                    rt.FillRoundedRectangle(&rounded(back, 5.0), brush);
                }
                brush.SetColor(&p.title_text);
                text(rt, glyph_format, "\u{E72B}", back, brush);
            }
            let title_x = left + TITLE_INSET + if fence.browsing { 28.0 } else { 0.0 };
            let title_area = bounds(title_x, top, left + width - CHEVRON, title_bottom);
            brush.SetColor(&p.title_text);
            text(rt, title_format, &fence.name, title_area, brush);

            if !fence.collapsed {
                let count = fence.items.len().to_string();
                brush.SetColor(&p.hint);
                text(
                    rt,
                    count_format,
                    &count,
                    bounds(
                        left + width - CHEVRON,
                        top,
                        left + width - 2.0 * BUTTON_GAP - BUTTON_W - BUTTON_GAP,
                        title_bottom,
                    ),
                    brush,
                );
            }

            let bx = left + width - BUTTON_GAP - BUTTON_W;
            let by = top + (TITLE_HEIGHT as f32 - BUTTON_H) / 2.0;
            let button = bounds(bx, by, bx + BUTTON_W, by + BUTTON_H);
            if frame.hover == Some(Hit::Collapse(index)) {
                brush.SetColor(&p.hover);
                rt.FillRoundedRectangle(&rounded(button, 5.0), brush);
            }
            let chevron = if fence.collapsed { "\u{E76C}" } else { "\u{E70D}" };
            brush.SetColor(&p.title_text);
            text(rt, glyph_format, chevron, button, brush);

            // The hairline edge goes last so it sits above the title bar.
            brush.SetColor(&p.edge);
            rt.DrawRoundedRectangle(&rounded(card, RADIUS), brush, 1.0, None);
        }
    }
}

unsafe fn draw_scrollbar(
    brush: &ID2D1SolidColorBrush,
    rt: &ID2D1RenderTarget,
    fence: &Placed,
    left: f32,
    top: f32,
    width: f32,
    height: f32,
    look: &Look,
) {
    let Some((track_top, track_bottom, thumb_top, thumb_height)) = scrollbar_metrics(fence, top, height) else {
        return;
    };
    unsafe {
        brush.SetColor(&look.well);
        rt.FillRoundedRectangle(
            &rounded(
                bounds(left + width - 5.0, track_top, left + width - 2.0, track_bottom),
                2.0,
            ),
            brush,
        );
        brush.SetColor(&look.hint);
        rt.FillRoundedRectangle(
            &rounded(
                bounds(
                    left + width - 5.0,
                    thumb_top,
                    left + width - 2.0,
                    thumb_top + thumb_height,
                ),
                2.0,
            ),
            brush,
        );
    }
}

fn scrollbar_metrics(fence: &Placed, top: f32, height: f32) -> Option<(f32, f32, f32, f32)> {
    let max_scroll = fence.max_scroll() as f32;
    if max_scroll <= 0.0 {
        return None;
    }
    let track_top = top + TITLE_HEIGHT as f32 + TAB_HEIGHT as f32 + 5.0;
    let track_bottom = top + height - 21.0;
    let track_height = (track_bottom - track_top).max(12.0);
    let view_height = (height - TITLE_HEIGHT as f32 - TAB_HEIGHT as f32).max(1.0);
    let content_height = view_height + max_scroll;
    let thumb_height = (track_height * view_height / content_height).clamp(18.0, track_height);
    let travel = (track_height - thumb_height).max(0.0);
    let thumb_top = track_top + travel * fence.scroll_y as f32 / max_scroll;
    Some((track_top, track_bottom, thumb_top, thumb_height))
}

#[allow(clippy::too_many_arguments)]
unsafe fn draw_item(
    brush: &ID2D1SolidColorBrush,
    rt: &ID2D1RenderTarget,
    frame: &Frame<'_>,
    item: &crate::desk::Item,
    rect: D2D_RECT_F,
    look: &Look,
    label_format: &IDWriteTextFormat,
    letter_format: &IDWriteTextFormat,
    bitmaps: &mut HashMap<String, (Arc<IconPixels>, ID2D1Bitmap)>,
    icon_size: u32,
    thumbnail: bool,
) {
    unsafe {
        let width = rect.right - rect.left;
        let size = (icon_size as f32).min(width - 10.0).max(16.0);
        let icon_top = rect.top + 5.0;
        let icon_left = rect.left + (width - size) / 2.0;
        let icon = bounds(icon_left, icon_top, icon_left + size, icon_top + size);
        draw_item_image(brush, rt, frame, item, icon, look, letter_format, bitmaps, thumbnail);
        brush.SetColor(&look.label);
        let name = if frame.show_labels { item.name.as_str() } else { "" };
        text(
            rt,
            label_format,
            name,
            bounds(rect.left + 3.0, icon.bottom + 2.0, rect.right - 3.0, rect.bottom - 2.0),
            brush,
        );
    }
}

unsafe fn draw_item_image(
    brush: &ID2D1SolidColorBrush,
    rt: &ID2D1RenderTarget,
    frame: &Frame<'_>,
    item: &crate::desk::Item,
    rect: D2D_RECT_F,
    look: &Look,
    letter_format: &IDWriteTextFormat,
    bitmaps: &mut HashMap<String, (Arc<IconPixels>, ID2D1Bitmap)>,
    thumbnail: bool,
) {
    let key = icon_key(&item.path, thumbnail);
    let image = unsafe { frame.icons.get(&key).and_then(|px| bitmap(bitmaps, rt, &key, px)) };
    match image {
        Some(image) => unsafe {
            rt.DrawBitmap(&image, Some(&rect), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
        },
        None => unsafe {
            let width = rect.right - rect.left;
            let height = rect.bottom - rect.top;
            brush.SetColor(&look.well);
            rt.FillRoundedRectangle(&rounded(rect, width.min(height) * 0.18), brush);
            if let Some(letter) = item.name.chars().next().filter(|c| !c.is_whitespace()) {
                brush.SetColor(&look.hint);
                text(rt, letter_format, &letter.to_uppercase().to_string(), rect, brush);
            }
        },
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn draw_details(
    brush: &ID2D1SolidColorBrush,
    rt: &ID2D1RenderTarget,
    frame: &Frame<'_>,
    fence: &Placed,
    index: usize,
    left: f32,
    top: f32,
    width: f32,
    label_format: &IDWriteTextFormat,
    header_format: &IDWriteTextFormat,
    letter_format: &IDWriteTextFormat,
    bitmaps: &mut HashMap<String, (Arc<IconPixels>, ID2D1Bitmap)>,
    look: &Look,
) {
    unsafe {
        let name_x = left + PAD as f32 + 28.0;
        let type_x = left + width * 0.45;
        let size_x = left + width * 0.64;
        let modified_x = left + width * 0.79;
        let header_top = top + TITLE_HEIGHT as f32 + TAB_HEIGHT as f32 + PAD as f32;
        brush.SetColor(&look.hint);
        text(
            rt,
            header_format,
            "Name",
            bounds(
                name_x,
                header_top,
                type_x - 5.0,
                header_top + DETAIL_HEADER_HEIGHT as f32,
            ),
            brush,
        );
        text(
            rt,
            header_format,
            "Type",
            bounds(
                type_x,
                header_top,
                size_x - 5.0,
                header_top + DETAIL_HEADER_HEIGHT as f32,
            ),
            brush,
        );
        text(
            rt,
            header_format,
            "Size",
            bounds(
                size_x,
                header_top,
                modified_x - 3.0,
                header_top + DETAIL_HEADER_HEIGHT as f32,
            ),
            brush,
        );
        text(
            rt,
            header_format,
            "Date modified",
            bounds(
                modified_x,
                header_top,
                left + width - PAD as f32,
                header_top + DETAIL_HEADER_HEIGHT as f32,
            ),
            brush,
        );
        brush.SetColor(&look.edge);
        rt.DrawLine(
            Vector2::new(left + PAD as f32, header_top + DETAIL_HEADER_HEIGHT as f32 - 2.0),
            Vector2::new(
                left + width - PAD as f32,
                header_top + DETAIL_HEADER_HEIGHT as f32 - 2.0,
            ),
            brush,
            1.0,
            None,
        );
        for (slot, item) in fence.items.iter().enumerate() {
            let row_top = header_top + DETAIL_HEADER_HEIGHT as f32 + slot as f32 * DETAIL_ROW_HEIGHT as f32
                - fence.scroll_y as f32;
            let row = bounds(
                left + 2.0,
                row_top,
                left + width - 2.0,
                row_top + DETAIL_ROW_HEIGHT as f32,
            );
            if frame.hover == Some(Hit::Item(index, slot)) {
                brush.SetColor(&look.hover);
                rt.FillRoundedRectangle(&rounded(row, 4.0), brush);
            }
            let icon = bounds(
                left + PAD as f32 + 2.0,
                row_top + 4.0,
                left + PAD as f32 + 22.0,
                row_top + 24.0,
            );
            draw_item_image(brush, rt, frame, item, icon, look, letter_format, bitmaps, false);
            brush.SetColor(&look.label);
            text(
                rt,
                label_format,
                &item.name,
                bounds(name_x, row_top, type_x - 5.0, row_top + DETAIL_ROW_HEIGHT as f32),
                brush,
            );
            let kind = item_type(item);
            text(
                rt,
                label_format,
                &kind,
                bounds(type_x, row_top, size_x - 4.0, row_top + DETAIL_ROW_HEIGHT as f32),
                brush,
            );
            let size = if item.is_dir {
                String::new()
            } else {
                format_size(item.size_bytes)
            };
            text(
                rt,
                label_format,
                &size,
                bounds(size_x, row_top, modified_x - 3.0, row_top + DETAIL_ROW_HEIGHT as f32),
                brush,
            );
            let modified = format_modified(item.modified);
            text(
                rt,
                label_format,
                &modified,
                bounds(
                    modified_x,
                    row_top,
                    left + width - PAD as f32,
                    row_top + DETAIL_ROW_HEIGHT as f32,
                ),
                brush,
            );
        }
    }
}

fn item_type(item: &crate::desk::Item) -> String {
    if item.is_dir {
        return "File folder".into();
    }
    match std::path::Path::new(&item.path)
        .extension()
        .and_then(|extension| extension.to_str())
    {
        Some(extension) if !extension.is_empty() => format!("{} file", extension.to_ascii_uppercase()),
        _ => "File".into(),
    }
}

fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn format_modified(modified: Option<SystemTime>) -> String {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::Storage::FileSystem::FileTimeToLocalFileTime;
    use windows::Win32::System::Time::FileTimeToSystemTime;
    let Some(modified) = modified else {
        return String::new();
    };
    let Ok(elapsed) = modified.duration_since(UNIX_EPOCH) else {
        return String::new();
    };
    let ticks = elapsed
        .as_secs()
        .saturating_add(11_644_473_600)
        .saturating_mul(10_000_000)
        .saturating_add((elapsed.subsec_nanos() / 100) as u64);
    let utc = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut local = FILETIME::default();
    let mut system = SYSTEMTIME::default();
    unsafe {
        if FileTimeToLocalFileTime(&utc, &mut local).is_err() || FileTimeToSystemTime(&local, &mut system).is_err() {
            return String::new();
        }
    }
    format!("{:02}/{:02}/{:04}", system.wMonth, system.wDay, system.wYear)
}

/// Caches Direct2D bitmaps per render target; a new target needs new bitmaps.
unsafe fn bitmap(
    cache: &mut HashMap<String, (Arc<IconPixels>, ID2D1Bitmap)>,
    rt: &ID2D1RenderTarget,
    key: &str,
    px: &Arc<IconPixels>,
) -> Option<ID2D1Bitmap> {
    if let Some((pixels, bitmap)) = cache.get(key)
        && Arc::ptr_eq(pixels, px)
    {
        return Some(bitmap.clone());
    }
    let props = D2D1_BITMAP_PROPERTIES {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: 96.0,
        dpiY: 96.0,
    };
    let bitmap = unsafe {
        rt.CreateBitmap(
            D2D_SIZE_U {
                width: px.width,
                height: px.height,
            },
            Some(px.bgra.as_ptr().cast()),
            px.width * 4,
            &props,
        )
        .ok()?
    };
    cache.insert(key.to_string(), (px.clone(), bitmap.clone()));
    Some(bitmap)
}

#[cfg(test)]
mod probe {
    use super::*;
    use crate::desk::{Desk, collect, desktop_dirs, separate};
    use windows::Win32::Graphics::Imaging::*;
    use windows::Win32::System::Com::*;
    use windows::core::{GUID, HSTRING};

    /// Draws the real desktop through the real renderer into a PNG, so the
    /// layout can be inspected without taking a screen capture.
    #[test]
    #[ignore = "writes a preview PNG and needs the real shell for icons"]
    fn renders_the_default_layout_to_a_png() {
        std::thread::spawn(|| unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let desk = crate::desk::Desk::load().0;
            let desk = if desk.fence.is_empty() {
                Desk::default_layout()
            } else {
                desk
            };
            let items = collect(&desktop_dirs());
            let mut placed = desk.layout(&items);
            separate(&mut placed, 900, 740);
            for fence in &placed {
                println!(
                    "fence {} at ({},{}) {}x{} with {} items",
                    fence.id,
                    fence.x,
                    fence.y,
                    fence.width,
                    fence.height,
                    fence.items.len()
                );
            }
            let mut icons = HashMap::new();
            for item in &items {
                if let Some(px) = crate::desk::icons::load(&item.path, 72) {
                    icons.insert(item.path.clone(), Arc::new(px));
                }
            }
            println!("items={} icons={}", items.len(), icons.len());

            let (width, height) = (1350u32, 1110u32);
            let factory: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory2, None, CLSCTX_INPROC_SERVER).unwrap();
            let bitmap = factory
                .CreateBitmap(width, height, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnLoad)
                .unwrap();
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).unwrap();
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 144.0,
                dpiY: 144.0,
                ..Default::default()
            };
            let rt = d2d.CreateWicBitmapRenderTarget(&bitmap, &props).unwrap();
            let renderer = Renderer::new().unwrap();
            let palette = Palette::new(true, false);
            let frame = Frame {
                palette: &palette,
                fences: &placed,
                icons: &icons,
                show_labels: true,
                hover: Some(Hit::Item(0, 0)),
            };
            let clear = D2D1_COLOR_F {
                r: 0.11,
                g: 0.12,
                b: 0.14,
                a: 1.0,
            };
            let mut bitmaps = HashMap::new();
            renderer.draw_to(&rt, &frame, &mut bitmaps, clear).unwrap();

            let path = std::env::temp_dir().join("opencode").join("desk-render.png");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let stream = factory.CreateStream().unwrap();
            stream
                .InitializeFromFilename(&HSTRING::from(path.as_os_str()), 0x4000_0000)
                .unwrap();
            let encoder = factory
                .CreateEncoder(&GUID_ContainerFormatPng, &GUID::zeroed())
                .unwrap();
            encoder.Initialize(&stream, WICBitmapEncoderNoCache).unwrap();
            let mut frame: Option<IWICBitmapFrameEncode> = None;
            encoder.CreateNewFrame(&mut frame, std::ptr::null_mut()).unwrap();
            let frame = frame.unwrap();
            frame.Initialize(None).unwrap();
            frame.SetSize(width, height).unwrap();
            let mut format = GUID_WICPixelFormat32bppPBGRA;
            frame.SetPixelFormat(&mut format).unwrap();
            frame.WriteSource(&bitmap, std::ptr::null()).unwrap();
            frame.Commit().unwrap();
            encoder.Commit().unwrap();
            println!("saved {}", path.display());
        })
        .join()
        .unwrap();
    }
}
