//! A frame read back for `screenshot` and `cl_avidemo` (`crate::screenshot`):
//! the swapchain image as the frame left it, gamma ramp and 2D included,
//! copied out after the last pass and before the present.

/// A frame's pixels: rows top to bottom, 3 bytes (R, G, B) a pixel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capture {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// The copy a frame's encoder carries, read once the frame is submitted.
pub(super) struct Readback {
    buf: wgpu::Buffer,
    width: u32,
    height: u32,
    /// Bytes a row in `buf`, padded to wgpu's copy alignment.
    row: u32,
    bgr: bool,
}

/// Queues `tex`'s copy into a mappable buffer on `enc`. `None` for a format
/// other than 8-bit RGBA or BGRA, or a texture without `COPY_SRC` (a
/// surface that lists no such usage).
pub(super) fn encode(
    device: &wgpu::Device,
    enc: &mut wgpu::CommandEncoder,
    tex: &wgpu::Texture,
) -> Option<Readback> {
    use wgpu::TextureFormat as F;
    let bgr = match tex.format() {
        F::Rgba8Unorm | F::Rgba8UnormSrgb => false,
        F::Bgra8Unorm | F::Bgra8UnormSrgb => true,
        other => {
            log::warn!("screenshot: cannot read a {other:?} frame");
            return None;
        }
    };
    if !tex.usage().contains(wgpu::TextureUsages::COPY_SRC) {
        log::warn!("screenshot: the swapchain cannot be read back");
        return None;
    }
    let (width, height) = (tex.width(), tex.height());
    let row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("screenshot readback"),
        size: u64::from(row * height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    enc.copy_texture_to_buffer(
        tex.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(height),
            },
        },
        tex.size(),
    );
    Some(Readback {
        buf,
        width,
        height,
        row,
        bgr,
    })
}

/// Waits for the submitted copy and repacks it as [`Capture`]'s RGB.
pub(super) fn finish(device: &wgpu::Device, rb: Readback) -> Option<Capture> {
    rb.buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
    let mapped = rb.buf.slice(..).get_mapped_range().ok()?;
    let (w, h) = (rb.width as usize, rb.height as usize);
    let mut rgb = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        let line = &mapped[y * rb.row as usize..][..w * 4];
        for px in line.as_chunks::<4>().0 {
            let [a, b, c, _] = *px;
            rgb.extend_from_slice(&if rb.bgr { [c, b, a] } else { [a, b, c] });
        }
    }
    Some(Capture {
        width: rb.width,
        height: rb.height,
        rgb,
    })
}
