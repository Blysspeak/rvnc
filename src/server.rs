use crate::capture::ScreenCapture;
use crate::rfb::*;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use std::io::Write;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufWriter};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

const TILE_SIZE: u16 = 64;

pub struct VncServer {
    capture: Arc<Mutex<ScreenCapture>>,
    port: u16,
    name: String,
}

impl VncServer {
    pub fn new(capture: ScreenCapture, port: u16, name: String) -> Self {
        Self {
            capture: Arc::new(Mutex::new(capture)),
            port,
            name,
        }
    }

    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind(format!("0.0.0.0:{}", self.port)).await?;
        log::info!("VNC server listening on port {}", self.port);

        loop {
            let (stream, addr) = listener.accept().await?;
            stream.set_nodelay(true)?;
            log::info!("Client connected: {}", addr);

            let capture = self.capture.clone();
            let name = self.name.clone();

            tokio::spawn(async move {
                if let Err(e) = handle_client(stream, capture, &name).await {
                    log::error!("Client {} error: {}", addr, e);
                }
                log::info!("Client {} disconnected", addr);
            });
        }
    }
}

async fn handle_client(
    stream: TcpStream,
    capture: Arc<Mutex<ScreenCapture>>,
    name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut stream = BufWriter::with_capacity(256 * 1024, stream);

    let (width, height) = {
        let cap = capture.lock().await;
        (cap.width, cap.height)
    };
    let pf = PixelFormat::default_32bit();

    // === Handshake ===
    stream.write_all(RFB_VERSION).await?;
    stream.flush().await?;

    let mut version = [0u8; 12];
    stream.read_exact(&mut version).await?;

    stream.write_all(&[1u8, SEC_NONE]).await?;
    stream.flush().await?;

    let mut sec = [0u8; 1];
    stream.read_exact(&mut sec).await?;

    stream.write_all(&0u32.to_be_bytes()).await?;
    stream.flush().await?;

    let mut shared = [0u8; 1];
    stream.read_exact(&mut shared).await?;

    let mut init = Vec::with_capacity(24 + name.len());
    init.extend_from_slice(&width.to_be_bytes());
    init.extend_from_slice(&height.to_be_bytes());
    init.extend_from_slice(&pf.to_bytes());
    let name_bytes = name.as_bytes();
    init.extend_from_slice(&(name_bytes.len() as u32).to_be_bytes());
    init.extend_from_slice(name_bytes);
    stream.write_all(&init).await?;
    stream.flush().await?;

    log::info!("Handshake OK, {}x{}", width, height);

    // Previous frame for delta detection
    let mut prev_frame: Vec<u8> = vec![0; (width as usize) * (height as usize) * 4];
    let mut client_pf = pf;
    let mut use_zrle = false;

    loop {
        let mut msg_type = [0u8; 1];
        stream.read_exact(&mut msg_type).await?;

        match msg_type[0] {
            MSG_SET_PIXEL_FORMAT => {
                let mut buf = [0u8; 19];
                stream.read_exact(&mut buf).await?;
                let mut pf_bytes = [0u8; 16];
                pf_bytes.copy_from_slice(&buf[3..19]);
                client_pf = PixelFormat::from_bytes(&pf_bytes);
            }
            MSG_SET_ENCODINGS => {
                let mut buf = [0u8; 3];
                stream.read_exact(&mut buf).await?;
                let count = u16::from_be_bytes([buf[1], buf[2]]) as usize;
                let mut enc_buf = vec![0u8; count * 4];
                stream.read_exact(&mut enc_buf).await?;
                let encodings: Vec<i32> = enc_buf
                    .chunks(4)
                    .map(|c| i32::from_be_bytes([c[0], c[1], c[2], c[3]]))
                    .collect();
                use_zrle = encodings.contains(&ENC_ZRLE);
                log::debug!("Encodings: {:?}, ZRLE={}", encodings, use_zrle);
            }
            MSG_FB_UPDATE_REQUEST => {
                let mut buf = [0u8; 9];
                stream.read_exact(&mut buf).await?;
                let incremental = buf[0] != 0;

                // Capture
                let cur_frame = {
                    let cap = capture.lock().await;
                    cap.capture_full()?
                };

                if !incremental {
                    // Full frame
                    let pixel_data = convert_pixels(&cur_frame, &client_pf);
                    if use_zrle {
                        send_zrle_update(&mut stream, 0, 0, width, height, &pixel_data, width).await?;
                    } else {
                        send_raw_update(&mut stream, 0, 0, width, height, &pixel_data).await?;
                    }
                    prev_frame.copy_from_slice(&cur_frame);
                } else {
                    // Delta: find dirty tiles
                    let dirty = find_dirty_tiles(&prev_frame, &cur_frame, width, height);

                    if dirty.is_empty() {
                        // Nothing changed — send empty update with 0 rects
                        let header = [MSG_FB_UPDATE, 0, 0, 0];
                        stream.write_all(&header).await?;
                        stream.flush().await?;
                    } else {
                        let converted = convert_pixels(&cur_frame, &client_pf);
                        send_dirty_tiles(&mut stream, &dirty, &converted, width, height, use_zrle).await?;
                        prev_frame.copy_from_slice(&cur_frame);
                    }
                }
            }
            MSG_KEY_EVENT => {
                let mut buf = [0u8; 7];
                stream.read_exact(&mut buf).await?;
            }
            MSG_POINTER_EVENT => {
                let mut buf = [0u8; 5];
                stream.read_exact(&mut buf).await?;
            }
            MSG_CLIENT_CUT_TEXT => {
                let mut buf = [0u8; 7];
                stream.read_exact(&mut buf).await?;
                let len = u32::from_be_bytes([buf[3], buf[4], buf[5], buf[6]]) as usize;
                let mut text = vec![0u8; len];
                stream.read_exact(&mut text).await?;
            }
            _ => break,
        }
    }

    Ok(())
}

#[derive(Debug)]
struct DirtyTile {
    x: u16,
    y: u16,
    w: u16,
    h: u16,
}

fn find_dirty_tiles(prev: &[u8], cur: &[u8], width: u16, height: u16) -> Vec<DirtyTile> {
    let mut dirty = Vec::new();
    let stride = width as usize * 4;

    let mut ty: u16 = 0;
    while ty < height {
        let th = TILE_SIZE.min(height - ty);
        let mut tx: u16 = 0;
        while tx < width {
            let tw = TILE_SIZE.min(width - tx);

            // Compare tile
            let mut changed = false;
            'check: for row in 0..th as usize {
                let y_off = (ty as usize + row) * stride;
                let x_start = tx as usize * 4;
                let x_end = (tx as usize + tw as usize) * 4;
                let slice_prev = &prev[y_off + x_start..y_off + x_end];
                let slice_cur = &cur[y_off + x_start..y_off + x_end];
                if slice_prev != slice_cur {
                    changed = true;
                    break 'check;
                }
            }

            if changed {
                dirty.push(DirtyTile { x: tx, y: ty, w: tw, h: th });
            }

            tx += TILE_SIZE;
        }
        ty += TILE_SIZE;
    }

    dirty
}

async fn send_dirty_tiles(
    stream: &mut BufWriter<TcpStream>,
    tiles: &[DirtyTile],
    pixels: &[u8],
    fb_width: u16,
    _fb_height: u16,
    use_zrle: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    // Header
    let mut header = [0u8; 4];
    header[0] = MSG_FB_UPDATE;
    header[2..4].copy_from_slice(&(tiles.len() as u16).to_be_bytes());
    stream.write_all(&header).await?;

    let stride = fb_width as usize * 4;

    for tile in tiles {
        if use_zrle {
            // Extract tile pixels
            let mut tile_data = Vec::with_capacity(tile.w as usize * tile.h as usize * 4);
            for row in 0..tile.h as usize {
                let y_off = (tile.y as usize + row) * stride;
                let x_start = tile.x as usize * 4;
                let x_end = (tile.x as usize + tile.w as usize) * 4;
                tile_data.extend_from_slice(&pixels[y_off + x_start..y_off + x_end]);
            }
            send_zrle_tile(stream, tile.x, tile.y, tile.w, tile.h, &tile_data).await?;
        } else {
            // Raw encoding per tile
            let mut rect_header = Vec::with_capacity(12);
            rect_header.extend_from_slice(&tile.x.to_be_bytes());
            rect_header.extend_from_slice(&tile.y.to_be_bytes());
            rect_header.extend_from_slice(&tile.w.to_be_bytes());
            rect_header.extend_from_slice(&tile.h.to_be_bytes());
            rect_header.extend_from_slice(&(ENC_RAW as u32).to_be_bytes());
            stream.write_all(&rect_header).await?;

            for row in 0..tile.h as usize {
                let y_off = (tile.y as usize + row) * stride;
                let x_start = tile.x as usize * 4;
                let x_end = (tile.x as usize + tile.w as usize) * 4;
                stream.write_all(&pixels[y_off + x_start..y_off + x_end]).await?;
            }
        }
    }

    stream.flush().await?;
    Ok(())
}

async fn send_zrle_tile(
    stream: &mut BufWriter<TcpStream>,
    x: u16, y: u16, w: u16, h: u16,
    pixels: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    // Compress with zlib
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());

    // ZRLE: for each 64x64 tile, write subencoding byte + pixel data
    // Subencoding 0 = raw CPIXEL
    encoder.write_all(&[0u8])?; // raw subencoding

    // Write CPIXEL (3 bytes per pixel for 32bpp true color)
    for chunk in pixels.chunks(4) {
        encoder.write_all(&chunk[..3])?; // BGR without padding
    }

    let compressed = encoder.finish()?;

    // Rect header
    let mut header = Vec::with_capacity(16);
    header.extend_from_slice(&x.to_be_bytes());
    header.extend_from_slice(&y.to_be_bytes());
    header.extend_from_slice(&w.to_be_bytes());
    header.extend_from_slice(&h.to_be_bytes());
    header.extend_from_slice(&(ENC_ZRLE as u32).to_be_bytes());
    header.extend_from_slice(&(compressed.len() as u32).to_be_bytes());

    stream.write_all(&header).await?;
    stream.write_all(&compressed).await?;

    Ok(())
}

async fn send_raw_update(
    stream: &mut BufWriter<TcpStream>,
    x: u16, y: u16, w: u16, h: u16,
    pixels: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut header = Vec::with_capacity(16);
    header.push(MSG_FB_UPDATE);
    header.push(0);
    header.extend_from_slice(&1u16.to_be_bytes());
    header.extend_from_slice(&x.to_be_bytes());
    header.extend_from_slice(&y.to_be_bytes());
    header.extend_from_slice(&w.to_be_bytes());
    header.extend_from_slice(&h.to_be_bytes());
    header.extend_from_slice(&(ENC_RAW as u32).to_be_bytes());

    stream.write_all(&header).await?;
    stream.write_all(pixels).await?;
    stream.flush().await?;
    Ok(())
}

async fn send_zrle_update(
    stream: &mut BufWriter<TcpStream>,
    x: u16, y: u16, w: u16, h: u16,
    pixels: &[u8],
    fb_width: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    // For full frame ZRLE, split into tiles ourselves
    let mut tiles = Vec::new();
    let mut ty = y;
    while ty < y + h {
        let th = TILE_SIZE.min(y + h - ty);
        let mut tx = x;
        while tx < x + w {
            let tw = TILE_SIZE.min(x + w - tx);
            tiles.push(DirtyTile { x: tx, y: ty, w: tw, h: th });
            tx += TILE_SIZE;
        }
        ty += TILE_SIZE;
    }

    send_dirty_tiles(stream, &tiles, pixels, fb_width, h, true).await
}

fn convert_pixels(bgra: &[u8], pf: &PixelFormat) -> Vec<u8> {
    if pf.bits_per_pixel == 32 && pf.red_shift == 16 && pf.green_shift == 8 && pf.blue_shift == 0 {
        return bgra.to_vec();
    }

    if pf.bits_per_pixel == 32 && pf.red_shift == 0 && pf.green_shift == 8 && pf.blue_shift == 16 {
        let mut out = Vec::with_capacity(bgra.len());
        for chunk in bgra.chunks(4) {
            out.push(chunk[2]);
            out.push(chunk[1]);
            out.push(chunk[0]);
            out.push(0);
        }
        return out;
    }

    bgra.to_vec()
}
