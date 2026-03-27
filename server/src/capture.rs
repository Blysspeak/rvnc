use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;

pub struct ScreenCapture {
    conn: RustConnection,
    root: u32,
    pub width: u16,
    pub height: u16,
}

impl ScreenCapture {
    pub fn new(display: Option<&str>) -> Result<Self, Box<dyn std::error::Error>> {
        let (conn, screen_num) = if let Some(d) = display {
            std::env::set_var("DISPLAY", d);
            RustConnection::connect(Some(d))?
        } else {
            RustConnection::connect(None)?
        };

        let screen = &conn.setup().roots[screen_num];
        let root = screen.root;
        let width = screen.width_in_pixels;
        let height = screen.height_in_pixels;

        log::info!("Connected to X11 display, screen {}x{}", width, height);

        Ok(Self { conn, root, width, height })
    }

    /// Capture full screen as BGRA pixels
    pub fn capture_full(&self) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let image = get_image(
            &self.conn,
            ImageFormat::Z_PIXMAP,
            self.root,
            0, 0,
            self.width, self.height,
            !0,
        )?.reply()?;

        Ok(image.data)
    }

    /// Capture a region as BGRA pixels
    pub fn capture_region(&self, x: i16, y: i16, w: u16, h: u16) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let image = get_image(
            &self.conn,
            ImageFormat::Z_PIXMAP,
            self.root,
            x, y,
            w, h,
            !0,
        )?.reply()?;

        Ok(image.data)
    }
}
