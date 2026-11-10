//! Embedded solo pet sheet, decoded once on the UI thread and uploaded per graphics device.

use crate::draw::Painter;
use crate::log;
use island_core::Point;
use island_core::pet::{Atlas, Sheet, sprite_dest};
use windows::Win32::Foundation::{E_INVALIDARG, RPC_E_CHANGED_MODE};
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap1;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory, WICBitmapDitherTypeNone,
    WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnLoad,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::core::{Error, Result};

const SOLO: &[u8] = include_bytes!("../assets/pet/pet-solo.png");
const JSON: &str = include_str!("../assets/pet/pet.json");

struct Image {
    bgra: Vec<u8>,
    width: u32,
    height: u32,
}

pub struct PetImages {
    solo: Image,
}

pub struct PetBitmaps {
    pub solo: ID2D1Bitmap1,
}

pub struct Sprite<'a> {
    pub bmp: &'a ID2D1Bitmap1,
    pub atlas: &'a Atlas,
    pub cell: (u32, u32),
    pub scale: f32,
}

impl Sprite<'_> {
    pub fn draw(&self, p: &Painter, at: Point, opacity: f32) {
        p.sprite(self.bmp, self.atlas.src(self.cell), sprite_dest(at, self.atlas, self.scale), opacity);
    }
}

impl PetImages {
    pub fn upload(&self, p: &Painter) -> Result<PetBitmaps> {
        Ok(PetBitmaps {
            solo: p.bitmap(&self.solo.bgra, self.solo.width, self.solo.height)?,
        })
    }
}

/// Only balances a successful CoInitializeEx, including S_FALSE; never an existing different apartment.
struct Apartment(bool);

impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

pub fn load() -> Option<(Sheet, PetImages)> {
    let result = (|| -> Result<(Sheet, PetImages)> {
        let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if initialized.is_err() && initialized != RPC_E_CHANGED_MODE {
            initialized.ok()?;
        }
        let _apartment = Apartment(initialized.is_ok());
        let sheet = Sheet::parse(JSON).map_err(|e| Error::new(E_INVALIDARG, e))?;
        let solo = decode(SOLO, &sheet.solo)?;
        Ok((sheet, PetImages { solo }))
    })();
    match result {
        Ok(pet) => Some(pet),
        Err(e) => {
            log::warning!("pet unavailable: {e}");
            None
        }
    }
}

fn decode(bytes: &[u8], atlas: &Atlas) -> Result<Image> {
    unsafe {
        let factory: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let stream = factory.CreateStream()?;
        // Embedded bytes outlive the stream, decoder and converter: WIC holds the memory by reference.
        stream.InitializeFromMemory(bytes)?;
        let decoder = factory.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnLoad)?;
        let frame = decoder.GetFrame(0)?;
        let (mut width, mut height) = (0, 0);
        frame.GetSize(&mut width, &mut height)?;
        if width == 0 || height == 0
            || atlas.cell_w.checked_mul(atlas.columns) != Some(width)
            || atlas.cell_h.checked_mul(atlas.rows) != Some(height)
        {
            return Err(Error::new(E_INVALIDARG, "pet atlas dimensions do not match pet.json"));
        }
        let converter = factory.CreateFormatConverter()?;
        converter.Initialize(
            &frame,
            &GUID_WICPixelFormat32bppPBGRA,
            WICBitmapDitherTypeNone,
            None,
            0.0,
            WICBitmapPaletteTypeCustom,
        )?;
        let stride = width.checked_mul(4).ok_or_else(|| Error::from_hresult(E_INVALIDARG))?;
        let len = stride.checked_mul(height).ok_or_else(|| Error::from_hresult(E_INVALIDARG))?;
        let mut bgra = vec![0; len as usize];
        converter.CopyPixels(std::ptr::null(), stride, &mut bgra)?;
        Ok(Image { bgra, width, height })
    }
}
