#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {}

use anyhow::{Context, Result, bail};
use skia_safe::{Canvas, IPoint, ImageInfo, Surface};

pub struct SurfaceRenderer {
    backend: Backend,
}

enum Backend {
    Raster(RasterBackend),
    #[cfg(target_os = "macos")]
    Metal(MetalBackend),
}

struct RasterBackend {
    surface: Option<Surface>,
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
            backend: Backend::Raster(RasterBackend { surface: None }),
        }
    }

    pub fn backend_name(&self) -> &'static str {
        match self.backend {
            Backend::Raster(_) => "raster",
            #[cfg(target_os = "macos")]
            Backend::Metal(_) => "metal",
        }
    }

    pub fn render<F>(&mut self, info: &ImageInfo, row_bytes: usize, draw: F) -> Result<Vec<u8>>
    where
        F: FnOnce(&Canvas) -> Result<()>,
    {
        let mut pixels = Vec::new();
        self.render_into(info, row_bytes, &mut pixels, draw)?;
        Ok(pixels)
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
            Backend::Raster(raster) => render_raster_into(raster, info, row_bytes, pixels, draw),
            #[cfg(target_os = "macos")]
            Backend::Metal(metal) => metal.render_into(info, row_bytes, pixels, draw),
        }
    }
}

fn render_raster_into<F>(
    backend: &mut RasterBackend,
    info: &ImageInfo,
    row_bytes: usize,
    pixels: &mut Vec<u8>,
    draw: F,
) -> Result<()>
where
    F: FnOnce(&Canvas) -> Result<()>,
{
    if backend.surface.is_none() {
        backend.surface =
            Some(Surface::new_raster(info, row_bytes, None).context("create raster surface")?);
    }
    let surface = backend
        .surface
        .as_mut()
        .expect("raster surface initialized");
    draw(surface.canvas())?;
    read_pixels_into(surface, info, row_bytes, pixels)
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
                    Budgeted::Yes,
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
        self.context.flush_and_submit();
        read_pixels_into(surface, info, row_bytes, pixels)
    }
}
