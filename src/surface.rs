#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {}

use anyhow::{Context, Result, bail};
use skia_safe::{Canvas, IPoint, ImageInfo, Surface, surfaces};

pub struct SurfaceRenderer {
    backend: Backend,
}

enum Backend {
    Raster,
    #[cfg(target_os = "macos")]
    Metal(MetalBackend),
}

impl SurfaceRenderer {
    pub fn new() -> Self {
        #[cfg(target_os = "macos")]
        if let Some(metal) = MetalBackend::new() {
            return Self {
                backend: Backend::Metal(metal),
            };
        }
        Self {
            backend: Backend::Raster,
        }
    }

    pub fn backend_name(&self) -> &'static str {
        match self.backend {
            Backend::Raster => "raster",
            #[cfg(target_os = "macos")]
            Backend::Metal(_) => "metal",
        }
    }

    pub fn render_into<F>(
        &mut self,
        info: &ImageInfo,
        row_bytes: usize,
        pixels: &mut Vec<u8>,
        draw: F,
    ) -> Result<()>
    where
        F: FnOnce(&Canvas) -> Result<()>,
    {
        match &mut self.backend {
            Backend::Raster => render_raster_into(info, row_bytes, pixels, draw),
            #[cfg(target_os = "macos")]
            Backend::Metal(metal) => metal.render_into(info, row_bytes, pixels, draw),
        }
    }
}

fn render_raster_into<F>(
    info: &ImageInfo,
    row_bytes: usize,
    pixels: &mut Vec<u8>,
    draw: F,
) -> Result<()>
where
    F: FnOnce(&Canvas) -> Result<()>,
{
    pixels.resize(info.compute_byte_size(row_bytes), 0);
    let mut surface = surfaces::wrap_pixels(info, pixels.as_mut_slice(), row_bytes, None)
        .context("create direct raster surface")?;
    draw(surface.canvas())?;
    Ok(())
}

fn read_pixels_into(
    surface: &mut Surface,
    info: &ImageInfo,
    row_bytes: usize,
    pixels: &mut Vec<u8>,
) -> Result<()> {
    pixels.resize(info.compute_byte_size(row_bytes), 0);
    if !surface.read_pixels(info, pixels, row_bytes, IPoint::new(0, 0)) {
        bail!("read RGBA pixels from surface");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
struct MetalBackend {
    context: skia_safe::gpu::DirectContext,
    _backend: skia_safe::gpu::mtl::BackendContext,
    _queue: objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2_metal::MTLCommandQueue>>,
    surface: Option<Surface>,
}

#[cfg(target_os = "macos")]
impl MetalBackend {
    fn new() -> Option<Self> {
        use objc2::{rc::Retained, runtime::ProtocolObject};
        use objc2_metal::{MTLCommandQueue, MTLCreateSystemDefaultDevice, MTLDevice};
        use skia_safe::gpu::{self, mtl};

        let device = MTLCreateSystemDefaultDevice()?;
        let queue = device.newCommandQueue()?;
        let backend = unsafe {
            mtl::BackendContext::new(
                Retained::as_ptr(&device) as mtl::Handle,
                Retained::as_ptr(&queue) as mtl::Handle,
            )
        };
        let context = gpu::direct_contexts::make_metal(&backend, None)?;
        let queue: Retained<ProtocolObject<dyn MTLCommandQueue>> = queue;
        Some(Self {
            context,
            _backend: backend,
            _queue: queue,
            surface: None,
        })
    }

    fn render_into<F>(
        &mut self,
        info: &ImageInfo,
        row_bytes: usize,
        pixels: &mut Vec<u8>,
        draw: F,
    ) -> Result<()>
    where
        F: FnOnce(&Canvas) -> Result<()>,
    {
        use skia_safe::gpu::{self, Budgeted, SurfaceOrigin};

        if self.surface.is_none() {
            self.surface = Some(
                gpu::surfaces::render_target(
                    &mut self.context,
                    Budgeted::No,
                    info,
                    0,
                    SurfaceOrigin::TopLeft,
                    None,
                    false,
                    false,
                )
                .context("create Metal render target")?,
            );
        }
        let surface = self.surface.as_mut().expect("Metal surface initialized");
        draw(surface.canvas())?;
        read_pixels_into(surface, info, row_bytes, pixels)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skia_safe::{AlphaType, Color, ColorSpace, ColorType, ISize};

    #[test]
    fn raster_backend_draws_directly_into_output_buffer() -> Result<()> {
        let info = ImageInfo::new(
            ISize::new(2, 1),
            ColorType::RGBA8888,
            AlphaType::Premul,
            ColorSpace::new_srgb(),
        );
        let mut pixels = Vec::new();
        render_raster_into(&info, 8, &mut pixels, |canvas| {
            canvas.clear(Color::from_argb(255, 12, 34, 56));
            Ok(())
        })?;
        assert_eq!(pixels, [12, 34, 56, 255, 12, 34, 56, 255]);
        Ok(())
    }
}
