//! Drawing primitives: the island shape, CSS-like shadows, layers, text, dots and images.
//! All coordinates are DIPs.

use crate::log;
use island_core::Rect;
use std::collections::HashMap;
use std::mem::ManuallyDrop;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_F, D2D_SIZE_U, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_COMPOSITE_MODE_SOURCE_OVER,
    D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    CLSID_D2D1Shadow, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_ARC_SEGMENT, D2D1_ARC_SIZE_SMALL, D2D1_BITMAP_OPTIONS_NONE, D2D1_BITMAP_OPTIONS_TARGET, D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
    D2D1_BITMAP_PROPERTIES1, D2D1_COMBINE_MODE_EXCLUDE, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT, D2D1_ELLIPSE,
    D2D1_INTERPOLATION_MODE_LINEAR, D2D1_LAYER_OPTIONS1_NONE, D2D1_LAYER_PARAMETERS1, D2D1_PROPERTY_TYPE_FLOAT,
    D2D1_PROPERTY_TYPE_VECTOR4, D2D1_ROUNDED_RECT, D2D1_SHADOW_PROP_BLUR_STANDARD_DEVIATION, D2D1_SHADOW_PROP_COLOR,
    D2D1_SWEEP_DIRECTION_CLOCKWISE, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, ID2D1Bitmap1, ID2D1BitmapBrush1,
    ID2D1DeviceContext, ID2D1Effect, ID2D1Factory1, ID2D1Geometry, ID2D1Image, ID2D1PathGeometry1,
    ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT,
    DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT, DWRITE_TEXT_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_TRIMMING,
    DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING_NO_WRAP, DWriteCreateFactory, IDWriteFactory,
    DWRITE_FONT_FEATURE, DWRITE_FONT_FEATURE_TAG_TABULAR_FIGURES, DWRITE_TEXT_METRICS, DWRITE_TEXT_RANGE,
    IDWriteFontCollection, IDWriteTextFormat, IDWriteTextLayout, IDWriteTextLayout1, IDWriteTypography,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Foundation::E_INVALIDARG;
use windows::core::{Error, HSTRING, Interface, Result, w};
use windows_numerics::{Matrix3x2, Vector2};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color {
        Color { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a }
    }

    pub const fn white(a: f32) -> Color {
        Color { r: 1.0, g: 1.0, b: 1.0, a }
    }

    pub const BLACK: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };

    pub fn with_alpha(self, a: f32) -> Color {
        Color { a, ..self }
    }

    /// Multiplies the alpha.
    pub fn fade(self, k: f32) -> Color {
        Color { a: self.a * k, ..self }
    }

    pub fn lerp(self, to: Color, t: f32) -> Color {
        let m = |a: f32, b: f32| a + (b - a) * t;
        Color { r: m(self.r, to.r), g: m(self.g, to.g), b: m(self.b, to.b), a: m(self.a, to.a) }
    }

    fn d2d(self) -> D2D1_COLOR_F {
        D2D1_COLOR_F { r: self.r, g: self.g, b: self.b, a: self.a }
    }
}

/// CSS `rgba(...)` text colours (§6.4).
pub const TEXT_PRIMARY: Color = Color::rgba(0xf5, 0xf5, 0xf7, 1.0);
pub const TEXT_SECONDARY: Color = Color::white(0.60);
pub const TEXT_TERTIARY: Color = Color::white(0.35);

#[derive(Clone, Copy)]
pub enum Align {
    Left,
    Center,
    Right,
}

fn rect_f(r: Rect) -> D2D_RECT_F {
    D2D_RECT_F { left: r.x, top: r.y, right: r.x + r.w, bottom: r.y + r.h }
}

fn v2(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

const IDENTITY: Matrix3x2 = Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: 0.0 };
/// The whole of a text layout.
const ALL_TEXT: DWRITE_TEXT_RANGE = DWRITE_TEXT_RANGE { startPosition: 0, length: u32::MAX };

/// The first installed of Inter, Segoe UI Variable Text, Segoe UI.
fn pick_family(dwrite: &IDWriteFactory) -> HSTRING {
    let names = [w!("Inter"), w!("Segoe UI Variable Text"), w!("Segoe UI")];
    let mut fonts: Option<IDWriteFontCollection> = None;
    unsafe {
        if dwrite.GetSystemFontCollection(&mut fonts, false).is_ok()
            && let Some(fonts) = fonts
        {
            for name in names {
                let (mut index, mut exists) = (0u32, Default::default());
                if fonts.FindFamilyName(name, &mut index, &mut exists).is_ok() && exists.as_bool() {
                    return HSTRING::from_wide(name.as_wide());
                }
            }
        }
    }
    HSTRING::from("Segoe UI")
}

pub struct Painter {
    pub dc: ID2D1DeviceContext,
    factory: ID2D1Factory1,
    dwrite: IDWriteFactory,
    brush: ID2D1SolidColorBrush,
    family: HSTRING,
    tabular: IDWriteTypography,
    formats: HashMap<(u32, u16), IDWriteTextFormat>,
    /// Shadow effects; one per shadow drawn this frame, reused across frames.
    shadows: Vec<ID2D1Effect>,
    shadows_used: usize,
}

impl Painter {
    pub fn new(dc: ID2D1DeviceContext, factory: ID2D1Factory1) -> Result<Painter> {
        unsafe {
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let brush = dc.CreateSolidColorBrush(&Color::BLACK.d2d(), None)?;
            dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let family = pick_family(&dwrite);
            log::info!("font family {family}");
            let tabular = dwrite.CreateTypography()?;
            tabular.AddFontFeature(DWRITE_FONT_FEATURE { nameTag: DWRITE_FONT_FEATURE_TAG_TABULAR_FIGURES, parameter: 1 })?;
            Ok(Painter {
                dc,
                factory,
                dwrite,
                brush,
                family,
                tabular,
                formats: HashMap::new(),
                shadows: Vec::new(),
                shadows_used: 0,
            })
        }
    }

    pub fn begin_frame(&mut self) {
        self.shadows_used = 0;
    }

    fn brush(&self, c: Color) -> &ID2D1SolidColorBrush {
        unsafe { self.brush.SetColor(&c.d2d()) };
        &self.brush
    }

    /// A rounded rect with per-corner radii [top-left, top-right, bottom-right, bottom-left].
    pub fn shape(&self, r: Rect, radii: [f32; 4]) -> Result<ID2D1Geometry> {
        let max = (r.w.min(r.h) / 2.0).max(0.0);
        let [tl, tr, br, bl] = radii.map(|x| x.clamp(0.0, max));
        let (l, t, rt, b) = (r.x, r.y, r.x + r.w, r.y + r.h);
        unsafe {
            let path: ID2D1PathGeometry1 = self.factory.CreatePathGeometry()?;
            let sink = path.Open()?;
            let arc = |x: f32, y: f32, rad: f32| D2D1_ARC_SEGMENT {
                point: v2(x, y),
                size: D2D_SIZE_F { width: rad, height: rad },
                rotationAngle: 0.0,
                sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                arcSize: D2D1_ARC_SIZE_SMALL,
            };
            sink.BeginFigure(v2(l + tl, t), D2D1_FIGURE_BEGIN_FILLED);
            sink.AddLine(v2(rt - tr, t));
            if tr > 0.0 {
                sink.AddArc(&arc(rt, t + tr, tr));
            }
            sink.AddLine(v2(rt, b - br));
            if br > 0.0 {
                sink.AddArc(&arc(rt - br, b, br));
            }
            sink.AddLine(v2(l + bl, b));
            if bl > 0.0 {
                sink.AddArc(&arc(l, b - bl, bl));
            }
            sink.AddLine(v2(l, t + tl));
            if tl > 0.0 {
                sink.AddArc(&arc(l + tl, t, tl));
            }
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close()?;
            path.cast()
        }
    }

    /// `outer` minus `inner`.
    pub fn exclude(&self, outer: &ID2D1Geometry, inner: &ID2D1Geometry) -> Result<ID2D1Geometry> {
        unsafe {
            let path: ID2D1PathGeometry1 = self.factory.CreatePathGeometry()?;
            let sink = path.Open()?;
            outer.CombineWithGeometry(inner, D2D1_COMBINE_MODE_EXCLUDE, None, 0.25, &sink)?;
            sink.Close()?;
            path.cast()
        }
    }

    pub fn rect_geometry(&self, r: Rect) -> Result<ID2D1Geometry> {
        unsafe { self.factory.CreateRectangleGeometry(&rect_f(r))?.cast() }
    }

    pub fn fill(&self, g: &ID2D1Geometry, c: Color) {
        unsafe { self.dc.FillGeometry(g, self.brush(c), None) }
    }

    /// CSS `box-shadow: dx dy blur color` of geometry `g` (no spread). Blur radius B is a
    /// Gaussian with standard deviation B/2.
    pub fn shadow(&mut self, g: &ID2D1Geometry, dx: f32, dy: f32, blur: f32, c: Color) -> Result<()> {
        unsafe {
            let list = self.dc.CreateCommandList()?;
            let old = self.dc.GetTarget()?;
            self.dc.SetTarget(&list);
            self.dc.FillGeometry(g, self.brush(Color::BLACK), None);
            self.dc.SetTarget(&old);
            list.Close()?;
            if self.shadows_used == self.shadows.len() {
                self.shadows.push(self.dc.CreateEffect(&CLSID_D2D1Shadow)?);
            }
            let fx = &self.shadows[self.shadows_used];
            self.shadows_used += 1;
            fx.SetInput(0, &list, true);
            fx.SetValue(
                D2D1_SHADOW_PROP_BLUR_STANDARD_DEVIATION.0 as u32,
                D2D1_PROPERTY_TYPE_FLOAT,
                &(blur / 2.0).to_le_bytes(),
            )?;
            let rgba: Vec<u8> = [c.r, c.g, c.b, c.a].iter().flat_map(|f| f.to_le_bytes()).collect();
            fx.SetValue(D2D1_SHADOW_PROP_COLOR.0 as u32, D2D1_PROPERTY_TYPE_VECTOR4, &rgba)?;
            let out: ID2D1Image = fx.GetOutput()?;
            self.dc.DrawImage(&out, Some(&v2(dx, dy)), None, D2D1_INTERPOLATION_MODE_LINEAR, D2D1_COMPOSITE_MODE_SOURCE_OVER);
        }
        Ok(())
    }

    /// Clips to `mask` (if any) and multiplies by `opacity` until `pop_layer`.
    pub fn push_layer(&self, mask: Option<&ID2D1Geometry>, opacity: f32) {
        let params = D2D1_LAYER_PARAMETERS1 {
            contentBounds: D2D_RECT_F { left: -1e6, top: -1e6, right: 1e6, bottom: 1e6 },
            geometricMask: ManuallyDrop::new(mask.cloned()),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: IDENTITY,
            opacity,
            opacityBrush: ManuallyDrop::new(None),
            layerOptions: D2D1_LAYER_OPTIONS1_NONE,
        };
        unsafe { self.dc.PushLayer(&params, None) };
        let mut params = params;
        unsafe { ManuallyDrop::drop(&mut params.geometricMask) };
    }

    pub fn pop_layer(&self) {
        unsafe { self.dc.PopLayer() }
    }

    pub fn transform(&self) -> Matrix3x2 {
        let mut m = IDENTITY;
        unsafe { self.dc.GetTransform(&mut m) };
        m
    }

    pub fn set_transform(&self, m: Matrix3x2) {
        unsafe { self.dc.SetTransform(&m) }
    }

    /// Scales what is drawn next by `k` around (cx, cy), on top of the current transform;
    /// returns the transform to restore.
    pub fn scale_about(&self, k: f32, cx: f32, cy: f32) -> Matrix3x2 {
        let s = Matrix3x2 { M11: k, M12: 0.0, M21: 0.0, M22: k, M31: cx - k * cx, M32: cy - k * cy };
        self.then(s)
    }

    /// Moves what is drawn next by (dx, dy), on top of the current transform; returns the
    /// transform to restore.
    pub fn translate(&self, dx: f32, dy: f32) -> Matrix3x2 {
        self.then(Matrix3x2 { M31: dx, M32: dy, ..IDENTITY })
    }

    fn then(&self, a: Matrix3x2) -> Matrix3x2 {
        let b = self.transform();
        // row vectors: p · a · b
        let m = Matrix3x2 {
            M11: a.M11 * b.M11 + a.M12 * b.M21,
            M12: a.M11 * b.M12 + a.M12 * b.M22,
            M21: a.M21 * b.M11 + a.M22 * b.M21,
            M22: a.M21 * b.M12 + a.M22 * b.M22,
            M31: a.M31 * b.M11 + a.M32 * b.M21 + b.M31,
            M32: a.M31 * b.M12 + a.M32 * b.M22 + b.M32,
        };
        self.set_transform(m);
        b
    }

    pub fn circle(&self, cx: f32, cy: f32, d: f32, c: Color) {
        let e = D2D1_ELLIPSE { point: v2(cx, cy), radiusX: d / 2.0, radiusY: d / 2.0 };
        unsafe { self.dc.FillEllipse(&e, self.brush(c)) }
    }

    pub fn circle_outline(&self, cx: f32, cy: f32, d: f32, c: Color, width: f32) {
        let e = D2D1_ELLIPSE { point: v2(cx, cy), radiusX: d / 2.0, radiusY: d / 2.0 };
        unsafe { self.dc.DrawEllipse(&e, self.brush(c), width, None) }
    }

    /// A filled polygon through `pts`.
    pub fn polygon(&self, pts: &[(f32, f32)], c: Color) -> Result<()> {
        let Some((&(x0, y0), rest)) = pts.split_first() else { return Ok(()) };
        unsafe {
            let path: ID2D1PathGeometry1 = self.factory.CreatePathGeometry()?;
            let sink = path.Open()?;
            sink.BeginFigure(v2(x0, y0), D2D1_FIGURE_BEGIN_FILLED);
            for &(x, y) in rest {
                sink.AddLine(v2(x, y));
            }
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close()?;
            self.dc.FillGeometry(&path, self.brush(c), None);
        }
        Ok(())
    }

    pub fn round_rect(&self, r: Rect, radius: f32, c: Color) {
        let rr = D2D1_ROUNDED_RECT { rect: rect_f(r), radiusX: radius, radiusY: radius };
        unsafe { self.dc.FillRoundedRectangle(&rr, self.brush(c)) }
    }

    pub fn line(&self, x0: f32, y0: f32, x1: f32, y1: f32, c: Color, width: f32) {
        unsafe { self.dc.DrawLine(v2(x0, y0), v2(x1, y1), self.brush(c), width, None) }
    }

    fn format(&mut self, size: f32, weight: u16) -> Result<IDWriteTextFormat> {
        let key = ((size * 100.0) as u32, weight);
        if let Some(f) = self.formats.get(&key) {
            return Ok(f.clone());
        }
        unsafe {
            let f = self.dwrite.CreateTextFormat(
                &self.family,
                None,
                DWRITE_FONT_WEIGHT(weight as i32),
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                w!("en-us"),
            )?;
            f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            let sign = self.dwrite.CreateEllipsisTrimmingSign(&f)?;
            let trim = DWRITE_TRIMMING {
                granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                delimiter: 0,
                delimiterCount: 0,
            };
            f.SetTrimming(&trim, &sign)?;
            self.formats.insert(key, f.clone());
            Ok(f)
        }
    }

    /// One line of text in `r`, vertically centred, ellipsised when it doesn't fit.
    pub fn layout(&mut self, s: &str, r: Rect, size: f32, weight: u16, align: Align) -> Result<IDWriteTextLayout> {
        let f = self.format(size, weight)?;
        let text: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            let l = self.dwrite.CreateTextLayout(&text, &f, r.w.max(0.0), r.h.max(0.0))?;
            let a: DWRITE_TEXT_ALIGNMENT = match align {
                Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
                Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
            };
            l.SetTextAlignment(a)?;
            Ok(l)
        }
    }

    /// `layout` with tabular figures.
    pub fn layout_tabular(&mut self, s: &str, r: Rect, size: f32, weight: u16, align: Align) -> Result<IDWriteTextLayout> {
        let l = self.layout(s, r, size, weight, align)?;
        unsafe { l.SetTypography(&self.tabular, ALL_TEXT)? };
        Ok(l)
    }

    /// `layout` with letter spacing (em).
    pub fn layout_tracked(&mut self, s: &str, r: Rect, size: f32, weight: u16, align: Align, tracking: f32) -> Result<IDWriteTextLayout> {
        let l = self.layout(s, r, size, weight, align)?;
        unsafe { l.cast::<IDWriteTextLayout1>()?.SetCharacterSpacing(0.0, tracking * size, 0.0, ALL_TEXT)? };
        Ok(l)
    }

    /// The laid-out text's width (after trimming).
    pub fn width(l: &IDWriteTextLayout) -> f32 {
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe {
            let _ = l.GetMetrics(&mut m);
        }
        m.width
    }

    pub fn draw_layout(&self, l: &IDWriteTextLayout, x: f32, y: f32, c: Color) {
        unsafe {
            self.dc.DrawTextLayout(v2(x, y), l, self.brush(c), D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT);
        }
    }

    pub fn text(&mut self, s: &str, r: Rect, size: f32, weight: u16, c: Color, align: Align) -> Result<()> {
        let l = self.layout(s, r, size, weight, align)?;
        self.draw_layout(&l, r.x, r.y, c);
        Ok(())
    }

    /// A bitmap from premultiplied BGRA; E_INVALIDARG unless `bgra` holds a whole w×h image.
    pub fn bitmap(&self, bgra: &[u8], w: u32, h: u32) -> Result<ID2D1Bitmap1> {
        if !island_core::color::bgra_fits(bgra, w, h) {
            return Err(Error::new(E_INVALIDARG, format!("{w}x{h} image in {} bytes", bgra.len())));
        }
        let props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
            ..Default::default()
        };
        unsafe {
            self.dc.CreateBitmap(
                D2D_SIZE_U { width: w, height: h },
                Some(bgra.as_ptr().cast()),
                w * 4,
                &props,
            )
        }
    }

    /// A transparent bitmap the size of the current target that can itself be drawn into.
    pub fn layer_bitmap(&self) -> Result<ID2D1Bitmap1> {
        unsafe {
            let target: ID2D1Bitmap1 = self.dc.GetTarget()?.cast()?;
            let (mut dx, mut dy) = (0.0, 0.0);
            self.dc.GetDpi(&mut dx, &mut dy);
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: dx,
                dpiY: dy,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
                ..Default::default()
            };
            self.dc.CreateBitmap(target.GetPixelSize(), None, 0, &props)
        }
    }

    /// Runs `draw` with `bmp` (cleared) as the target, then restores the frame's target.
    pub fn draw_into(&mut self, bmp: &ID2D1Bitmap1, draw: impl FnOnce(&mut Painter) -> Result<()>) -> Result<()> {
        unsafe {
            let old = self.dc.GetTarget()?;
            self.dc.SetTarget(bmp);
            self.dc.Clear(Some(&Color::BLACK.with_alpha(0.0).d2d()));
            let r = draw(self);
            self.dc.SetTarget(&old);
            r
        }
    }

    /// `bmp` 1:1 over the whole target.
    pub fn blit(&self, bmp: &ID2D1Bitmap1) {
        unsafe {
            self.dc.DrawBitmap(bmp, None, 1.0, D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR, None, None);
        }
    }

    fn image_brush(&self, bmp: &ID2D1Bitmap1, r: Rect, opacity: f32) -> Result<ID2D1BitmapBrush1> {
        unsafe {
            let brush = self.dc.CreateBitmapBrush(bmp, None, None)?;
            let s = bmp.GetSize();
            let m = Matrix3x2 {
                M11: r.w / s.width.max(1.0),
                M12: 0.0,
                M21: 0.0,
                M22: r.h / s.height.max(1.0),
                M31: r.x,
                M32: r.y,
            };
            brush.SetTransform(&m);
            brush.SetOpacity(opacity);
            Ok(brush)
        }
    }

    /// `bmp` stretched into `r`, clipped to a rounded rect of `radius`.
    pub fn image(&self, bmp: &ID2D1Bitmap1, r: Rect, radius: f32, opacity: f32) -> Result<()> {
        let brush = self.image_brush(bmp, r, opacity)?;
        let rr = D2D1_ROUNDED_RECT { rect: rect_f(r), radiusX: radius, radiusY: radius };
        unsafe { self.dc.FillRoundedRectangle(&rr, &brush) };
        Ok(())
    }
}
